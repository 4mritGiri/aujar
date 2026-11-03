/// An sRGB color with 8 bits per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Build from channels in `0.0..=1.0` (as the desktop portal reports them).
    /// Out-of-range values are clamped.
    pub fn from_unit(r: f64, g: f64, b: f64) -> Self {
        let channel = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;

        Self::new(channel(r), channel(g), channel(b))
    }

    /// Parse `#RGB`, `#RRGGBB`, `RGB` or `RRGGBB` (case-insensitive).
    pub fn parse_hex(input: &str) -> Option<Self> {
        let digits = input.trim().trim_start_matches('#');

        if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }

        match digits.len() {
            3 => {
                let mut values = digits
                    .chars()
                    .map(|c| c.to_digit(16).map(|digit| (digit * 17) as u8));

                Some(Self::new(values.next()??, values.next()??, values.next()??))
            }
            6 => {
                let channel =
                    |range: std::ops::Range<usize>| u8::from_str_radix(&digits[range], 16).ok();

                Some(Self::new(channel(0..2)?, channel(2..4)?, channel(4..6)?))
            }
            _ => None,
        }
    }

    /// `#RRGGBB`
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// `rgb(255, 136, 0)`
    pub fn to_rgb_string(self) -> String {
        format!("rgb({}, {}, {})", self.r, self.g, self.b)
    }

    /// Hue in degrees (0-359), saturation and lightness in percent.
    pub fn to_hsl(self) -> (u16, u8, u8) {
        let r = f64::from(self.r) / 255.0;
        let g = f64::from(self.g) / 255.0;
        let b = f64::from(self.b) / 255.0;

        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let lightness = (max + min) / 2.0;

        if delta == 0.0 {
            return (0, 0, (lightness * 100.0).round() as u8);
        }

        let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());

        let sector = if max == r {
            ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        };

        let hue = ((sector * 60.0).round() as u16) % 360;

        (
            hue,
            (saturation * 100.0).round() as u8,
            (lightness * 100.0).round() as u8,
        )
    }

    /// `hsl(32, 100%, 50%)`
    pub fn to_hsl_string(self) -> String {
        let (h, s, l) = self.to_hsl();

        format!("hsl({h}, {s}%, {l}%)")
    }
}

#[cfg(test)]
mod tests {
    use super::Rgb;

    #[test]
    fn parses_hex_forms() {
        let orange = Some(Rgb::new(255, 136, 0));

        assert_eq!(Rgb::parse_hex("#FF8800"), orange);
        assert_eq!(Rgb::parse_hex("ff8800"), orange);
        assert_eq!(Rgb::parse_hex("  #f80 "), orange);
    }

    #[test]
    fn rejects_bad_hex() {
        assert_eq!(Rgb::parse_hex("#12"), None);
        assert_eq!(Rgb::parse_hex("#GGGGGG"), None);
        assert_eq!(Rgb::parse_hex("#1234567"), None);
        assert_eq!(Rgb::parse_hex(""), None);
    }

    #[test]
    fn formats() {
        let orange = Rgb::new(255, 136, 0);

        assert_eq!(orange.to_hex(), "#FF8800");
        assert_eq!(orange.to_rgb_string(), "rgb(255, 136, 0)");
        assert_eq!(orange.to_hsl_string(), "hsl(32, 100%, 50%)");
    }

    #[test]
    fn hsl_edge_cases() {
        assert_eq!(Rgb::new(0, 0, 0).to_hsl(), (0, 0, 0));
        assert_eq!(Rgb::new(255, 255, 255).to_hsl(), (0, 0, 100));
        assert_eq!(Rgb::new(255, 0, 0).to_hsl(), (0, 100, 50));
        assert_eq!(Rgb::new(0, 0, 255).to_hsl(), (240, 100, 50));
    }

    #[test]
    fn unit_channels_round_and_clamp() {
        assert_eq!(Rgb::from_unit(1.0, 0.5, 0.0), Rgb::new(255, 128, 0));
        assert_eq!(Rgb::from_unit(2.0, -1.0, 0.0), Rgb::new(255, 0, 0));
    }
}
