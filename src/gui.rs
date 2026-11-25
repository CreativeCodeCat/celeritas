//! The window: draws the `Screen` grid with egui and forwards keyboard input to the PTY.

use std::io::Write;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use eframe::egui::{self, Align2, Color32, FontId, Key, Modifiers, Sense, Vec2};
use portable_pty::{Child, MasterPty, PtySize};

use crate::config::{Colors, Config, Rgb};
use crate::screen::{Color, Screen};
use crate::PtyMsg;

/// One terminal window: owns the shell process and the screen it writes into.
pub struct Terminal {
    config: Config,
    screen: Screen,
    parser: vte::Parser,
    rx: Receiver<PtyMsg>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    shell_exited: bool,
    /// Whether the window has been sized to the configured columns and lines yet.
    sized: bool,
}

impl Terminal {
    pub fn new(
        config: Config,
        rx: Receiver<PtyMsg>,
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        child: Box<dyn Child + Send + Sync>,
    ) -> Self {
        Terminal {
            screen: Screen::new(config.columns, config.lines),
            config,
            parser: vte::Parser::new(),
            rx,
            master,
            writer,
            child,
            shell_exited: false,
            sized: false,
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        if self.writer.write_all(bytes).and_then(|_| self.writer.flush()).is_err() {
            self.shell_exited = true;
        }
    }

    /// Feed everything the shell has written since the last frame into the screen.
    fn pump_output(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(PtyMsg::Data(bytes)) => {
                    for &b in &bytes {
                        self.parser.advance(&mut self.screen, b);
                    }
                    // Answer terminal queries such as fish's device-attributes probe.
                    let replies = self.screen.take_replies();
                    if !replies.is_empty() {
                        self.send(&replies);
                    }
                }
                Ok(PtyMsg::Closed) | Err(TryRecvError::Disconnected) => {
                    self.shell_exited = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
    }

    /// Forward this frame's keyboard and clipboard input to the shell.
    fn handle_input(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        for event in events {
            let bytes = match event {
                egui::Event::Text(text) | egui::Event::Paste(text) => text.into_bytes(),
                // No text selection yet, so copy/cut act as the usual terminal control keys.
                egui::Event::Copy => vec![0x03], // Ctrl-C (interrupt)
                egui::Event::Cut => vec![0x18],  // Ctrl-X
                egui::Event::Key { key, pressed: true, modifiers, .. } => {
                    match key_bytes(key, modifiers) {
                        Some(bytes) => bytes,
                        None => continue,
                    }
                }
                _ => continue,
            };
            self.send(&bytes);
        }
    }

    /// Lay out the grid to fill the window, resizing the screen and PTY when it changes.
    fn layout_grid(&mut self, ui: &mut egui::Ui) -> (egui::Response, egui::Painter, Vec2, usize, usize) {
        let font = FontId::monospace(self.config.font_size);
        let fg = rgb(self.config.colors.foreground);
        let cell = ui.painter().layout_no_wrap("M".to_string(), font, fg).size();

        if !self.sized {
            // First frame: make the window fit the configured columns and lines.
            self.sized = true;
            let want = Vec2::new(self.config.columns as f32 * cell.x, self.config.lines as f32 * cell.y);
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(want));
        }

        let avail = ui.available_size();
        let cols = (avail.x / cell.x).floor().max(1.0) as usize;
        let rows = (avail.y / cell.y).floor().max(1.0) as usize;
        if cols != self.screen.cols || rows != self.screen.rows {
            self.screen.resize(cols, rows);
            let _ = self.master.resize(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            });
        }

        let (response, painter) =
            ui.allocate_painter(Vec2::new(cols as f32 * cell.x, rows as f32 * cell.y), Sense::hover());
        (response, painter, cell, cols, rows)
    }

