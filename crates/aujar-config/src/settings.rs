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
                shortcut("yt", "YouTube", "https://www.youtube.com/results?search_query="),
                shortcut("gh", "GitHub", "https://github.com/search?q="),
                shortcut("w", "Wikipedia", "https://en.wikipedia.org/w/index.php?search="),
                shortcut("maps", "OpenStreetMap", "https://www.openstreetmap.org/search?query="),
            ],
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
        self.files.max_depth = self.files.max_depth.clamp(1, 8);
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
}
