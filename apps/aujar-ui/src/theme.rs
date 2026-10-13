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
}

impl Palette {
    pub fn from_settings(settings: &Settings) -> Self {
        let accent = settings.appearance.accent.rgb();

        match settings.appearance.theme {
            Theme::Dark => Self {
                card: 0x1e1f26f6,
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
                menu: 0x2a2b33fa,
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
            },
        }
    }
}
