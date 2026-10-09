//! Aujar launcher UI (GPUI spike).
//!
//! Design: the UI holds **no logic**. A background thread (tokio) talks to the
//! daemon over IPC; the GPUI thread only renders state and forwards keystrokes.
//!
//! Keys: type to search, Up/Down to select, Enter to launch, Esc to close.
//! Limitation: plain key events only (no IME / composition yet).

use anyhow::{Result, bail};
use aujar_ipc::{
    Event, ExecuteRequest, LauncherCommand, Request, Response, SearchHit, SearchRequest, send,
    subscribe,
};
use gpui::{
    App, Bounds, Context, FocusHandle, Focusable, KeyDownEvent, Window, WindowBounds,
    WindowHandle, WindowOptions, div, prelude::*, px, rgb, size,
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

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = &keystroke.modifiers;

        match keystroke.key.as_str() {
            "escape" => {
                window.remove_window();
                return;
            }
            "enter" => {
                self.launch_selected(window, cx);
                return;
            }
            "down" => {
                if !self.results.is_empty() {
                    self.selected = (self.selected + 1) % self.results.len();
                    cx.notify();
                }
                return;
            }
            "up" => {
                if !self.results.is_empty() {
                    self.selected = (self.selected + self.results.len() - 1) % self.results.len();
                    cx.notify();
                }
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

        // Shortcuts (Ctrl/Alt/Super chords) are not text.
        if modifiers.control || modifiers.alt || modifiers.platform {
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

        if hit.kind == "calculation" {
            self.status = Some("Calculation results can't be launched (clipboard is planned)".into());
            cx.notify();
            return;
        }

        let _ = self.requests.try_send(UiRequest::Execute(hit.id.clone()));
        window.remove_window();
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
            ("Type to search".to_owned(), rgb(0x7f849c))
        } else {
            (format!("{}\u{258F}", self.query), rgb(0xcdd6f4))
        };

        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .flex_col()
            .size_full()
            .gap_2()
            .p_4()
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .child(div().text_xl().text_color(query_color).child(query_line))
            .children(self.results.iter().enumerate().map(|(index, hit)| {
                let background = if index == self.selected {
                    rgb(0x313244)
                } else {
                    rgb(0x1e1e2e)
                };

                div()
                    .flex()
                    .flex_col()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(background)
                    .child(div().child(hit.title.clone()))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x7f849c))
                            .child(hit.subtitle.clone().unwrap_or_default()),
                    )
            }))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xf38ba8))
                    .child(self.status.clone().unwrap_or_default()),
            )
    }
}

fn open_launcher(
    cx: &mut App,
    requests: async_channel::Sender<UiRequest>,
) -> Option<WindowHandle<LauncherView>> {
    let bounds = Bounds::centered(None, size(px(640.), px(420.)), cx);

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
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
                        view.results = results;
                        view.selected = 0;
                        view.status = None;
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

    application().run(move |cx: &mut App| {
        cx.spawn(async move |cx| {
            let mut window: Option<WindowHandle<LauncherView>> = None;

            while let Ok(message) = message_rx.recv().await {
                let _ = cx.update(|cx| handle_message(cx, &mut window, &request_tx, message));
            }
        })
        .detach();
    });
}
