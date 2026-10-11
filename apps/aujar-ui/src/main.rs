//! Aujar launcher UI (GPUI): a Spotlight-style search window with PowerToys-style
//! plugins, and a settings window.
//!
//! Design: the UI holds **no search logic**. A background thread (tokio) talks to the
//! daemon over IPC; the GPUI thread only renders state and forwards keystrokes.
//!
//! Keys: type to search, Up/Down/Tab to select, Enter to launch, Ctrl+Enter to show a
//! file in its folder, Ctrl+C to copy, Alt+1..9 to launch a row, Esc to clear/close.
//! Limitation: plain key events only (no IME / composition yet).

mod settings_view;
mod theme;

use anyhow::{Result, bail};
use aujar_config::Settings;
use aujar_ipc::{
    Event, ExecuteRequest, LauncherCommand, Request, Response, SearchHit, SearchRequest, send,
    subscribe,
};
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, FocusHandle, Focusable, KeyDownEvent,
    SharedString, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowOptions,
    div, img, prelude::*, px, rgb, rgba, size,
};
use gpui_platform::application;
use settings_view::SettingsView;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use theme::Palette;

/// Window height; the card inside is content-sized and the rest is transparent.
const WINDOW_H: f32 = 580.0;

/// Bridge thread -> UI thread.
enum UiMessage {
    Launcher(LauncherCommand),
    Results {
        query: String,
        results: Vec<SearchHit>,
    },
    Failed(String),
    OpenSettings,
}

/// UI thread -> bridge thread.
enum UiRequest {
    Search(String),
    Execute(String),
    OpenSettings,
}

// ---- Sections ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Answer,
    Applications,
    Commands,
    Files,
    Web,
}

fn section_of(kind: &str) -> Section {
    match kind {
        "calculation" => Section::Answer,
        "application" => Section::Applications,
        "system" | "command" | "action" => Section::Commands,
        "file" | "folder" => Section::Files,
        _ => Section::Web,
    }
}

fn section_title(section: Section) -> &'static str {
    match section {
        Section::Answer => "Answer",
        Section::Applications => "Applications",
        Section::Commands => "Commands",
        Section::Files => "Files & Folders",
        Section::Web => "Web",
    }
}

/// (badge, Enter-action label) for a result kind.
fn kind_meta(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "application" => ("App", "Open"),
        "calculation" => ("Answer", "Copy"),
        "file" => ("File", "Open"),
        "folder" => ("Folder", "Open"),
        "system" => ("System", "Run"),
        "command" => ("Command", "Run"),
        "action" => ("Aujar", "Open"),
        _ => ("Web", "Open"),
    }
}

/// Results that end the session or the machine ask for a second Enter.
fn is_dangerous(id: &str) -> bool {
    matches!(id, "system:restart" | "system:shutdown" | "system:logout")
}

// ---- Web helpers ------------------------------------------------------------------------

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }

    out
}

/// `github.com/rust-lang`, `https://example.org/x` -> a full URL; anything else -> `None`.
fn url_from_query(query: &str) -> Option<String> {
    let query = query.trim();

    if query.contains(char::is_whitespace) || query.len() < 4 {
        return None;
    }

    if query.starts_with("http://") || query.starts_with("https://") {
        return Some(query.to_owned());
    }

    let host = query.split('/').next()?;
    let (name, tld) = host.rsplit_once('.')?;

    let looks_like_host = !name.is_empty()
        && tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic())
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');

    looks_like_host.then(|| format!("https://{query}"))
}

fn is_web_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

fn web_hit(title: String, subtitle: String, url: String) -> SearchHit {
    SearchHit {
        id: format!("url:{url}"),
        title,
        subtitle: Some(subtitle),
        kind: "web".to_owned(),
        score: 0,
        icon: None,
    }
}

// ---- Launcher view ----------------------------------------------------------------------

