//! Search plugins, grouped by what they touch. Plugins that contact an outside service say so.

use super::{EditTarget, SettingsView, nepse_key_status, save_nepse_key};
use gpui::{AnyElement, Context, SharedString, div, prelude::*};

impl SettingsView {
    pub(super) fn plugins_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut online: Vec<AnyElement> = vec![
            toggle_row_badged!(self, cx, "p-currency", "Currency converter", "Online", "100 usd to eur. Downloads exchange rates (curl) about twice a day", plugins.currency),
            toggle_row_badged!(self, cx, "p-dict", "Dictionary", "Online", "define word, meaning of word, Ctrl+L. Sends only that word to dictionaryapi.dev", plugins.dictionary),
            toggle_row_badged!(self, cx, "p-weather", "Weather", "Online", "weather paris. Sends only the place to Open-Meteo", plugins.weather),
            toggle_row!(self, cx, "p-imperial", "Fahrenheit and mph", "Show weather in imperial units", weather.imperial),
            toggle_row_badged!(self, cx, "p-stocks", "Stocks", "Online", "stock aapl, $tsla. Sends only the ticker to Yahoo Finance (delayed quotes)", plugins.stocks),
            toggle_row_badged!(self, cx, "p-nepse", "NEPSE stocks", "Online", "nepse nabil, nepse nic asia, nepse depth nabil, nepse news nabil, nepse sectors, nepse holidays, nepse gainers, nepse alert nabil above 600. Sends the symbol and your API key to warehouse.fintechnepal.com.np", plugins.nepse),
        ];

        // The key lives in its own owner-only file, not in settings.toml.
        let (has_key, key_hint) = self.nepse_key.clone();

        let mut key_controls = div().flex().flex_row().gap_2().child(
            self.pill(
                SharedString::from("nepse-key-set"),
                if has_key { "Change" } else { "Set key" },
                !has_key,
            )
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.start_edit(EditTarget::NepseKey, String::new(), cx);
            })),
        );

        if has_key && std::env::var_os("AUJAR_NEPSE_KEY").is_none() {
            key_controls = key_controls.child(
                self.pill(SharedString::from("nepse-key-remove"), "Remove", false)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        if let Err(error) = save_nepse_key("") {
                            this.status = Some(error);
                        } else {
                            this.status = Some("NEPSE key removed".to_owned());
                        }

                        this.nepse_key = nepse_key_status();
                        cx.notify();
                    })),
            );
        }

        online.push(self.row("NEPSE API key", &key_hint, key_controls));

        if self.editing.as_ref().is_some_and(|edit| edit.target == EditTarget::NepseKey) {
            online.extend(self.editor_row(
                "Paste your Fintech Nepal API key. It is saved only on this computer and sent only to warehouse.fintechnepal.com.np.",
                true,
                cx,
            ));
        }

        online.push(toggle_row_badged!(self, cx, "p-sports", "Sports scores", "Online", "nba, premier league, nfl lakers. Sends only the league to ESPN", plugins.sports));

        vec![
            self.section(
                "On this computer",
                vec![
                    toggle_row!(self, cx, "p-apps", "Applications", "Installed desktop applications", plugins.applications),
                    toggle_row!(self, cx, "p-files", "Files & folders", "Search file names under your home folder", plugins.files),
                    toggle_row!(self, cx, "p-windows", "Window switching", "< name lists open windows. GNOME needs the extension: aujar gnome-extension install", plugins.windows),
                    toggle_row!(self, cx, "p-system", "System commands", "Lock, sleep, restart, shut down, log out, settings", plugins.system),
                    toggle_row!(self, cx, "p-clip", "Clipboard history", "Type clip or cb. Records what you copy (in memory only)", plugins.clipboard),
                ],
            ),
            self.section(
                "Instant answers",
                vec![
                    toggle_row!(self, cx, "p-calc", "Calculator", "Math: 2*(3+4), 2^10, 10 % 3", plugins.calculator),
                    toggle_row!(self, cx, "p-units", "Unit converter", "10 km to mi, 100 f to c, 2 gb to mb", plugins.units),
                ],
            ),
            self.section("Online lookups", online),
            self.section(
                "Advanced",
                vec![toggle_row_badged!(
                    self, cx, "p-shell", "Shell commands", "Runs commands",
                    "> command runs it in a shell. Runs arbitrary commands: off by default",
                    plugins.shell
                )],
            ),
        ]
    }
}
