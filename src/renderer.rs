//! OpenGL renderer: draws the `Screen` grid as textured and coloured quads.
//!
//! Each frame the whole grid is turned into one vertex list. Glyphs are rasterised by
//! `FontSet` on first use and packed into a single-channel texture atlas.

use std::collections::HashMap;

use glow::HasContext;

use crate::config::{Colors, CursorStyle, Rgb};
use crate::font::{FaceStyle, FontSet};
use crate::screen::{Cell, Color, Screen};

/// Side length of the glyph atlas texture, in pixels.
const ATLAS_SIZE: i32 = 2048;
/// Vertex mode: a solid colour.
const MODE_SOLID: f32 = 0.0;
/// Vertex mode: a glyph, whose alpha comes from the atlas.
const MODE_GLYPH: f32 = 1.0;
/// Horizontal shift of the top of an italic glyph, as a fraction of its height (about 11 degrees).
/// Floats per vertex: position (2), atlas coordinates (2), colour (4), mode (1).
const FLOATS_PER_VERTEX: usize = 9;

const SELECTION_COLOR: [f32; 4] = [0.23, 0.56, 0.93, 0.55];
const CURSOR_COLOR: [f32; 4] = [0.87, 0.87, 0.87, 0.56];

const VERTEX_SHADER: &str = r#"#version 330 core
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_uv;
layout(location = 2) in vec4 a_color;
layout(location = 3) in float a_mode;
uniform vec2 u_size;
out vec2 v_uv;
out vec4 v_color;
flat out float v_mode;
void main() {
    vec2 ndc = a_pos / u_size * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    v_uv = a_uv;
    v_color = a_color;
    v_mode = a_mode;
}
"#;

const FRAGMENT_SHADER: &str = r#"#version 330 core
in vec2 v_uv;
in vec4 v_color;
flat in float v_mode;
uniform sampler2D u_atlas;
out vec4 frag_color;
void main() {
    if (v_mode > 0.5) {
        frag_color = vec4(v_color.rgb, v_color.a * texture(u_atlas, v_uv).r);
    } else {
        frag_color = v_color;
    }
}
"#;

/// Extra things drawn over the grid.
#[derive(Default)]
pub struct Overlay {
    /// Selected range, as ordered (row, column) logical positions.
    pub selection: Option<((usize, usize), (usize, usize))>,
    /// Whether the selection is a rectangle rather than a stream of text.
    pub selection_block: bool,
    /// Cursor position on the view, if it should be drawn.
    pub cursor: Option<(usize, usize)>,
    pub cursor_style: CursorStyle,
    /// Underline and beam thickness as a fraction of the cell.
    pub cursor_thickness: f32,
    /// Whether the window has keyboard focus.
    pub focused: bool,
    /// Draw a block cursor as an outline while the window is unfocused.
    pub unfocused_hollow: bool,
}

/// A glyph in the atlas. Zero-sized glyphs (such as spaces) have no texture area.
#[derive(Clone, Copy)]
struct Glyph {
    uv: [f32; 4],
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

pub struct Renderer {
    gl: glow::Context,
    program: glow::Program,
    u_size: Option<glow::UniformLocation>,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    atlas: glow::Texture,
    atlas_x: i32,
    atlas_y: i32,
    row_height: i32,
    glyphs: HashMap<(char, FaceStyle), Option<Glyph>>,
    pub fonts: FontSet,
    /// Window opacity: the default background is drawn at this alpha.
    pub opacity: f32,
    /// Blank space around the grid, in physical pixels (`window.padding`, scaled).
    padding: (f32, f32),
    /// Share spare window space evenly around the grid (`window.dynamic_padding`).
    dynamic_padding: bool,
    /// Where the top-left cell of the grid is drawn, in physical pixels.
    origin: (f32, f32),
    vertices: Vec<f32>,
    size: (u32, u32),
}

impl Renderer {
    /// Create the GL resources. `gl` must come from a current context.
    pub fn new(gl: glow::Context, fonts: FontSet, size: (u32, u32)) -> Result<Renderer, String> {
        unsafe {
            let program = build_program(&gl)?;
            let u_size = gl.get_uniform_location(program, "u_size");

            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let stride = (FLOATS_PER_VERTEX * 4) as i32;
            // position, atlas coordinates, colour, mode
            for (index, count, offset) in [(0u32, 2, 0), (1, 2, 8), (2, 4, 16), (3, 1, 32)] {
                gl.enable_vertex_attrib_array(index);
                gl.vertex_attrib_pointer_f32(index, count, glow::FLOAT, false, stride, offset);
            }

            let atlas = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(atlas));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::R8 as i32,
                ATLAS_SIZE,
                ATLAS_SIZE,
                0,
                glow::RED,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );

