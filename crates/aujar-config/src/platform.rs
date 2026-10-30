//! Where Aujar keeps its files, on each operating system.
//!
//! Linux follows the XDG base directories (unchanged from before). macOS uses
//! `~/Library/Application Support/Aujar`; Windows uses `%APPDATA%\Aujar` for settings and
//! `%LOCALAPPDATA%\Aujar` for state. Every function has a `*_for` form taking the OS and an
//! environment lookup, so all three layouts are tested on any machine.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
}

impl Os {
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

fn real_env(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

fn var(env: Env<'_>, name: &str) -> Option<PathBuf> {
    env(name)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// An absolute path from `name`, as the XDG specification requires.
fn absolute(env: Env<'_>, name: &str) -> Option<PathBuf> {
    var(env, name).filter(|path| path.is_absolute())
}

/// The user's home directory.
pub fn home_dir_for(os: Os, env: Env<'_>) -> Option<PathBuf> {
    match os {
        Os::Windows => var(env, "USERPROFILE"),
        Os::Linux | Os::MacOs => var(env, "HOME"),
    }
}

pub fn home_dir() -> Option<PathBuf> {
    home_dir_for(Os::current(), &real_env)
}

/// Directory for settings and other things the user may edit.
pub fn config_dir_for(os: Os, env: Env<'_>) -> PathBuf {
    let base = match os {
        Os::Linux => absolute(env, "XDG_CONFIG_HOME")
            .or_else(|| home_dir_for(os, env).map(|home| home.join(".config"))),
        Os::MacOs => home_dir_for(os, env).map(|home| home.join("Library/Application Support")),
        Os::Windows => var(env, "APPDATA")
            .or_else(|| home_dir_for(os, env).map(|home| home.join("AppData").join("Roaming"))),
    };

    base.unwrap_or_else(std::env::temp_dir)
        .join(app_dir_name(os))
}

/// Directory for state the program writes (history, audit log).
pub fn state_dir_for(os: Os, env: Env<'_>) -> PathBuf {
    let base = match os {
        Os::Linux => absolute(env, "XDG_STATE_HOME")
            .or_else(|| home_dir_for(os, env).map(|home| home.join(".local/state"))),
        Os::MacOs => home_dir_for(os, env).map(|home| home.join("Library/Application Support")),
        Os::Windows => var(env, "LOCALAPPDATA")
            .or_else(|| home_dir_for(os, env).map(|home| home.join("AppData").join("Local"))),
    };

    base.unwrap_or_else(std::env::temp_dir)
        .join(app_dir_name(os))
}

/// Where received files go unless configured otherwise: `Downloads/Aujar` in the home
/// directory, on every platform.
pub fn downloads_dir_for(os: Os, env: Env<'_>) -> Option<PathBuf> {
    home_dir_for(os, env).map(|home| home.join("Downloads").join("Aujar"))
}

pub fn config_dir() -> PathBuf {
    config_dir_for(Os::current(), &real_env)
}

pub fn state_dir() -> PathBuf {
    state_dir_for(Os::current(), &real_env)
}

pub fn downloads_dir() -> Option<PathBuf> {
    downloads_dir_for(Os::current(), &real_env)
}

/// The per-user directory for sockets and other short-lived files.
pub fn runtime_dir_for(os: Os, env: Env<'_>) -> PathBuf {
    match os {
        Os::Linux => absolute(env, "XDG_RUNTIME_DIR"),
        // macOS gives every user a private temporary directory in `$TMPDIR`.
        Os::MacOs => absolute(env, "TMPDIR"),
        Os::Windows => var(env, "LOCALAPPDATA"),
    }
    .unwrap_or_else(std::env::temp_dir)
}

pub fn runtime_dir() -> PathBuf {
    runtime_dir_for(Os::current(), &real_env)
}

fn app_dir_name(os: Os) -> &'static str {
    match os {
        Os::Linux => "aujar",
        Os::MacOs | Os::Windows => "Aujar",
    }
}

/// `~` or `~/rest` expanded against `home`; other paths are returned unchanged.
pub fn expand_home(path: &Path, home: Option<&Path>) -> Option<PathBuf> {
    match path.strip_prefix("~") {
        Ok(rest) => home.map(|home| home.join(rest)),
        Err(_) => Some(path.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let pairs = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<Vec<_>>();

        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn linux_follows_xdg_and_falls_back_to_home() {
        let base = std::env::temp_dir().join("aujar-platform-tests");
        let xdg_root = base.join("config");
        let home = base.join("home");
        let xdg_root_text = xdg_root.to_string_lossy();
        let home_text = home.to_string_lossy();
        let xdg = env(&[("XDG_CONFIG_HOME", &xdg_root_text), ("HOME", &home_text)]);
        let plain = env(&[("HOME", &home_text)]);

        assert_eq!(config_dir_for(Os::Linux, &xdg), xdg_root.join("aujar"));
        assert_eq!(
            config_dir_for(Os::Linux, &plain),
            home.join(".config/aujar")
        );
        assert_eq!(
            state_dir_for(Os::Linux, &plain),
            home.join(".local/state/aujar")
        );
    }

    #[test]
    fn linux_ignores_relative_xdg_values() {
        let home = std::env::temp_dir().join("aujar-platform-tests/home");
        let home_text = home.to_string_lossy();
        let relative = env(&[("XDG_CONFIG_HOME", "relative/dir"), ("HOME", &home_text)]);

        assert_eq!(
            config_dir_for(Os::Linux, &relative),
            home.join(".config/aujar")
        );
    }

    #[test]
    fn macos_uses_application_support() {
        let home = std::env::temp_dir().join("aujar-platform-tests/home");
        let home_text = home.to_string_lossy();
        let mac = env(&[("HOME", &home_text)]);

        assert_eq!(
            config_dir_for(Os::MacOs, &mac),
            home.join("Library/Application Support/Aujar")
        );
        assert_eq!(
            config_dir_for(Os::MacOs, &mac),
            state_dir_for(Os::MacOs, &mac)
        );
        assert_eq!(
            downloads_dir_for(Os::MacOs, &mac),
            Some(home.join("Downloads/Aujar"))
        );
        // Linux keeps the capitalised downloads folder name it always had.
        let linux_home = std::env::temp_dir().join("aujar-platform-tests/linux-home");
        let linux_home_text = linux_home.to_string_lossy();
        assert_eq!(
            downloads_dir_for(Os::Linux, &env(&[("HOME", &linux_home_text)])),
            Some(linux_home.join("Downloads/Aujar"))
        );
    }

    #[test]
    fn windows_uses_appdata_and_localappdata() {
        let base = std::env::temp_dir().join("aujar-platform-tests");
        let roaming = base.join("Roaming");
        let local = base.join("Local");
        let home = base.join("home");
        let roaming_text = roaming.to_string_lossy();
        let local_text = local.to_string_lossy();
        let home_text = home.to_string_lossy();
        let win = env(&[
            ("APPDATA", &roaming_text),
            ("LOCALAPPDATA", &local_text),
            ("USERPROFILE", &home_text),
        ]);

        assert_eq!(config_dir_for(Os::Windows, &win), roaming.join("Aujar"));
        assert_eq!(state_dir_for(Os::Windows, &win), local.join("Aujar"));
        assert_eq!(
            downloads_dir_for(Os::Windows, &win),
            Some(home.join("Downloads/Aujar"))
        );
        assert_eq!(home_dir_for(Os::Windows, &win), Some(home));
    }

    #[test]
    fn missing_environment_falls_back_to_the_temp_dir_not_a_panic() {
        let nothing = env(&[]);

        for os in [Os::Linux, Os::MacOs, Os::Windows] {
            assert!(config_dir_for(os, &nothing).starts_with(std::env::temp_dir()));
            assert_eq!(downloads_dir_for(os, &nothing), None);
        }
    }

    #[test]
    fn runtime_dirs() {
        let base = std::env::temp_dir().join("aujar-platform-tests");
        let linux_runtime = base.join("linux-runtime");
        let mac_runtime = base.join("mac-runtime");
        let linux_runtime_text = linux_runtime.to_string_lossy();
        let mac_runtime_text = mac_runtime.to_string_lossy();
        let linux = env(&[("XDG_RUNTIME_DIR", &linux_runtime_text)]);
        let mac = env(&[("TMPDIR", &mac_runtime_text)]);

        assert_eq!(runtime_dir_for(Os::Linux, &linux), linux_runtime);
        assert_eq!(runtime_dir_for(Os::MacOs, &mac), mac_runtime);
        assert_eq!(runtime_dir_for(Os::Linux, &env(&[])), std::env::temp_dir());
    }

    #[test]
    fn home_expansion() {
        let home = std::env::temp_dir().join("aujar-platform-tests/home");

        assert_eq!(
            expand_home(Path::new("~/Dl"), Some(&home)),
            Some(home.join("Dl"))
        );
        assert_eq!(expand_home(&home, None), Some(home.clone()));
        assert_eq!(expand_home(Path::new("~/x"), None), None);
    }

    #[test]
    fn current_os_matches_the_build_target() {
        let expected = if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        };

        assert_eq!(Os::current(), expected);
    }
}
