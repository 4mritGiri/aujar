//! The settings window: a macOS-style window with its own title bar (traffic lights, sidebar
//! toggle, back/forward, a breadcrumb pill, search and a menu), a sidebar of pages and a
//! stack of grouped rows.
//!
//! Every change is applied and saved immediately to `~/.config/aujar/settings.toml`; the
//! launcher re-reads it each time it opens, and the daemon reloads its plugins and Mouse
//! Borders when the file changes.
//!
//! The window draws its own frame (client-side decorations): the title bar moves the window
//! (double-click zooms it), the edges resize it, and the corner radius, opacity and blur come
//! from the Appearance settings.
//!
//! This file is the shell (state, title bar, sidebar and the building blocks); each page
//! lives in `settings_view/<page>.rs`.

use crate::theme::{Palette, rounded_px};
use aujar_config::Settings;
use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, KeyDownEvent, MouseButton, ResizeEdge,
    SharedString, Stateful, Window, WindowBackgroundAppearance, div, prelude::*, px, rgb, rgba,
    svg,
};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Appearance,
    Search,
    Web,
    MouseBorders,
    Files,
    History,
    General,
}

impl Page {
    /// The top group of the sidebar.
    const MAIN: [Page; 4] = [
        Page::Appearance,
        Page::Search,
        Page::Web,
        Page::MouseBorders,
    ];
    /// The collapsible "Data" group.
    const DATA: [Page; 2] = [Page::Files, Page::History];
    /// Every page, in sidebar order (used by search).
    const ALL: [Page; 7] = [
        Page::Appearance,
        Page::Search,
        Page::Web,
        Page::MouseBorders,
        Page::Files,
        Page::History,
        Page::General,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Search => "Search plugins",
            Self::Web => "Web search",
            Self::MouseBorders => "Mouse Borders",
            Self::Files => "Files",
            Self::History => "History",
            Self::General => "General",
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Self::Appearance => "Colors, window style, and what the launcher shows",
            Self::Search => "Choose what the launcher can find and answer",
            Self::Web => "Where the \"search the web\" row and keyword shortcuts go",
            Self::MouseBorders => "Use one keyboard and mouse across your computers",
            Self::Files => "Which folders the launcher looks through",
            Self::History => "Recent items and the clipboard",
            Self::General => "Shortcuts, config file and about",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Appearance => "icons/sliders.svg",
            Self::Search => "icons/search.svg",
            Self::Web => "icons/globe.svg",
            Self::MouseBorders => "icons/mouse.svg",
            Self::Files => "icons/folder.svg",
            Self::History => "icons/history.svg",
            Self::General => "icons/app.svg",
        }
    }

    /// Words that should find this page in the settings search.
    fn keywords(self) -> &'static str {
        match self {
            Self::Appearance => {
                "theme dark light accent color colour opacity transparency translucent blur \
                 corner radius rounded traffic lights window buttons width results icons footer \
                 hints preview recent frequent close click away move drag movable position \
                 launcher spotlight top center"
            }
            Self::Search => {
                "plugins applications apps files folders windows system clipboard calculator \
                 units currency dictionary weather stocks sports shell online"
            }
            Self::Web => "web search engine duckduckgo google brave bing startpage shortcuts keyword",
            Self::MouseBorders => "mouse keyboard share machines network clipboard files cursor",
            Self::Files => "files index indexed roots depth folders directories",
            Self::History => "history recent frequent clear clipboard clip launch",
            Self::General => "general shortcut keyboard keys config file about version",
        }
    }

    /// Every word of `query` appears in the title or keywords.
    fn matches(self, query: &str) -> bool {
        let haystack = format!("{} {}", self.title(), self.keywords()).to_lowercase();

        query
            .to_lowercase()
            .split_whitespace()
            .all(|word| haystack.contains(word))
    }
}

