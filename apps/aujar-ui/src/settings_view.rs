//! The settings window: a sidebar of pages, each a stack of grouped rows.
//!
//! Every change is applied and saved immediately to `~/.config/aujar/settings.toml`; the
//! launcher re-reads it each time it opens, and the daemon reloads its plugins and Mouse
//! Borders when the file changes.
//!
//! This file is the shell (state, sidebar, header and the building blocks); each page lives
//! in `settings_view/<page>.rs`.

use crate::theme::Palette;
use aujar_config::Settings;
use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, KeyDownEvent, SharedString, Stateful, Window,
    div, prelude::*, px, rgb, rgba, svg,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Appearance,
    Search,
    Web,
    MouseBorders,
    General,
}

impl Page {
    const ALL: [Page; 5] = [
        Page::Appearance,
        Page::Search,
        Page::Web,
        Page::MouseBorders,
        Page::General,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Search => "Search plugins",
            Self::Web => "Web search",
            Self::MouseBorders => "Mouse Borders",
            Self::General => "General",
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Self::Appearance => "Colors, and what the launcher window shows",
            Self::Search => "Choose what the launcher can find and answer",
            Self::Web => "Where the \"search the web\" row and keyword shortcuts go",
            Self::MouseBorders => "Use one keyboard and mouse across your computers",
            Self::General => "Shortcut, files and history",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Appearance => "icons/sliders.svg",
            Self::Search => "icons/search.svg",
            Self::Web => "icons/globe.svg",
            Self::MouseBorders => "icons/mouse.svg",
            Self::General => "icons/app.svg",
        }
    }
}

pub struct SettingsView {
    settings: Settings,
    palette: Palette,
    status: Option<String>,
    page: Page,
    mouse_borders: mouse_borders::State,
    pub focus_handle: FocusHandle,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = Settings::load_or_default();

        Self::keep_mouse_borders_status_fresh(cx);

        Self {
            palette: Palette::from_settings(&settings),
            settings,
            status: None,
            page: Page::Appearance,
            mouse_borders: mouse_borders::State::default(),
            focus_handle: cx.focus_handle(),
        }
    }

    /// Apply a change, save it and refresh the colors.
    fn mutate(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Settings)) {
        change(&mut self.settings);

        self.settings = self.settings.clone().sanitized();
        self.palette = Palette::from_settings(&self.settings);
        self.status = Some(match self.settings.save() {
            Ok(()) => "Saved".to_owned(),
            Err(error) => format!("Could not save: {error:#}"),
        });

        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, _cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            window.remove_window();
        }
    }

    // ---- Building blocks ----

    /// A titled group of rows in a rounded card.
    fn section(&self, title: &str, rows: Vec<AnyElement>) -> AnyElement {
        let pal = self.palette;

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .px_1()
                    .text_xs()
                    .text_color(rgb(pal.text_faint))
                    .child(title.to_uppercase()),
            )
            .child(self.card(rows))
            .into_any_element()
    }

    /// Rows in a rounded card, separated by hairlines, without a title.
    fn card(&self, rows: Vec<AnyElement>) -> AnyElement {
        let pal = self.palette;

        div()
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(rgba(pal.border))
            .bg(rgba(pal.subtle))
            .children(rows.into_iter().enumerate().map(|(index, row)| {
                div()
                    .when(index > 0, |this| {
                        this.border_t_1().border_color(rgba(pal.divider))
                    })
                    .child(row)
            }))
            .into_any_element()
    }

    fn row(&self, label: &str, hint: &str, control: impl IntoElement) -> AnyElement {
        self.row_with_badge(label, None, hint, control)
    }

    /// A row whose label may carry a small badge, e.g. `Online` for plugins that use the
    /// network.
    fn row_with_badge(
        &self,
        label: &str,
        badge: Option<&str>,
        hint: &str,
        control: impl IntoElement,
    ) -> AnyElement {
        let pal = self.palette;

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_4()
            .px_4()
            .py_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(pal.text))
                                    .child(label.to_owned()),
                            )
                            .children(badge.map(|badge| {
                                div()
                                    .px_2()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgba(pal.keycap_border))
                                    .text_xs()
                                    .text_color(rgb(pal.text_dim))
                                    .child(badge.to_owned())
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_dim))
                            .child(hint.to_owned()),
                    ),
            )
            .child(control)
            .into_any_element()
    }

    fn switch(&self, id: &'static str, on: bool) -> Stateful<gpui::Div> {
        let pal = self.palette;

        div()
            .id(id)
            .flex()
            .items_center()
            .w(px(40.))
            .h(px(22.))
            .px_1()
            .rounded_full()
            .cursor_pointer()
            .bg(rgb(if on { pal.accent } else { pal.switch_off }))
            .when(on, |this| this.justify_end())
            .child(div().size(px(16.)).rounded_full().bg(rgb(0xffffff)))
    }

    fn pill(&self, id: SharedString, label: &str, selected: bool) -> Stateful<gpui::Div> {
        let pal = self.palette;

        div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .border_1()
            .border_color(rgba(pal.keycap_border))
            .when(selected, |this| {
                this.bg(rgb(pal.accent)).text_color(rgb(pal.on_accent))
            })
            .when(!selected, |this| {
                this.text_color(rgb(pal.text))
                    .hover(|style| style.bg(rgba(pal.hover)))
            })
            .child(label.to_owned())
    }

    fn glyph(&self, path: &'static str, size: f32, color: u32) -> impl IntoElement {
        svg()
            .path(path)
            .size(px(size))
            .flex_shrink_0()
            .text_color(rgb(color))
    }

    /// A row of mutually exclusive choices.
    fn choices(&self, pills: Vec<Stateful<gpui::Div>>) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .justify_end()
            .gap_2()
            .children(pills)
            .into_any_element()
    }

    fn nav_item(&self, page: Page, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        let pal = self.palette;
        let selected = self.page == page;

        div()
            .id(SharedString::from(format!("nav-{page:?}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .when(selected, |this| {
                this.bg(rgb(pal.accent)).text_color(rgb(pal.on_accent))
            })
            .when(!selected, |this| {
                this.text_color(rgb(pal.text))
                    .hover(|style| style.bg(rgba(pal.hover)))
            })
            .child(self.glyph(
                page.icon(),
                16.,
                if selected {
                    pal.on_accent
                } else {
                    pal.text_dim
                },
            ))
            .child(page.title())
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.page = page;
                cx.notify();

                if page == Page::MouseBorders {
                    this.refresh_mouse_borders(cx);
                }
            }))
    }
}

