//! History: recent and frequent launches, and the clipboard history.

use super::SettingsView;
use gpui::{AnyElement, Context, prelude::*};

impl SettingsView {
    pub(super) fn history_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
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
                vec![
                    toggle_row!(
                        self,
                        cx,
                        "hist-recents",
                        "Recent & frequent",
                        "Show what you use most when the search box is empty",
                        appearance.show_recents
                    ),
                    self.row(
                        "Launch history",
                        "Recent and frequent items (stored in ~/.local/state/aujar)",
                        clear_history,
                    ),
                ],
            ),
            self.section(
                "Clipboard",
                vec![toggle_row!(
                    self,
                    cx,
                    "hist-clip",
                    "Clipboard history",
                    "Type clip or cb in the launcher. Records what you copy, in memory only",
                    plugins.clipboard
                )],
            ),
        ]
    }
}
