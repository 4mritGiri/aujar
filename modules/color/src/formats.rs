//! Color formats: the PowerToys `%Re`, `%GrX`, `%Hu` ... template syntax (plus the older
//! `{hex}`, `{r}`, ... tokens that existing settings files still use), color names and the
//! conversions behind them.

use crate::Rgb;

/// A color shown in every model a format can ask for.
struct Values {
    rgb: Rgb,
    /// Degrees, `0.0..360.0`.
    hue: f64,
    hsl_s: f64,
    hsl_l: f64,
    hsv_s: f64,
    hsv_v: f64,
    hsi_s: f64,
    hsi_i: f64,
    whiteness: f64,
    blackness: f64,
    cmyk: [f64; 4],
    xyz: [f64; 3],
    lab: [f64; 3],
}

impl Values {
    fn new(rgb: Rgb) -> Self {
        let (r, g, b) = rgb.unit();
        let (hue, hsv_s, hsv_v) = rgb.to_hsv();
        let (_, hsl_s, hsl_l) = rgb.to_hsl_f64();

        let min = r.min(g).min(b);
        let hsi_i = (r + g + b) / 3.0;
        let hsi_s = if hsi_i == 0.0 { 0.0 } else { 1.0 - min / hsi_i };

        let key = 1.0 - hsv_v;
        let cmyk = if key >= 1.0 {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            [
                (1.0 - r - key) / (1.0 - key),
                (1.0 - g - key) / (1.0 - key),
                (1.0 - b - key) / (1.0 - key),
                key,
            ]
        };

        let xyz = to_xyz(r, g, b);
        let lab = to_lab(xyz);

        Self {
            rgb,
            hue,
            hsl_s,
            hsl_l,
            hsv_s,
            hsv_v,
            hsi_s,
            hsi_i,
            whiteness: min,
            blackness: key,
            cmyk,
            xyz,
            lab,
        }
    }

    /// Value of a two-letter `%XX` parameter that is not a red, green, blue or alpha channel.
    fn parameter(&self, name: &str) -> Option<String> {
        let percent = |value: f64| whole(value * 100.0);

        Some(match name {
            "Cy" => percent(self.cmyk[0]),
            "Ma" => percent(self.cmyk[1]),
            "Ye" => percent(self.cmyk[2]),
            "Bk" => percent(self.cmyk[3]),
            "Hu" => whole(self.hue.round() % 360.0),
            "Si" => percent(self.hsi_s),
            "Sl" => percent(self.hsl_s),
            "Sb" => percent(self.hsv_s),
            "Br" | "Va" => percent(self.hsv_v),
            "In" => percent(self.hsi_i),
            "Hn" => natural_hue(self.hue),
            "Ll" => percent(self.hsl_l),
            "Lc" => whole(self.lab[0]),
            "Wh" => percent(self.whiteness),
            "Bn" => percent(self.blackness),
            "Ca" => whole(self.lab[1]),
            "Cb" => whole(self.lab[2]),
            "Xv" => decimals(self.xyz[0]),
            "Yv" => decimals(self.xyz[1]),
            "Zv" => decimals(self.xyz[2]),
            "Dv" => {
                (u32::from(self.rgb.r) << 16 | u32::from(self.rgb.g) << 8 | u32::from(self.rgb.b))
                    .to_string()
            }
            "Na" => self.rgb.name().to_owned(),
            _ => return None,
        })
    }
}

impl Rgb {
    /// Channels in `0.0..=1.0`.
    fn unit(self) -> (f64, f64, f64) {
        (
            f64::from(self.r) / 255.0,
            f64::from(self.g) / 255.0,
            f64::from(self.b) / 255.0,
        )
    }

    /// `0xRRGGBB`, as GPUI and most toolkits want it.
    pub fn to_u32(self) -> u32 {
        u32::from(self.r) << 16 | u32::from(self.g) << 8 | u32::from(self.b)
    }

