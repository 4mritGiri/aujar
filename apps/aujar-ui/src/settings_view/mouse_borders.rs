//! Mouse Borders: turn it on, pair machines over the network (or with a pairing code),
//! arrange them by dragging, tune how switching works. Status refreshes by itself while this
//! page is open.
//!
//! Key and layout changes work on files, so they do not need the daemon; only the live status
//! asks it. Nothing here may wait on the network on the UI thread: scans run on the background
//! executor and pairing runs on its own threads, polled twice a second.

use std::{net::SocketAddr, time::Duration};

use aujar_config::{EasyMouse, Machine, Settings, SwitchKeys};
use aujar_ipc::{MouseBordersRequest, MouseBordersResponse, MouseBordersStatus, Request, Response};
use aujar_mouse_borders::{
    discovery::{self, DiscoveredDevice},
    keystore::KeyStore,
    lan_pair::{self, HostConfig, PairState},
    layout::{MachineName, PLANE_COLS, PLANE_ROWS},
    net,
    pairing::PairingCode,
    secure::SecurityKey,
    settings_edit,
};
use gpui::{
    AnyElement, ClipboardItem, Context, FontWeight, SharedString, div, prelude::*, px, rgb, rgba,
};

use super::{Page, SettingsView};

/// How often the page looks at a running pairing and repaints it.
const TICK: Duration = Duration::from_millis(500);

/// The daemon status is asked every this many ticks.
const REFRESH_EVERY: u32 = 4;

/// How long a scan listens for answers.
const SCAN_TIME: Duration = Duration::from_secs(2);

/// Size of one cell of the layout board.
const CELL_W: f32 = 118.;
const CELL_H: f32 = 58.;

/// Two-step actions: the first click arms them, the second performs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Confirm {
    NewKey,
    Join,
}

struct Note {
    text: String,
    error: bool,
}

/// What is being dragged on the layout board.
#[derive(Clone)]
struct DraggedMachine {
    name: String,
}

#[derive(Default)]
pub(super) struct State {
    status: Option<Result<MouseBordersStatus, String>>,
    note: Option<Note>,
    confirm: Option<Confirm>,
    refreshing: bool,
    discovered_devices: Vec<DiscoveredDevice>,
    scanning: bool,
    /// A pairing window this machine opened.
    host: Option<lan_pair::Handle>,
    /// A pairing this machine started with another one.
    guest: Option<lan_pair::Handle>,
    /// A code that arrived but needs the person to agree to replace the key first.
    pending_join: Option<PairingCode>,
}

/// Ask the daemon for the status. Blocking: run it off the UI thread.
fn fetch_status() -> Result<MouseBordersStatus, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;

    runtime.block_on(async {
        let request = Request::MouseBorders(MouseBordersRequest::Status);
        let answer = tokio::time::timeout(
            Duration::from_secs(2),
            aujar_ipc::send(crate::socket_path(), request),
        )
        .await;

        match answer {
            Err(_) => Err("The Aujar daemon did not answer.".to_owned()),
            Ok(Err(_)) => Err(
                "The Aujar daemon is not running. Start it with `systemctl --user start aujar`."
                    .to_owned(),
            ),
            Ok(Ok(Response::MouseBorders(MouseBordersResponse::Status(status)))) => Ok(*status),
            Ok(Ok(Response::Error(message))) => Err(message),
            Ok(Ok(_)) => Err("The daemon sent an unexpected answer.".to_owned()),
        }
    })
}

/// The security key, created if there is none yet.
fn ensure_key() -> Result<SecurityKey, String> {
    let store = KeyStore::user_default();

    match store.load().map_err(|error| error.to_string())? {
        Some(key) => Ok(key),
        None => store.generate(false).map_err(|error| error.to_string()),
    }
}

