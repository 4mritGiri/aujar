//! Web search: the engine and the keyword shortcuts.

use super::{EditTarget, SettingsView};
use aujar_config::{Engine, Web};
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

        let editing = self.editing.as_ref().map(|edit| edit.target.clone());
        let mut shortcuts: Vec<AnyElement> = Vec::new();

        for (index, shortcut) in self.settings.web.shortcuts.iter().enumerate() {
            let line = format!("{} | {} | {}", shortcut.keyword, shortcut.name, shortcut.url);

            let controls = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .text_sm()
                        .bg(gpui::rgba(pal.subtle))
                        .text_color(rgb(pal.text))
                        .child(shortcut.keyword.clone()),
                )
                .child(
                    self.pill(SharedString::from(format!("sc-edit-{index}")), "Edit", false)
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.start_edit(EditTarget::Shortcut(Some(index)), line.clone(), cx);
                        })),
                )
                .child(
                    self.pill(SharedString::from(format!("sc-del-{index}")), "Remove", false)
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.editing = None;
                            this.mutate(cx, |settings| {
                                if index < settings.web.shortcuts.len() {
                                    settings.web.shortcuts.remove(index);
                                }
                            });
                        })),
                );

            shortcuts.push(self.row(
                &shortcut.name,
                &format!("Type `{} your query`", shortcut.keyword),
                controls,
            ));

            if editing == Some(EditTarget::Shortcut(Some(index))) {
                shortcuts.extend(self.editor_row(
                    "Format: keyword | Name | URL. The search text is added at the end of the URL.",
                    false,
                    cx,
                ));
            }
        }

        let missing = Web::suggested_shortcuts()
            .into_iter()
            .filter(|suggested| {
                !self
                    .settings
                    .web
                    .shortcuts
                    .iter()
                    .any(|shortcut| shortcut.keyword.eq_ignore_ascii_case(&suggested.keyword))
            })
            .count();

        let mut add_controls = div().flex().flex_row().gap_2().child(
            self.pill(SharedString::from("sc-add"), "Add shortcut", true)
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.start_edit(EditTarget::Shortcut(None), String::new(), cx);
                })),
        );

        if missing > 0 {
            add_controls = add_controls.child(
                self.pill(
                    SharedString::from("sc-suggested"),
                    &format!("Add {missing} suggested"),
                    false,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.editing = None;
                    this.mutate(cx, |settings| {
                        for suggested in Web::suggested_shortcuts() {
                            if !settings
                                .web
                                .shortcuts
                                .iter()
                                .any(|shortcut| shortcut.keyword.eq_ignore_ascii_case(&suggested.keyword))
                            {
                                settings.web.shortcuts.push(suggested);
                            }
                        }
                    });
                })),
            );
        }

        shortcuts.push(self.row(
            "New shortcut",
            "For example: so | Stack Overflow | https://stackoverflow.com/search?q=",
            add_controls,
        ));

        if editing == Some(EditTarget::Shortcut(None)) {
            shortcuts.extend(self.editor_row(
                "Format: keyword | Name | URL. Example: so | Stack Overflow | https://stackoverflow.com/search?q=",
                false,
                cx,
            ));
        }

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
