//! Aujar launcher UI (GPUI spike).
//!
//! Design: the UI holds **no logic**. A background thread (tokio) talks to the
//! daemon over IPC; the GPUI thread only renders state and forwards keystrokes.
//!
//! Keys: type to search, Up/Down to select, Enter to launch (or copy a calculation), Esc to close.
//! Limitation: plain key events only (no IME / composition yet).

use anyhow::{Result, bail};
use aujar_ipc::{
    Event, ExecuteRequest, LauncherCommand, Request, Response, SearchHit, SearchRequest, send,
    subscribe,
};
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, FocusHandle, Focusable, KeyDownEvent,
    SharedString, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowOptions,
    div, prelude::*, px, rgb, rgba, size,
};
use gpui_platform::application;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// Bridge thread -> UI thread.
enum UiMessage {
    Launcher(LauncherCommand),
    Results {
        query: String,
        results: Vec<SearchHit>,
    },
    Failed(String),
}

/// UI thread -> bridge thread.
enum UiRequest {
    Search(String),
    Execute(String),
}

// ---- Look & feel (macOS Spotlight-inspired, dark) -------------------------------------
const WINDOW_W: f32 = 680.0;
const WINDOW_H: f32 = 520.0;
const SEARCH_URL: &str = "https://duckduckgo.com/?q=";

const BG: u32 = 0x1c1c22f2;
const BORDER: u32 = 0xffffff1f;
const DIVIDER: u32 = 0xffffff14;
const TEXT: u32 = 0xf2f2f7;
const TEXT_DIM: u32 = 0x9a9aa6;
const ACCENT: u32 = 0x0a64d8;
const ON_ACCENT_DIM: u32 = 0xffffffb8;
const HOVER: u32 = 0xffffff12;
const ERROR: u32 = 0xff6b81;

const AVATAR_COLORS: [u32; 6] = [0x5e5ce6, 0x30a46c, 0xe5732d, 0xd6409f, 0x2f8fd8, 0x8e6bd8];

/// Section order in the result list; also the sort key for results.
fn kind_rank(kind: &str) -> u8 {
    match kind {
        "calculation" => 0,
        "application" => 1,
        _ => 2,
    }
}

fn section_title(kind: &str) -> &'static str {
    match kind {
        "calculation" => "Calculator",
        "application" => "Applications",
        _ => "Web search",
    }
}

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

struct LauncherView {
    query: String,
    results: Vec<SearchHit>,
    selected: usize,
    status: Option<String>,
    requests: async_channel::Sender<UiRequest>,
    focus_handle: FocusHandle,
}

impl LauncherView {
    fn new(requests: async_channel::Sender<UiRequest>, cx: &mut Context<Self>) -> Self {
        Self {
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            status: None,
            requests,
            focus_handle: cx.focus_handle(),
        }
    }

    fn request_search(&mut self) {
        self.selected = 0;
        self.status = None;

        if self.query.trim().is_empty() {
            self.results.clear();
            return;
        }

        let _ = self.requests.try_send(UiRequest::Search(self.query.clone()));
    }

    /// Group results into Spotlight-style sections and add the web-search fallback.
    fn set_results(&mut self, mut results: Vec<SearchHit>) {
        results.sort_by_key(|hit| kind_rank(&hit.kind)); // stable: keeps ranking inside a section

        let query = self.query.trim();

        if !query.is_empty() {
            results.push(SearchHit {
                id: format!("web:{query}"),
                title: format!("Search the web for \u{201c}{query}\u{201d}"),
                subtitle: Some("Opens your default browser".to_owned()),
                kind: "web".to_owned(),
                score: 0,
            });
        }

        self.results = results;
        self.selected = 0;
        self.status = None;
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.results.len() as isize;

        if len > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
            cx.notify();
        }
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
                    self.launch_selected(window, cx);
                }
                return;
            }
        }

        // Emacs/readline-style chords.
        if modifiers.control {
            match key {
                "n" | "j" => self.move_selection(1, cx),
                "p" | "k" => self.move_selection(-1, cx),
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
                        self.query
                            .extend(line.chars().filter(|c| !c.is_control()));
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
                self.launch_selected(window, cx);
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

    fn launch_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hit) = self.results.get(self.selected) else {
            return;
        };

        match hit.kind.as_str() {
            // Calculations are not launchable: copy the answer instead. The UI process stays
            // alive, so it keeps owning the Wayland clipboard after the window closes.
            "calculation" => {
                cx.write_to_clipboard(ClipboardItem::new_string(hit.title.clone()));
            }
            "web" => {
                let text = hit.id.strip_prefix("web:").unwrap_or(&hit.id);
                cx.open_url(&format!("{SEARCH_URL}{}", percent_encode(text)));
            }
            _ => {
                let _ = self.requests.try_send(UiRequest::Execute(hit.id.clone()));
            }
        }

        window.remove_window();
    }

    fn render_avatar(title: &str, selected: bool) -> impl IntoElement {
        let initial = title
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_owned());
        let color = AVATAR_COLORS[title.bytes().map(usize::from).sum::<usize>() % AVATAR_COLORS.len()];

        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(34.))
            .flex_shrink_0()
            .rounded_lg()
            .bg(rgb(color))
            .text_color(rgb(0xffffff))
            .text_lg()
            .when(selected, |this| this.border_1().border_color(rgba(0xffffff55)))
            .child(initial)
    }

    fn render_row(&self, index: usize, hit: &SearchHit, cx: &mut Context<Self>) -> AnyElement {
        let selected = index == self.selected;
        let title_color = if selected { rgb(0xffffff) } else { rgb(TEXT) };
        let sub_color = if selected { rgba(ON_ACCENT_DIM) } else { rgba(TEXT_DIM << 8 | 0xff) };

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
                this.launch_selected(window, cx);
            }));

        let base = if selected {
            base.bg(rgb(ACCENT))
        } else {
            base.hover(|style| style.bg(rgba(HOVER)))
        };

        let hint = if index < 9 {
            format!("Alt+{}", index + 1)
        } else {
            String::new()
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
                        .child(div().text_xs().text_color(sub_color).child(
                            hit.subtitle.clone().unwrap_or_else(|| "Calculator".to_owned()),
                        ))
                        .child(div().text_3xl().text_color(title_color).child(hit.title.clone())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(sub_color)
                        .child(if selected { "Enter to copy".to_owned() } else { hint }),
                )
                .into_any_element();
        }

        base.py_2()
            .child(Self::render_avatar(&hit.title, selected))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .child(div().text_base().text_color(title_color).truncate().child(hit.title.clone()))
                    .when_some(hit.subtitle.clone(), |this, subtitle| {
                        this.child(div().text_xs().text_color(sub_color).truncate().child(subtitle))
                    }),
            )
            .child(div().text_xs().text_color(sub_color).child(hint))
            .into_any_element()
    }
}