impl SettingsView {
    pub(super) fn refresh_mouse_borders(&mut self, cx: &mut Context<Self>) {
        if self.mouse_borders.refreshing {
            return;
        }

        self.mouse_borders.refreshing = true;

        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async { fetch_status() })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.mouse_borders.refreshing = false;
                this.mouse_borders.status = Some(status);
                cx.notify();
            });
        })
        .detach();
    }

    /// Every half second while this window lives: move a running pairing along and, every
    /// couple of seconds while this page is showing, re-ask the daemon. Stops when the window
    /// closes.
    pub(super) fn keep_mouse_borders_status_fresh(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut tick: u32 = 0;

            loop {
                let alive = this.update(cx, |this, cx| {
                    this.poll_pairing(cx);

                    if tick % REFRESH_EVERY == 0 && this.page == Page::MouseBorders {
                        this.refresh_mouse_borders(cx);
                    }
                });

                if alive.is_err() {
                    break;
                }

                tick = tick.wrapping_add(1);
                cx.background_executor().timer(TICK).await;
            }
        })
        .detach();
    }

    /// Look for machines that are waiting to pair.
    ///
    /// The scan waits on the network, so it runs on the background executor. It must never
    /// run on the UI thread: a socket that waits there freezes the whole window ("not
    /// responding").
    pub(super) fn scan_for_devices(&mut self, cx: &mut Context<Self>) {
        if self.mouse_borders.scanning {
            return;
        }

        self.mouse_borders.scanning = true;
        self.mouse_borders.discovered_devices.clear();
        self.set_note("Looking for devices\u{2026}", false, cx);

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { discovery::scan(SCAN_TIME) })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.mouse_borders.scanning = false;

                match result {
                    Ok(devices) if devices.is_empty() => this.set_note(
                        "No device found. On the other machine open Mouse Borders and choose \
                         \u{201c}Start pairing\u{201d}, and check that both are on the same network.",
                        false,
                        cx,
                    ),
                    Ok(devices) => {
                        let count = devices.len();

                        this.mouse_borders.discovered_devices = devices;
                        this.set_note(
                            if count == 1 {
                                "Found 1 device. Choose Pair next to it.".to_owned()
                            } else {
                                format!("Found {count} devices. Choose Pair next to one.")
                            },
                            false,
                            cx,
                        );
                    }
                    Err(error) => this.set_note(format!("Could not scan: {error}"), true, cx),
                }

                cx.notify();
            });
        })
        .detach();
    }

    fn set_note(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.mouse_borders.note = Some(Note {
            text: text.into(),
            error,
        });

        cx.notify();
    }

    // ---- Pairing over the network ----

    fn active_pairing(&self) -> Option<&lan_pair::Handle> {
        self.mouse_borders
            .host
            .as_ref()
            .or(self.mouse_borders.guest.as_ref())
    }

    fn stop_pairing(&mut self) {
        for handle in [self.mouse_borders.host.take(), self.mouse_borders.guest.take()]
            .into_iter()
            .flatten()
        {
            handle.cancel();
        }
    }

    /// Open a pairing window: other machines can find this one and pair with it.
    fn start_pairing(&mut self, cx: &mut Context<Self>) {
        self.stop_pairing();

        let (code, next) = match self.build_pairing_code() {
            Ok(built) => built,
            Err(error) => {
                self.set_note(error, true, cx);

                return;
            }
        };

        let config = HostConfig {
            name: settings_edit::local_name(&next.mouse_borders).to_string(),
            sharing_port: next.mouse_borders.port,
            code,
            window: lan_pair::DEFAULT_WINDOW,
        };

        match lan_pair::host(config) {
            Ok(handle) => {
                self.mouse_borders.host = Some(handle);
                self.mouse_borders.confirm = None;
                self.mouse_borders.note = None;
                // This machine is in the layout and sharing is on, as in the code it hands out.
                self.mutate(cx, move |settings| *settings = next);
            }
            Err(error) => self.set_note(format!("Could not start pairing: {error}"), true, cx),
        }
    }

    /// Pair with a machine found by a scan.
    fn pair_with(&mut self, device: &DiscoveredDevice, cx: &mut Context<Self>) {
        self.stop_pairing();

        let local = settings_edit::local_name(&self.settings.mouse_borders).to_string();
        let address = SocketAddr::new(device.address, device.port);

        self.mouse_borders.guest = Some(lan_pair::join(address, local, device.name.clone()));
        self.mouse_borders.note = None;
        cx.notify();
    }

    fn answer_code_check(&mut self, matches: bool, cx: &mut Context<Self>) {
        if let Some(handle) = self.active_pairing() {
            handle.confirm(matches);
        }

        cx.notify();
    }

    fn cancel_pairing(&mut self, cx: &mut Context<Self>) {
        self.stop_pairing();
        self.set_note("Pairing cancelled.", false, cx);
    }

    /// Called every tick: collect the outcome of a pairing that ended.
    fn poll_pairing(&mut self, cx: &mut Context<Self>) {
        let host = self.mouse_borders.host.as_ref().map(|handle| handle.state());
        let guest = self.mouse_borders.guest.as_ref().map(|handle| handle.state());

        if host.is_none() && guest.is_none() {
            return;
        }

        match host {
            Some(PairState::Sent) => {
                self.mouse_borders.host = None;
                self.set_note(
                    "Paired. The other machine joins the layout and connects by itself.",
                    false,
                    cx,
                );
            }
            Some(PairState::Failed(why)) => {
                self.mouse_borders.host = None;
                self.set_note(why, true, cx);
            }
            _ => {}
        }

        match guest {
            Some(PairState::Received(text)) => {
                self.mouse_borders.guest = None;

                match PairingCode::parse(&text) {
                    Ok(code) => self.join_with_code(code, cx),
                    Err(error) => self.set_note(
                        format!("The other machine sent a pairing code that is not valid: {error}"),
                        true,
                        cx,
                    ),
                }
            }
            Some(PairState::Failed(why)) => {
                self.mouse_borders.guest = None;
                self.set_note(why, true, cx);
            }
            _ => {}
        }

        // Repaint for the countdown and the state changes.
        cx.notify();
    }

    // ---- Actions ----

    /// The pairing code for the other machines, and the settings it implies: this machine in
    /// the layout and sharing on. The settings are not saved here.
    fn build_pairing_code(&self) -> Result<(String, Settings), String> {
        let mut next = self.settings.clone();
        let local = settings_edit::local_name(&next.mouse_borders);

        settings_edit::add_machine(&mut next.mouse_borders, &local, None)
            .map_err(|error| error.to_string())?;

        let key = ensure_key()?;
        let host = net::pick_host(&net::local_networks());
        let code = PairingCode::from_settings(&next.mouse_borders, key, &local, host)
            .map(|code| code.encode())
            .map_err(|error| error.to_string())?;

        next.mouse_borders.enabled = true;

        Ok((code, next))
    }

    fn copy_pairing_code(&mut self, cx: &mut Context<Self>) {
        match self.build_pairing_code() {
            Ok((code, next)) => {
                cx.write_to_clipboard(ClipboardItem::new_string(code));
                self.mouse_borders.confirm = None;
                self.mutate(cx, move |settings| *settings = next);
                self.set_note("Pairing code copied to the clipboard.", false, cx);
            }
            Err(error) => self.set_note(error, true, cx),
        }
    }

    fn join_from_clipboard(&mut self, cx: &mut Context<Self>) {
        // The second click on "Replace key and join" uses the code that was already read.
        if self.mouse_borders.confirm == Some(Confirm::Join)
            && let Some(code) = self.mouse_borders.pending_join.take()
        {
            self.join_with_code(code, cx);

            return;
        }

        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.set_note(
                "Clipboard is empty. Copy a pairing code from the first machine first.",
                true,
                cx,
            );

            return;
        };

        match PairingCode::parse(&text) {
            Ok(code) => self.join_with_code(code, cx),
            Err(error) => self.set_note(format!("Invalid pairing code: {error}"), true, cx),
        }
    }

    /// Adopt a pairing code: its key, its layout, and switch sharing on.
    fn join_with_code(&mut self, code: PairingCode, cx: &mut Context<Self>) {
        let store = KeyStore::user_default();

        match store.load() {
            Ok(Some(existing))
                if existing != code.key && self.mouse_borders.confirm != Some(Confirm::Join) =>
            {
                self.mouse_borders.confirm = Some(Confirm::Join);
                self.mouse_borders.pending_join = Some(code);
                self.set_note(
                    "This machine already has a different security key. \
                     Click 'Replace key and join' to use the new one \
                     (machines using the old key will disconnect).",
                    false,
                    cx,
                );

                return;
            }
            Err(error) => {
                self.set_note(error.to_string(), true, cx);

                return;
            }
            _ => {}
        }

        let mut next = self.settings.clone();
        let local = settings_edit::local_name(&next.mouse_borders);

        let joined = match code.apply(&mut next.mouse_borders, &local) {
            Ok(joined) => joined,
            Err(error) => {
                self.set_note(error.to_string(), true, cx);

                return;
            }
        };

        if let Err(error) = store.save(&code.key, true) {
            self.set_note(error.to_string(), true, cx);

            return;
        }

        self.mouse_borders.confirm = None;
        self.mouse_borders.pending_join = None;
        next.mouse_borders.enabled = true;
        self.mutate(cx, move |settings| *settings = next);
        self.set_note(
            format!(
                "Paired. {} machines are in the layout; sharing is on.",
                joined.machines,
            ),
            false,
            cx,
        );
    }

    fn new_key(&mut self, cx: &mut Context<Self>) {
        let store = KeyStore::user_default();
        let has_key = matches!(store.load(), Ok(Some(_)));

        if has_key && self.mouse_borders.confirm != Some(Confirm::NewKey) {
            self.mouse_borders.confirm = Some(Confirm::NewKey);
            self.set_note(
                "Click again to create a new key. Every other machine must then join again with a new pairing code.",
                false,
                cx,
            );

            return;
        }

        self.mouse_borders.confirm = None;
        // A pairing window open now would hand out the old key.
        self.stop_pairing();

        match store.generate(true) {
            Ok(_) => self.set_note(
                "New security key created. Copy a pairing code to connect your other machines.",
                false,
                cx,
            ),
            Err(error) => self.set_note(error.to_string(), true, cx),
        }
    }

    // ---- Page ----

    pub(super) fn mouse_borders_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mb = &self.settings.mouse_borders;
        let local = settings_edit::local_name(mb);

        // Enable and identity.
        let enable = vec![
            toggle_row!(
                self,
                cx,
                "mb-enabled",
                "Share keyboard and mouse",
                "Lets other machines you pair with use this one, and the other way around. Opens a network listener while on.",
                mouse_borders.enabled
            ),
            self.row(
                "This machine",
                "The name your other machines see. Change it with machine_name in the config file.",
                self.dim_text(local.as_str()),
            ),
        ];

        // Help section at the top
        let step = |text: &str| div().child(text.to_owned());
        let help_section = vec![
            div()
                .px_4()
                .py_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(self.palette.text))
                        .child("Connect two machines in a minute:"),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_xs()
                        .text_color(rgb(self.palette.text_dim))
                        .child(step("1. On the first machine choose \u{201c}Start pairing\u{201d}."))
                        .child(step("2. On the other one choose \u{201c}Find devices\u{201d}, then Pair."))
                        .child(step("3. Both screens show a code. If they match, click \u{201c}Codes match\u{201d} on both."))
                        .child(step("4. Drag the machines in Layout to match your desk, then move the pointer past a screen edge to switch.")),
                )
                .into_any_element(),
        ];

        vec![
            self.section("Quick Start", help_section),
            self.section("Status", self.status_rows(cx)),
            self.section("Share keyboard and mouse", enable),
            self.section("Connect machines", self.connect_rows(cx)),
            self.section("Layout", self.layout_rows(cx, &local)),
            self.section(
                "Switching",
                vec![
                    self.row("Move between machines with the mouse", "What it takes to cross a screen edge.", self.easy_mouse_choices(cx)),
                    toggle_row!(self, cx, "mb-wrap", "Wrap around", "Past the last machine, continue on the first.", mouse_borders.wrap_mouse),
                    toggle_row!(self, cx, "mb-corners", "Block screen corners", "Do not switch when the pointer is in a corner (avoids accidents).", mouse_borders.block_corners),
                    toggle_row!(self, cx, "mb-relative", "Move relatively", "For machines with different resolutions or several monitors.", mouse_borders.move_relatively),
                    toggle_row!(self, cx, "mb-hide", "Hide the pointer while away", "Hides this machine's pointer while you control another one (X11 only).", mouse_borders.hide_mouse_at_edge),
                ],
            ),
            self.section(
                "Sharing",
                vec![
                    toggle_row!(self, cx, "mb-clipboard", "Share the clipboard", "Text and images follow you to the machine you switch to.", mouse_borders.share_clipboard),
                    toggle_row!(self, cx, "mb-files", "Receive files", "Accept files sent from your machines (aujar mwb send).", mouse_borders.transfer_files),
                    stepper_row!(self, cx, "mb-filesize", "Largest file", "Files above this size are refused (100 MB at most).", 10, " MB", mouse_borders.max_file_mb),
                    toggle_row!(self, cx, "mb-awake", "Keep machines awake", "No screen saver or auto-lock on any machine while you are working.", mouse_borders.block_screensaver),
                    toggle_row!(self, cx, "mb-lock", "Allow remote lock", "Let your other machines lock this one (Ctrl+Alt+L).", mouse_borders.allow_remote_lock),
                ],
            ),
            self.section(
                "Security & shortcuts",
                vec![
                    toggle_row!(self, cx, "mb-subnet", "Same network only", "Refuse connections from outside this machine's local network.", mouse_borders.same_subnet_only),
                    self.row("Jump to a machine", "Ctrl+Alt plus a number. Function keys conflict with virtual terminals (Ctrl+Alt+F1\u{2013}F7).", self.switch_keys_choices(cx)),
                    self.row("Other shortcuts", &self.shortcut_help(), div()),
                    self.row("Permissions", "Reading and injecting input needs the `input` group on Linux. Details: docs/MOUSE_BORDERS.md", div()),
                ],
            ),
        ]
    }

    fn dim_text(&self, text: &str) -> impl IntoElement {
        div()
            .text_sm()
            .text_color(rgb(self.palette.text_dim))
            .child(text.to_owned())
    }

    fn dot(&self, color: u32) -> impl IntoElement {
        div()
            .size(px(10.))
            .rounded_full()
            .flex_shrink_0()
            .bg(rgb(color))
    }

    fn status_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let pal = self.palette;

        let (headline, detail, color) = match &self.mouse_borders.status {
            None => ("Checking\u{2026}".to_owned(), String::new(), pal.text_faint),
            Some(Err(message)) => (
                "Aujar is not running".to_owned(),
                message.clone(),
                pal.danger,
            ),
            Some(Ok(status)) => match status.state.as_str() {
                "running" => {
                    let connected = status.peers.iter().filter(|peer| peer.connected).count();
                    let holder = if status.focus == status.local {
                        "this machine".to_owned()
                    } else {
                        status.focus.clone()
                    };

                    (
                        format!("Running as {}", status.local),
                        format!(
                            "{connected} of {} other machines connected. Keyboard and mouse are on {holder}.",
                            status.peers.len()
                        ),
                        pal.positive,
                    )
                }
                "disabled" => (
                    "Off".to_owned(),
                    "Turn on \u{201c}Share keyboard and mouse\u{201d} below.".to_owned(),
                    pal.text_faint,
                ),
                "waiting_for_key" => (
                    "Waiting for a security key".to_owned(),
                    "Copy a pairing code on one machine and join from it on the others.".to_owned(),
                    pal.danger,
                ),
                "blocked" => (
                    "Blocked by policy".to_owned(),
                    status.detail.clone(),
                    pal.danger,
                ),
                _ => ("Not working".to_owned(), status.detail.clone(), pal.danger),
            },
        };

        let refresh = self
            .pill("mb-refresh".into(), "Refresh", false)
            .on_click(cx.listener(|this, _event, _window, cx| this.refresh_mouse_borders(cx)));

        let mut rows: Vec<AnyElement> =
            vec![
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .child(self.dot(color))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .gap_1()
                            .child(div().text_sm().text_color(rgb(pal.text)).child(headline))
                            .children((!detail.is_empty()).then(|| {
                                div().text_xs().text_color(rgb(pal.text_dim)).child(detail)
                            })),
                    )
                    .child(refresh)
                    .into_any_element(),
            ];

        if let Some(Ok(status)) = &self.mouse_borders.status {
            for (label, problem) in [
                (
                    "Switching at screen edges is unavailable",
                    &status.edge_switching_unavailable,
                ),
                (
                    "This machine cannot capture its own input",
                    &status.capture_unavailable,
                ),
            ] {
                if let Some(problem) = problem {
                    rows.push(
                        div()
                            .px_4()
                            .py_3()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_sm().text_color(rgb(pal.danger)).child(label))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(pal.text_dim))
                                    .child(problem.clone()),
                            )
                            .into_any_element(),
                    );
                }
            }

            for peer in &status.peers {
                let hint = if peer.connected {
                    let screen = peer
                        .screen
                        .map(|(w, h)| format!(" \u{b7} {w}\u{d7}{h}"))
                        .unwrap_or_default();

                    format!("{}{screen}", peer.address.clone().unwrap_or_default())
                } else {
                    peer.last_error
                        .clone()
                        .unwrap_or_else(|| "Not connected yet".to_owned())
                };

                let chip = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(self.dot(if peer.connected {
                        pal.positive
                    } else {
                        pal.text_faint
                    }))
                    .child(div().text_xs().text_color(rgb(pal.text_dim)).child(
                        if peer.connected {
                            "Connected"
                        } else {
                            "Offline"
                        },
                    ));

                rows.push(self.row(&peer.name, &hint, chip));
            }
        }

        rows
    }

    /// The panel shown while a pairing is running: what is going on, and what to click.
    fn pairing_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pal = self.palette;
        let handle = self.active_pairing()?;
        let hosting = self.mouse_borders.host.is_some();

        let (title, detail, code, can_answer) = match handle.state() {
            PairState::Waiting if hosting => (
                "Waiting for another machine\u{2026}".to_owned(),
                "On the other machine open Mouse Borders, choose \u{201c}Find devices\u{201d}, then \
                 Pair next to this machine. This closes by itself after two minutes."
                    .to_owned(),
                None,
                false,
            ),
            PairState::Waiting => (
                "Connecting\u{2026}".to_owned(),
                "Setting up a private connection.".to_owned(),
                None,
                false,
            ),
            PairState::Verify { sas, peer } => (
                format!("Check this code with {peer}"),
                format!(
                    "{peer} must show exactly the same code. If it does, choose \u{201c}Codes match\u{201d} \
                     here and there. If it does not, someone else may be on the network: choose \
                     \u{201c}They differ\u{201d}."
                ),
                Some(sas),
                true,
            ),
            PairState::Confirmed => (
                "Waiting for the other machine\u{2026}".to_owned(),
                "You confirmed the code. The other machine has to confirm it too.".to_owned(),
                None,
                false,
            ),
            PairState::Received(_) | PairState::Sent | PairState::Failed(_) => (
                "Finishing\u{2026}".to_owned(),
                String::new(),
                None,
                false,
            ),
        };

        let buttons = if can_answer {
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(
                    self.pill("mb-code-match".into(), "Codes match", true)
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.answer_code_check(true, cx)
                        })),
                )
                .child(
                    self.pill("mb-code-differ".into(), "They differ", false)
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.answer_code_check(false, cx)
                        })),
                )
        } else {
            div().flex().flex_row().gap_2().child(
                self.pill("mb-pair-cancel".into(), "Cancel", false)
                    .on_click(cx.listener(|this, _event, _window, cx| this.cancel_pairing(cx))),
            )
        };

        Some(
            div()
                .px_4()
                .py_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(self.dot(pal.accent))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(pal.text))
                                .child(title),
                        ),
                )
                .children(code.map(|code| {
                    div()
                        .text_3xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text))
                        .child(code)
                }))
                .children((!detail.is_empty()).then(|| {
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_dim))
                        .child(detail)
                }))
                .child(buttons)
                .into_any_element(),
        )
    }

    fn connect_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let pal = self.palette;
        let confirm = self.mouse_borders.confirm;

        let mut rows: Vec<AnyElement> = Vec::new();

        if let Some(panel) = self.pairing_panel(cx) {
            rows.push(panel);
        } else {
            let start = self
                .pill("mb-start-pairing".into(), "Start pairing", true)
                .on_click(cx.listener(|this, _event, _window, cx| this.start_pairing(cx)));

            rows.push(self.row(
                "Pair a new device",
                "Makes this machine visible for two minutes. Nothing to copy or paste: you compare a code on both screens.",
                start,
            ));

            let scanning = self.mouse_borders.scanning;
            let scan = self
                .pill(
                    "mb-scan".into(),
                    if scanning {
                        "Looking\u{2026}"
                    } else {
                        "Find devices"
                    },
                    false,
                )
                .on_click(cx.listener(|this, _event, _window, cx| this.scan_for_devices(cx)));

            rows.push(self.row(
                "Find devices",
                "Look for a machine that is showing \u{201c}Waiting for another machine\u{201d}. Both must be on the same network.",
                scan,
            ));

            for device in &self.mouse_borders.discovered_devices {
                let control = if device.pairing_open {
                    let device = device.clone();

                    self.pill(
                        SharedString::from(format!("mb-pair-{}", device.address)),
                        "Pair",
                        true,
                    )
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.pair_with(&device, cx)
                    }))
                    .into_any_element()
                } else {
                    self.dim_text("Not accepting pairing").into_any_element()
                };

                rows.push(self.row(
                    &device.name,
                    &format!("{}:{}", device.address, device.port),
                    control,
                ));
            }
        }

        // The old way, for networks that block discovery or machines that cannot reach each
        // other directly.
        let copy = self
            .pill("mb-copy-code".into(), "Copy pairing code", false)
            .on_click(cx.listener(|this, _event, _window, cx| this.copy_pairing_code(cx)));
        let join = self
            .pill(
                "mb-join".into(),
                if confirm == Some(Confirm::Join) {
                    "Replace key and join"
                } else {
                    "Join from clipboard"
                },
                confirm == Some(Confirm::Join),
            )
            .on_click(cx.listener(|this, _event, _window, cx| this.join_from_clipboard(cx)));

        rows.push(self.row(
            "Pairing code",
            "Without discovery: copy the code here, send it to the other machine yourself, and click Join from clipboard there. The code contains your security key: treat it like a password.",
            div().flex().flex_row().gap_2().child(copy).child(join),
        ));

        let new_key = self
            .pill(
                "mb-new-key".into(),
                if confirm == Some(Confirm::NewKey) {
                    "Click again to confirm"
                } else {
                    "New key"
                },
                confirm == Some(Confirm::NewKey),
            )
            .on_click(cx.listener(|this, _event, _window, cx| this.new_key(cx)));

        let key_hint = match KeyStore::user_default().load() {
            Ok(Some(_)) => "This machine has a security key.".to_owned(),
            Ok(None) => "No key yet: one is created when you start pairing.".to_owned(),
            Err(error) => error.to_string(),
        };

        rows.push(self.row("Security key", &key_hint, new_key));

        if let Some(note) = &self.mouse_borders.note {
            rows.push(
                div()
                    .px_4()
                    .py_3()
                    .text_sm()
                    .text_color(rgb(if note.error { pal.danger } else { pal.text_dim }))
                    .child(note.text.clone())
                    .into_any_element(),
            );
        }

        rows
    }

    /// One cell of the layout board: a machine you can drag, or an empty place you can drop on.
    fn board_cell(
        &self,
        cx: &mut Context<Self>,
        index: u8,
        machine: Option<&Machine>,
        local: &MachineName,
    ) -> AnyElement {
        let pal = self.palette;
        let accent = pal.accent;
        let hover = pal.hover;

        let cell = div()
            .id(SharedString::from(format!("mb-cell-{index}")))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1()
            .w(px(CELL_W))
            .h(px(CELL_H))
            .rounded_md()
            .border_1()
            // Lights up while a machine is dragged over it.
            .drag_over::<DraggedMachine>(move |style, _dragged, _window, _cx| {
                style.border_color(rgb(accent)).bg(rgba(hover))
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedMachine, _window, cx| {
                let name = dragged.name.clone();

                this.mutate(cx, move |settings| {
                    settings_edit::place_machine(&mut settings.mouse_borders, &name, index);
                });
            }));

        let Some(machine) = machine else {
            return cell.border_color(rgba(pal.divider)).into_any_element();
        };

        let is_local = machine.name == local.as_str();
        let name = machine.name.clone();

        cell.border_color(if is_local {
            rgb(pal.accent)
        } else {
            rgba(pal.keycap_border)
        })
        .bg(rgba(pal.hover))
        .cursor_pointer()
        .on_drag(
            DraggedMachine { name: name.clone() },
            move |dragged, _offset, _window, cx| {
                let title = dragged.name.clone();

                cx.new(|_| crate::DragGhost { title, accent })
            },
        )
        .child(div().text_sm().text_color(rgb(pal.text)).child(name))
        .children(is_local.then(|| {
            div()
                .text_xs()
                .text_color(rgb(pal.accent))
                .child("this machine")
        }))
        .into_any_element()
    }

    fn layout_rows(&self, cx: &mut Context<Self>, local: &MachineName) -> Vec<AnyElement> {
        let pal = self.palette;
        let mb = &self.settings.mouse_borders;
        let board = settings_edit::board(mb);

        // Rows of cells, like the display arrangement in an operating system.
        let mut canvas = div().flex().flex_col().gap_2();

        for row in 0..PLANE_ROWS {
            let mut line = div().flex().flex_row().gap_2();

            for col in 0..PLANE_COLS {
                let index = row * PLANE_COLS + col;

                line = line.child(self.board_cell(cx, index, board[usize::from(index)], local));
            }

            canvas = canvas.child(line);
        }

        let mut rows: Vec<AnyElement> = vec![
            div()
                .px_4()
                .py_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(canvas)
                .child(
                    div().text_xs().text_color(rgb(pal.text_dim)).child(
                        "Drag a machine to where it sits on your desk. Machines in the same row \
                         switch left and right; machines in the same column switch up and down. \
                         Drop on another machine to swap places.",
                    ),
                )
                .into_any_element(),
        ];

        for machine in mb.machines.iter() {
            let name = machine.name.clone();
            let hint = if name == local.as_str() {
                "This machine".to_owned()
            } else if machine.address.is_empty() {
                "Found by its name".to_owned()
            } else {
                machine.address.clone()
            };

            let remove = {
                let name = name.clone();

                self.pill(
                    SharedString::from(format!("mb-remove-{name}")),
                    "Remove",
                    false,
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    let name = name.clone();

                    this.mutate(cx, move |settings| {
                        settings_edit::remove_machine(&mut settings.mouse_borders, &name);
                    })
                }))
            };

            rows.push(self.row(&name, &hint, remove));
        }

        if !mb
            .machines
            .iter()
            .any(|machine| machine.name == local.as_str())
        {
            let add = self
                .pill("mb-add-local".into(), &format!("Add {local}"), true)
                .on_click(cx.listener(|this, _event, _window, cx| {
                    let mut next = this.settings.clone();
                    let local = settings_edit::local_name(&next.mouse_borders);

                    match settings_edit::add_machine(&mut next.mouse_borders, &local, None) {
                        Ok(_) => this.mutate(cx, move |settings| *settings = next),
                        Err(error) => this.set_note(error.to_string(), true, cx),
                    }
                }));

            rows.push(self.row(
                "This machine is not in the layout",
                "Add it, then pair the others.",
                add,
            ));
        }

        rows
    }

    fn easy_mouse_choices(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.settings.mouse_borders.easy_mouse;

        self.choices(
            [
                (EasyMouse::Enabled, "Always"),
                (EasyMouse::RequireShift, "Hold Shift"),
                (EasyMouse::RequireCtrl, "Hold Ctrl"),
                (EasyMouse::Disabled, "Off"),
            ]
            .map(|(mode, label)| {
                self.pill(
                    SharedString::from(format!("mb-easy-{mode:?}")),
                    label,
                    current == mode,
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.mutate(cx, |settings| settings.mouse_borders.easy_mouse = mode)
                }))
            })
            .into(),
        )
    }

    fn switch_keys_choices(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.settings.mouse_borders.hotkeys.switch_keys;

        self.choices(
            [
                (SwitchKeys::Digits, "1\u{2013}4"),
                (SwitchKeys::FunctionKeys, "F1\u{2013}F4"),
            ]
            .map(|(keys, label)| {
                self.pill(
                    SharedString::from(format!("mb-keys-{keys:?}")),
                    label,
                    current == keys,
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.mutate(cx, |settings| {
                        settings.mouse_borders.hotkeys.switch_keys = keys
                    })
                }))
            })
            .into(),
        )
    }

    fn shortcut_help(&self) -> String {
        let hotkeys = &self.settings.mouse_borders.hotkeys;
        let key = |value: &str| value.to_uppercase();

        format!(
            "Ctrl+Alt+{} easy mouse \u{b7} {} lock all \u{b7} {} reconnect \u{b7} {} multi-machine input",
            key(&hotkeys.toggle_easy_mouse),
            key(&hotkeys.lock_machines),
            key(&hotkeys.reconnect),
            key(&hotkeys.multi_machine_input),
        )
    }
}