struct LauncherView {
    query: String,
    results: Vec<SearchHit>,
    selected: usize,
    status: Option<String>,
    /// Id of a dangerous result that is waiting for a second Enter.
    confirming: Option<String>,
    settings: Settings,
    palette: Palette,
    requests: async_channel::Sender<UiRequest>,
    focus_handle: FocusHandle,
}

impl LauncherView {
    fn new(
        requests: async_channel::Sender<UiRequest>,
        settings: Settings,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            status: None,
            confirming: None,
            palette: Palette::from_settings(&settings),
            settings,
            requests,
            focus_handle: cx.focus_handle(),
        }
    }

    fn request_search(&mut self) {
        self.selected = 0;
        self.status = None;
        self.confirming = None;

        if self.query.trim().is_empty() {
            self.results.clear();
            return;
        }

        let _ = self.requests.try_send(UiRequest::Search(self.query.clone()));
    }

    /// Add the UI-side results (settings, web), group into sections and cap each section.
    fn set_results(&mut self, mut hits: Vec<SearchHit>) {
        let query = self.query.trim().to_owned();
        let lower = query.to_lowercase();
        let is_shell = query.starts_with('>');

        if !is_shell && lower.len() >= 3 && "aujar settings preferences".contains(&lower) {
            hits.push(SearchHit {
                id: "ui:settings".to_owned(),
                title: "Aujar Settings".to_owned(),
                subtitle: Some("Appearance, plugins and web search".to_owned()),
                kind: "action".to_owned(),
                score: 0,
                icon: None,
            });
        }

        if !is_shell && !query.is_empty() {
            let engine = self.settings.web.engine;
            let mut web = Vec::new();

            if let Some(url) = url_from_query(&query) {
                web.push(web_hit(format!("Open {query}"), url.clone(), url));
            }

            // `g rust lifetimes`, `yt lofi`, ... and `? anything` for the default engine.
            if let Some((keyword, rest)) = query.split_once(char::is_whitespace) {
                let rest = rest.trim();

                if !rest.is_empty() {
                    if keyword == "?" {
                        web.push(web_hit(
                            format!("Search {} for \u{201c}{rest}\u{201d}", engine.name()),
                            "Default search engine".to_owned(),
                            format!("{}{}", engine.url(), percent_encode(rest)),
                        ));
                    } else if let Some(shortcut) = self
                        .settings
                        .web
                        .shortcuts
                        .iter()
                        .find(|shortcut| shortcut.keyword.eq_ignore_ascii_case(keyword))
                    {
                        web.push(web_hit(
                            format!("Search {} for \u{201c}{rest}\u{201d}", shortcut.name),
                            format!("Shortcut `{}`", shortcut.keyword),
                            format!("{}{}", shortcut.url, percent_encode(rest)),
                        ));
                    }
                }
            }

            web.push(web_hit(
                format!("Search {} for \u{201c}{query}\u{201d}", engine.name()),
                "Opens your default browser".to_owned(),
                format!("{}{}", engine.url(), percent_encode(&query)),
            ));

            hits.extend(web);
        }

        // Stable sort keeps the daemon's ranking inside each section.
        hits.sort_by_key(|hit| section_of(&hit.kind) as u8);

        let apps_cap = self.settings.appearance.max_results as usize;
        let mut counts = [0usize; 5];

        hits.retain(|hit| {
            let section = section_of(&hit.kind);
            let cap = match section {
                Section::Answer => 2,
                Section::Applications => apps_cap,
                Section::Commands => 4,
                Section::Files => apps_cap.min(6),
                Section::Web => 3,
            };

            counts[section as usize] += 1;
            counts[section as usize] <= cap
        });

        self.results = hits;
        self.selected = 0;
        self.status = None;
        self.confirming = None;
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.results.len() as isize;

        if len > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
            self.confirming = None;
            cx.notify();
        }
    }

    /// What Ctrl+C copies for a result: a path, a URL, or the answer itself.
    fn copy_text(hit: &SearchHit) -> String {
        if let Some(path) = hit.id.strip_prefix("files:open:") {
            return path.to_owned();
        }

        if let Some(url) = hit.id.strip_prefix("url:") {
            return url.to_owned();
        }

        hit.title.clone()
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = &keystroke.modifiers;
        let key = keystroke.key.as_str();

        // Alt+1..9 launches that row directly (Spotlight's Cmd+1..9).
        if modifiers.alt {
            if let Some(number) = key.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
                if number <= self.results.len() {
                    self.selected = number - 1;
                    self.launch_selected(false, window, cx);
                }
                return;
            }
        }

        // Emacs/readline-style chords and copy.
        if modifiers.control {
            match key {
                "n" | "j" => self.move_selection(1, cx),
                "p" | "k" => self.move_selection(-1, cx),
                "enter" => self.launch_selected(true, window, cx),
                "c" => {
                    if let Some(hit) = self.results.get(self.selected) {
                        cx.write_to_clipboard(ClipboardItem::new_string(Self::copy_text(hit)));
                        window.remove_window();
                    }
                }
                "u" => {
                    self.query.clear();
                    self.request_search();
                    cx.notify();
                }
                "w" => {
                    let trimmed = self.query.trim_end().len();
                    let keep = self.query[..trimmed]
                        .rfind(char::is_whitespace)
                        .map_or(0, |index| index + 1);
                    self.query.truncate(keep);
                    self.request_search();
                    cx.notify();
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        let line = text.lines().next().unwrap_or_default();
                        self.query.extend(line.chars().filter(|c| !c.is_control()));
                        self.request_search();
                        cx.notify();
                    }
                }
                _ => {}
            }
            return;
        }

        match key {
            "escape" => {
                // First Esc clears the query (like Spotlight), second closes.
                if self.query.is_empty() {
                    window.remove_window();
                } else {
                    self.query.clear();
                    self.request_search();
                    cx.notify();
                }
                return;
            }
            "enter" => {
                self.launch_selected(false, window, cx);
                return;
            }
            "down" => {
                self.move_selection(1, cx);
                return;
            }
            "up" => {
                self.move_selection(-1, cx);
                return;
            }
            "tab" => {
                self.move_selection(if modifiers.shift { -1 } else { 1 }, cx);
                return;
            }
            "backspace" => {
                self.query.pop();
                self.request_search();
                cx.notify();
                return;
            }
            _ => {}
        }

        // Other chords (Super, ...) are not text.
        if modifiers.alt || modifiers.platform {
            return;
        }

        if let Some(text) = keystroke
            .key_char
            .as_ref()
            .filter(|text| !text.chars().any(char::is_control))
        {
            self.query.push_str(text);
            self.request_search();
            cx.notify();
        }
    }

    /// Enter (or Ctrl+Enter when `reveal` is set: show a file in its folder).
    fn launch_selected(&mut self, reveal: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hit) = self.results.get(self.selected).cloned() else {
            return;
        };

        if is_dangerous(&hit.id) && self.confirming.as_deref() != Some(hit.id.as_str()) {
            self.confirming = Some(hit.id);
            cx.notify();
            return;
        }

        match hit.kind.as_str() {
            // Calculations are not launchable: copy the answer instead. The UI process stays
            // alive, so it keeps owning the Wayland clipboard after the window closes.
            "calculation" => {
                cx.write_to_clipboard(ClipboardItem::new_string(hit.title.clone()));
            }
            "web" => {
                if let Some(url) = hit.id.strip_prefix("url:").filter(|url| is_web_url(url)) {
                    cx.open_url(url);
                }
            }
            "action" => {
                let _ = self.requests.try_send(UiRequest::OpenSettings);
            }
            "file" | "folder" if reveal => {
                if let Some(path) = hit.id.strip_prefix("files:open:") {
                    let _ = self
                        .requests
                        .try_send(UiRequest::Execute(format!("files:reveal:{path}")));
                }
            }
            _ => {
                let _ = self.requests.try_send(UiRequest::Execute(hit.id.clone()));
            }
        }

        window.remove_window();
    }

    // ---- Rendering ----

    fn leading(&self, hit: &SearchHit) -> AnyElement {
        if self.settings.appearance.show_icons {
            if let Some(icon) = hit.icon.as_deref() {
                return img(PathBuf::from(icon))
                    .size(px(36.))
                    .flex_shrink_0()
                    .into_any_element();
            }
        }

        let (color, glyph) = match hit.kind.as_str() {
            "folder" => (0xd9a23a, "F".to_owned()),
            "file" => (0x6b7a90, "D".to_owned()),
            "system" => (0xc2414f, "S".to_owned()),
            "command" => (0x3d3d48, ">".to_owned()),
            "action" => (self.settings.appearance.accent.rgb(), "A".to_owned()),
            "web" => (0x2f8fd8, "\u{2197}".to_owned()),
            _ => {
                const PALETTE: [u32; 6] =
                    [0x5e5ce6, 0x30a46c, 0xe5732d, 0xd6409f, 0x2f8fd8, 0x8e6bd8];
                let initial = hit
                    .title
                    .chars()
                    .find(|c| c.is_alphanumeric())
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_else(|| "?".to_owned());
                let sum: usize = hit.title.bytes().map(usize::from).sum();

                (PALETTE[sum % PALETTE.len()], initial)
            }
        };

        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(36.))
            .flex_shrink_0()
            .rounded_lg()
            .bg(rgb(color))
            .text_color(rgb(0xffffff))
            .text_lg()
            .child(glyph)
            .into_any_element()
    }

    fn keycap(&self, label: &str) -> impl IntoElement {
        div()
            .px_2()
            .py_0p5()
            .rounded_md()
            .border_1()
            .border_color(rgba(self.palette.keycap_border))
            .bg(rgba(self.palette.subtle))
            .text_xs()
            .text_color(rgb(self.palette.text_dim))
            .child(label.to_owned())
    }

    fn render_row(&self, index: usize, hit: &SearchHit, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;
        let selected = index == self.selected;
        let dangerous = is_dangerous(&hit.id);
        let confirming = self.confirming.as_deref() == Some(hit.id.as_str());

        let title_color = if selected { rgb(pal.on_accent) } else { rgb(pal.text) };
        let sub_color = if selected { rgba(pal.on_accent_dim) } else { rgb(pal.text_dim) };

        let base = div()
            .id(SharedString::from(format!("row-{index}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .mx_2()
            .px_3()
            .rounded_lg()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event, window, cx| {
                this.selected = index;
                this.launch_selected(false, window, cx);
            }));

        let base = if selected {
            base.bg(rgb(if dangerous { pal.danger } else { pal.accent }))
        } else {
            base.hover(|style| style.bg(rgba(pal.hover)))
        };

        let (badge, _) = kind_meta(&hit.kind);
        let trailing = if selected {
            if confirming { "Enter again".to_owned() } else { "Enter".to_owned() }
        } else if index < 9 {
            format!("Alt+{}", index + 1)
        } else {
            badge.to_owned()
        };

        if hit.kind == "calculation" {
            // Answer card: the result is the star, like Spotlight's calculator.
            return base
                .py_3()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(sub_color)
                                .child(hit.subtitle.clone().unwrap_or_else(|| "Answer".to_owned())),
                        )
                        .child(div().text_3xl().text_color(title_color).child(hit.title.clone())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(sub_color)
                        .child(if selected { "Enter to copy".to_owned() } else { trailing }),
                )
                .into_any_element();
        }

        let subtitle = if confirming {
            Some("Press Enter again to confirm".to_owned())
        } else {
            hit.subtitle.clone()
        };

        base.py_2()
            .child(self.leading(hit))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .child(div().text_base().text_color(title_color).truncate().child(hit.title.clone()))
                    .when_some(subtitle, |this, subtitle| {
                        this.child(div().text_xs().text_color(sub_color).truncate().child(subtitle))
                    }),
            )
            .child(div().text_xs().text_color(sub_color).child(trailing))
            .into_any_element()
    }

    fn render_footer(&self) -> impl IntoElement {
        let pal = self.palette;
        let mut hints: Vec<(&'static str, &'static str)> = Vec::new();

        match self.results.get(self.selected) {
            Some(hit) => {
                hints.push(("Enter", kind_meta(&hit.kind).1));

                match hit.kind.as_str() {
                    "file" | "folder" => {
                        hints.push(("Ctrl+Enter", "Show in folder"));
                        hints.push(("Ctrl+C", "Copy path"));
                    }
                    "calculation" => {}
                    _ => hints.push(("Ctrl+C", "Copy")),
                }

                hints.push(("Alt+1-9", "Quick open"));
            }
            None => {
                hints.push(("g query", "Web shortcut"));
                hints.push(("> cmd", "Run command"));
                hints.push(("10 km to mi", "Convert"));
            }
        }

        hints.push(("Esc", "Close"));

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .px_5()
            .py_2()
            .border_t_1()
            .border_color(rgba(pal.divider))
            .bg(rgba(pal.subtle))
            .children(hints.into_iter().map(|(keys, label)| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .child(self.keycap(keys))
                    .child(div().text_xs().text_color(rgb(pal.text_dim)).child(label))
            }))
    }
}

