//! General: the launcher shortcut, the config file and history.

use super::SettingsView;
use aujar_config::Settings;
use gpui::{AnyElement, Context, div, prelude::*};

impl SettingsView {
    pub(super) fn general_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let open_config = self
            .pill("open-config".into(), "Open config file", false)
            .on_click(cx.listener(|this, _event, _window, cx| {
                // Make sure the file exists so the editor has something to open.
                this.mutate(cx, |_| {});

                let _ = std::process::Command::new("xdg-open")
                    .arg(Settings::path())
                    .spawn();
            }));

        let clear_history = self
            .pill("clear-history".into(), "Clear history", false)
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.status = Some(match aujar_config::History::default().save() {
                    Ok(()) => "History cleared".to_owned(),
                    Err(error) => format!("Could not clear: {error:#}"),
                });
                cx.notify();
            }));

        vec![
            self.section(
                "Launcher",
                vec![self.row(
                    "Keyboard shortcut",
                    "Set with `aujar shortcut install --keys '<Super>space'` (GNOME), or bind `aujar launcher toggle` in your desktop's keyboard settings.",
                    div(),
                )],
            ),
            self.section(
                "Files",
                vec![
                    self.row("Config file", &Settings::path().display().to_string(), open_config),
                    self.row(
                        "Launch history",
                        "Recent and frequent items (stored in ~/.local/state/aujar)",
                        clear_history,
                    ),
                ],
            ),
            self.section(
                "About",
                vec![self.row(
                    "Aujar",
                    &format!("Version {}", env!("CARGO_PKG_VERSION")),
                    div(),
                )],
            ),
        ]
    }
}
