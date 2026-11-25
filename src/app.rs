//! The window and its event loop: ties the shell, the screen, the renderer and input together.

use std::error::Error;
use std::ffi::CStr;
use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::mpsc::TryRecvError;

use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes};

use crate::clipboard::Clipboard;
use crate::config::Config;
use crate::font::FontSet;
use crate::input::{key_bytes, Mods};
use crate::pty::{Pty, PtyMsg};
use crate::renderer::{Overlay, Renderer};
use crate::screen::Screen;

/// Lines moved per mouse-wheel notch.
const WHEEL_LINES: f32 = 3.0;
/// With `CELERITAS_SCREENSHOT` set, the window saves its first frame after this many redraws.
const SCREENSHOT_FRAME: u32 = 10;

/// A mouse selection in logical screen coordinates: (row, column) pairs.
#[derive(Clone, Copy, PartialEq)]
struct Selection {
    anchor: (usize, usize),
    head: (usize, usize),
}

impl Selection {
    /// The selected range, ordered, or `None` if nothing is selected.
    fn range(&self) -> Option<((usize, usize), (usize, usize))> {
        match self.anchor.cmp(&self.head) {
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Less => Some((self.anchor, self.head)),
            std::cmp::Ordering::Greater => Some((self.head, self.anchor)),
        }
    }
}

/// Everything that exists once the window and its GL context do.
struct Graphics {
    window: Window,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    renderer: Renderer,
}

pub struct App {
    config: Config,
    pty: Pty,
    screen: Screen,
    parser: vte::Parser,
    gfx: Option<Graphics>,
    clipboard: Option<Clipboard>,
    mods: Mods,
    cursor_pos: (f64, f64),
    selection: Option<Selection>,
    selecting: bool,
    screenshot: Option<PathBuf>,
    frames: u32,
}

impl App {
    pub fn new(config: Config, pty: Pty, screenshot: Option<PathBuf>) -> App {
        let mut screen = Screen::new(config.columns, config.lines);
        screen.set_history_limit(config.scrollback);
        App {
            config,
            pty,
            screen,
            parser: vte::Parser::new(),
            gfx: None,
            clipboard: None,
            mods: Mods::default(),
            cursor_pos: (0.0, 0.0),
            selection: None,
            selecting: false,
            screenshot,
            frames: 0,
        }
    }

