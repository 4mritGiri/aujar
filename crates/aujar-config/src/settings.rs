//! User-facing settings, shared by the daemon (which providers are on) and the UI
//! (look and feel). Stored as TOML in `~/.config/aujar/settings.toml`.
//!
//! Every field has a default, so a missing file or missing keys are fine.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    Light,
}

/// Where the launcher appears when it opens. Wayland compositors choose the position of a
/// new window themselves, so this is honoured on X11 and macOS-style setups only; dragging
/// the window works everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LauncherPosition {
    /// Centered on the screen.
    Center,
    /// Horizontally centered, in the upper part of the screen (like Spotlight).
    Top,
    /// Wherever it was when you last moved it.
    Remember,
}

impl LauncherPosition {
    pub const ALL: [LauncherPosition; 3] = [
        LauncherPosition::Top,
        LauncherPosition::Center,
        LauncherPosition::Remember,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            LauncherPosition::Center => "Center",
            LauncherPosition::Top => "Top",
            LauncherPosition::Remember => "Last place",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    Blue,
    Purple,
    Pink,
    Orange,
    Green,
    Teal,
}

impl Accent {
    pub const ALL: [Accent; 6] = [
        Accent::Blue,
        Accent::Purple,
        Accent::Pink,
        Accent::Orange,
        Accent::Green,
        Accent::Teal,
    ];

