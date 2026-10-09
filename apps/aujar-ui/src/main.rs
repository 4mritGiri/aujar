//! Aujar launcher UI: GPUI spike.
//!
//! Goal of the spike (see docs/UI_SPIKE.md): prove that a GPUI window opens on
//! Wayland/X11, renders on the GPU, and can be shown/hidden by daemon events.
//!
//! Design: the UI holds **no logic**. A background thread (tokio) talks to the
//! daemon over IPC and hands ready-made messages to the GPUI thread through a
//! channel.

use anyhow::{Result, bail};
use aujar_ipc::{
    Event, LauncherCommand, Request, Response, SearchHit, SearchRequest, send, subscribe,
};
use gpui::{
    App, Bounds, Context, Window, WindowBounds, WindowHandle, WindowOptions, div,
    prelude::*, px, rgb, size,
};
use gpui_platform::application;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// What the bridge thread tells the UI thread.
struct UiMessage {
    command: LauncherCommand,
    results: Vec<SearchHit>,
}

struct LauncherView {
    results: Vec<SearchHit>,
}

impl Render for LauncherView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap_2()
            .p_4()
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .child(div().text_xl().child("Aujar"))
            .children(self.results.iter().map(|hit| {
                div()
                    .flex()
                    .flex_col()
                    .child(div().child(hit.title.clone()))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x7f849c))
                            .child(hit.subtitle.clone().unwrap_or_default()),
                    )
            }))
    }
}

fn open_launcher(cx: &mut App, results: Vec<SearchHit>) -> Option<WindowHandle<LauncherView>> {
    let bounds = Bounds::centered(None, size(px(640.), px(420.)), cx);

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        app_id: Some("aujar-launcher".into()),
        ..Default::default()
    };

    cx.open_window(options, |_, cx| cx.new(|_| LauncherView { results }))
        .ok()
}

fn handle_message(
    cx: &mut App,
    window: &mut Option<WindowHandle<LauncherView>>,
    message: UiMessage,
) {
    // Forget the handle if the user closed the window themselves.
    let closed = window
        .as_ref()
        .is_some_and(|handle| handle.update(cx, |_, _, _| ()).is_err());

    if closed {
        *window = None;
    }

    let is_open = window.is_some();

    let want_open = match message.command {
        LauncherCommand::Show => true,
        LauncherCommand::Hide => false,
        LauncherCommand::Toggle => !is_open,
    };

    match (is_open, want_open) {
        (false, true) => *window = open_launcher(cx, message.results),
        (true, true) => {
            if let Some(handle) = window.as_ref() {
                let _ = handle.update(cx, |view, _, cx| {
                    view.results = message.results;
                    cx.notify();
                });
            }
        }
        (true, false) => {
            if let Some(handle) = window.take() {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
        }
        (false, false) => {}
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

async fn run_session(
    socket: &Path,
    query: &str,
    tx: &async_channel::Sender<UiMessage>,
) -> Result<()> {
    let mut stream = subscribe(socket).await?;

    eprintln!("aujar-ui: subscribed to the daemon");

    while let Some(event) = stream.recv().await? {
        match event {
            Event::Launcher(command) => {
                let results = search(socket, query).await.unwrap_or_else(|error| {
                    eprintln!("aujar-ui: search failed: {error:#}");
                    Vec::new()
                });

                if tx.send(UiMessage { command, results }).await.is_err() {
                    return Ok(());
                }
            }
            Event::Shutdown => break,
        }
    }

    Ok(())
}

/// Talks to the daemon on its own thread; reconnects if the daemon restarts.
fn spawn_bridge(socket: PathBuf, query: String, tx: async_channel::Sender<UiMessage>) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");

        runtime.block_on(async move {
            loop {
                if let Err(error) = run_session(&socket, &query, &tx).await {
                    eprintln!("aujar-ui: {error:#} (retrying)");
                }

                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    });
}

fn main() {
    // Until text input exists, results come from a fixed query: `aujar-ui [query]`.
    let query = std::env::args().nth(1).unwrap_or_else(|| "fire".to_owned());
    let (tx, rx) = async_channel::unbounded::<UiMessage>();

    spawn_bridge(socket_path(), query, tx);

    application().run(move |cx: &mut App| {
        cx.spawn(async move |cx| {
            let mut window: Option<WindowHandle<LauncherView>> = None;

            while let Ok(message) = rx.recv().await {
                let _ = cx.update(|cx| handle_message(cx, &mut window, message));
            }
        })
        .detach();
    });
}
