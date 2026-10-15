//! Where the security key lives on disk.
//!
//! The key is a secret, so it never goes into `settings.toml` (which is world-readable by
//! default). It has its own file, created with mode `0600`, and a key file that other users
//! can read is refused rather than trusted.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use crate::{
    error::{Error, Result},
    secure::SecurityKey,
};

#[derive(Debug, Clone)]
pub struct KeyStore {
    path: PathBuf,
}

impl KeyStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `mouse-borders.key` next to the user's `settings.toml`.
    pub fn user_default() -> Self {
        Self::new(aujar_config::Settings::path().with_file_name("mouse-borders.key"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The stored key, or `None` if none was created yet.
    pub fn load(&self) -> Result<Option<SecurityKey>> {
        let metadata = match fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };

        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::Key(format!(
                "{} can be read by other users; run `chmod 600 {}` (or create a new key)",
                self.path.display(),
                self.path.display()
            )));
        }

        let text = fs::read_to_string(&self.path)?;

        SecurityKey::parse(text.trim()).map(Some).map_err(|error| {
            Error::Key(format!(
                "{} is not a valid key file: {error}",
                self.path.display()
            ))
        })
    }

    /// Store `key`. Refuses to replace an existing key unless `overwrite` is set, because
    /// replacing it disconnects every machine paired with the old one.
    pub fn save(&self, key: &SecurityKey, overwrite: bool) -> Result<()> {
        if !overwrite && self.path.exists() {
            return Err(Error::Key(format!(
                "a key already exists at {}; pass --force to replace it",
                self.path.display()
            )));
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Write a private temporary file, then rename, so the key is never visible with
        // loose permissions and a crash cannot leave half a key behind.
        let temp = self.path.with_extension("key.tmp");
        let _ = fs::remove_file(&temp);

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;

        writeln!(file, "{}", key.to_display())?;
        file.sync_all()?;

        fs::rename(&temp, &self.path)?;

        Ok(())
    }

    pub fn generate(&self, overwrite: bool) -> Result<SecurityKey> {
        let key = SecurityKey::generate()?;

        self.save(&key, overwrite)?;

        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, KeyStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::new(dir.path().join("nested").join("mouse-borders.key"));

        (dir, store)
    }

    #[test]
    fn missing_key_is_none() {
        let (_dir, store) = store();

        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn generated_key_is_stored_privately_and_reloads() {
        let (_dir, store) = store();
        let key = store.generate(false).unwrap();

        let mode = fs::metadata(store.path()).unwrap().permissions().mode();

        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(store.load().unwrap().unwrap(), key);
    }

    #[test]
    fn existing_key_is_only_replaced_when_asked() {
        let (_dir, store) = store();
        let first = store.generate(false).unwrap();

        assert!(store.generate(false).is_err());
        assert_eq!(store.load().unwrap().unwrap(), first);

        let second = store.generate(true).unwrap();

        assert_ne!(first, second);
        assert_eq!(store.load().unwrap().unwrap(), second);
    }

    #[test]
    fn world_readable_key_files_are_refused() {
        let (_dir, store) = store();
        store.generate(false).unwrap();

        fs::set_permissions(store.path(), fs::Permissions::from_mode(0o644)).unwrap();

        let error = store.load().unwrap_err().to_string();
        assert!(error.contains("chmod 600"), "{error}");
    }

    #[test]
    fn corrupt_key_files_are_reported() {
        let (_dir, store) = store();
        store.generate(false).unwrap();
        fs::write(store.path(), "not a key\n").unwrap();
        fs::set_permissions(store.path(), fs::Permissions::from_mode(0o600)).unwrap();

        assert!(
            store
                .load()
                .unwrap_err()
                .to_string()
                .contains("not a valid key")
        );
    }
}