    /// Hue in degrees (`0.0..360.0`), saturation and value in `0.0..=1.0`.
    pub fn to_hsv(self) -> (f64, f64, f64) {
        let (r, g, b) = self.unit();
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;

        let hue = if delta == 0.0 {
            0.0
        } else if max == r {
            ((g - b) / delta).rem_euclid(6.0) * 60.0
        } else if max == g {
            ((b - r) / delta + 2.0) * 60.0
        } else {
            ((r - g) / delta + 4.0) * 60.0
        };
        let saturation = if max == 0.0 { 0.0 } else { delta / max };

        (hue, saturation, max)
    }

    /// Hue in degrees, saturation and lightness in `0.0..=1.0`.
    fn to_hsl_f64(self) -> (f64, f64, f64) {
        let (r, g, b) = self.unit();
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let lightness = (max + min) / 2.0;
        let (hue, _, _) = self.to_hsv();
        let delta = max - min;
        let saturation = if delta == 0.0 {
            0.0
        } else {
            delta / (1.0 - (2.0 * lightness - 1.0).abs())
        };

        (hue, saturation, lightness)
    }

    /// Hue in degrees (any value, wrapped), saturation and value in `0.0..=1.0`.
    pub fn from_hsv(hue: f64, saturation: f64, value: f64) -> Self {
        let saturation = saturation.clamp(0.0, 1.0);
        let value = value.clamp(0.0, 1.0);
        let sector = hue.rem_euclid(360.0) / 60.0;
        let chroma = value * saturation;
        let second = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
        let offset = value - chroma;

        let (r, g, b) = match sector as u8 {
            0 => (chroma, second, 0.0),
            1 => (second, chroma, 0.0),
            2 => (0.0, chroma, second),
            3 => (0.0, second, chroma),
            4 => (second, 0.0, chroma),
            _ => (chroma, 0.0, second),
        };

        Self::from_unit(r + offset, g + offset, b + offset)
    }

    /// Hue in degrees (any value, wrapped), saturation and lightness in `0.0..=1.0`.
    fn from_hsl(hue: f64, saturation: f64, lightness: f64) -> Self {
        let lightness = lightness.clamp(0.0, 1.0);
        let saturation = saturation.clamp(0.0, 1.0);
        let value = lightness + saturation * lightness.min(1.0 - lightness);
        let hsv_saturation = if value == 0.0 {
            0.0
        } else {
            2.0 * (1.0 - lightness / value)
        };

        Self::from_hsv(hue, hsv_saturation, value)
    }

    /// Colors close to this one, as the editor's color bar shows them: `count` lighter shades
    /// (nearest first) and `count` darker shades (nearest first), same hue and saturation.
    pub fn similar(self, count: usize) -> (Vec<Self>, Vec<Self>) {
        let (hue, saturation, lightness) = self.to_hsl_f64();
        let count_f = count.max(1) as f64;

        let shades = |target: f64| -> Vec<Self> {
            (1..=count)
                .map(|step| {
                    let amount = step as f64 / (count_f + 1.0);
                    Self::from_hsl(hue, saturation, lightness + (target - lightness) * amount)
                })
                .collect()
        };

        (shades(0.96), shades(0.06))
    }

    /// A high-level name such as "Light Green", "Green" or "Dark Green".
    pub fn name(self) -> &'static str {
        let (h, s, l) = self.to_hsl();

        if s < 8 {
            return if l < 18 {
                "Black"
            } else if l > 88 {
                "White"
            } else {
                "Gray"
            };
        }

        let hue_name = match h {
            15..=44 => "Orange",
            45..=74 => "Yellow",
            75..=104 => "Lime",
            105..=149 => "Green",
            150..=179 => "Mint",
            180..=204 => "Cyan",
            205..=224 => "Azure",
            225..=254 => "Blue",
            255..=284 => "Violet",
            285..=314 => "Magenta",
            315..=344 => "Rose",
            _ => "Red",
        };

