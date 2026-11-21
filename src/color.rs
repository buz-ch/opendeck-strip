use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const BLACK: Rgb = Rgb(0, 0, 0);
    pub const WHITE: Rgb = Rgb(255, 255, 255);

    pub fn parse(s: &str) -> Option<Rgb> {
        let s = s.trim();
        if let Some(hex) = s.strip_prefix('#') {
            if !hex.is_ascii() {
                return None;
            }
            return match hex.len() {
                3 => {
                    let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
                    let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
                    let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
                    Some(Rgb(r * 17, g * 17, b * 17))
                }
                6 => Some(Rgb(
                    u8::from_str_radix(&hex[0..2], 16).ok()?,
                    u8::from_str_radix(&hex[2..4], 16).ok()?,
                    u8::from_str_radix(&hex[4..6], 16).ok()?,
                )),
                8 => Some(Rgb(
                    u8::from_str_radix(&hex[0..2], 16).ok()?,
                    u8::from_str_radix(&hex[2..4], 16).ok()?,
                    u8::from_str_radix(&hex[4..6], 16).ok()?,
                )),
                _ => None,
            };
        }
        named(s)
    }

    pub fn to_linear(self) -> [f32; 3] {
        [
            srgb_to_linear(self.0),
            srgb_to_linear(self.1),
            srgb_to_linear(self.2),
        ]
    }

    pub fn from_linear(v: [f32; 3]) -> Rgb {
        Rgb(
            linear_to_srgb(v[0]),
            linear_to_srgb(v[1]),
            linear_to_srgb(v[2]),
        )
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

/// The small named palette shared by the status file and inline markup.
pub fn named(name: &str) -> Option<Rgb> {
    Some(match name.to_ascii_lowercase().as_str() {
        "black" => Rgb(0, 0, 0),
        "white" => Rgb(255, 255, 255),
        "red" => Rgb(255, 80, 80),
        "green" => Rgb(80, 220, 100),
        "blue" => Rgb(90, 150, 255),
        "yellow" => Rgb(255, 220, 80),
        "orange" => Rgb(255, 150, 40),
        "grey" | "gray" => Rgb(160, 160, 160),
        "darkgrey" | "darkgray" => Rgb(80, 80, 80),
        _ => return None,
    })
}

fn srgb_to_linear(v: u8) -> f32 {
    (v as f32 / 255.0).powf(2.2)
}

fn linear_to_srgb(v: f32) -> u8 {
    (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8
}

/// Blend `src` over `dst` with `alpha` coverage (0..=1) in linear light.
pub fn blend_linear(dst: Rgb, src: Rgb, alpha: f32) -> Rgb {
    let a = alpha.clamp(0.0, 1.0);
    if a <= 0.0 {
        return dst;
    }
    if a >= 1.0 {
        return src;
    }
    let d = dst.to_linear();
    let s = src.to_linear();
    Rgb::from_linear([
        d[0] + (s[0] - d[0]) * a,
        d[1] + (s[1] - d[1]) * a,
        d[2] + (s[2] - d[2]) * a,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colors() {
        assert_eq!(Rgb::parse("#fff"), Some(Rgb::WHITE));
        assert_eq!(Rgb::parse("#ff8800"), Some(Rgb(255, 136, 0)));
        assert_eq!(Rgb::parse("RED"), Some(Rgb(255, 80, 80)));
        assert_eq!(Rgb::parse("#ff8800ff"), Some(Rgb(255, 136, 0)));
        assert_eq!(Rgb::parse("nope"), None);
        assert_eq!(Rgb::parse("#gg0000"), None);
    }

    #[test]
    fn non_ascii_hex_is_rejected_not_sliced() {
        // `#aé` is three bytes, so it would enter the three-digit branch.
        assert_eq!(Rgb::parse("#aé"), None);
        // Six bytes whose second character straddles a byte slice boundary.
        assert_eq!(Rgb::parse("#éabcd"), None);
        assert_eq!(Rgb::parse("#😀😀"), None);
    }

    #[test]
    fn blend_endpoints_are_exact() {
        let dst = Rgb(10, 20, 30);
        assert_eq!(blend_linear(dst, Rgb(200, 100, 50), 0.0), dst);
        assert_eq!(blend_linear(dst, Rgb(200, 100, 50), 1.0), Rgb(200, 100, 50));
    }

    #[test]
    fn half_blend_is_between() {
        let out = blend_linear(Rgb::BLACK, Rgb::WHITE, 0.5);
        assert!(out.0 > 170 && out.0 < 190, "{out}");
    }
}
