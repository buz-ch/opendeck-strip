use std::path::Path;
use std::sync::LazyLock;

use ab_glyph::{Font, FontArc, GlyphId, PxScale, PxScaleFont};

use crate::config::Config;

const SANS_REGULAR: &[u8] = include_bytes!("../../assets/fonts/SourceSans3-Regular.otf");
const SANS_BOLD: &[u8] = include_bytes!("../../assets/fonts/SourceSans3-Bold.otf");
const SANS_ITALIC: &[u8] = include_bytes!("../../assets/fonts/SourceSans3-It.otf");
const SANS_BOLD_ITALIC: &[u8] = include_bytes!("../../assets/fonts/SourceSans3-BoldIt.otf");
const MONO_REGULAR: &[u8] = include_bytes!("../../assets/fonts/SourceCodePro-Regular.otf");
const MONO_BOLD: &[u8] = include_bytes!("../../assets/fonts/SourceCodePro-Bold.otf");
const MONO_ITALIC: &[u8] = include_bytes!("../../assets/fonts/SourceCodePro-It.otf");
const MONO_BOLD_ITALIC: &[u8] = include_bytes!("../../assets/fonts/SourceCodePro-BoldIt.otf");

#[derive(Clone)]
pub struct Face {
    pub font: FontArc,
    pub synthetic_bold: bool,
    pub synthetic_italic: bool,
}

#[derive(Clone)]
pub struct Family {
    regular: Face,
    bold: Face,
    italic: Face,
    bold_italic: Face,
}

impl Family {
    pub fn face(&self, bold: bool, italic: bool) -> &Face {
        match (bold, italic) {
            (false, false) => &self.regular,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (true, true) => &self.bold_italic,
        }
    }
}

#[derive(Clone)]
pub struct Fonts {
    sans: Family,
    mono: Family,
}

pub enum GlyphSource<'a> {
    Glyph { face: &'a Face, id: GlyphId },
    Placeholder { face: &'a Face },
}

impl Fonts {
    pub fn embedded() -> &'static Fonts {
        static FONTS: LazyLock<Fonts> = LazyLock::new(|| Fonts {
            sans: embedded_family(SANS_REGULAR, SANS_BOLD, SANS_ITALIC, SANS_BOLD_ITALIC),
            mono: embedded_family(MONO_REGULAR, MONO_BOLD, MONO_ITALIC, MONO_BOLD_ITALIC),
        });
        &FONTS
    }

    pub fn load(config: &Config) -> Fonts {
        let fallback = Self::embedded();
        Fonts {
            sans: custom_family(
                "sans",
                &fallback.sans,
                [
                    config.font_sans.as_deref(),
                    config.font_sans_bold.as_deref(),
                    config.font_sans_italic.as_deref(),
                    config.font_sans_bold_italic.as_deref(),
                ],
            ),
            mono: custom_family(
                "mono",
                &fallback.mono,
                [
                    config.font_mono.as_deref(),
                    config.font_mono_bold.as_deref(),
                    config.font_mono_italic.as_deref(),
                    config.font_mono_bold_italic.as_deref(),
                ],
            ),
        }
    }

    pub fn face(&self, mono: bool, bold: bool, italic: bool) -> &Face {
        if mono {
            self.mono.face(bold, italic)
        } else {
            self.sans.face(bold, italic)
        }
    }

    /// Primary face, then the other embedded family, then a visible placeholder.
    pub fn resolve(&self, mono: bool, bold: bool, italic: bool, c: char) -> GlyphSource<'_> {
        let primary = self.face(mono, bold, italic);
        let id = primary.font.glyph_id(c);
        if id.0 != 0 {
            return GlyphSource::Glyph { face: primary, id };
        }
        let secondary = self.face(!mono, bold, italic);
        let id = secondary.font.glyph_id(c);
        if id.0 != 0 {
            return GlyphSource::Glyph {
                face: secondary,
                id,
            };
        }
        GlyphSource::Placeholder { face: primary }
    }
}

pub fn scaled(face: &Face, x: f32, y: f32) -> PxScaleFont<&FontArc> {
    face.font.as_scaled(PxScale { x, y })
}

fn embedded_family(regular: &[u8], bold: &[u8], italic: &[u8], bold_italic: &[u8]) -> Family {
    Family {
        regular: real(regular),
        bold: real(bold),
        italic: real(italic),
        bold_italic: real(bold_italic),
    }
}