        if l < 24 {
            match hue_name {
                "Orange" | "Yellow" | "Lime" => "Dark Brown",
                "Green" | "Mint" => "Dark Green",
                "Cyan" | "Azure" => "Dark Teal",
                "Blue" => "Dark Blue",
                "Violet" | "Magenta" => "Dark Purple",
                "Rose" | "Red" => "Dark Red",
                _ => "Dark Gray",
            }
        } else if l > 82 {
            match hue_name {
                "Orange" | "Yellow" => "Light Yellow",
                "Lime" | "Green" => "Light Green",
                "Mint" | "Cyan" => "Light Cyan",
                "Azure" | "Blue" => "Light Blue",
                "Violet" | "Magenta" => "Light Purple",
                "Rose" | "Red" => "Light Pink",
                _ => "Light Gray",
            }
        } else {
            hue_name
        }
    }

    /// Render a color format template.
    ///
    /// PowerToys syntax: `%Re %Gr %Bl %Al` (red, green, blue, alpha; add `b h H x X f F` for the
    /// number style, e.g. `%ReX` is two uppercase hex digits), `%Cy %Ma %Ye %Bk`, `%Hu`, `%Si`,
    /// `%Sl`, `%Sb`, `%Br`, `%In`, `%Hn`, `%Ll`, `%Lc`, `%Va`, `%Wh`, `%Bn`, `%Ca`, `%Cb`, `%Xv`,
    /// `%Yv`, `%Zv`, `%Dv` and `%Na`. Anything else is copied as written, so `hsl(%Hu, %Sl%, %Ll%)`
    /// works. The older `{hex}`, `{r}`, `{g}`, `{b}`, `{h}`, `{s}`, `{l}`, `{v}`, `{c}`, `{m}`,
    /// `{y}` and `{k}` tokens still work too.
    pub fn format(self, template: &str) -> String {
        let values = Values::new(self);

        legacy_tokens(self, &powertoys_tokens(&values, template))
    }
}

fn powertoys_tokens(values: &Values, template: &str) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut output = String::with_capacity(template.len() + 16);
    let mut index = 0;

    while index < chars.len() {
        if chars[index] == '%' && index + 2 < chars.len() {
            let name: String = chars[index + 1..index + 3].iter().collect();
            let channel = match name.as_str() {
                "Re" => Some(values.rgb.r),
                "Gr" => Some(values.rgb.g),
                "Bl" => Some(values.rgb.b),
                "Al" => Some(255),
                _ => None,
            };

            if let Some(value) = channel {
                let style = chars
                    .get(index + 3)
                    .copied()
                    .filter(|c| matches!(c, 'b' | 'h' | 'H' | 'x' | 'X' | 'f' | 'F'));

                output.push_str(&channel_text(value, style));
                index += 3 + usize::from(style.is_some());
                continue;
            }

            if let Some(text) = values.parameter(&name) {
                output.push_str(&text);
                index += 3;
                continue;
            }
        }

        output.push(chars[index]);
        index += 1;
    }

    output
}

fn channel_text(value: u8, style: Option<char>) -> String {
    match style {
        Some('h') => format!("{:x}", value / 16),
        Some('H') => format!("{:X}", value / 16),
        Some('x') => format!("{value:02x}"),
        Some('X') => format!("{value:02X}"),
        Some('f') => decimals(f64::from(value) / 255.0),
        Some('F') => {
            let text = decimals(f64::from(value) / 255.0);

            text.strip_prefix('0')
                .filter(|rest| !rest.is_empty())
                .map_or(text.clone(), str::to_owned)
        }
        _ => value.to_string(),
    }
}

