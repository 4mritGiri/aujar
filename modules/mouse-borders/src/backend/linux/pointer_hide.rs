//! Hiding the pointer on GNOME (Wayland), through the Aujar Windows Shell extension.
//!
//! Wayland gives an application no way to hide the pointer of the whole desktop, and the
//! desktop's input capture leaves it showing at the screen edge. GNOME Shell can hide it, so
//! the extension in `packaging/gnome-extension` offers `dev.aujar.Pointer.SetHidden` on the
//! session bus. Without the extension (or without GNOME) this does nothing, and the pointer
//! simply stays where it is.

use std::process::{Command, Stdio};

const BUS_NAME: &str = "dev.aujar.Windows";
const OBJECT_PATH: &str = "/dev/aujar/Pointer";
const METHOD: &str = "dev.aujar.Pointer.SetHidden";

/// The `gdbus` arguments that hide or show the pointer.
fn gdbus_args(hidden: bool) -> Vec<String> {
    [
        "call",
        "--session",
        "--timeout",
        "2",
        "--dest",
        BUS_NAME,
        "--object-path",
        OBJECT_PATH,
        "--method",
        METHOD,
        if hidden { "true" } else { "false" },
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// Ask the extension to hide (or show) the pointer. Best effort, and it never makes the caller
/// wait: the call runs on a thread of its own.
pub(super) fn set_hidden(hidden: bool) {
    let args = gdbus_args(hidden);

    let _ = std::thread::Builder::new()
        .name("aujar-pointer".into())
        .spawn(move || {
            let _ = Command::new("gdbus")
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_call_goes_to_the_extension_with_the_wanted_state() {
        let hide = gdbus_args(true);
        let show = gdbus_args(false);

        assert_eq!(hide.last().map(String::as_str), Some("true"));
        assert_eq!(show.last().map(String::as_str), Some("false"));

        // Everything but the last word is the same, and it is a session-bus method call.
        assert_eq!(hide[..hide.len() - 1], show[..show.len() - 1]);
        assert!(hide.contains(&"dev.aujar.Windows".to_owned()));
        assert!(hide.contains(&"/dev/aujar/Pointer".to_owned()));
        assert!(hide.contains(&"dev.aujar.Pointer.SetHidden".to_owned()));
        assert_eq!(hide[0], "call");
        assert_eq!(hide[1], "--session");
    }
}
