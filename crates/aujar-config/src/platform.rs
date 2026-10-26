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

    fn env(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs = pairs.to_vec();

        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn linux_follows_xdg_and_falls_back_to_home() {
        let xdg = env(&[("XDG_CONFIG_HOME", "/cfg"), ("HOME", "/home/me")]);
        let plain = env(&[("HOME", "/home/me")]);

        assert_eq!(config_dir_for(Os::Linux, &xdg), Path::new("/cfg/aujar"));
        assert_eq!(
            config_dir_for(Os::Linux, &plain),
            Path::new("/home/me/.config/aujar")
        );
        assert_eq!(
            state_dir_for(Os::Linux, &plain),
            Path::new("/home/me/.local/state/aujar")
        );
    }

    #[test]
    fn linux_ignores_relative_xdg_values() {
        let relative = env(&[("XDG_CONFIG_HOME", "relative/dir"), ("HOME", "/home/me")]);

        assert_eq!(
            config_dir_for(Os::Linux, &relative),
            Path::new("/home/me/.config/aujar")
        );
    }

    #[test]
    fn macos_uses_application_support() {
        let mac = env(&[("HOME", "/Users/me")]);

        assert_eq!(
            config_dir_for(Os::MacOs, &mac),
            Path::new("/Users/me/Library/Application Support/Aujar")
        );
        assert_eq!(
            config_dir_for(Os::MacOs, &mac),
            state_dir_for(Os::MacOs, &mac)
        );
        assert_eq!(
            downloads_dir_for(Os::MacOs, &mac),
            Some(PathBuf::from("/Users/me/Downloads/Aujar"))
        );
        // Linux keeps the capitalised folder name it always had.
        assert_eq!(
            downloads_dir_for(Os::Linux, &env(&[("HOME", "/home/me")])),
            Some(PathBuf::from("/home/me/Downloads/Aujar"))
        );
    }

    #[test]
    fn windows_uses_appdata_and_localappdata() {
        let win = env(&[
            ("APPDATA", r"C:\Users\me\AppData\Roaming"),
            ("LOCALAPPDATA", r"C:\Users\me\AppData\Local"),
            ("USERPROFILE", r"C:\Users\me"),
        ]);

        assert_eq!(
            config_dir_for(Os::Windows, &win),
            Path::new(r"C:\Users\me\AppData\Roaming").join("Aujar")
        );
        assert_eq!(
            state_dir_for(Os::Windows, &win),
            Path::new(r"C:\Users\me\AppData\Local").join("Aujar")
        );
        assert_eq!(
            downloads_dir_for(Os::Windows, &win),
            Some(Path::new(r"C:\Users\me").join("Downloads").join("Aujar"))
        );
        assert_eq!(
            home_dir_for(Os::Windows, &win),
            Some(PathBuf::from(r"C:\Users\me"))
        );
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
        let linux = env(&[("XDG_RUNTIME_DIR", "/run/user/1000")]);
        let mac = env(&[("TMPDIR", "/var/folders/ab/T/")]);

        assert_eq!(
            runtime_dir_for(Os::Linux, &linux),
            Path::new("/run/user/1000")
        );
        assert_eq!(
            runtime_dir_for(Os::MacOs, &mac),
            Path::new("/var/folders/ab/T/")
        );
        assert_eq!(runtime_dir_for(Os::Linux, &env(&[])), std::env::temp_dir());
    }

    #[test]
    fn home_expansion() {
        let home = Path::new("/home/me");

        assert_eq!(
            expand_home(Path::new("~/Dl"), Some(home)),
            Some(PathBuf::from("/home/me/Dl"))
        );
        assert_eq!(
            expand_home(Path::new("/abs"), None),
            Some(PathBuf::from("/abs"))
        );
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
