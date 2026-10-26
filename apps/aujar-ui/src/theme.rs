//! Colors for the launcher and the settings window, derived from the user's settings.

use aujar_config::{Settings, Theme};

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Card background, `0xRRGGBBAA`.
    pub card: u32,
    pub border: u32,
    pub divider: u32,
    pub text: u32,
    pub text_dim: u32,
    /// Fainter than `text_dim`: section titles and trailing labels.
    pub text_faint: u32,
    /// Selected-row background.
    pub accent: u32,
    pub on_accent: u32,
    pub on_accent_dim: u32,
    /// Highlight behind selected text in the search field, `0xRRGGBBAA`.
    pub selection: u32,
    pub hover: u32,
    /// Footer bar and keycap background.
    pub subtle: u32,
    pub keycap_border: u32,
    pub danger: u32,
    pub switch_off: u32,
    /// Price up / score win.
    pub positive: u32,
    /// Price down.
    pub negative: u32,
    /// Context menu background (opaque enough to read over any result).
    pub menu: u32,
    /// Settings sidebar background, `0xRRGGBBAA`.
    pub sidebar: u32,
    /// Selected sidebar item background: the accent, softly tinted, `0xRRGGBBAA`.
    pub accent_soft: u32,
    /// Pill in the settings title bar, `0xRRGGBBAA`.
    pub pill: u32,
    /// Accent lightened enough to read as an icon or label on a dark surface.
    pub accent_fg: u32,
    /// Window corner radius in pixels.
    pub radius: f32,
}

/// Replace the alpha byte of a `0xRRGGBBAA` color.
const fn with_alpha(color: u32, alpha: u8) -> u32 {
    (color & 0xffff_ff00) | alpha as u32
}

/// Blend `color` (`0xRRGGBB`) toward white by `percent` (0..=100).
const fn mix_white(color: u32, percent: u32) -> u32 {
    const fn channel(color: u32, shift: u32, percent: u32) -> u32 {
        let value = (color >> shift) & 0xff;

        value + (255 - value) * percent / 100
    }

    (channel(color, 16, percent) << 16)
        | (channel(color, 8, percent) << 8)
        | channel(color, 0, percent)
}

/// Give any styled element the same corner radius, in pixels, on all four corners (GPUI only
/// ships fixed `rounded_*` sizes).
pub fn rounded_px<E: gpui::Styled>(mut element: E, radius: f32) -> E {
    let radius = gpui::AbsoluteLength::Pixels(gpui::px(radius));
    let radii = &mut element.style().corner_radii;

    radii.top_left = Some(radius);
    radii.top_right = Some(radius);
    radii.bottom_right = Some(radius);
    radii.bottom_left = Some(radius);

    element
}

/// Scale the alpha byte of a `0xRRGGBBAA` color by `percent` (0..=100).
const fn scale_alpha(color: u32, percent: u32) -> u32 {
    let alpha = (color & 0xff) * percent / 100;

    with_alpha(color, alpha as u8)
}

impl Palette {
    pub fn from_settings(settings: &Settings) -> Self {
        let accent = settings.appearance.accent.rgb();
        let opacity = settings.appearance.opacity.clamp(60, 100);
        let radius = settings.appearance.corner_radius.min(28) as f32;

        let mut palette = match settings.appearance.theme {
            Theme::Dark => Self {
                card: 0x14172af6,
                border: 0xffffff1c,
                divider: 0xffffff12,
                text: 0xf5f5f7,
                text_dim: 0xa1a1aa,
                text_faint: 0x71717a,
                accent,
                on_accent: 0xffffff,
                on_accent_dim: 0xffffffc4,
                selection: (accent << 8) | 0x77,
                hover: 0xffffff10,
                subtle: 0xffffff0a,
                keycap_border: 0xffffff22,
                danger: 0xff6b81,
                switch_off: 0x4a4a55,
                positive: 0x34d27b,
                negative: 0xff6b6b,
                menu: 0x23273bfa,
                sidebar: 0xffffff08,
                accent_soft: (accent << 8) | 0x38,
                pill: 0xffffff0d,
                accent_fg: mix_white(accent, 40),
                radius,
            },
            Theme::Light => Self {
                card: 0xfafafcf8,
                border: 0x0000001f,
                divider: 0x00000012,
                text: 0x1c1c21,
                text_dim: 0x62626c,
                text_faint: 0x8e8e98,
                accent,
                on_accent: 0xffffff,
                on_accent_dim: 0xffffffd0,
                selection: (accent << 8) | 0x55,
                hover: 0x0000000b,
                subtle: 0x00000008,
                keycap_border: 0x00000022,
                danger: 0xd1264a,
                switch_off: 0xc4c4cc,
                positive: 0x1a9d57,
                negative: 0xd93636,
                menu: 0xffffffff,
                sidebar: 0x00000007,
                accent_soft: (accent << 8) | 0x26,
                pill: 0x0000000a,
                accent_fg: accent,
                radius,
            },
        };

        // Opacity only affects the big translucent surfaces; text and controls stay crisp.
        palette.card = scale_alpha(palette.card, opacity);
        palette.menu = scale_alpha(palette.menu, (opacity + 4).min(100));

        palette
    }
}
