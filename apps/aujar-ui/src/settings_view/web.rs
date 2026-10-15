//! Web search: the engine and the keyword shortcuts.

use super::SettingsView;
use aujar_config::Engine;
use gpui::{AnyElement, Context, SharedString, div, prelude::*, rgb};

impl SettingsView {
    pub(super) fn web_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let pal = self.palette;

        let engines = self.choices(
            Engine::ALL
                .map(|engine| {
                    let selected = self.settings.web.engine == engine;

                    self.pill(
                        SharedString::from(format!("engine-{}", engine.name())),
                        engine.name(),
                        selected,
                    )
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.mutate(cx, |settings| settings.web.engine = engine)
                    }))
                })
                .into(),
        );

        let mut shortcuts: Vec<AnyElement> = self
            .settings
            .web
            .shortcuts
            .iter()
            .map(|shortcut| {
                self.row(
                    &shortcut.name,
                    &format!("Type `{} your query`", shortcut.keyword),
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .text_sm()
                        .bg(gpui::rgba(pal.subtle))
                        .text_color(rgb(pal.text))
                        .child(shortcut.keyword.clone()),
                )
            })
            .collect();

        shortcuts.push(self.row(
            "More shortcuts",
            "Add your own in the config file (General page), under [[web.shortcuts]].",
            div(),
        ));

        vec![
            self.section(
                "Search engine",
                vec![self.row(
                    "Default engine",
                    "Used by the \"Search the web\" row and `? query`",
                    engines,
                )],
            ),
            self.section("Keyword shortcuts", shortcuts),
        ]
    }
}