/// The `{hex}` style tokens older settings files use.
fn legacy_tokens(color: Rgb, template: &str) -> String {
    if !template.contains('{') {
        return template.to_owned();
    }

    let values = Values::new(color);
    let (_, hsl_s, _) = color.to_hsl();
    let saturation = if template.starts_with("hsl") {
        hsl_s.to_string()
    } else {
        whole(values.hsv_s * 100.0)
    };

    [
        ("{hex}", color.to_hex()),
        ("{r}", color.r.to_string()),
        ("{g}", color.g.to_string()),
        ("{b}", color.b.to_string()),
        ("{h}", whole(values.hue.round() % 360.0)),
        ("{s}", saturation),
        ("{l}", whole(values.hsl_l * 100.0)),
        ("{v}", whole(values.hsv_v * 100.0)),
        ("{c}", whole(values.cmyk[0] * 100.0)),
        ("{m}", whole(values.cmyk[1] * 100.0)),
        ("{y}", whole(values.cmyk[2] * 100.0)),
        ("{k}", whole(values.cmyk[3] * 100.0)),
    ]
    .into_iter()
    .fold(template.to_owned(), |text, (token, value)| {
        text.replace(token, &value)
    })
}

/// A whole number, rounded, never `-0`.
fn whole(value: f64) -> String {
    let rounded = value.round();

    format!("{}", if rounded == 0.0 { 0.0 } else { rounded })
}

/// Up to two decimals, no trailing zeros (`0.5`, `1`, `41.24`).
fn decimals(value: f64) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');

    if text.is_empty() || text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// NCol hue: a letter for the sixth of the color wheel and how far into it, such as `R53`.
fn natural_hue(hue: f64) -> String {
    let hue = hue.rem_euclid(360.0);
    let sector = (hue / 60.0).floor() as usize;
    let letter = ["R", "Y", "G", "C", "B", "M"][sector.min(5)];

    format!("{letter}{}", whole((hue % 60.0) / 60.0 * 100.0))
}

/// sRGB (D65) to CIE XYZ, scaled so white is `Y = 100`.
fn to_xyz(r: f64, g: f64, b: f64) -> [f64; 3] {
    let linear = |c: f64| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (linear(r), linear(g), linear(b));

    [
        (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) * 100.0,
        (0.2126729 * r + 0.7151522 * g + 0.0721750 * b) * 100.0,
        (0.0193339 * r + 0.1191920 * g + 0.9503041 * b) * 100.0,
    ]
}