    /// `0xRRGGBB`.
    pub const fn rgb(self) -> u32 {
        match self {
            Accent::Blue => 0x0a64d8,
            Accent::Purple => 0x7c4dff,
            Accent::Pink => 0xd6409f,
            Accent::Orange => 0xe5732d,
            Accent::Green => 0x2f9e5b,
            Accent::Teal => 0x13a3a8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    DuckDuckGo,
    Google,
    Brave,
    Bing,
    Startpage,
}

impl Engine {
    pub const ALL: [Engine; 5] = [
        Engine::DuckDuckGo,
        Engine::Google,
        Engine::Brave,
        Engine::Bing,
        Engine::Startpage,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Engine::DuckDuckGo => "DuckDuckGo",
            Engine::Google => "Google",
            Engine::Brave => "Brave",
            Engine::Bing => "Bing",
            Engine::Startpage => "Startpage",
        }
    }

    /// URL prefix; the percent-encoded query is appended.
    pub const fn url(self) -> &'static str {
        match self {
            Engine::DuckDuckGo => "https://duckduckgo.com/?q=",
            Engine::Google => "https://www.google.com/search?q=",
            Engine::Brave => "https://search.brave.com/search?q=",
            Engine::Bing => "https://www.bing.com/search?q=",
            Engine::Startpage => "https://www.startpage.com/do/search?q=",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: Theme,
    pub accent: Accent,
    /// Launcher width in pixels.
    pub width: u32,
    /// Maximum application results shown (other sections are capped lower).
    pub max_results: u32,
    pub show_icons: bool,
    /// Keyboard-hint bar at the bottom of the launcher.
    pub show_footer: bool,
    /// Details panel on the right showing the selected result.
    pub show_preview: bool,
    /// Recent and frequently used items when the search box is empty.
    pub show_recents: bool,
    /// Hide the launcher when it loses focus (click elsewhere).
    pub close_on_blur: bool,
    /// Window opacity in percent (both windows). Lower values let the desktop show through.
    pub opacity: u32,
    /// Corner radius of the windows, in pixels.
    pub corner_radius: u32,
    /// Ask the compositor to blur what is behind the windows, where it supports that.
    pub blur: bool,
    /// Draw macOS-style close / minimize / zoom buttons in the settings window.
    pub traffic_lights: bool,
    /// The launcher can be moved by dragging its search bar.
    pub launcher_movable: bool,
    /// Where the launcher opens.
    pub launcher_position: LauncherPosition,
    /// Last launcher position (`x`, `y` in pixels), saved when you move it. Used when
    /// `launcher_position = "remember"`.
    pub launcher_origin: Option<(i32, i32)>,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            accent: Accent::Blue,
            width: 720,
            max_results: 6,
            show_icons: true,
            show_footer: true,
            show_preview: true,
            show_recents: true,
            close_on_blur: true,
            opacity: 96,
            corner_radius: 16,
            blur: false,
            traffic_lights: true,
            launcher_movable: true,
            launcher_position: LauncherPosition::Top,
            launcher_origin: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Plugins {
    pub applications: bool,
    pub calculator: bool,
    pub units: bool,
    /// Currency conversion. Downloads exchange rates (via `curl`) about twice a day,
    /// so it is off by default.
    pub currency: bool,
    /// Clipboard history (`clip` / `cb` in the launcher). Kept in memory only. Records
    /// everything you copy, so it is off by default.
    pub clipboard: bool,
    /// Files and folders under `files.roots`.
    pub files: bool,
    /// Lock, sleep, restart, shut down, log out, system settings.
    pub system: bool,
    /// `> command` runs a shell command. Off by default: it runs arbitrary commands.
    pub shell: bool,
    /// `define word`. Only the word you typed is sent, to dictionaryapi.dev.
    pub dictionary: bool,
    /// `weather city`. Only the place you typed is sent, to Open-Meteo.
    pub weather: bool,
    /// `stock aapl`. Only the ticker you typed is sent, to Yahoo Finance.
    pub stocks: bool,
    /// `nepse nabil`. Only the symbol is sent, to the Fintech Nepal NEPSE API (needs a key).
    pub nepse: bool,
    /// `nba`, `premier league`. Only the league is sent, to ESPN.
    pub sports: bool,
    /// `< name` switches to an open window. Needs a backend for your session (GNOME
    /// extension, Sway, Hyprland or X11 with wmctrl).
    pub windows: bool,
}

impl Default for Plugins {
    fn default() -> Self {
        Self {
            applications: true,
            calculator: true,
            units: true,
            currency: false,
            clipboard: false,
            files: true,
            system: true,
            shell: false,
            dictionary: true,
            weather: true,
            stocks: true,
            nepse: true,
            sports: true,
            windows: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Weather {
    /// Fahrenheit and mph instead of Celsius and km/h.
    pub imperial: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Files {
    /// Directories to index. `~` is expanded.
    pub roots: Vec<String>,
    pub max_depth: u32,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            roots: vec!["~".to_owned()],
            max_depth: 4,
        }
    }
}

/// `keyword query` opens `url` with the query appended (PowerToys-style web shortcuts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebShortcut {
    pub keyword: String,
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Web {
    pub engine: Engine,
    pub shortcuts: Vec<WebShortcut>,
}

impl Default for Web {
    fn default() -> Self {
        let shortcut = |keyword: &str, name: &str, url: &str| WebShortcut {
            keyword: keyword.to_owned(),
            name: name.to_owned(),
            url: url.to_owned(),
        };

        Self {
            engine: Engine::DuckDuckGo,
            shortcuts: vec![
                shortcut("g", "Google", "https://www.google.com/search?q="),
                shortcut(
                    "yt",
                    "YouTube",
                    "https://www.youtube.com/results?search_query=",
                ),
                shortcut("gh", "GitHub", "https://github.com/search?q="),
                shortcut(
                    "w",
                    "Wikipedia",
                    "https://en.wikipedia.org/w/index.php?search=",
                ),
                shortcut(
                    "maps",
                    "OpenStreetMap",
                    "https://www.openstreetmap.org/search?query=",
                ),
            ],
        }
    }
}

/// How machines are arranged on the desk: one row of up to four, or a 2x2 matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatrixMode {
    Row,
    Grid,
}

/// When moving the pointer past a screen edge hands control to the neighbouring machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EasyMouse {
    /// Only the keyboard shortcuts switch machines.
    Disabled,
    /// Crossing an edge switches.
    Enabled,
    /// Crossing an edge switches only while Shift is held.
    RequireShift,
    /// Crossing an edge switches only while Ctrl is held.
    RequireCtrl,
}

/// Which keys jump straight to a machine (`Ctrl+Alt+<key>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwitchKeys {
    /// `1` to `4`. The default: nothing else binds these with Ctrl+Alt.
    Digits,
    /// `F1` to `F4`. Linux reserves Ctrl+Alt+F1..F7 for virtual terminals, so the desktop
    /// switches away while you are not controlling another machine.
    FunctionKeys,
}

/// One machine in the layout. `slot` is its place on the desk, `0..=3`: left to right in
/// `row` mode; row-major (`0 1 / 2 3`) in `grid` mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    pub name: String,
    pub slot: u8,
    /// `host` or `host:port` to dial. Empty means resolve `name` through the system
    /// resolver. Machines listed without an address still connect when they dial us.
    #[serde(default)]
    pub address: String,
}

/// Letters used with Ctrl+Alt. Physical (QWERTY) key positions, independent of layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseBordersHotkeys {
    pub switch_keys: SwitchKeys,
    pub toggle_easy_mouse: String,
    pub lock_machines: String,
    pub reconnect: String,
    pub multi_machine_input: String,
}

impl Default for MouseBordersHotkeys {
    fn default() -> Self {
        Self {
            switch_keys: SwitchKeys::Digits,
            toggle_easy_mouse: "e".to_owned(),
            lock_machines: "l".to_owned(),
            reconnect: "r".to_owned(),
            multi_machine_input: "m".to_owned(),
        }
    }
}

/// Share one keyboard and mouse (and clipboard and files) between up to four Linux machines
/// running Aujar. Off by default: enabling it opens a network listener.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseBorders {
    pub enabled: bool,
    /// This machine's name in the layout. Empty means the hostname.
    pub machine_name: String,
    pub port: u16,
    pub matrix: MatrixMode,
    /// Every machine including this one. Identical on all machines, so one file can be
    /// copied around.
    pub machines: Vec<Machine>,
    /// Continue from the last machine back to the first (per row/column).
    pub wrap_mouse: bool,
    pub easy_mouse: EasyMouse,
    /// Ignore edge crossings that start in a screen corner.
    pub block_corners: bool,
    /// Hide the local pointer while another machine is controlled (X11 only).
    pub hide_mouse_at_edge: bool,
    /// Send relative motion instead of absolute positions: for machines with different
    /// resolutions or several monitors.
    pub move_relatively: bool,
    /// Refuse peers that are not on a subnet of one of this machine's interfaces.
    pub same_subnet_only: bool,
    pub share_clipboard: bool,
    pub transfer_files: bool,
    /// Largest file accepted, in MiB (hard cap 100).
    pub max_file_mb: u32,
    /// Where received files go. Empty means `~/Downloads/Aujar`.
    pub receive_dir: String,
    /// Keep every machine awake while sharing (no screen saver or idle lock).
    pub block_screensaver: bool,
    /// Let a connected machine lock this one (`Ctrl+Alt+L` on any of them).
    pub allow_remote_lock: bool,
    /// Screen size override, `WIDTHxHEIGHT`, when it cannot be detected (Wayland).
    pub screen: String,
    pub hotkeys: MouseBordersHotkeys,
}