    fn create_graphics(&self, event_loop: &ActiveEventLoop) -> Result<Graphics, Box<dyn Error>> {
        let scale = event_loop.primary_monitor().map_or(1.0, |m| m.scale_factor());
        let fonts = FontSet::load(&self.config.faces, self.config.font_size * scale as f32)?;
        let size = PhysicalSize::new(
            (self.config.columns as f32 * fonts.cell_width).ceil() as u32,
            (self.config.lines as f32 * fonts.cell_height).ceil() as u32,
        );

        let attributes = WindowAttributes::default()
            .with_title(self.config.title.clone())
            .with_inner_size(size)
            .with_decorations(self.config.decorations);
        let (window, gl_config) = DisplayBuilder::new()
            .with_window_attributes(Some(attributes))
            .build(event_loop, ConfigTemplateBuilder::new(), |mut configs| {
                configs.next().expect("no OpenGL configuration available")
            })?;
        let window = window.ok_or("the window could not be created")?;

        let raw_window = window.window_handle()?.as_raw();
        let display = gl_config.display();
        let context_attributes = ContextAttributesBuilder::new().build(Some(raw_window));
        // SAFETY: the window handle outlives the context, which is dropped with `Graphics`.
        let not_current = unsafe { display.create_context(&gl_config, &context_attributes)? };
        let surface_attributes = window.build_surface_attributes(SurfaceAttributesBuilder::new())?;
        // SAFETY: as above, the surface is owned by the same `Graphics` value.
        let surface = unsafe { display.create_window_surface(&gl_config, &surface_attributes)? };
        let context = not_current.make_current(&surface)?;
        let _ = surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::new(1).unwrap()));

        // SAFETY: the context is current on this thread, and the loader only reads symbols.
        let gl = unsafe { glow::Context::from_loader_function_cstr(|name: &CStr| display.get_proc_address(name)) };
        let inner = window.inner_size();
        let renderer = Renderer::new(gl, fonts, (inner.width, inner.height))?;

        Ok(Graphics { window, surface, context, renderer })
    }

    /// Feed the shell's output to the screen. Returns `true` once the shell has exited.
    fn pump_output(&mut self) -> bool {
        loop {
            match self.pty.try_recv() {
                Ok(PtyMsg::Data(bytes)) => {
                    for &b in &bytes {
                        self.parser.advance(&mut self.screen, b);
                    }
                    // Answer terminal queries such as fish's device-attributes probe.
                    let replies = self.screen.take_replies();
                    if !replies.is_empty() {
                        let _ = self.pty.write(&replies);
                    }
                }
                Ok(PtyMsg::Closed) | Err(TryRecvError::Disconnected) => return true,
                Err(TryRecvError::Empty) => return false,
            }
        }
    }

    fn request_redraw(&self) {
        if let Some(gfx) = &self.gfx {
            gfx.window.request_redraw();
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let Some(gfx) = self.gfx.as_mut() else {
            return;
        };
        let overlay = Overlay {
            selection: self.selection.and_then(|s| s.range()),
            cursor: self.screen.at_bottom().then(|| self.screen.cursor()),
        };
        gfx.renderer.draw(&self.screen, &self.config.colors, &overlay);

        self.frames += 1;
        if let Some(path) = &self.screenshot {
            if self.frames == SCREENSHOT_FRAME {
                let (w, h) = (gfx.renderer.size().0, gfx.renderer.size().1);
                if let Err(e) = write_ppm(path, &gfx.renderer.read_pixels(), w, h) {
                    eprintln!("celeritas: could not save screenshot: {e}");
                }
                event_loop.exit();
                return;
            }
        }
        let _ = gfx.surface.swap_buffers(&gfx.context);
    }

    fn resize(&mut self, width: u32, height: u32) {
        let Some(gfx) = self.gfx.as_mut() else {
            return;
        };
        // A minimised window reports zero size; there is nothing to draw then.
        let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) else {
            return;
        };
        gfx.surface.resize(&gfx.context, w, h);
        gfx.renderer.resize(width, height);

        let cols = (width as f32 / gfx.renderer.fonts.cell_width).floor().max(1.0) as usize;
        let rows = (height as f32 / gfx.renderer.fonts.cell_height).floor().max(1.0) as usize;
        if cols != self.screen.cols || rows != self.screen.rows {
            self.screen.resize(cols, rows);
            self.pty.resize(cols as u16, rows as u16);
        }
        gfx.window.request_redraw();
    }

    /// Reload the fonts at a new scale factor, such as after moving to another monitor.
    fn rescale(&mut self, scale: f64) {
        let Ok(fonts) = FontSet::load(&self.config.faces, self.config.font_size * scale as f32) else {
            return;
        };
        let size = match self.gfx.as_mut() {
            Some(gfx) => {
                gfx.renderer.set_fonts(fonts);
                gfx.window.inner_size()
            }
            None => return,
        };
        self.resize(size.width, size.height);
    }

    fn write(&mut self, bytes: &[u8]) {
        let _ = self.pty.write(bytes);
    }

    fn key_press(&mut self, key: &Key, text: Option<&str>) {
        let m = self.mods;
        let is_char = |want: &str| matches!(key, Key::Character(c) if c.eq_ignore_ascii_case(want));

        if m.ctrl && m.shift && is_char("c") {
            self.copy_selection();
            return;
        }
        if m.ctrl && is_char("v") {
            self.paste();
            return;
        }
        if m.shift && !m.ctrl && !m.alt && !self.screen.in_alt_screen() {
            let page = self.screen.rows.saturating_sub(1).max(1) as isize;
            match key {
                Key::Named(NamedKey::PageUp) => {
                    self.screen.scroll_view(page);
                    self.request_redraw();
                    return;
                }
                Key::Named(NamedKey::PageDown) => {
                    self.screen.scroll_view(-page);
                    self.request_redraw();
                    return;
                }
                _ => {}
            }
        }
        if let Some(bytes) = key_bytes(key, text, m) {
            self.screen.reset_view();
            self.write(&bytes);
        }
    }

    fn copy_selection(&mut self) {
        let Some((start, end)) = self.selection.and_then(|s| s.range()) else {
            return;
        };
        let text = self.screen.text_between(start, end);
        if let (false, Some(clipboard)) = (text.is_empty(), self.clipboard.as_mut()) {
            clipboard.set(text);
        }
    }

    fn paste(&mut self) {
        let Some(text) = self.clipboard.as_mut().and_then(|c| c.get()) else {
            return;
        };
        // The shell expects carriage returns for new lines, as the Enter key sends.
        self.screen.reset_view();
        self.write(text.replace('\n', "\r").as_bytes());
    }

    /// The logical (row, column) under the mouse pointer, clamped to the grid.
    fn grid_position(&self) -> Option<(usize, usize)> {
        let gfx = self.gfx.as_ref()?;
        let fonts = &gfx.renderer.fonts;
        let col = (self.cursor_pos.0 as f32 / fonts.cell_width).floor().max(0.0) as usize;
        let row = (self.cursor_pos.1 as f32 / fonts.cell_height).floor().max(0.0) as usize;
        Some((
            self.screen.first_visible_row() + row.min(self.screen.rows - 1),
            col.min(self.screen.cols - 1),
        ))
    }

    fn start_selection(&mut self) {
        if let Some(p) = self.grid_position() {
            self.selection = Some(Selection { anchor: p, head: p });
            self.selecting = true;
            self.request_redraw();
        }
    }

    fn extend_selection(&mut self) {
        if let Some(p) = self.grid_position() {
            if let Some(sel) = self.selection.as_mut() {
                sel.head = p;
                self.request_redraw();
            }
        }
    }

    fn wheel(&mut self, delta: MouseScrollDelta) {
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => (y * WHEEL_LINES).round() as isize,
            MouseScrollDelta::PixelDelta(p) => {
                let cell_height = self.gfx.as_ref().map_or(1.0, |g| g.renderer.fonts.cell_height as f64);
                (p.y / cell_height).round() as isize
            }
        };
        if lines != 0 {
            self.screen.scroll_view(lines);
            self.request_redraw();
        }
    }
}