/// Entries of the "..." menu in the title bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    Search,
    OpenConfig,
    ClearHistory,
    Shortcuts,
    About,
}

impl MenuAction {
    const ALL: [(MenuAction, &'static str, &'static str); 5] = [
        (MenuAction::Search, "Search settings", "Ctrl+F"),
        (MenuAction::OpenConfig, "Open config file", ""),
        (MenuAction::ClearHistory, "Clear launch history", ""),
        (MenuAction::Shortcuts, "Keyboard shortcuts", ""),
        (MenuAction::About, "About Aujar", ""),
    ];
}

const MENU_W: f32 = 236.0;
const TITLEBAR_H: f32 = 52.0;
/// Thickness of the invisible resize borders.
const RESIZE_EDGE: f32 = 5.0;
const RESIZE_CORNER: f32 = 12.0;

pub struct SettingsView {
    settings: Settings,
    palette: Palette,
    status: Option<String>,
    page: Page,
    mouse_borders: mouse_borders::State,
    pub focus_handle: FocusHandle,
    sidebar_open: bool,
    data_open: bool,
    back: Vec<Page>,
    forward: Vec<Page>,
    menu_open: bool,
    /// `Some` while the settings search is open.
    search: Option<String>,
    /// The blur setting last applied to the window.
    applied_blur: bool,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = Settings::load_or_default();

        Self::keep_mouse_borders_status_fresh(cx);

        Self {
            palette: Palette::from_settings(&settings),
            applied_blur: settings.appearance.blur,
            settings,
            status: None,
            page: Page::Appearance,
            mouse_borders: mouse_borders::State::default(),
            focus_handle: cx.focus_handle(),
            sidebar_open: true,
            data_open: true,
            back: Vec::new(),
            forward: Vec::new(),
            menu_open: false,
            search: None,
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

    // ---- Navigation ----

    fn show_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;

        if page == Page::MouseBorders {
            self.refresh_mouse_borders(cx);
        }

        cx.notify();
    }

    /// Go to `page` and remember where we came from (for the back button).
    fn navigate(&mut self, page: Page, cx: &mut Context<Self>) {
        if page != self.page {
            self.back.push(self.page);
            self.forward.clear();
        }

        self.show_page(page, cx);
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(previous) = self.back.pop() {
            self.forward.push(self.page);
            self.show_page(previous, cx);
        }
    }

    fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = self.forward.pop() {
            self.back.push(self.page);
            self.show_page(next, cx);
        }
    }

    // ---- Search and menu ----

    fn open_search(&mut self, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.sidebar_open = true;
        self.search = Some(String::new());
        cx.notify();
    }

    fn close_search(&mut self, cx: &mut Context<Self>) {
        self.search = None;
        cx.notify();
    }

    /// The pages the search currently lets through (all of them when it is closed or empty).
    fn visible(&self, page: Page) -> bool {
        match self.search.as_deref().map(str::trim) {
            Some(query) if !query.is_empty() => page.matches(query),
            _ => true,
        }
    }

    fn first_match(&self) -> Option<Page> {
        Page::ALL.into_iter().find(|page| self.visible(*page))
    }

    fn run_menu(&mut self, action: MenuAction, cx: &mut Context<Self>) {
        self.menu_open = false;

        match action {
            MenuAction::Search => self.open_search(cx),
            MenuAction::OpenConfig => {
                // Make sure the file exists so the editor has something to open.
                self.mutate(cx, |_| {});

                let _ = Command::new("xdg-open").arg(Settings::path()).spawn();
            }
            MenuAction::ClearHistory => {
                self.status = Some(match aujar_config::History::default().save() {
                    Ok(()) => "History cleared".to_owned(),
                    Err(error) => format!("Could not clear: {error:#}"),
                });
                cx.notify();
            }
            MenuAction::Shortcuts | MenuAction::About => self.navigate(Page::General, cx),
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = &event.keystroke.modifiers;

        if self.menu_open {
            if key == "escape" {
                self.menu_open = false;
                cx.notify();
            }

            return;
        }

        if self.search.is_some() {
            match key {
                "escape" => self.search = None,
                "enter" => {
                    if let Some(page) = self.first_match() {
                        self.navigate(page, cx);
                        self.search = None;
                    }
                }
                "backspace" => {
                    if let Some(query) = self.search.as_mut() {
                        query.pop();
                    }
                }
                _ if !modifiers.control && !modifiers.alt && !modifiers.platform => {
                    let text = event
                        .keystroke
                        .key_char
                        .as_ref()
                        .filter(|text| !text.chars().any(char::is_control));

                    if let (Some(text), Some(query)) = (text, self.search.as_mut()) {
                        if query.chars().count() < 40 {
                            query.push_str(text);
                        }
                    }
                }
                _ => {}
            }

            cx.notify();
            return;
        }

        match key {
            "f" if modifiers.control => self.open_search(cx),
            "left" if modifiers.alt => self.go_back(cx),
            "right" if modifiers.alt => self.go_forward(cx),
            "escape" => window.remove_window(),
            _ => {}
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

        rounded_px(div(), 12.)
            .flex()
            .flex_col()
            .border_1()
            .border_color(rgba(pal.border))
            .bg(rgba(pal.subtle))
            .overflow_hidden()
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

    /// A keyboard key, drawn as a small keycap.
    fn keycap(&self, label: &str) -> impl IntoElement {
        let pal = self.palette;

        div()
            .px_2()
            .py_0p5()
            .rounded_md()
            .border_1()
            .border_color(rgba(pal.keycap_border))
            .bg(rgba(pal.subtle))
            .text_xs()
            .text_color(rgb(pal.text_dim))
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

    // ---- Title bar ----

    /// A title-bar icon button. Presses do not start a window drag.
    fn icon_button(
        &self,
        id: &'static str,
        glyph: &'static str,
        enabled: bool,
        active: bool,
    ) -> Stateful<gpui::Div> {
        let pal = self.palette;

        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(34.))
            .h(px(28.))
            .flex_shrink_0()
            .rounded_md()
            .when(active, |this| this.bg(rgba(pal.pill)))
            .when(enabled, |this| {
                this.cursor_pointer().hover(|style| style.bg(rgba(pal.hover)))
            })
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| cx.stop_propagation())
            .child(self.glyph(
                glyph,
                17.,
                if enabled { pal.text_dim } else { pal.text_faint },
            ))
    }

    /// One of the red / yellow / green window buttons.
    fn traffic_light(&self, id: &'static str, color: u32, active: bool) -> Stateful<gpui::Div> {
        let pal = self.palette;

        div()
            .id(id)
            .size(px(13.))
            .flex_shrink_0()
            .rounded_full()
            .cursor_pointer()
            .bg(rgb(if active { color } else { pal.switch_off }))
            .hover(|style| style.opacity(0.8))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| cx.stop_propagation())
    }

    /// The pill in the middle of the title bar: `Aujar / Page`, or the search field.
    fn title_pill(&self) -> AnyElement {
        let pal = self.palette;

        let pill = rounded_px(div(), 15.)
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_2()
            .flex_1()
            .min_w(px(0.))
            .h(px(30.))
            .px_4()
            .bg(rgba(pal.pill))
            .text_sm();

        match &self.search {
            Some(query) => pill
                .child(self.glyph("icons/search.svg", 15., pal.text_faint))
                .child(if query.is_empty() {
                    div()
                        .text_color(rgb(pal.text_faint))
                        .child("Search settings\u{2026}")
                } else {
                    div().text_color(rgb(pal.text)).child(query.clone())
                })
                .child(div().w(px(2.)).h(px(16.)).bg(rgb(pal.accent)))
                .into_any_element(),
            None => pill
                .child(div().text_color(rgb(pal.text_dim)).child("Aujar"))
                .child(div().text_color(rgb(pal.text_faint)).child("/"))
                .child(div().text_color(rgb(pal.text)).child(self.page.title()))
                .into_any_element(),
        }
    }

    fn titlebar(&self, active: bool, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;

        let lights = self.settings.appearance.traffic_lights.then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .pr_2()
                .child(self.traffic_light("light-close", 0xff5f57, active).on_click(
                    cx.listener(|_this, _event, window, _cx| window.remove_window()),
                ))
                .child(self.traffic_light("light-min", 0xfebc2e, active).on_click(
                    cx.listener(|_this, _event, window, _cx| window.minimize_window()),
                ))
                .child(self.traffic_light("light-zoom", 0x28c840, active).on_click(
                    cx.listener(|_this, _event, window, _cx| window.zoom_window()),
                ))
        });

        let can_back = !self.back.is_empty();
        let can_forward = !self.forward.is_empty();

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .flex_shrink_0()
            .h(px(TITLEBAR_H))
            .px_4()
            // The empty parts of the title bar move the window; double-click zooms it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, event: &gpui::MouseDownEvent, window, _cx| {
                    if event.click_count >= 2 {
                        window.zoom_window();
                    } else {
                        window.start_window_move();
                    }
                }),
            )
            .children(lights)
            .child(
                self.icon_button("tb-sidebar", "icons/sidebar.svg", true, !self.sidebar_open)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.sidebar_open = !this.sidebar_open;
                        cx.notify();
                    })),
            )
            .child(
                self.icon_button("tb-back", "icons/chevron-left.svg", can_back, false)
                    .on_click(cx.listener(|this, _event, _window, cx| this.go_back(cx))),
            )
            .child(
                self.icon_button("tb-forward", "icons/chevron-right.svg", can_forward, false)
                    .on_click(cx.listener(|this, _event, _window, cx| this.go_forward(cx))),
            )
            .child(self.title_pill())
            .child(
                self.icon_button("tb-search", "icons/search.svg", true, self.search.is_some())
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        if this.search.is_some() {
                            this.close_search(cx);
                        } else {
                            this.open_search(cx);
                        }
                    })),
            )
            .child(
                self.icon_button("tb-more", "icons/more.svg", true, self.menu_open)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.menu_open = !this.menu_open;
                        cx.notify();
                    })),
            )
            .border_b_1()
            .border_color(rgba(pal.divider))
            .into_any_element()
    }

    /// The "..." menu, with a transparent backdrop that dismisses it.
    fn render_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.menu_open {
            return None;
        }

        let pal = self.palette;

        let rows = MenuAction::ALL.iter().enumerate().map(|(index, (action, label, shortcut))| {
            let action = *action;

            div()
                .child(
                    div()
                        .id(SharedString::from(format!("menu-{index}")))
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .mx_1()
                        .px_3()
                        .h(px(32.))
                        .rounded_md()
                        .cursor_pointer()
                        .text_color(rgb(pal.text))
                        .hover(|style| style.bg(rgba(pal.hover)))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.run_menu(action, cx);
                        }))
                        .child(div().text_sm().child(*label))
                        .child(div().text_xs().text_color(rgb(pal.text_faint)).child(*shortcut)),
                )
                // A hairline between the editing entries and the navigation ones.
                .when(index == 2, |this| {
                    this.pb_1().border_b_1().border_color(rgba(pal.divider)).mb_1()
                })
        });

        let dismiss = |this: &mut Self, _event: &gpui::MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>| {
            this.menu_open = false;
            cx.notify();
        };

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .on_mouse_down(MouseButton::Left, cx.listener(dismiss))
                        .on_mouse_down(MouseButton::Right, cx.listener(dismiss)),
                )
                .child(
                    rounded_px(div(), 12.)
                        .absolute()
                        .top(px(TITLEBAR_H - 6.))
                        .right(px(14.))
                        .w(px(MENU_W))
                        .py_1()
                        .border_1()
                        .border_color(rgba(pal.border))
                        .bg(rgba(pal.menu))
                        .occlude()
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    // ---- Sidebar ----

    fn nav_item(&self, page: Page, indent: bool, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        let pal = self.palette;
        let selected = self.page == page;

        div()
            .id(SharedString::from(format!("nav-{page:?}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_3()
            .py(px(7.))
            .when(indent, |this| this.ml_4())
            .rounded_lg()
            .cursor_pointer()
            .text_sm()
            .when(selected, |this| {
                this.bg(rgba(pal.accent_soft)).text_color(rgb(pal.text))
            })
            .when(!selected, |this| {
                this.text_color(rgb(pal.text))
                    .hover(|style| style.bg(rgba(pal.hover)))
            })
            .child(self.glyph(
                page.icon(),
                17.,
                if selected { pal.accent_fg } else { pal.text_dim },
            ))
            .child(page.title())
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.navigate(page, cx);
            }))
    }

    fn divider(&self) -> AnyElement {
        div()
            .mx_3()
            .my_2()
            .h(px(1.))
            .bg(rgba(self.palette.divider))
            .into_any_element()
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;
        let searching = self.search.as_deref().is_some_and(|query| !query.trim().is_empty());

        let main: Vec<AnyElement> = Page::MAIN
            .into_iter()
            .filter(|page| self.visible(*page))
            .map(|page| self.nav_item(page, false, cx).into_any_element())
            .collect();

        let data: Vec<Page> = Page::DATA
            .into_iter()
            .filter(|page| self.visible(*page))
            .collect();
        // While searching, matching children are shown even when the group is collapsed.
        let data_shown = self.data_open || searching;

        let data_group = (!data.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .id("nav-data")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .px_3()
                        .py(px(7.))
                        .rounded_lg()
                        .cursor_pointer()
                        .text_sm()
                        .text_color(rgb(pal.text))
                        .hover(|style| style.bg(rgba(pal.hover)))
                        .child(self.glyph("icons/database.svg", 17., pal.text_dim))
                        .child(div().flex_1().child("Data"))
                        .child(self.glyph(
                            if data_shown { "icons/chevron-down.svg" } else { "icons/chevron-right.svg" },
                            14.,
                            pal.text_faint,
                        ))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.data_open = !this.data_open;
                            cx.notify();
                        })),
                )
                .children(
                    data_shown
                        .then(|| {
                            data.iter()
                                .map(|page| self.nav_item(*page, true, cx).into_any_element())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                )
        });

        let general = self
            .visible(Page::General)
            .then(|| self.nav_item(Page::General, false, cx));
        let nothing = self.search.is_some() && searching && self.first_match().is_none();

        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(232.))
            .h_full()
            .gap_1()
            .px_3()
            .pt_3()
            .pb_3()
            .bg(rgba(pal.sidebar))
            .border_r_1()
            .border_color(rgba(pal.divider))
            .children(main)
            .when(data_group.is_some(), |this| this.child(self.divider()))
            .children(data_group)
            .when(nothing, |this| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_sm()
                        .text_color(rgb(pal.text_faint))
                        .child("No matching settings"),
                )
            })
            .child(div().flex_1())
            .when(general.is_some(), |this| this.child(self.divider()))
            .children(general)
            .into_any_element()
    }

    // ---- Window edges ----

    /// Invisible strips around the window that resize it (the window draws its own frame).
    fn resize_handles(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let edge = RESIZE_EDGE;
        let corner = RESIZE_CORNER;

        let handle = |edge_kind: ResizeEdge, cx: &mut Context<Self>| {
            div().absolute().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_this, _event, window, _cx| window.start_window_resize(edge_kind)),
            )
        };

        vec![
            handle(ResizeEdge::Top, cx)
                .top_0()
                .left(px(corner))
                .right(px(corner))
                .h(px(edge))
                .cursor_ns_resize()
                .into_any_element(),
            handle(ResizeEdge::Bottom, cx)
                .bottom_0()
                .left(px(corner))
                .right(px(corner))
                .h(px(edge))
                .cursor_ns_resize()
                .into_any_element(),
            handle(ResizeEdge::Left, cx)
                .left_0()
                .top(px(corner))
                .bottom(px(corner))
                .w(px(edge))
                .cursor_ew_resize()
                .into_any_element(),
            handle(ResizeEdge::Right, cx)
                .right_0()
                .top(px(corner))
                .bottom(px(corner))
                .w(px(edge))
                .cursor_ew_resize()
                .into_any_element(),
            handle(ResizeEdge::TopLeft, cx)
                .top_0()
                .left_0()
                .size(px(corner))
                .cursor_nwse_resize()
                .into_any_element(),
            handle(ResizeEdge::TopRight, cx)
                .top_0()
                .right_0()
                .size(px(corner))
                .cursor_nesw_resize()
                .into_any_element(),
            handle(ResizeEdge::BottomLeft, cx)
                .bottom_0()
                .left_0()
                .size(px(corner))
                .cursor_nesw_resize()
                .into_any_element(),
            handle(ResizeEdge::BottomRight, cx)
                .bottom_0()
                .right_0()
                .size(px(corner))
                .cursor_nwse_resize()
                .into_any_element(),
        ]
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
mod files;
mod general;
mod history;
mod mouse_borders;
mod plugins;
mod web;

