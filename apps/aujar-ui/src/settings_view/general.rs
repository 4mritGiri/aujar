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
                "Launcher keys",
                vec![
                    self.row("Open a result", "Launch the selected row", self.keycap("Enter").into_any_element()),
                    self.row("Show in folder", "Reveal the selected file in your file manager", self.keycap("Ctrl+Enter").into_any_element()),
                    self.row("Actions menu", "Also: right-click a row", self.keycap("Ctrl+K").into_any_element()),
                    self.row("Look up a word", "Define the selected text or the query", self.keycap("Ctrl+L").into_any_element()),
                    self.row("Copy", "Copy the path, URL or answer of the selected row", self.keycap("Ctrl+C").into_any_element()),
                    self.row("Launch row 1 to 9", "Skip the arrow keys", self.keycap("Alt+1\u{2026}9").into_any_element()),
                    self.row("Settings", "Open this window from the launcher", self.keycap("Ctrl+,").into_any_element()),
                    self.row("Close", "First press clears the search, the second closes", self.keycap("Esc").into_any_element()),
                ],
            ),
            self.section(
                "Settings window",
                vec![
                    self.row("Search settings", "Find a setting by name", self.keycap("Ctrl+F").into_any_element()),
                    self.row("Back / forward", "Between pages you visited", self.keycap("Alt+\u{2190} / \u{2192}").into_any_element()),
                    self.row("Move the window", "Drag the title bar. Double-click it to zoom", div()),
                ],
            ),
            self.section(
                "Files",
                vec![self.row("Config file", &Settings::path().display().to_string(), open_config)],
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
