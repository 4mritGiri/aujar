//! The global shortcuts the daemon binds, shared by the Unix and Windows daemons: the launcher,
//! Aujar Snip and Text Extractor. Each press becomes a launcher event for the UI.

use aujar_config::Settings;
use aujar_hotkeys::{HotkeyEvent, ShortcutSpec};
use aujar_ipc::LauncherCommand;

pub(crate) const LAUNCHER: &str = "toggle-launcher";
pub(crate) const SNIP: &str = "snip";
pub(crate) const TEXT: &str = "text-extractor";

/// The launcher's key (the portal treats it as a suggestion on Linux).
const LAUNCHER_KEYS: &str = "CTRL+ALT+space";

pub(crate) fn specs(settings: &Settings) -> Vec<ShortcutSpec> {
    vec![
        ShortcutSpec {
            id: LAUNCHER.to_owned(),
            description: "Show or hide the Aujar launcher".to_owned(),
            trigger: LAUNCHER_KEYS.to_owned(),
        },
        ShortcutSpec {
            id: SNIP.to_owned(),
            description: "Aujar Snip: select part of the screen".to_owned(),
            trigger: settings.snip.shortcut.clone(),
        },
        ShortcutSpec {
            id: TEXT.to_owned(),
            description: "Text Extractor: copy the text in part of the screen".to_owned(),
            trigger: settings.text_extractor.shortcut.clone(),
        },
    ]
}

/// The launcher event a shortcut sends.
pub(crate) fn command(id: &str) -> Option<LauncherCommand> {
    match id {
        LAUNCHER => Some(LauncherCommand::Toggle),
        SNIP => Some(LauncherCommand::Snip),
        TEXT => Some(LauncherCommand::SnipText),
        _ => None,
    }
}

/// What `aujar doctor` shows about the shortcuts: `bound: ...` first (it looks for that prefix),
/// then the ones that failed.
#[derive(Debug, Default)]
pub(crate) struct Status {
    bound: Vec<String>,
    failed: Vec<String>,
    unavailable: Option<String>,
}

impl Status {
    /// Record an event; `true` when the status text changed.
    pub fn record(&mut self, event: &HotkeyEvent) -> bool {
        match event {
            HotkeyEvent::Bound { id, trigger } => self.bound.push(format!("{id} = {trigger}")),
            HotkeyEvent::Failed { id, reason } => self.failed.push(format!("{id}: {reason}")),
            HotkeyEvent::Unavailable(reason) => self.unavailable = Some(reason.clone()),
            HotkeyEvent::Activated(_) => return false,
        }

        true
    }

    pub fn text(&self) -> String {
        let mut parts = Vec::new();

        if !self.bound.is_empty() {
            parts.push(format!("bound: {}", self.bound.join(", ")));
        }

        if !self.failed.is_empty() {
            parts.push(format!("failed: {}", self.failed.join("; ")));
        }

        if let Some(reason) = &self.unavailable {
            parts.push(format!("unavailable: {reason}"));
        }

        if parts.is_empty() {
            "waiting for the desktop".to_owned()
        } else {
            parts.join(" | ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shortcut_sends_an_event() {
        for spec in specs(&Settings::default()) {
            assert!(command(&spec.id).is_some(), "{}", spec.id);
            assert!(
                spec.trigger.parse::<aujar_hotkeys::Trigger>().is_ok(),
                "default {} = {} does not parse",
                spec.id,
                spec.trigger
            );
        }

        assert_eq!(command("unknown"), None);
    }

    #[test]
    fn status_puts_bound_shortcuts_first() {
        let mut status = Status::default();

        assert_eq!(status.text(), "waiting for the desktop");

        status.record(&HotkeyEvent::Bound {
            id: SNIP.into(),
            trigger: "Win+Shift+X".into(),
        });
        status.record(&HotkeyEvent::Failed {
            id: TEXT.into(),
            reason: "taken".into(),
        });
        assert!(!status.record(&HotkeyEvent::Activated(SNIP.into())));

        assert_eq!(
            status.text(),
            "bound: snip = Win+Shift+X | failed: text-extractor: taken"
        );
    }
}