    fn paint_grid(&self, painter: &egui::Painter, origin: egui::Pos2, cell: Vec2, cols: usize, rows: usize) {
        let colors = &self.config.colors;
        let font = FontId::monospace(self.config.font_size);
        let (cur_x, cur_y) = self.screen.cursor();
        for y in 0..rows {
            for x in 0..cols {
                let c = self.screen.cell(x, y);
                let mut fg = resolve(c.fg, colors.foreground, c.bold, colors);
                let mut bg = resolve(c.bg, colors.background, false, colors);
                if c.reverse {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let pos = origin + Vec2::new(x as f32 * cell.x, y as f32 * cell.y);
                let rect = egui::Rect::from_min_size(pos, cell);
                if bg != rgb(colors.background) {
                    painter.rect_filled(rect, 0.0, bg);
                }
                if c.ch != ' ' {
                    painter.text(pos, Align2::LEFT_TOP, c.ch, font.clone(), fg);
                }
                if (x, y) == (cur_x, cur_y) {
                    painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(0xdd, 0xdd, 0xdd, 0x90));
                }
            }
        }
    }
}

impl eframe::App for Terminal {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump_output();
        if self.shell_exited {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.handle_input(&ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(rgb(self.config.colors.background)))
            .show(ui, |ui| {
                let (response, painter, cell, cols, rows) = self.layout_grid(ui);
                self.paint_grid(&painter, response.rect.min, cell, cols, rows);
            });

        // Shell output arrives on another thread, so keep polling for it.
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Translate a key press into the bytes a terminal program expects.
fn key_bytes(key: Key, m: Modifiers) -> Option<Vec<u8>> {
    let seq: &[u8] = match key {
        Key::Enter => b"\r",
        Key::Backspace => b"\x7f",
        Key::Tab if m.shift => b"\x1b[Z",
        Key::Tab => b"\t",
        Key::Escape => b"\x1b",
        Key::ArrowUp => b"\x1b[A",
        Key::ArrowDown => b"\x1b[B",
        Key::ArrowRight => b"\x1b[C",
        Key::ArrowLeft => b"\x1b[D",
        Key::Home => b"\x1b[H",
        Key::End => b"\x1b[F",
        Key::Insert => b"\x1b[2~",
        Key::Delete => b"\x1b[3~",
        Key::PageUp => b"\x1b[5~",
        Key::PageDown => b"\x1b[6~",
        _ => {
            // Ctrl+letter sends the matching control byte (Ctrl-A = 0x01, Ctrl-C = 0x03, ...).
            let name = key.name();
            if m.ctrl && name.len() == 1 {
                let c = name.as_bytes()[0].to_ascii_lowercase();
                if c.is_ascii_lowercase() {
                    return Some(vec![c & 0x1f]);
                }
            }
            return None;
        }
    };
    let mut out = Vec::with_capacity(seq.len() + 1);
    if m.alt {
        out.push(0x1b); // Alt sends ESC first
    }
    out.extend_from_slice(seq);
    Some(out)
}

fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// Turn a terminal colour into pixels. `default` is used for the terminal's own default colour.
fn resolve(color: Color, default: Rgb, bold: bool, colors: &Colors) -> Color32 {
    match color {
        Color::Default => rgb(default),
        Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
        // Bold text uses the bright variant of the basic colours, when configured.
        Color::Indexed(n) if bold && n < 8 && colors.draw_bold_text_with_bright_colors => {
            rgb(colors.bright[n as usize])
        }
        Color::Indexed(n) => palette(n, colors),
    }
}

/// The 256-colour palette: the configured 16 ANSI colours, then the xterm colour cube and greys.
fn palette(n: u8, colors: &Colors) -> Color32 {
    let basic = |i: usize| rgb(colors.normal[i]);
    match n {
        0..=7 => basic(n as usize),
        8..=15 => rgb(colors.bright[(n - 8) as usize]),
        16..=231 => {
            let i = n - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            Color32::from_rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        _ => {
            let g = 8 + 10 * (n - 232);
            Color32::from_rgb(g, g, g)
        }
    }
}
