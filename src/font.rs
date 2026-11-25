//! Font loading and glyph rasterisation, via crossfont (FreeType and fontconfig on Linux).
//!
//! crossfont also finds fallback fonts for characters the chosen face lacks (such as
//! Nerd Font icons), so the caller only asks for a character in a style.

use std::error::Error;

use crossfont::{Error as FontError, FontDesc, FontKey, GlyphKey, RasterizedGlyph, Rasterize, Rasterizer, Size, Style};

use crate::config::{FontFace, FontFaces};

/// Which of the four faces a glyph comes from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FaceStyle {
    Normal,
    Bold,
    Italic,
    BoldItalic,
}

impl FaceStyle {
    pub fn new(bold: bool, italic: bool) -> FaceStyle {
        match (bold, italic) {
            (false, false) => FaceStyle::Normal,
            (true, false) => FaceStyle::Bold,
            (false, true) => FaceStyle::Italic,
            (true, true) => FaceStyle::BoldItalic,
        }
    }
}

/// The loaded faces at one size, plus the cell geometry they imply.
pub struct FontSet {
    rasterizer: Rasterizer,
    keys: [FontKey; 4],
    synthetic_italic: bool,
    synthetic_bold_italic: bool,
    size: Size,
    /// Width of one terminal cell, in pixels.
    pub cell_width: f32,
    /// Height of one terminal cell, in pixels.
    pub cell_height: f32,
    /// Distance from the top of a cell down to the text baseline, in pixels.
    pub baseline: f32,
}

impl FontSet {
    /// Load the four faces at `px` pixels. Missing faces fall back to the normal face,
    /// and a missing normal face falls back to the system monospace font.
    pub fn load(faces: &FontFaces, px: f32) -> Result<FontSet, Box<dyn Error>> {
        let mut rasterizer = Rasterizer::new()?;
        let size = Size::from_px(px);

        let normal_face = faces.normal.clone().unwrap_or_else(|| face("monospace", "Regular"));
        let normal = match rasterizer.load_font(&desc(&normal_face, "Regular"), size) {
            Ok(key) => key,
            Err(e) => {
                eprintln!("celeritas: font {:?} not usable ({e}); using monospace", normal_face.family);
                rasterizer.load_font(&desc(&face("monospace", "Regular"), "Regular"), size)?
            }
        };

        let family = normal_face.family.clone();
        // Returns the key, and whether a real face was loaded from the configuration.
        let load_variant = |rasterizer: &mut Rasterizer, configured: &Option<FontFace>, style: &str| -> (FontKey, bool) {
            let wanted = configured.clone().unwrap_or_else(|| face(&family, style));
            match rasterizer.load_font(&desc(&wanted, style), size) {
                Ok(key) => (key, configured.is_some()),
                Err(_) => (normal, false),
            }
        };
        let (bold, _) = load_variant(&mut rasterizer, &faces.bold, "Bold");
        // fontconfig returns a regular face for a style it lacks, so a real italic face
        // only counts when the configuration names one.
        let (italic, italic_real) = load_variant(&mut rasterizer, &faces.italic, "Italic");
        let (bold_italic, bold_italic_real) = load_variant(&mut rasterizer, &faces.bold_italic, "Bold Italic");

        let metrics = rasterizer.metrics(normal, size)?;
        // Some rasterizer setups report no line height or descent. Fall back to values
        // typical of monospace fonts at this size.
        let cell_width = metrics.average_advance.max(1.0) as f32;
        let cell_height = if metrics.line_height > 0.0 {
            metrics.line_height.round() as f32
        } else {
            (px * 1.2).ceil()
        };
        // The descent is negative, so this puts the baseline above the bottom of the cell.
        let descent = if metrics.descent != 0.0 { metrics.descent } else { -(px * 0.2).ceil() };
        let baseline = cell_height + descent;

        Ok(FontSet {
            rasterizer,
            keys: [normal, bold, italic, bold_italic],
            synthetic_italic: !italic_real,
            synthetic_bold_italic: !bold_italic_real,
            size,
            cell_width,
            cell_height,
            baseline,
        })
    }

    /// Whether italic text in this style must be slanted in software.
    pub fn slants(&self, style: FaceStyle) -> bool {
        match style {
            FaceStyle::Italic => self.synthetic_italic,
            FaceStyle::BoldItalic => self.synthetic_bold_italic,
            _ => false,
        }
    }

    /// Rasterise one character in a face. `None` if the font system gives nothing for it.
    pub fn rasterize(&mut self, ch: char, style: FaceStyle) -> Option<RasterizedGlyph> {
        let key = GlyphKey {
            character: ch,
            font_key: self.keys[style as usize],
            size: self.size,
        };
        match self.rasterizer.get_glyph(key) {
            Ok(glyph) => Some(glyph),
            // The rasterizer returns a placeholder for characters no font has.
            Err(FontError::MissingGlyph(glyph)) => Some(glyph),
            Err(_) => None,
        }
    }
}

fn face(family: &str, style: &str) -> FontFace {
    FontFace { family: family.to_string(), style: Some(style.to_string()) }
}

fn desc(face: &FontFace, default_style: &str) -> FontDesc {
    let style = face.style.clone().unwrap_or_else(|| default_style.to_string());
    FontDesc::new(face.family.clone(), Style::Specific(style))
}
