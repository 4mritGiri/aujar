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
}

impl Palette {
    pub fn from_settings(settings: &Settings) -> Self {
        let accent = settings.appearance.accent.rgb();

        match settings.appearance.theme {
            Theme::Dark => Self {
                card: 0x1c1c22f4,
                border: 0xffffff22,
                divider: 0xffffff14,
                text: 0xf2f2f7,
                text_dim: 0x9a9aa6,
                accent,
                on_accent: 0xffffff,
                on_accent_dim: 0xffffffbb,
                selection: (accent << 8) | 0x77,
                hover: 0xffffff12,
                subtle: 0xffffff0d,
                keycap_border: 0xffffff26,
                danger: 0xff6b81,
                switch_off: 0x4a4a55,
            },
            Theme::Light => Self {
                card: 0xf8f8fbf6,
                border: 0x00000022,
                divider: 0x00000014,
                text: 0x1d1d22,
                text_dim: 0x6c6c78,
                accent,
                on_accent: 0xffffff,
                on_accent_dim: 0xffffffcc,
                selection: (accent << 8) | 0x55,
                hover: 0x0000000d,
                subtle: 0x00000009,
                keycap_border: 0x00000026,
                danger: 0xd1264a,
                switch_off: 0xc4c4cc,
            },
        }
    }
}
