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
                eprintln!("Celeritas: font {:?} not usable ({e}); using monospace", normal_face.family);
                rasterizer.load_font(&desc(&face("monospace", "Regular"), "Regular"), size)?
            }
        };

        let family = normal_face.family.clone();
        // A variant without a real face of its own uses the closest face that has one:
        // italic uses the normal face, and bold italic uses the bold face.
        let load_variant = |rasterizer: &mut Rasterizer, configured: &Option<FontFace>, style: &str, auto: bool, fallback: FontKey| -> FontKey {
            let wanted = match configured {
                Some(configured) => Some(configured.clone()),
                None if auto => system_face(&family, style),
                None => Some(face(&family, style)),
            };
            let Some(wanted) = wanted else {
                return fallback;
            };
            rasterizer.load_font(&desc(&wanted, style), size).unwrap_or(fallback)
        };
        let bold = load_variant(&mut rasterizer, &faces.bold, "Bold", false, normal);
        let italic = load_variant(&mut rasterizer, &faces.italic, "Italic", true, normal);
        let bold_italic = load_variant(&mut rasterizer, &faces.bold_italic, "Bold Italic", true, bold);

        // FreeType only has the size once a glyph has been rasterised, and the metrics
        // are not valid before that. So measure a glyph first and take the cell width
        // from its advance, which is in pixels.
        let probe = GlyphKey { character: 'M', font_key: normal, size };
        let advance = match rasterizer.get_glyph(probe) {
            Ok(glyph) | Err(FontError::MissingGlyph(glyph)) => glyph.advance.0,
            Err(_) => 0,
        };
        let metrics = rasterizer.metrics(normal, size)?;
        let cell_width = if advance > 0 {
            advance as f32
        } else {
            metrics.average_advance.max(1.0) as f32
        };
        // Some rasterizer setups report no line height or descent. Fall back to values
        // typical of monospace fonts at this size.
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
            size,
            cell_width,
            cell_height,
            baseline,
        })
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

/// The families fontconfig has a face for, matching `pattern`.
fn families_matching(pattern: &str) -> Vec<String> {
    let Ok(output) = std::process::Command::new("fc-list").arg(pattern).arg("family").output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split(',').next())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

/// A real face in `style` (such as "Italic") for the normal face: the same family if the
/// system has one, otherwise the first monospace family that has one. `None` if there is none.
fn system_face(family: &str, style: &str) -> Option<FontFace> {
    if families_matching(&format!("{family}:style={style}")).iter().any(|f| f == family) {
        return Some(face(family, style));
    }
    families_matching(&format!(":spacing=mono:style={style}"))
        .into_iter()
        .next()
        .map(|other| face(&other, style))
}

fn face(family: &str, style: &str) -> FontFace {
    FontFace { family: family.to_string(), style: Some(style.to_string()) }
}

fn desc(face: &FontFace, default_style: &str) -> FontDesc {
    let style = face.style.clone().unwrap_or_else(|| default_style.to_string());
    FontDesc::new(face.family.clone(), Style::Specific(style))
}