/// CIE XYZ (D65) to CIE L*a*b*.
fn to_lab([x, y, z]: [f64; 3]) -> [f64; 3] {
    let f = |t: f64| {
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let (fx, fy, fz) = (f(x / 95.047), f(y / 100.0), f(z / 108.883));

    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

#[cfg(test)]
mod tests {
    use super::Rgb;

    const ORANGE: Rgb = Rgb::new(255, 136, 0);

    fn format(template: &str) -> String {
        ORANGE.format(template)
    }

    #[test]
    fn powertoys_default_formats() {
        assert_eq!(format("%ReX%GrX%BlX"), "FF8800");
        assert_eq!(format("rgb(%Re, %Gr, %Bl)"), "rgb(255, 136, 0)");
        assert_eq!(format("hsl(%Hu, %Sl%, %Ll%)"), "hsl(32, 100%, 50%)");
        assert_eq!(format("hsv(%Hu, %Sb%, %Va%)"), "hsv(32, 100%, 100%)");
        assert_eq!(format("hsb(%Hu, %Sb%, %Br%)"), "hsb(32, 100%, 100%)");
        assert_eq!(
            format("cmyk(%Cy%, %Ma%, %Ye%, %Bk%)"),
            "cmyk(0%, 47%, 100%, 0%)"
        );
        assert_eq!(format("hsi(%Hu, %Si%, %In%)"), "hsi(32, 100%, 51%)");
        assert_eq!(format("hwb(%Hu, %Wh%, %Bn%)"), "hwb(32, 0%, 0%)");
        assert_eq!(format("%Hn, %Wh%, %Bn%"), "R53, 0%, 0%");
        assert_eq!(format("%Dv"), "16746496");
        assert_eq!(format("0xFF%ReX%GrX%BlX"), "0xFFFF8800");
    }

    #[test]
    fn channel_styles() {
        assert_eq!(format("%Reb"), "255");
        assert_eq!(format("%Rex%Grx%Blx"), "ff8800");
        assert_eq!(format("%ReX"), "FF");
        assert_eq!(format("%Reh%Grh%Blh"), "f80");
        assert_eq!(format("%ReH%GrH%BlH"), "F80");
        assert_eq!(format("%Ref"), "1");
        assert_eq!(format("%Grf"), "0.53");
        assert_eq!(format("%GrF"), ".53");
        assert_eq!(format("%Blf"), "0");
        assert_eq!(format("%Al"), "255");
    }

    #[test]
    fn names_and_literals() {
        assert_eq!(format("%Na"), "Orange");
        assert_eq!(
            format("_'new Color (R = %Re, G = %Gr, B = %Bl)'_"),
            "_'new Color (R = 255, G = 136, B = 0)'_"
        );
        // A lone percent sign and unknown parameters are plain text.
        assert_eq!(format("100% sure %Zz"), "100% sure %Zz");
        assert_eq!(format("%"), "%");
        assert_eq!(format("%R"), "%R");
    }

    #[test]
    fn cie_values() {
        let red = Rgb::new(255, 0, 0);

        assert_eq!(red.format("XYZ(%Xv, %Yv, %Zv)"), "XYZ(41.25, 21.27, 1.93)");
        assert_eq!(red.format("CIELab(%Lc, %Ca, %Cb)"), "CIELab(53, 80, 67)");
        assert_eq!(Rgb::new(255, 255, 255).format("%Lc, %Ca, %Cb"), "100, 0, 0");
    }

    #[test]
    fn older_tokens_still_work() {
        assert_eq!(format("{hex}"), "#FF8800");
        assert_eq!(format("rgb({r}, {g}, {b})"), "rgb(255, 136, 0)");
        assert_eq!(format("hsl({h}, {s}%, {l}%)"), "hsl(32, 100%, 50%)");
        assert_eq!(format("hsv({h}, {s}%, {v}%)"), "hsv(32, 100%, 100%)");
        assert_eq!(
            format("cmyk({c}%, {m}%, {y}%, {k}%)"),
            "cmyk(0%, 47%, 100%, 0%)"
        );
    }

    #[test]
    fn hsv_round_trips() {
        for color in [
            Rgb::new(0, 0, 0),
            Rgb::new(255, 255, 255),
            Rgb::new(255, 136, 0),
            Rgb::new(12, 200, 99),
            Rgb::new(108, 140, 255),
            Rgb::new(1, 2, 3),
        ] {
            let (h, s, v) = color.to_hsv();

            assert_eq!(Rgb::from_hsv(h, s, v), color, "{color:?}");
        }

        assert_eq!(Rgb::from_hsv(0.0, 1.0, 1.0), Rgb::new(255, 0, 0));
        assert_eq!(Rgb::from_hsv(360.0 + 120.0, 1.0, 1.0), Rgb::new(0, 255, 0));
    }

    #[test]
    fn similar_colors_get_lighter_and_darker() {
        let base = Rgb::new(108, 140, 255);
        let (lighter, darker) = base.similar(6);

        assert_eq!(lighter.len(), 6);
        assert_eq!(darker.len(), 6);

        let lightness = |color: &Rgb| color.to_hsl().2;

        assert!(
            lighter
                .windows(2)
                .all(|pair| lightness(&pair[0]) <= lightness(&pair[1]))
        );
        assert!(
            darker
                .windows(2)
                .all(|pair| lightness(&pair[0]) >= lightness(&pair[1]))
        );
        assert!(lightness(&lighter[0]) >= lightness(&base));
        assert!(lightness(&darker[0]) <= lightness(&base));
    }

    #[test]
    fn color_names_are_high_level() {
        assert_eq!(Rgb::new(0, 0, 0).name(), "Black");
        assert_eq!(Rgb::new(255, 255, 255).name(), "White");
        assert_eq!(Rgb::new(0, 255, 0).name(), "Green");
        assert_eq!(Rgb::new(0, 80, 0).name(), "Dark Green");
        assert_eq!(Rgb::new(170, 255, 170).name(), "Light Green");
    }
}
