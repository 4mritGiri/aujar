use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub socket_path: Option<PathBuf>,
    pub launcher_hotkey: String,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            socket_path: None,
            launcher_hotkey: "Alt+Space".into(),
        }
    }
}

pub fn config_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()))
        .join("aujar/config.toml")
}
pub fn load() -> Result<Config> {
    let p = config_path();
    if !p.exists() {
        return Ok(Config::default());
    };
    Ok(toml::from_str(&fs::read_to_string(p)?)?)
}
