//! Mouse Borders: turn it on, connect machines with a pairing code, arrange them, tune how
//! switching works. Status refreshes by itself while this page is open.
//!
//! Key and layout changes work on files, so they do not need the daemon; only the live status
//! asks it.

use std::time::Duration;

use aujar_config::{EasyMouse, MatrixMode, SwitchKeys};
use aujar_ipc::{MouseBordersRequest, MouseBordersResponse, MouseBordersStatus, Request, Response};
use aujar_mouse_borders::{
    keystore::KeyStore, net, pairing::PairingCode, secure::SecurityKey, settings_edit,
};
use gpui::{AnyElement, ClipboardItem, Context, SharedString, div, prelude::*, px, rgb, rgba};

use super::{Page, SettingsView};

/// How often the status refreshes while the page is open.
const REFRESH: Duration = Duration::from_secs(2);

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

#[derive(Default)]
pub(super) struct State {
    status: Option<Result<MouseBordersStatus, String>>,
    note: Option<Note>,
    confirm: Option<Confirm>,
    refreshing: bool,
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

    /// Re-ask the daemon every couple of seconds while this page is showing; stops when the
    /// window closes.
    pub(super) fn keep_mouse_borders_status_fresh(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let alive = this.update(cx, |this, cx| {
                    if this.page == Page::MouseBorders {
                        this.refresh_mouse_borders(cx);
                    }
                });

                if alive.is_err() {
                    break;
                }

                cx.background_executor().timer(REFRESH).await;
            }
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

    // ---- Actions ----

    fn copy_pairing_code(&mut self, cx: &mut Context<Self>) {
        let mut next = self.settings.clone();
        let local = settings_edit::local_name(&next.mouse_borders);

        let code = (|| -> Result<String, String> {
            settings_edit::add_machine(&mut next.mouse_borders, &local, None)
                .map_err(|error| error.to_string())?;

            let key = ensure_key()?;
            let host = net::pick_host(&net::local_networks());

            PairingCode::from_settings(&next.mouse_borders, key, &local, host)
                .map(|code| code.encode())
                .map_err(|error| error.to_string())
        })();

        match code {
            Ok(code) => {
                cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                next.mouse_borders.enabled = true;
                self.mouse_borders.confirm = None;
                self.mutate(cx, move |settings| *settings = next);
                self.set_note(
                    format!("Pairing code copied to clipboard!\n\n{}", code),
                    false,
                    cx,
                );
            }
            Err(error) => self.set_note(error, true, cx),
        }
    }

    fn join_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.set_note(
                "Clipboard is empty. Copy a pairing code from the first machine first.",
                true,
                cx,
            );

