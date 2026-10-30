//! NexDesk: whether it is installed and running, an Open button, and the launcher switch for
//! `rdp <name>`. Aujar only starts NexDesk; connections, passwords and the vault stay inside
//! NexDesk (docs/NEXDESK.md).
//!
//! The status comes from the daemon (`aujar nexdesk status`) and refreshes while the page is
//! open.

use std::time::Duration;

use aujar_ipc::{NexDeskRequest, NexDeskResponse, NexDeskStatus, Request, Response};
use gpui::{AnyElement, Context, div, prelude::*, px, rgb};

use super::{Page, SettingsView};

/// How often the status refreshes while the page is open.
const REFRESH: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct State {
    status: Option<Result<NexDeskStatus, String>>,
    /// The result of the last click on Open: the text and whether it is an error.
    note: Option<(String, bool)>,
    refreshing: bool,
}

/// Ask the daemon. Blocking: run it off the UI thread.
fn ask(request: NexDeskRequest) -> Result<NexDeskResponse, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;

    runtime.block_on(async {
        let answer = tokio::time::timeout(
            Duration::from_secs(2),
            aujar_ipc::send(crate::socket_path(), Request::NexDesk(request)),
        )
        .await;

        match answer {
            Err(_) => Err("The Aujar daemon did not answer.".to_owned()),
            Ok(Err(_)) => Err(
                "The Aujar daemon is not running. Start it with `systemctl --user start aujar`."
                    .to_owned(),
            ),
            Ok(Ok(Response::NexDesk(response))) => Ok(response),
            Ok(Ok(Response::Error(message))) => Err(message),
            Ok(Ok(_)) => Err("The daemon sent an unexpected answer.".to_owned()),
        }
    })
}

fn fetch_status() -> Result<NexDeskStatus, String> {
    match ask(NexDeskRequest::Status)? {
        NexDeskResponse::Status(status) => Ok(status),
        NexDeskResponse::Opened { .. } => Err("The daemon sent an unexpected answer.".to_owned()),
    }
}

impl SettingsView {
    pub(super) fn refresh_nexdesk(&mut self, cx: &mut Context<Self>) {
        if self.nexdesk.refreshing {
            return;
        }

        self.nexdesk.refreshing = true;

        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async { fetch_status() })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.nexdesk.refreshing = false;
                this.nexdesk.status = Some(status);
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-ask the daemon every couple of seconds while this page is showing; stops when the
    /// window closes.
    pub(super) fn keep_nexdesk_status_fresh(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let alive = this.update(cx, |this, cx| {
                    if this.page == Page::NexDesk {
                        this.refresh_nexdesk(cx);
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

    fn open_nexdesk(&mut self, cx: &mut Context<Self>) {
        self.nexdesk.note = None;

        cx.spawn(async move |this, cx| {
            let answer = cx
                .background_executor()
                .spawn(async { ask(NexDeskRequest::Open) })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.nexdesk.note = Some(match answer {
                    Ok(NexDeskResponse::Opened {
                        pid,
                        already_running: true,
                    }) => (format!("NexDesk is already open (pid {pid})."), false),
                    Ok(NexDeskResponse::Opened { pid, .. }) => {
                        (format!("Opened NexDesk (pid {pid})."), false)
                    }
                    Ok(NexDeskResponse::Status(_)) => {
                        ("The daemon sent an unexpected answer.".to_owned(), true)
                    }
                    Err(message) => (message, true),
                });

                this.refresh_nexdesk(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn nexdesk_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            self.section("Status", self.nexdesk_status_rows(cx)),
            self.section(
                "Launcher",
                vec![toggle_row!(
                    self,
                    cx,
                    "nx-plugin",
                    "NexDesk connections",
                    "Type rdp or nexdesk in the launcher to list your saved connections; Enter opens NexDesk's password prompt for one. Aujar never sees the password.",
                    plugins.nexdesk
                )],
            ),
            self.section(
                "About",
                vec![self.row(
                    "How it works",
                    "Aujar starts NexDesk as a separate program and reads only the names of your saved connections. Details: docs/NEXDESK.md",
                    div(),
                )],
            ),
        ]
    }

    fn nexdesk_status_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let pal = self.palette;

        let (headline, detail, color) = match &self.nexdesk.status {
            None => ("Checking\u{2026}".to_owned(), String::new(), pal.text_faint),
            Some(Err(message)) => (
                "Aujar is not running".to_owned(),
                message.clone(),
                pal.danger,
            ),
            Some(Ok(status)) if !status.installed => (
                "NexDesk is not installed".to_owned(),
                "Put `nexdesk` and `nexdesk-rdp` next to `aujar` or on PATH, or set AUJAR_NEXDESK_BIN for the daemon. See docs/NEXDESK.md."
                    .to_owned(),
                pal.text_faint,
            ),
            Some(Ok(status)) => {
                let path = status.path.clone().unwrap_or_default();

                match status.running_pid {
                    Some(pid) => (
                        format!("NexDesk is open (pid {pid})"),
                        path,
                        pal.positive,
                    ),
                    None => ("NexDesk is installed".to_owned(), path, pal.positive),
                }
            }
        };

        let installed = matches!(&self.nexdesk.status, Some(Ok(status)) if status.installed);

        let refresh = self
            .pill("nx-refresh".into(), "Refresh", false)
            .on_click(cx.listener(|this, _event, _window, cx| this.refresh_nexdesk(cx)));

        let mut buttons = div().flex().flex_row().gap_2().child(refresh);

        if installed {
            buttons = buttons.child(
                self.pill("nx-open".into(), "Open NexDesk", true)
                    .on_click(cx.listener(|this, _event, _window, cx| this.open_nexdesk(cx))),
            );
        }

        let mut rows: Vec<AnyElement> =
            vec![
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .child(
                        div()
                            .size(px(10.))
                            .rounded_full()
                            .flex_shrink_0()
                            .bg(rgb(color)),
                    )
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
                    .child(buttons)
                    .into_any_element(),
            ];

        if let Some((text, error)) = &self.nexdesk.note {
            rows.push(
                div()
                    .px_4()
                    .py_3()
                    .text_sm()
                    .text_color(rgb(if *error { pal.danger } else { pal.text_dim }))
                    .child(text.clone())
                    .into_any_element(),
            );
        }

        rows
    }
}