            gl.enable(glow::BLEND);
            // Premultiplied alpha output, so a transparent window composites correctly.
            gl.blend_func_separate(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA, glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.viewport(0, 0, size.0 as i32, size.1 as i32);

            Ok(Renderer {
                gl,
                program,
                u_size,
                vao,
                vbo,
                atlas,
                atlas_x: 0,
                atlas_y: 0,
                row_height: 0,
                glyphs: HashMap::new(),
                fonts,
                opacity: 1.0,
                padding: (0.0, 0.0),
                dynamic_padding: false,
                origin: (0.0, 0.0),
                vertices: Vec::new(),
                size,
            })
        }
    }

    /// Set the padding (physical pixels) and whether spare space is shared around the grid.
    /// Call `layout` afterwards.
    pub fn set_padding(&mut self, padding: (f32, f32), dynamic: bool) {
        self.padding = padding;
        self.dynamic_padding = dynamic;
    }

    /// Place a grid of `cols` by `rows` cells in the window, leaving the padding around it.
    pub fn layout(&mut self, cols: usize, rows: usize) {
        let (cw, ch) = (self.fonts.cell_width, self.fonts.cell_height);
        let (pad_x, pad_y) = self.padding;
        let (mut x, mut y) = (pad_x, pad_y);
        if self.dynamic_padding {
            let spare_x = self.size.0 as f32 - 2.0 * pad_x - cols as f32 * cw;
            let spare_y = self.size.1 as f32 - 2.0 * pad_y - rows as f32 * ch;
            x += (spare_x / 2.0).floor().max(0.0);
            y += (spare_y / 2.0).floor().max(0.0);
        }
        self.origin = (x, y);
    }

    /// The padding (physical pixels) around the grid.
    pub fn padding(&self) -> (f32, f32) {
        self.padding
    }

    /// Where the top-left cell of the grid is drawn, in physical pixels.
    pub fn origin(&self) -> (f32, f32) {
        self.origin
    }