impl Focusable for LauncherView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LauncherView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;

        let (query_line, query_color) = if self.query.is_empty() {
            ("Search apps, files, math and the web".to_owned(), rgb(pal.text_dim))
        } else {
            (format!("{}\u{258F}", self.query), rgb(pal.text))
        };

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut last_section: Option<Section> = None;

        for index in 0..self.results.len() {
            let hit = self.results[index].clone();
            let section = section_of(&hit.kind);

            if last_section != Some(section) {
                rows.push(
                    div()
                        .px_5()
                        .pt_3()
                        .pb_1()
                        .text_xs()
                        .text_color(rgb(pal.text_dim))
                        .child(section_title(section))
                        .into_any_element(),
                );
                last_section = Some(section);
            }

            rows.push(self.render_row(index, &hit, cx));
        }

        let show_footer = self.settings.appearance.show_footer;

        // The window is transparent: only this card is painted, so with no results it
        // collapses to the search bar (plus the hint bar), like Spotlight.
        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .flex_col()
            .w_full()
            .bg(rgba(pal.card))
            .border_1()
            .border_color(rgba(pal.border))
            .rounded_xl()
            .overflow_hidden()
            .text_color(rgb(pal.text))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_5()
                    .py_4()
                    .child(
                        div()
                            .size(px(18.))
                            .flex_shrink_0()
                            .rounded_full()
                            .border_2()
                            .border_color(rgb(pal.text_dim)),
                    )
                    .child(div().flex_1().text_2xl().text_color(query_color).child(query_line))
                    .child(self.keycap("Esc")),
            )
            .when(!rows.is_empty(), |this| {
                this.child(div().h(px(1.)).w_full().bg(rgba(pal.divider)))
                    .child(div().flex().flex_col().pb_2().children(rows))
            })
            .when_some(self.status.clone(), |this, status| {
                this.child(
                    div()
                        .px_5()
                        .pb_3()
                        .text_sm()
                        .text_color(rgb(pal.danger))
                        .child(status),
                )
            })
            .when(show_footer, |this| this.child(self.render_footer()))
    }
}