impl MouseBorders {
    pub const DEFAULT_PORT: u16 = 15101;
    pub const MAX_FILE_MB: u32 = 100;
}

impl Default for MouseBorders {
    fn default() -> Self {
        Self {
            enabled: false,
            machine_name: String::new(),
            port: Self::DEFAULT_PORT,
            matrix: MatrixMode::Row,
            machines: Vec::new(),
            wrap_mouse: false,
            easy_mouse: EasyMouse::Enabled,
            block_corners: false,
            hide_mouse_at_edge: false,
            move_relatively: false,
            same_subnet_only: true,
            share_clipboard: true,
            transfer_files: true,
            max_file_mb: Self::MAX_FILE_MB,
            receive_dir: String::new(),
            block_screensaver: true,
            allow_remote_lock: true,
            screen: String::new(),
            hotkeys: MouseBordersHotkeys::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub appearance: Appearance,
    pub plugins: Plugins,
    pub files: Files,
    pub web: Web,
    pub weather: Weather,
    pub mouse_borders: MouseBorders,
}

impl Settings {
    /// `$XDG_CONFIG_HOME/aujar/settings.toml`, else `~/.config/aujar/settings.toml`.
    pub fn path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(|| PathBuf::from("/tmp"));

        base.join("aujar").join("settings.toml")
    }

    /// Parse a settings file. A missing file gives the defaults.
    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        let loaded: anyhow::Result<Self> = match fs::read_to_string(path) {
            Ok(content) => Ok(toml::from_str(&content)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        };

        loaded.map(Self::sanitized)
    }

    /// Never fails: a broken file falls back to the defaults.
    pub fn load_or_default() -> Self {
        Self::load_from(&Self::path()).unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Write-then-rename so a crash never leaves a half-written file.
        let temp = path.with_extension("toml.tmp");
        fs::write(&temp, toml::to_string_pretty(self)?)?;
        fs::rename(&temp, &path)?;

        Ok(())
    }

    /// Clamp values into ranges the UI can render.
    pub fn sanitized(mut self) -> Self {
        self.appearance.width = self.appearance.width.clamp(480, 1000);
        self.appearance.max_results = self.appearance.max_results.clamp(3, 12);
        self.appearance.opacity = self.appearance.opacity.clamp(60, 100);
        self.appearance.corner_radius = self.appearance.corner_radius.clamp(0, 28);
        self.files.max_depth = self.files.max_depth.clamp(1, 8);

        let mb = &mut self.mouse_borders;
        if mb.port == 0 {
            mb.port = MouseBorders::DEFAULT_PORT;
        }
        mb.max_file_mb = mb.max_file_mb.clamp(1, MouseBorders::MAX_FILE_MB);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        let settings: Settings = toml::from_str("").unwrap();

        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let settings: Settings = toml::from_str("[plugins]\nshell = true\n").unwrap();

        assert!(settings.plugins.shell);
        assert!(settings.plugins.files);
        assert_eq!(settings.appearance, Appearance::default());
    }

    #[test]
    fn round_trips_through_toml() {
        let mut settings = Settings::default();
        settings.appearance.theme = Theme::Light;
        settings.appearance.accent = Accent::Pink;
        settings.web.engine = Engine::Brave;

        let text = toml::to_string_pretty(&settings).unwrap();

        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
    }

    #[test]
    fn new_plugins_default_on_and_old_files_still_load() {
        // A settings file written before these options existed.
        let settings: Settings = toml::from_str("[plugins]\nfiles = false\n").unwrap();

        assert!(settings.plugins.dictionary && settings.plugins.windows);
        assert!(!settings.plugins.files);
        assert!(!settings.weather.imperial);
    }

    #[test]
    fn sanitized_clamps() {
        let mut settings = Settings::default();
        settings.appearance.width = 5;
        settings.appearance.max_results = 500;

        let settings = settings.sanitized();

        assert_eq!(settings.appearance.width, 480);
        assert_eq!(settings.appearance.max_results, 12);
    }

    #[test]
    fn sanitized_clamps_window_look() {
        let mut settings = Settings::default();
        settings.appearance.opacity = 3;
        settings.appearance.corner_radius = 400;

        let settings = settings.sanitized();

        assert_eq!(settings.appearance.opacity, 60);
        assert_eq!(settings.appearance.corner_radius, 28);
    }

    #[test]
    fn old_appearance_files_get_the_new_window_defaults() {
        let settings: Settings = toml::from_str("[appearance]\nwidth = 600\n").unwrap();

        assert_eq!(settings.appearance.width, 600);
        assert!(settings.appearance.launcher_movable);
        assert!(settings.appearance.traffic_lights);
        assert_eq!(settings.appearance.launcher_position, LauncherPosition::Top);
        assert_eq!(settings.appearance.launcher_origin, None);
    }

    #[test]
    fn launcher_origin_round_trips() {
        let mut settings = Settings::default();
        settings.appearance.launcher_position = LauncherPosition::Remember;
        settings.appearance.launcher_origin = Some((-40, 220));

        let text = toml::to_string_pretty(&settings).unwrap();

        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
    }

    #[test]
    fn mouse_borders_is_off_and_subnet_restricted_by_default() {
        let settings = Settings::default().mouse_borders;

        assert!(!settings.enabled);
        assert!(settings.same_subnet_only);
        assert_eq!(settings.port, MouseBorders::DEFAULT_PORT);
    }

    #[test]
    fn mouse_borders_parses_a_layout() {
        let text = r#"
            [mouse_borders]
            enabled = true
            matrix = "grid"
            easy_mouse = "require_ctrl"

            [[mouse_borders.machines]]
            name = "desk"
            slot = 0

            [[mouse_borders.machines]]
            name = "laptop"
            slot = 1
            address = "192.168.1.20:15101"
        "#;
        let settings: Settings = toml::from_str(text).unwrap();
        let mb = settings.mouse_borders;

        assert!(mb.enabled);
        assert_eq!(mb.matrix, MatrixMode::Grid);
        assert_eq!(mb.easy_mouse, EasyMouse::RequireCtrl);
        assert_eq!(mb.machines.len(), 2);
        assert_eq!(mb.machines[0].address, "");
        assert_eq!(mb.machines[1].slot, 1);
    }

    #[test]
    fn sanitized_bounds_mouse_borders_values() {
        let mut settings = Settings::default();
        settings.mouse_borders.port = 0;
        settings.mouse_borders.max_file_mb = 5000;

        let settings = settings.sanitized();

        assert_eq!(settings.mouse_borders.port, MouseBorders::DEFAULT_PORT);
        assert_eq!(settings.mouse_borders.max_file_mb, 100);
    }
}