    /// The size of the drawing area, in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.size = (width, height);
        unsafe { self.gl.viewport(0, 0, width as i32, height as i32) };
    }

    /// Use new fonts (for example after a DPI change). The atlas is refilled on demand.
    pub fn set_fonts(&mut self, fonts: FontSet) {
        self.fonts = fonts;
        self.reset_atlas();
    }

    /// Draw the grid for `screen` into the current framebuffer.
    pub fn draw(&mut self, screen: &Screen, colors: &Colors, overlay: &Overlay) {
        self.build_vertices(screen, colors, overlay);
        let (w, h) = self.size;
        let background = rgba(colors.background, 1.0);
        unsafe {
            let gl = &self.gl;
            let a = self.opacity;
            gl.clear_color(background[0] * a, background[1] * a, background[2] * a, a);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.use_program(Some(self.program));
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&self.vertices), glow::STREAM_DRAW);
            gl.uniform_2_f32(self.u_size.as_ref(), w as f32, h as f32);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.atlas));
            gl.draw_arrays(glow::TRIANGLES, 0, (self.vertices.len() / FLOATS_PER_VERTEX) as i32);
        }
    }

    /// The last drawn frame as RGBA bytes, bottom row first (OpenGL's order).
    pub fn read_pixels(&self) -> Vec<u8> {
        let (w, h) = self.size;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        unsafe {
            self.gl.read_pixels(
                0,
                0,
                w as i32,
                h as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut buf)),
            );
        }
        buf
    }

    fn build_vertices(&mut self, screen: &Screen, colors: &Colors, overlay: &Overlay) {
        self.vertices.clear();
        let (cw, ch, baseline) = (self.fonts.cell_width, self.fonts.cell_height, self.fonts.baseline);
        let top = screen.first_visible_row();

        // Backgrounds and selection first, so text drawn afterwards is never covered.
        for y in 0..screen.rows {
            for x in 0..screen.cols {
                let cell = screen.visible_cell(x, y);
                let (_, bg) = cell_colors(&cell, colors);
                let (px, py) = (self.origin.0 + x as f32 * cw, self.origin.1 + y as f32 * ch);
                let selected = overlay.selection.is_some_and(|(s, e)| {
                    let p = (top + y, x);
                    if overlay.selection_block {
                        let (c0, c1) = (s.1.min(e.1), s.1.max(e.1));
                        p.0 >= s.0 && p.0 <= e.0 && x >= c0 && x <= c1
                    } else {
                        p >= s && p <= e
                    }
                });
                if selected {
                    push_quad(&mut self.vertices, px, py, cw, ch, [0.0; 4], SELECTION_COLOR, MODE_SOLID);
                } else if bg != colors.background {
                    push_quad(&mut self.vertices, px, py, cw, ch, [0.0; 4], rgba(bg, 1.0), MODE_SOLID);
                }
            }
        }

        for y in 0..screen.rows {
            for x in 0..screen.cols {
                let cell = screen.visible_cell(x, y);
                let (fg, _) = cell_colors(&cell, colors);
                let (px, py) = (self.origin.0 + x as f32 * cw, self.origin.1 + y as f32 * ch);

                // '\0' is the right half of a wide character; its glyph is drawn by the left half.
                if cell.ch != ' ' && cell.ch != '\0' {
                    let style = FaceStyle::new(cell.bold, cell.italic);
                    // Combining marks are drawn over their base character, at the same origin.
                    for ch in std::iter::once(cell.ch).chain(cell.marks.into_iter().flatten()) {
                        if let Some(glyph) = self.glyph(ch, style) {
                            if glyph.width > 0.0 && glyph.height > 0.0 {
                                push_quad(
                                    &mut self.vertices,
                                    px + glyph.left,
                                    py + baseline - glyph.top,
                                    glyph.width,
                                    glyph.height,
                                    glyph.uv,
                                    rgba(fg, 1.0),
                                    MODE_GLYPH,
                                );
                            }
                        }
                    }
                }
                if cell.underline {
                    push_quad(&mut self.vertices, px, py + ch - 1.0, cw, 1.0, [0.0; 4], rgba(fg, 1.0), MODE_SOLID);
                }
                if cell.strikethrough {
                    push_quad(&mut self.vertices, px, py + ch / 2.0, cw, 1.0, [0.0; 4], rgba(fg, 1.0), MODE_SOLID);
                }
                if overlay.cursor == Some((x, y)) {
                    let hollow = !overlay.focused && overlay.unfocused_hollow && overlay.cursor_style == CursorStyle::Block;
                    if hollow {
                        // A one-pixel outline around the cell.
                        let edges = [
                            (px, py, cw, 1.0),
                            (px, py + ch - 1.0, cw, 1.0),
                            (px, py, 1.0, ch),
                            (px + cw - 1.0, py, 1.0, ch),
                        ];
                        for (x0, y0, w, h) in edges {
                            push_quad(&mut self.vertices, x0, y0, w, h, [0.0; 4], CURSOR_COLOR, MODE_SOLID);
                        }
                    } else {
                        let thick_h = (ch * overlay.cursor_thickness).max(1.0);
                        let thick_w = (cw * overlay.cursor_thickness).max(1.0);
                        let (x0, y0, w, h) = match overlay.cursor_style {
                            CursorStyle::Block => (px, py, cw, ch),
                            CursorStyle::Underline => (px, py + ch - thick_h, cw, thick_h),
                            CursorStyle::Beam => (px, py, thick_w, ch),
                        };
                        push_quad(&mut self.vertices, x0, y0, w, h, [0.0; 4], CURSOR_COLOR, MODE_SOLID);
                    }
                }
            }
        }
    }

    /// The atlas entry for a character, rasterising and uploading it on first use.
    fn glyph(&mut self, ch: char, style: FaceStyle) -> Option<Glyph> {
        if let Some(cached) = self.glyphs.get(&(ch, style)) {
            return *cached;
        }
        let entry = self.rasterize_into_atlas(ch, style);
        self.glyphs.insert((ch, style), entry);
        entry
    }

    fn rasterize_into_atlas(&mut self, ch: char, style: FaceStyle) -> Option<Glyph> {
        let raster = self.fonts.rasterize(ch, style)?;
        let (w, h) = (raster.width, raster.height);
        if w <= 0 || h <= 0 {
            return Some(Glyph { uv: [0.0; 4], left: 0.0, top: 0.0, width: 0.0, height: 0.0 });
        }
        if w > ATLAS_SIZE || h > ATLAS_SIZE {
            return None;
        }
        let alpha = alpha_mask(&raster);
        let (x, y) = self.allocate(w, h);
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(self.atlas));
            self.gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            self.gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                x,
                y,
                w,
                h,
                glow::RED,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&alpha)),
            );
        }
        let size = ATLAS_SIZE as f32;
        Some(Glyph {
            uv: [
                x as f32 / size,
                y as f32 / size,
                (x + w) as f32 / size,
                (y + h) as f32 / size,
            ],
            left: raster.left as f32,
            top: raster.top as f32,
            width: w as f32,
            height: h as f32,
        })
    }

    /// Find room for a `w` by `h` glyph. If the atlas is full it starts again from empty;
    /// glyphs already placed this frame may then show the wrong shape for one frame.
    fn allocate(&mut self, w: i32, h: i32) -> (i32, i32) {
        if self.atlas_x + w > ATLAS_SIZE {
            self.atlas_x = 0;
            self.atlas_y += self.row_height;
            self.row_height = 0;
        }
        if self.atlas_y + h > ATLAS_SIZE {
            self.reset_atlas();
        }
        let pos = (self.atlas_x, self.atlas_y);
        self.atlas_x += w;
        self.row_height = self.row_height.max(h);
        pos
    }

    fn reset_atlas(&mut self) {
        self.glyphs.clear();
        self.atlas_x = 0;
        self.atlas_y = 0;
        self.row_height = 0;
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_program(self.program);
            self.gl.delete_texture(self.atlas);
            self.gl.delete_buffer(self.vbo);
            self.gl.delete_vertex_array(self.vao);
        }
    }
}