fn open_launcher(
    cx: &mut App,
    requests: async_channel::Sender<UiRequest>,
) -> Option<WindowHandle<LauncherView>> {
    // Re-read the settings every time, so changes apply on the next open.
    let settings = Settings::load_or_default();
    let width = settings.appearance.width as f32;
    let bounds = Bounds::centered(None, size(px(width), px(WINDOW_H)), cx);

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("aujar-launcher".into()),
        ..Default::default()
    };

    cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| LauncherView::new(requests, settings, cx));
        let focus_handle = view.read(cx).focus_handle.clone();

        // Without focus, key events never reach the view.
        window.focus(&focus_handle, cx);

        view
    })
    .ok()
}

fn open_settings(cx: &mut App) -> Option<WindowHandle<SettingsView>> {
    let bounds = Bounds::centered(None, size(px(760.), px(640.)), cx);

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        app_id: Some("aujar-settings".into()),
        ..Default::default()
    };

    cx.open_window(options, |window, cx| {
        let view = cx.new(SettingsView::new);
        let focus_handle = view.read(cx).focus_handle.clone();

        window.focus(&focus_handle, cx);

        view
    })
    .ok()
}

// ---- Window bookkeeping -----------------------------------------------------------------

#[derive(Default)]
struct UiState {
    launcher: Option<WindowHandle<LauncherView>>,
    settings: Option<WindowHandle<SettingsView>>,
}