impl Focusable for LauncherView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LauncherView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (query_line, query_color) = if self.query.is_empty() {
            ("Search apps, math and the web".to_owned(), rgb(TEXT_DIM))
        } else {
            (format!("{}\u{258F}", self.query), rgb(TEXT))
        };

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut last_kind: Option<String> = None;

        for index in 0..self.results.len() {
            let hit = self.results[index].clone();

            if last_kind.as_deref().map(kind_rank) != Some(kind_rank(&hit.kind)) {
                rows.push(
                    div()
                        .px_5()
                        .pt_3()
                        .pb_1()
                        .text_xs()
                        .text_color(rgb(TEXT_DIM))
                        .child(section_title(&hit.kind))
                        .into_any_element(),
                );
                last_kind = Some(hit.kind.clone());
            }

            rows.push(self.render_row(index, &hit, cx));
        }

        // The window is transparent: only this card is painted, so with no results it
        // collapses to just the search bar, like Spotlight.
        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .flex_col()
            .w_full()
            .bg(rgba(BG))
            .border_1()
            .border_color(rgba(BORDER))
            .rounded_xl()
            .overflow_hidden()
            .text_color(rgb(TEXT))
            .child(
                div()
                    .px_5()
                    .py_4()
                    .text_2xl()
                    .text_color(query_color)
                    .child(query_line),
            )
            .when(!rows.is_empty(), |this| {
                this.child(div().h(px(1.)).w_full().bg(rgba(DIVIDER)))
                    .child(div().flex().flex_col().pb_2().children(rows))
            })
            .when_some(self.status.clone(), |this, status| {
                this.child(div().px_5().pb_3().text_sm().text_color(rgb(ERROR)).child(status))
            })
    }
}

fn open_launcher(
    cx: &mut App,
    requests: async_channel::Sender<UiRequest>,
) -> Option<WindowHandle<LauncherView>> {
    let bounds = Bounds::centered(None, size(px(WINDOW_W), px(WINDOW_H)), cx);

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("aujar-launcher".into()),
        ..Default::default()
    };

    cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| LauncherView::new(requests, cx));
        let focus_handle = view.read(cx).focus_handle.clone();

        // Without focus, key events never reach the view.
        window.focus(&focus_handle, cx);

        view
    })
    .ok()
}

fn handle_message(
    cx: &mut App,
    window: &mut Option<WindowHandle<LauncherView>>,
    requests: &async_channel::Sender<UiRequest>,
    message: UiMessage,
) {
    // Forget the handle if the user closed the window (Esc, Enter, window manager).
    let closed = window
        .as_ref()
        .is_some_and(|handle| handle.update(cx, |_, _, _| ()).is_err());

    if closed {
        *window = None;
    }

    match message {
        UiMessage::Launcher(command) => {
            let is_open = window.is_some();

            let want_open = match command {
                LauncherCommand::Show => true,
                LauncherCommand::Hide => false,
                LauncherCommand::Toggle => !is_open,
            };

            match (is_open, want_open) {
                (false, true) => *window = open_launcher(cx, requests.clone()),
                (true, false) => {
                    if let Some(handle) = window.take() {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    }
                }
                _ => {}
            }
        }
        UiMessage::Results { query, results } => {
            if let Some(handle) = window.as_ref() {
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
            if let Some(handle) = window.as_ref() {
                let _ = handle.update(cx, |view, _, cx| {
                    view.status = Some(message);
                    cx.notify();
                });
            }
        }
    }
}

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
        limit: Some(8),
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
            let mut window: Option<WindowHandle<LauncherView>> = None;

            while let Ok(message) = message_rx.recv().await {
                let _ = cx.update(|cx| handle_message(cx, &mut window, &request_tx, message));
            }
        })
        .detach();
    });
}