unsafe fn build_program(gl: &glow::Context) -> Result<glow::Program, String> {
    let program = gl.create_program()?;
    let mut shaders = Vec::new();
    for (kind, source) in [(glow::VERTEX_SHADER, VERTEX_SHADER), (glow::FRAGMENT_SHADER, FRAGMENT_SHADER)] {
        let shader = gl.create_shader(kind)?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            return Err(gl.get_shader_info_log(shader));
        }
        gl.attach_shader(program, shader);
        shaders.push(shader);
    }
    gl.link_program(program);
    for shader in shaders {
        gl.detach_shader(program, shader);
        gl.delete_shader(shader);
    }
    if !gl.get_program_link_status(program) {
        return Err(gl.get_program_info_log(program));
    }
    Ok(program)
}

/// The per-channel alpha of a rasterised glyph, one byte per pixel.
fn alpha_mask(glyph: &crossfont::RasterizedGlyph) -> Vec<u8> {
    match &glyph.buffer {
        crossfont::BitmapBuffer::Rgb(px) => px
            .chunks_exact(3)
            .map(|c| ((c[0] as u16 + c[1] as u16 + c[2] as u16) / 3) as u8)
            .collect(),
        crossfont::BitmapBuffer::Rgba(px) => px.chunks_exact(4).map(|c| c[3]).collect(),
    }
}

/// Append two triangles covering a rectangle.
fn push_quad(v: &mut Vec<f32>, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4], color: [f32; 4], mode: f32) {
    let [u0, v0, u1, v1] = uv;
    let corners = [
        (x, y, u0, v0),
        (x + w, y, u1, v0),
        (x + w, y + h, u1, v1),
        (x, y, u0, v0),
        (x + w, y + h, u1, v1),
        (x, y + h, u0, v1),
    ];
    for (px, py, pu, pv) in corners {
        v.extend_from_slice(&[px, py, pu, pv, color[0], color[1], color[2], color[3], mode]);
    }
}

fn as_bytes(floats: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding and any bit pattern is a valid u8.
    unsafe { std::slice::from_raw_parts(floats.as_ptr() as *const u8, floats.len() * 4) }
}

fn rgba(c: Rgb, alpha: f32) -> [f32; 4] {
    [c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0, alpha]
}

/// The foreground and background colours a cell is drawn with, after reverse and dim.
fn cell_colors(cell: &Cell, colors: &Colors) -> (Rgb, Rgb) {
    let mut fg = resolve(cell.fg, colors.foreground, cell.bold, colors);
    let mut bg = resolve(cell.bg, colors.background, false, colors);
    if cell.dim {
        fg = mix(fg, bg, 0.5);
    }
    if cell.reverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    (fg, bg)
}

/// Blend `a` towards `b` by `t` (0 = all `a`, 1 = all `b`).
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgb(lerp(a.0, b.0), lerp(a.1, b.1), lerp(a.2, b.2))
}

/// Turn a terminal colour into RGB. `default` is used for the terminal's own default colour.
fn resolve(color: Color, default: Rgb, bold: bool, colors: &Colors) -> Rgb {
    match color {
        Color::Default => default,
        Color::Rgb(r, g, b) => Rgb(r, g, b),
        // Bold text uses the bright variant of the basic colours, when configured.
        Color::Indexed(n) if bold && n < 8 && colors.draw_bold_text_with_bright_colors => colors.bright[n as usize],
        Color::Indexed(n) => palette(n, colors),
    }
}

/// The 256-colour palette: the configured 16 ANSI colours, then the standard colour cube and greys.
fn palette(n: u8, colors: &Colors) -> Rgb {
    match n {
        0..=7 => colors.normal[n as usize],
        8..=15 => colors.bright[(n - 8) as usize],
        16..=231 => {
            let i = n - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            Rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        _ => {
            let g = 8 + 10 * (n - 232);
            Rgb(g, g, g)
        }
    }
}