fn handle_message(
    cx: &mut App,
    state: &mut UiState,
    requests: &async_channel::Sender<UiRequest>,
    message: UiMessage,
) {
    // Forget handles of windows the user already closed (Esc, Enter, window manager).
    if state
        .launcher
        .as_ref()
        .is_some_and(|handle| handle.update(cx, |_, _, _| ()).is_err())
    {
        state.launcher = None;
    }

    if state
        .settings
        .as_ref()
        .is_some_and(|handle| handle.update(cx, |_, _, _| ()).is_err())
    {
        state.settings = None;
    }

    match message {
        UiMessage::Launcher(LauncherCommand::Settings) | UiMessage::OpenSettings => {
            // Replace any existing settings window: it re-reads the file and comes to front.
            if let Some(handle) = state.settings.take() {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }

            state.settings = open_settings(cx);
        }
        UiMessage::Launcher(command) => {
            let is_open = state.launcher.is_some();

            let want_open = match command {
                LauncherCommand::Show => true,
                LauncherCommand::Hide => false,
                _ => !is_open,
            };

            match (is_open, want_open) {
                (false, true) => state.launcher = open_launcher(cx, requests.clone()),
                (true, false) => {
                    if let Some(handle) = state.launcher.take() {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    }
                }
                _ => {}
            }
        }
        UiMessage::Results { query, results } => {
            if let Some(handle) = state.launcher.as_ref() {
                let _ = handle.update(cx, |view, _, cx| {
                    // Ignore answers for a query the user has already edited.
                    if view.query == query {
                        view.set_results(results);
                        cx.notify();
                    }
                });
            }
        }
        UiMessage::Failed(message) => {
            if let Some(handle) = state.launcher.as_ref() {
                let _ = handle.update(cx, |view, _, cx| {
                    view.status = Some(message);
                    cx.notify();
                });
            }
        }
    }
}