/// A switch row bound to a boolean field of [`Settings`].
macro_rules! toggle_row {
    ($this:ident, $cx:ident, $id:literal, $label:expr, $hint:expr, $($field:tt)+) => {{
        let on = $this.settings.$($field)+;
        let control = $this.switch($id, on).on_click($cx.listener(|this, _event, _window, cx| {
            this.mutate(cx, |settings| settings.$($field)+ = !settings.$($field)+);
        }));

        $this.row($label, $hint, control)
    }};
}

/// Like [`toggle_row!`], with a badge next to the label.
macro_rules! toggle_row_badged {
    ($this:ident, $cx:ident, $id:literal, $label:expr, $badge:expr, $hint:expr, $($field:tt)+) => {{
        let on = $this.settings.$($field)+;
        let control = $this.switch($id, on).on_click($cx.listener(|this, _event, _window, cx| {
            this.mutate(cx, |settings| settings.$($field)+ = !settings.$($field)+);
        }));

        $this.row_with_badge($label, Some($badge), $hint, control)
    }};
}

/// `- value +` stepper bound to a `u32` field, moving by `$step` within the clamp in `sanitized`.
macro_rules! stepper_row {
    ($this:ident, $cx:ident, $id:literal, $label:expr, $hint:expr, $step:expr, $suffix:expr, $($field:tt)+) => {{
        let value = $this.settings.$($field)+;
        let pal = $this.palette;

        let minus = $this
            .pill(SharedString::from(concat!($id, "-minus")), "\u{2212}", false)
            .on_click($cx.listener(|this, _event, _window, cx| {
                this.mutate(cx, |settings| {
                    settings.$($field)+ = settings.$($field)+.saturating_sub($step)
                });
            }));
        let plus = $this
            .pill(SharedString::from(concat!($id, "-plus")), "+", false)
            .on_click($cx.listener(|this, _event, _window, cx| {
                this.mutate(cx, |settings| {
                    settings.$($field)+ = settings.$($field)+.saturating_add($step)
                });
            }));

        let control = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(minus)
            .child(
                div()
                    .min_w(px(64.))
                    .flex()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(pal.text))
                    .child(format!("{value}{}", $suffix)),
            )
            .child(plus);

        $this.row($label, $hint, control)
    }};
}

// Pages come after the macros so they can use them.
mod appearance;
mod general;
mod mouse_borders;
mod plugins;
mod web;

impl Focusable for SettingsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let page = self.page;

        let content = match page {
            Page::Appearance => self.appearance_page(cx),
            Page::Search => self.plugins_page(cx),
            Page::Web => self.web_page(cx),
            Page::MouseBorders => self.mouse_borders_page(cx),
            Page::General => self.general_page(cx),
        };

        let sidebar = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(212.))
            .h_full()
            .gap_1()
            .p_3()
            .bg(rgba(pal.subtle))
            .border_r_1()
            .border_color(rgba(pal.divider))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .px_3()
                    .pt_2()
                    .pb_4()
                    .child(div().text_lg().text_color(rgb(pal.text)).child("Aujar"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_dim))
                            .child("Settings"),
                    ),
            )
            .children(Page::ALL.map(|page| self.nav_item(page, cx)))
            .child(div().flex_1())
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(pal.text_faint))
                    .child(
                        self.status
                            .clone()
                            .unwrap_or_else(|| "Changes save automatically".to_owned()),
                    ),
            );

        let header = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_6()
            .py_4()
            .border_b_1()
            .border_color(rgba(pal.divider))
            .child(div().text_xl().child(page.title()))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(pal.text_dim))
                    .child(page.blurb()),
            );

        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .flex_row()
            .size_full()
            .bg(rgba(pal.card))
            .text_color(rgb(pal.text))
            .child(sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .child(header)
                    .child(
                        div()
                            .id(SharedString::from(format!("scroll-{page:?}")))
                            .flex()
                            .flex_col()
                            .flex_1()
                            .gap_5()
                            .p_6()
                            .overflow_y_scroll()
                            .children(content),
                    ),
            )
    }
}
