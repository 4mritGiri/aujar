//! Small file-system helpers that behave sensibly on every platform.
//!
//! On Unix, secrets get mode `0600` and private directories `0700`. Windows has no mode
//! bits: files created under the user's profile are private to that user by the profile's
//! default ACL, so nothing extra is done there.

use std::{
    fs::{DirBuilder, File, Metadata, OpenOptions},
    io,
    path::Path,
};

/// Create a new file for writing, failing if it exists, readable only by the owner.
pub fn create_new_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();

    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }

    options.open(path)
}

/// Create a directory (and parents) accessible only by the owner.
pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();

    builder.recursive(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        builder.mode(0o700);
    }

    builder.create(path)
}

/// Whether other users can read or write the file. Always `false` where there are no mode
/// bits to inspect (Windows).
pub fn is_shared_with_others(metadata: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        metadata.permissions().mode() & 0o077 != 0
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_files_are_exclusive_and_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");

        create_new_private_file(&path).unwrap();

        assert!(
            create_new_private_file(&path).is_err(),
            "must not overwrite"
        );
        assert!(!is_shared_with_others(&std::fs::metadata(&path).unwrap()));
    }

    #[test]
    fn private_directories_can_be_nested_and_repeated() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");

        create_private_dir_all(&nested).unwrap();
        create_private_dir_all(&nested).unwrap();

        assert!(nested.is_dir());
        assert!(!is_shared_with_others(&std::fs::metadata(&nested).unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn loose_modes_are_detected() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loose");

        std::fs::write(&path, "x").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert!(is_shared_with_others(&std::fs::metadata(&path).unwrap()));
    }
}
