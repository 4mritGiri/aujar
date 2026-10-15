//! Appearance: theme, accent and what the launcher window shows.

use super::SettingsView;
use aujar_config::{Accent, Theme};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px, rgb};

impl SettingsView {
    pub(super) fn appearance_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let pal = self.palette;

        let theme_control = self.choices(vec![
            self.pill(
                "theme-dark".into(),
                "Dark",
                self.settings.appearance.theme == Theme::Dark,
            )
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.mutate(cx, |settings| {
                    settings.appearance.theme = aujar_config::Theme::Dark
                })
            })),
            self.pill(
                "theme-light".into(),
                "Light",
                self.settings.appearance.theme == Theme::Light,
            )
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.mutate(cx, |settings| {
                    settings.appearance.theme = aujar_config::Theme::Light
                })
            })),
        ]);

        let accent_control = div()
            .flex()
            .flex_row()
            .gap_2()
            .children(Accent::ALL.map(|accent| {
                let selected = self.settings.appearance.accent == accent;

                div()
                    .id(SharedString::from(format!("accent-{accent:?}")))
                    .size(px(24.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(rgb(accent.rgb()))
                    .border_2()
                    .border_color(if selected {
                        rgb(pal.text)
                    } else {
                        rgb(accent.rgb())
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.mutate(cx, |settings| settings.appearance.accent = accent)
                    }))
            }));

        vec![
            self.section(
                "Theme",
                vec![
                    self.row("Theme", "Launcher and settings colors", theme_control),
                    self.row("Accent color", "Selected rows and switches", accent_control),
                ],
            ),
            self.section(
                "Launcher window",
                vec![
                    stepper_row!(
                        self,
                        cx,
                        "width",
                        "Width",
                        "Launcher width in pixels",
                        40,
                        " px",
                        appearance.width
                    ),
                    stepper_row!(
                        self,
                        cx,
                        "max",
                        "Results per section",
                        "Applications shown (other sections show fewer)",
                        1,
                        "",
                        appearance.max_results
                    ),
                    toggle_row!(
                        self,
                        cx,
                        "icons",
                        "App icons",
                        "Show application icons next to results",
                        appearance.show_icons
                    ),
                    toggle_row!(
                        self,
                        cx,
                        "footer",
                        "Keyboard hints",
                        "Show the shortcut bar at the bottom",
                        appearance.show_footer
                    ),
                    toggle_row!(
                        self,
                        cx,
                        "preview",
                        "Preview pane",
                        "Details of the selected result on the right",
                        appearance.show_preview
                    ),
                    toggle_row!(
                        self,
                        cx,
                        "recents",
                        "Recent & frequent",
                        "Show what you use most when the search box is empty",
                        appearance.show_recents
                    ),
                    toggle_row!(
                        self,
                        cx,
                        "blur",
                        "Close when clicking away",
                        "Hide the launcher when it loses focus",
                        appearance.close_on_blur
                    ),
                ],
            ),
        ]
    }
}