fn real(bytes: &[u8]) -> Face {
    Face {
        font: parse(bytes).expect("embedded font is valid"),
        synthetic_bold: false,
        synthetic_italic: false,
    }
}

fn parse(bytes: &[u8]) -> Result<FontArc, ab_glyph::InvalidFont> {
    FontArc::try_from_vec(bytes.to_vec())
}

fn load_file(path: &Path) -> Option<FontArc> {
    match std::fs::read(path) {
        Ok(bytes) => match parse(&bytes) {
            Ok(font) => Some(font),
            Err(err) => {
                log::warn!("Font {} is not usable: {err}", path.display());
                None
            }
        },
        Err(err) => {
            log::warn!("Cannot read font {}: {err}", path.display());
            None
        }
    }
}

/// Custom paths replace the embedded faces one by one. Missing faces fall back
/// to synthetic styling so a single-font configuration still works.
fn custom_family(name: &str, embedded: &Family, paths: [Option<&Path>; 4]) -> Family {
    let any = paths.iter().any(Option::is_some);
    if !any {
        return clone_family(embedded);
    }

    let regular = paths[0].and_then(load_file);
    let bold = paths[1].and_then(load_file);
    let italic = paths[2].and_then(load_file);
    let bold_italic = paths[3].and_then(load_file);

    let Some(regular) = regular else {
        log::warn!("No {name} regular font loaded; using the embedded family");
        return clone_family(embedded);
    };

    let bold_present = bold.is_some();
    let italic_present = italic.is_some();
    let bold_face = match bold {
        Some(font) => Face {
            font,
            synthetic_bold: false,
            synthetic_italic: false,
        },
        None => Face {
            font: regular.clone(),
            synthetic_bold: true,
            synthetic_italic: false,
        },
    };
    let italic_face = match italic {
        Some(font) => Face {
            font,
            synthetic_bold: false,
            synthetic_italic: false,
        },
        None => Face {
            font: regular.clone(),
            synthetic_bold: false,
            synthetic_italic: true,
        },
    };
    let bold_italic_face = if let Some(font) = bold_italic {
        Face {
            font,
            synthetic_bold: false,
            synthetic_italic: false,
        }
    } else if bold_present {
        Face {
            font: bold_face.font.clone(),
            synthetic_bold: false,
            synthetic_italic: true,
        }
    } else if italic_present {
        Face {
            font: italic_face.font.clone(),
            synthetic_bold: true,
            synthetic_italic: false,
        }
    } else {
        Face {
            font: regular.clone(),
            synthetic_bold: true,
            synthetic_italic: true,
        }
    };

    Family {
        regular: Face {
            font: regular,
            synthetic_bold: false,
            synthetic_italic: false,
        },
        bold: bold_face,
        italic: italic_face,
        bold_italic: bold_italic_face,
    }
}

fn clone_family(family: &Family) -> Family {
    Family {
        regular: clone_face(&family.regular),
        bold: clone_face(&family.bold),
        italic: clone_face(&family.italic),
        bold_italic: clone_face(&family.bold_italic),
    }
}

fn clone_face(face: &Face) -> Face {
    Face {
        font: face.font.clone(),
        synthetic_bold: face.synthetic_bold,
        synthetic_italic: face.synthetic_italic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fonts_parse_and_cover_ascii() {
        let fonts = Fonts::embedded();
        for &mono in &[false, true] {
            for &bold in &[false, true] {
                for &italic in &[false, true] {
                    let face = fonts.face(mono, bold, italic);
                    for c in "Hello, world! 0123…".chars() {
                        assert_ne!(face.font.glyph_id(c).0, 0, "{c:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn non_latin_falls_back_then_placeholder() {
        let fonts = Fonts::embedded();
        match fonts.resolve(false, false, false, '日') {
            GlyphSource::Glyph { .. } | GlyphSource::Placeholder { .. } => {}
        }
    }

    #[test]
    fn missing_face_uses_synthetic_style() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("font.otf");
        std::fs::write(&path, SANS_REGULAR).unwrap();
        let config = Config {
            font_sans: Some(path),
            ..Default::default()
        };
        let fonts = Fonts::load(&config);
        assert!(fonts.sans.bold.synthetic_bold);
        assert!(fonts.sans.italic.synthetic_italic);
        assert!(fonts.sans.bold_italic.synthetic_bold);
        assert!(fonts.sans.bold_italic.synthetic_italic);
    }
}