// ---- Daemon bridge ----------------------------------------------------------------------

fn socket_path() -> PathBuf {
    if let Some(socket) = std::env::var_os("AUJAR_SOCKET") {
        return PathBuf::from(socket);
    }

    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join("aujar.sock"),
        None => PathBuf::from("/tmp/aujar.sock"),
    }
}

async fn search(socket: &Path, query: &str) -> Result<Vec<SearchHit>> {
    let request = Request::Search(SearchRequest {
        query: query.to_owned(),
        // The UI caps each section itself; ask for enough to fill all of them.
        limit: Some(40),
    });

    match send(socket, request).await? {
        Response::Search(response) => Ok(response.results),
        Response::Error(message) => bail!(message),
        other => bail!("unexpected response: {other:?}"),
    }
}

async fn execute(socket: &Path, id: &str) -> Result<()> {
    let request = Request::Execute(ExecuteRequest { id: id.to_owned() });

    match send(socket, request).await? {
        Response::Executed(done) => {
            eprintln!("aujar-ui: launched {}", done.title);
            Ok(())
        }
        Response::Error(message) => bail!(message),
        other => bail!("unexpected response: {other:?}"),
    }
}

/// Handles search/execute requests from the UI, one fresh connection each.
async fn serve_requests(
    socket: PathBuf,
    tx: async_channel::Sender<UiMessage>,
    requests: async_channel::Receiver<UiRequest>,
) {
    while let Ok(request) = requests.recv().await {
        match request {
            UiRequest::Search(query) => {
                let message = match search(&socket, &query).await {
                    Ok(results) => UiMessage::Results { query, results },
                    Err(error) => UiMessage::Failed(format!("{error:#}")),
                };

                if tx.send(message).await.is_err() {
                    break;
                }
            }
            UiRequest::Execute(id) => {
                if let Err(error) = execute(&socket, &id).await {
                    eprintln!("aujar-ui: launch failed: {error:#}");
                }
            }
            UiRequest::OpenSettings => {
                if tx.send(UiMessage::OpenSettings).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// One subscription to the daemon's event stream.
async fn run_session(socket: &Path, tx: &async_channel::Sender<UiMessage>) -> Result<()> {
    let mut stream = subscribe(socket).await?;

    eprintln!("aujar-ui: subscribed to the daemon");

    while let Some(event) = stream.recv().await? {
        match event {
            Event::Launcher(command) => {
                if tx.send(UiMessage::Launcher(command)).await.is_err() {
                    return Ok(());
                }
            }
            Event::Shutdown => break,
        }
    }

    Ok(())
}

/// Talks to the daemon on its own thread; reconnects if the daemon restarts.
fn spawn_bridge(
    socket: PathBuf,
    tx: async_channel::Sender<UiMessage>,
    requests: async_channel::Receiver<UiRequest>,
) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");

        runtime.block_on(async move {
            tokio::spawn(serve_requests(socket.clone(), tx.clone(), requests));

            loop {
                if let Err(error) = run_session(&socket, &tx).await {
                    eprintln!("aujar-ui: {error:#} (retrying)");
                }

                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    });
}

fn main() {
    let (message_tx, message_rx) = async_channel::unbounded::<UiMessage>();
    let (request_tx, request_rx) = async_channel::unbounded::<UiRequest>();

    spawn_bridge(socket_path(), message_tx, request_rx);

    // Closing the launcher window (Esc / Enter) must not end the process: the UI has to
    // stay alive to receive the next toggle. By default GPUI quits with the last window.
    application()
        .with_quit_mode(gpui::QuitMode::Explicit)
        .run(move |cx: &mut App| {
            cx.spawn(async move |cx| {
                let mut state = UiState::default();

                while let Ok(message) = message_rx.recv().await {
                    let _ = cx.update(|cx| handle_message(cx, &mut state, &request_tx, message));
                }
            })
            .detach();
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_urls() {
        assert_eq!(url_from_query("github.com/rust-lang").as_deref(), Some("https://github.com/rust-lang"));
        assert_eq!(url_from_query("https://example.org").as_deref(), Some("https://example.org"));
        assert_eq!(url_from_query("3.14"), None);
        assert_eq!(url_from_query("hello world.com"), None);
        assert_eq!(url_from_query("firefox"), None);
    }

    #[test]
    fn encodes_queries() {
        assert_eq!(percent_encode("rust lifetimes"), "rust+lifetimes");
        assert_eq!(percent_encode("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn only_power_actions_need_confirmation() {
        assert!(is_dangerous("system:shutdown"));
        assert!(!is_dangerous("system:lock"));
        assert!(!is_dangerous("apps:firefox.desktop"));
    }
}