impl Focusable for SettingsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Blur is a window property, so apply a changed setting here, where we have the window.
        if self.applied_blur != self.settings.appearance.blur {
            self.applied_blur = self.settings.appearance.blur;

            window.set_background_appearance(if self.applied_blur {
                WindowBackgroundAppearance::Blurred
            } else {
                WindowBackgroundAppearance::Transparent
            });
        }

        let pal = self.palette;
        let page = self.page;
        let active = window.is_window_active();
        // A maximized or fullscreen window fills the screen: no rounded corners, no edges.
        let filled = window.is_maximized() || window.is_fullscreen();

        let content = match page {
            Page::Appearance => self.appearance_page(cx),
            Page::Search => self.plugins_page(cx),
            Page::Web => self.web_page(cx),
            Page::MouseBorders => self.mouse_borders_page(cx),
            Page::Files => self.files_page(cx),
            Page::History => self.history_page(cx),
            Page::General => self.general_page(cx),
        };

        let titlebar = self.titlebar(active, cx);
        let menu = self.render_menu(cx);
        let handles = if filled { Vec::new() } else { self.resize_handles(cx) };

        let header = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .px_6()
            .pt_5()
            .pb_1()
            .child(
                div()
                    .text_2xl()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(page.title()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(pal.text_dim))
                    .child(page.blurb()),
            );

        let status = div()
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .h(px(34.))
            .border_t_1()
            .border_color(rgba(pal.divider))
            .text_xs()
            .text_color(rgb(pal.text_faint))
            .child(
                self.status
                    .clone()
                    .unwrap_or_else(|| "Changes save automatically".to_owned()),
            );

        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.))
            .when(self.sidebar_open, |this| this.child(self.sidebar(cx)))
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
                            .px_6()
                            .py_4()
                            .overflow_y_scroll()
                            .children(content),
                    )
                    .child(status),
            );

        let frame = rounded_px(div(), if filled { 0. } else { pal.radius })
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(rgba(pal.card))
            .when(!filled, |this| this.border_1().border_color(rgba(pal.border)))
            .text_color(rgb(pal.text))
            .child(titlebar)
            .child(body);

        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .relative()
            .size_full()
            .child(frame)
            .children(handles)
            .when_some(menu, |this, menu| this.child(menu))
    }
}
