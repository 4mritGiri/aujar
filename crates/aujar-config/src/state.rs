//! State the daemon keeps across restarts: what it was doing when it last ran, so a restart
//! (or a crash, or a reboot) does not silently undo it. Stored in `state.toml` in the state
//! directory (`~/.local/state/aujar/` on Linux), readable only by you.
//!
//! Writing is crash-safe: the new content goes to a temporary file first and replaces the old
//! one in a single rename, so a crash leaves either the old or the new file, never half of one.
//! Reading never fails: a missing file is the default state, and a file that cannot be read is
//! moved aside (as `state.toml.bad`) and replaced by the default, so a damaged file cannot stop
//! the daemon from starting.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// What Awake (keeping the computer from sleeping) was doing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AwakeMode {
    /// Not keeping the computer awake.
    #[default]
    Off,
    /// Keeping it awake until told otherwise.
    Indefinite,
    /// Keeping it awake until `until`.
    Timed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AwakeState {
    pub mode: AwakeMode,
    /// Unix seconds at which a [`AwakeMode::Timed`] mode ends. Unused otherwise.
    pub until: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub awake: AwakeState,
}

impl State {
    /// `$XDG_STATE_HOME/aujar/state.toml` on Linux (else `~/.local/state/aujar/`), and the
    /// platform's application-data directory elsewhere.
    pub fn path() -> PathBuf {
        crate::platform::state_dir().join("state.toml")
    }

    /// Never fails (see the module docs).
    pub fn load_or_default() -> Self {
        Self::load_from(&Self::path())
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&Self::path())
    }

    pub fn load_from(path: &Path) -> Self {
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(_) => return Self::default(),
        };

        match toml::from_str(&content) {
            Ok(state) => state,
            Err(_) => {
                // Keep the damaged file for inspection, and start clean.
                let _ = fs::rename(path, path.with_extension("toml.bad"));

                Self::default()
            }
        }
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Next to the real file, so the rename stays on one filesystem; the process id keeps two
        // writers from sharing one temporary file.
        let temp = path.with_extension(format!("toml.{}.tmp", std::process::id()));

        fs::write(&temp, toml::to_string_pretty(self)?)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
        }

        if let Err(error) = fs::rename(&temp, path) {
            let _ = fs::remove_file(&temp);

            return Err(error.into());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "aujar-state-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));

        (dir.clone(), dir.join("state.toml"))
    }

    #[test]
    fn a_missing_file_is_the_default_state() {
        let (_dir, path) = scratch();

        assert_eq!(State::load_from(&path), State::default());
        assert_eq!(State::default().awake.mode, AwakeMode::Off);
    }

    #[test]
    fn saved_state_comes_back() {
        let (dir, path) = scratch();
        let state = State {
            awake: AwakeState {
                mode: AwakeMode::Timed,
                until: 1_800_000_000,
            },
        };

        state.save_to(&path).unwrap();

        assert_eq!(State::load_from(&path), state);

        // No temporary file is left behind.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();

        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_again_replaces_the_old_state() {
        let (dir, path) = scratch();

        State {
            awake: AwakeState {
                mode: AwakeMode::Indefinite,
                until: 0,
            },
        }
        .save_to(&path)
        .unwrap();
        State::default().save_to(&path).unwrap();

        assert_eq!(State::load_from(&path).awake.mode, AwakeMode::Off);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_damaged_file_is_moved_aside_and_the_default_used() {
        let (dir, path) = scratch();

        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, "this is [not toml").unwrap();

        assert_eq!(State::load_from(&path), State::default());
        assert!(!path.exists(), "the damaged file must not be read again");
        assert!(path.with_extension("toml.bad").exists());

        // And saving works afterwards.
        State::default().save_to(&path).unwrap();
        assert!(path.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn unknown_or_missing_fields_do_not_break_loading() {
        let (dir, path) = scratch();

        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, "future_thing = 1\n[awake]\nmode = \"indefinite\"\n").unwrap();

        let state = State::load_from(&path);

        assert_eq!(state.awake.mode, AwakeMode::Indefinite);
        assert_eq!(state.awake.until, 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_state_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, path) = scratch();

        State::default().save_to(&path).unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = fs::remove_dir_all(dir);
    }
}
