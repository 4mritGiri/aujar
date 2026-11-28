//! Whether the desktop lets Aujar take screenshots without asking (GNOME).
//!
//! The daemon registers itself with the portal as `dev.aujar.Aujar` (for global shortcuts), so
//! GNOME treats its screenshot requests as Aujar's. A non-interactive screenshot from an app
//! needs that app's permission; GNOME asks with a dialog, but only the focused app may show
//! one, and the daemon has no window. So the user grants it once, explicitly, with
//! `aujar snip allow`: the same entry the "Allow" button of GNOME's dialog stores, in the
//! portal permission store (`screenshot` table). `aujar snip allow --revoke` removes it.
//!
//! Uses `gdbus` (GLib), which every GNOME desktop has.

use anyhow::{Context, Result, bail};
use std::process::Command;

/// The app id the daemon registers with the portal (`aujar_hotkeys::APP_ID`).
pub const PORTAL_APP_ID: &str = "dev.aujar.Aujar";

const STORE: [&str; 6] = [
    "--session",
    "--dest",
    "org.freedesktop.impl.portal.PermissionStore",
    "--object-path",
    "/org/freedesktop/impl/portal/PermissionStore",
    "--method",
];
const METHOD: &str = "org.freedesktop.impl.portal.PermissionStore";

fn gdbus(method: &str, args: &[&str]) -> Result<String> {
    let output = Command::new("gdbus")
        .arg("call")
        .args(STORE)
        .arg(format!("{METHOD}.{method}"))
        .args(args)
        .output()
        .context("could not run `gdbus` (install libglib2.0-bin / glib2)")?;

    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// What the store says for `app_id` in `gdbus` output such as
/// `({'': ['yes'], 'dev.aujar.Aujar': ['no']}, <byte 0x00>)`.
pub fn parse_lookup(output: &str, app_id: &str) -> Option<bool> {
    let key = format!("'{app_id}': [");
    let rest = &output[output.find(&key)? + key.len()..];
    let values = &rest[..rest.find(']')?];

    if values.contains("'yes'") {
        Some(true)
    } else if values.contains("'no'") {
        Some(false)
    } else {
        None
    }
}

/// `Some(true)` allowed, `Some(false)` refused, `None` never decided.
///
/// macOS: the Screen Recording permission (granted or not; never "undecided" to the app).
/// Windows: always allowed.
#[cfg(target_os = "macos")]
pub fn status() -> Result<Option<bool>> {
    Ok(Some(crate::macos::screen_recording_allowed(false)))
}

#[cfg(windows)]
pub fn status() -> Result<Option<bool>> {
    Ok(Some(true))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn status() -> Result<Option<bool>> {
    gnome_status()
}

#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn gnome_status() -> Result<Option<bool>> {
    match gdbus("Lookup", &["screenshot", "screenshot"]) {
        Ok(output) => Ok(parse_lookup(&output, PORTAL_APP_ID)),
        // No entry at all yet: nothing was ever decided.
        Err(error) if error.to_string().contains("No entry") => Ok(None),
        Err(error) => Err(error),
    }
}

/// Allow Aujar to take screenshots without asking, or forget that (`allow == false`).
///
/// macOS shows its own prompt the first time (granting happens in System Settings, which an app
/// cannot do for the user); Windows needs nothing.
#[cfg(target_os = "macos")]
pub fn set(allow: bool) -> Result<()> {
    if !allow {
        anyhow::bail!("turn Aujar off in System Settings > Privacy & Security > Screen Recording");
    }

    if !crate::macos::screen_recording_allowed(true) {
        anyhow::bail!(
            "turn on Aujar in System Settings > Privacy & Security > Screen Recording, then \
             restart Aujar"
        );
    }

    Ok(())
}

#[cfg(windows)]
pub fn set(_allow: bool) -> Result<()> {
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn set(allow: bool) -> Result<()> {
    gnome_set(allow)
}

#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn gnome_set(allow: bool) -> Result<()> {
    if allow {
        gdbus(
            "SetPermission",
            &["screenshot", "true", "screenshot", PORTAL_APP_ID, "['yes']"],
        )
        .context("could not save the screenshot permission")?;
    } else {
        match gdbus(
            "DeletePermission",
            &["screenshot", "screenshot", PORTAL_APP_ID],
        ) {
            Ok(_) => {}
            Err(error) if error.to_string().contains("No entry") => {}
            Err(error) => return Err(error.context("could not remove the screenshot permission")),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_store_answer() {
        let both = "({'': ['yes'], 'dev.aujar.Aujar': ['yes']}, <byte 0x00>)";
        let refused = "({'dev.aujar.Aujar': ['no'], '': ['yes']}, <byte 0x00>)";
        let other = "({'': ['yes']}, <byte 0x00>)";

        assert_eq!(parse_lookup(both, PORTAL_APP_ID), Some(true));
        assert_eq!(parse_lookup(refused, PORTAL_APP_ID), Some(false));
        assert_eq!(parse_lookup(other, PORTAL_APP_ID), None);
        // The host entry `''` is not Aujar's.
        assert_eq!(parse_lookup(other, ""), Some(true));
    }
}
