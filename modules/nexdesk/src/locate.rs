//! Finding the `nexdesk` executable.
//!
//! Order: `AUJAR_NEXDESK_BIN`, then next to the running `aujar` binary (how packages install
//! it, see docs/NEXDESK.md), then `PATH`. If `AUJAR_NEXDESK_BIN` is set but does not point at
//! an executable file, nothing is found: a wrong override is reported, not silently ignored.

use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// Environment variable that overrides where the `nexdesk` executable is looked for.
pub const BIN_ENV: &str = "AUJAR_NEXDESK_BIN";

/// File name of the NexDesk manager executable.
pub const BIN_NAME: &str = if cfg!(windows) {
    "nexdesk.exe"
} else {
    "nexdesk"
};

/// Locate the executable using the real environment.
pub fn locate() -> Option<PathBuf> {
    let exe = env::current_exe().ok();

    locate_in(
        env::var_os(BIN_ENV).as_deref(),
        exe.as_deref().and_then(Path::parent),
        env::var_os("PATH").as_deref(),
    )
}

/// Locate the executable from explicit inputs (what [`locate`] feeds in).
pub fn locate_in(
    explicit: Option<&OsStr>,
    exe_dir: Option<&Path>,
    path_var: Option<&OsStr>,
) -> Option<PathBuf> {
    if let Some(explicit) = explicit.filter(|value| !value.is_empty()) {
        let candidate = PathBuf::from(explicit);

        return is_executable(&candidate).then_some(candidate);
    }

    if let Some(dir) = exe_dir {
        let candidate = dir.join(BIN_NAME);

        if is_executable(&candidate) {
            return Some(candidate);
        }
    }

    env::split_paths(path_var?)
        .map(|dir| dir.join(BIN_NAME))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file())
        .unwrap_or(false)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn fake_binary(dir: &Path, mode: u32) -> PathBuf {
        let path = dir.join(BIN_NAME);

        fs::write(&path, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();

        path
    }

    #[test]
    fn explicit_override_wins() {
        let explicit = tempfile::tempdir().unwrap();
        let beside = tempfile::tempdir().unwrap();
        let wanted = fake_binary(explicit.path(), 0o755);
        fake_binary(beside.path(), 0o755);

        let found = locate_in(Some(wanted.as_os_str()), Some(beside.path()), None);

        assert_eq!(found, Some(wanted));
    }

    #[test]
    fn bad_override_is_not_replaced_by_a_fallback() {
        let beside = tempfile::tempdir().unwrap();
        fake_binary(beside.path(), 0o755);

        let found = locate_in(
            Some(OsStr::new("/definitely/not/here")),
            Some(beside.path()),
            None,
        );

        assert_eq!(found, None);
    }

    #[test]
    fn empty_override_is_ignored() {
        let beside = tempfile::tempdir().unwrap();
        let wanted = fake_binary(beside.path(), 0o755);

        assert_eq!(
            locate_in(Some(OsStr::new("")), Some(beside.path()), None),
            Some(wanted)
        );
    }

    #[test]
    fn beside_the_executable_beats_path() {
        let beside = tempfile::tempdir().unwrap();
        let on_path = tempfile::tempdir().unwrap();
        let wanted = fake_binary(beside.path(), 0o755);
        fake_binary(on_path.path(), 0o755);

        let path_var = env::join_paths([on_path.path()]).unwrap();

        assert_eq!(
            locate_in(None, Some(beside.path()), Some(path_var.as_os_str())),
            Some(wanted)
        );
    }

    #[test]
    fn falls_back_to_path() {
        let beside = tempfile::tempdir().unwrap();
        let on_path = tempfile::tempdir().unwrap();
        let wanted = fake_binary(on_path.path(), 0o755);

        let path_var = env::join_paths([on_path.path()]).unwrap();

        assert_eq!(
            locate_in(None, Some(beside.path()), Some(path_var.as_os_str())),
            Some(wanted)
        );
    }

    #[test]
    fn non_executable_files_are_skipped() {
        let beside = tempfile::tempdir().unwrap();
        fake_binary(beside.path(), 0o644);

        assert_eq!(locate_in(None, Some(beside.path()), None), None);
    }
}