impl ApplicationHandler<()> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        match self.create_graphics(event_loop) {
            Ok(gfx) => {
                self.clipboard = Clipboard::new();
                self.gfx = Some(gfx);
                self.request_redraw();
            }
            Err(e) => {
                eprintln!("celeritas: {e}");
                event_loop.exit();
            }
        }
    }

    /// The reader thread woke us: the shell printed something.
    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: ()) {
        if self.pump_output() {
            event_loop.exit();
            return;
        }
        self.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: winit::window::WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.resize(size.width, size.height),
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => self.rescale(scale_factor),
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.mods = Mods { shift: state.shift_key(), ctrl: state.control_key(), alt: state.alt_key() };
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                self.key_press(&event.logical_key, event.text.as_deref());
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
                if self.selecting {
                    self.extend_selection();
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => match state {
                ElementState::Pressed => self.start_selection(),
                ElementState::Released => self.selecting = false,
            },
            WindowEvent::MouseWheel { delta, .. } => self.wheel(delta),
            _ => {}
        }
    }
}

/// Save an RGBA frame as a binary PPM. OpenGL returns rows bottom-up, so they are flipped.
fn write_ppm(path: &Path, rgba: &[u8], width: u32, height: u32) -> std::io::Result<()> {
    let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
    for row in (0..height).rev() {
        for x in 0..width {
            let i = ((row * width + x) * 4) as usize;
            out.extend_from_slice(&rgba[i..i + 3]);
        }
    }
    fs::write(path, out)
}