            return;
        };

        let code = match PairingCode::parse(&text) {
            Ok(code) => code,
            Err(error) => {
                self.set_note(
                    format!("Invalid pairing code: {}", error),
                    true,
                    cx,
                );

                return;
            }
        };

        let store = KeyStore::user_default();

        match store.load() {
            Ok(Some(existing))
                if existing != code.key && self.mouse_borders.confirm != Some(Confirm::Join) =>
            {
                self.mouse_borders.confirm = Some(Confirm::Join);
                self.set_note(
                    "This machine already has a different security key. \
                     Click 'Replace key and join' again to use the new code \
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
        next.mouse_borders.enabled = true;
        self.mutate(cx, move |settings| *settings = next);
        self.set_note(
            format!(
                "\u{2705} Successfully joined! {} machines in the layout. \
                 Mouse and keyboard sharing is now active.",
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
                        .child("Quick Start:"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(self.palette.text_dim))
                        .child(
                            "1. On your first machine: Click 'Copy pairing code' below \u{2014} it's automatically copied. \
                             2. On each other machine: Click 'Join from clipboard' \u{2014} the code is automatically detected. \
                             3. Move your mouse to the screen edge to switch between machines.",
                        ),
                )
                .into_any_element()
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

    fn connect_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let pal = self.palette;
        let confirm = self.mouse_borders.confirm;

        let copy = self
            .pill("mb-copy-code".into(), "Copy pairing code", true)
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
            Ok(None) => "No key yet: one is created when you copy a pairing code.".to_owned(),
            Err(error) => error.to_string(),
        };

        // Check if we have a pairing code to display
        let has_key = KeyStore::user_default().load().is_ok_and(|k| k.is_some());
        let mb = &self.settings.mouse_borders;
        let local = settings_edit::local_name(mb);
        
        // Generate pairing code for display
        let pairing_code = if has_key {
            let key = KeyStore::user_default().load().ok().flatten();
            let host = aujar_mouse_borders::net::pick_host(&aujar_mouse_borders::net::local_networks());
            
            if let (Some(key), Some(host)) = (key, host) {
                let mut temp_settings = mb.clone();
                if let Ok(_) = settings_edit::add_machine(&mut temp_settings, &local, None) {
                    if let Ok(code) = PairingCode::from_settings(&temp_settings, key, &local, Some(host)) {
                        Some(code.encode())
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let mut rows = Vec::new();
        
        // Pairing instructions section
        if let Some(code) = pairing_code {
            // Show the pairing code prominently
            let code_display = div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(pal.text_dim))
                        .child("Share this code with other machines:"),
                )
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgba(pal.hover))
                        .border_1()
                        .border_color(rgba(pal.keycap_border))
                        .text_sm()
                        .child(code.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_faint))
                        .child("Contains your security key - treat it like a password"),
                );
            
            rows.push(self.row(
                "Pairing code",
                "Copy this on your first machine, then paste on others to connect.",
                div().flex().flex_col().gap_2()
                    .child(code_display)
                    .child(div().flex().flex_row().gap_2().child(copy)),
            ));
        } else {
            rows.push(self.row(
                "Pairing code",
                "On the first machine, copy the code. On each other machine, click Join.",
                div().flex().flex_row().gap_2().child(copy).child(join),
            ));
        }
        
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

    fn layout_rows(
        &self,
        cx: &mut Context<Self>,
        local: &aujar_mouse_borders::layout::MachineName,
    ) -> Vec<AnyElement> {
        let pal = self.palette;
        let mb = &self.settings.mouse_borders;
        let slots = settings_edit::slots(mb);

        let cell = |slot: usize| -> AnyElement {
            let machine = slots[slot];
            let is_local = machine.is_some_and(|machine| machine.name == local.as_str());

            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_1()
                .w(px(132.))
                .h(px(54.))
                .rounded_md()
                .border_1()
                .border_color(if is_local {
                    rgb(pal.accent)
                } else {
                    rgba(pal.keycap_border)
                })
                .bg(rgba(pal.hover))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(if machine.is_some() {
                            pal.text
                        } else {
                            pal.text_faint
                        }))
                        .child(machine.map_or("empty".to_owned(), |machine| machine.name.clone())),
                )
                .children(is_local.then(|| {
                    div()
                        .text_xs()
                        .text_color(rgb(pal.accent))
                        .child("this machine")
                }))
                .into_any_element()
        };

        let diagram = match mb.matrix {
            MatrixMode::Row => div().flex().flex_row().gap_2().children((0..4).map(cell)),
            MatrixMode::Grid => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().flex().flex_row().gap_2().children((0..2).map(cell)))
                .child(div().flex().flex_row().gap_2().children((2..4).map(cell))),
        };

        let shape = self.choices(vec![
            self.pill("mb-row".into(), "One row", mb.matrix == MatrixMode::Row)
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.mutate(cx, |settings| {
                        settings.mouse_borders.matrix = MatrixMode::Row
                    })
                })),
            self.pill(
                "mb-grid".into(),
                "2 \u{d7} 2",
                mb.matrix == MatrixMode::Grid,
            )
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.mutate(cx, |settings| {
                    settings.mouse_borders.matrix = MatrixMode::Grid
                })
            })),
        ]);

        let mut rows: Vec<AnyElement> = vec![
            div().px_4().py_4().child(diagram).into_any_element(),
            self.row("Arrangement", "How your machines sit on the desk. Slots are numbered left to right (row by row in 2 \u{d7} 2).", shape),
        ];

        for (index, machine) in slots.iter().enumerate() {
            let Some(machine) = machine else { continue };
            let name = machine.name.clone();
            let hint = if name == local.as_str() {
                "This machine".to_owned()
            } else if machine.address.is_empty() {
                "Found by its name".to_owned()
            } else {
                machine.address.clone()
            };

            let step = |id: &str, label: &str, direction: i8, cx: &mut Context<Self>| {
                let name = name.clone();

                self.pill(SharedString::from(format!("mb-{id}-{name}")), label, false)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        let name = name.clone();

                        this.mutate(cx, move |settings| {
                            settings_edit::move_machine(
                                &mut settings.mouse_borders,
                                &name,
                                direction,
                            );
                        })
                    }))
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

            let controls = div()
                .flex()
                .flex_row()
                .gap_2()
                .child(step("left", "\u{25c0}", -1, cx))
                .child(step("right", "\u{25b6}", 1, cx))
                .child(remove);

            rows.push(self.row(&format!("#{} {}", index + 1, name), &hint, controls));
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
                "Add it, then connect the others with a pairing code.",
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
