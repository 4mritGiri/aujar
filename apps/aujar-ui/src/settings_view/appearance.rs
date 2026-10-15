//! Appearance: theme, window style (opacity, corners, blur, window buttons) and what the
//! launcher window shows and where it opens.

use super::SettingsView;
use aujar_config::{Accent, LauncherPosition, Theme};
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

        let positions = self.choices(
            LauncherPosition::ALL
                .into_iter()
                .map(|position| {
                    self.pill(
                        SharedString::from(format!("launcher-pos-{}", position.name())),
                        position.name(),
                        self.settings.appearance.launcher_position == position,
                    )
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.mutate(cx, |settings| {
                            settings.appearance.launcher_position = position
                        })
                    }))
                })
                .collect(),
        );

        vec![
            self.section(
                "Theme",
                vec![
                    self.row("Theme", "Launcher and settings colors", theme_control),
                    self.row("Accent color", "Selected rows, sidebar and switches", accent_control),
                ],
            ),
            self.section(
                "Window style",
                vec![
                    stepper_row!(
                        self,
                        cx,
                        "opacity",
                        "Opacity",
                        "How solid the windows are. Lower lets the desktop show through",
                        5,
                        " %",
                        appearance.opacity
                    ),
                    stepper_row!(
                        self,
                        cx,
                        "radius",
                        "Corner radius",
                        "Roundness of the launcher and settings windows",
                        2,
                        " px",
                        appearance.corner_radius
                    ),
                    toggle_row!(
                        self,
                        cx,
                        "win-blur",
                        "Blur behind windows",
                        "Needs a compositor that supports it (KDE, Hyprland). GNOME ignores it",
                        appearance.blur
                    ),
                    toggle_row!(
                        self,
                        cx,
                        "traffic",
                        "Window buttons",
                        "Red / yellow / green close, minimize and zoom in the settings window. Esc always closes it",
                        appearance.traffic_lights
                    ),
                ],
            ),
            self.section(
                "Launcher window",
                vec![
                    toggle_row!(
                        self,
                        cx,
                        "movable",
                        "Move by dragging",
                        "Drag the search bar or the shortcut bar to move the launcher",
                        appearance.launcher_movable
                    ),
                    self.row(
                        "Opens at",
                        "Where the launcher appears. Wayland lets the compositor place new windows, so this works on X11; dragging works everywhere",
                        positions,
                    ),
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
                        "Spotlight-style preview of the selected file, folder or result on the right",
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
