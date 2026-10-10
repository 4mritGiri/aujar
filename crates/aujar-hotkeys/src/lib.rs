//! Native global hotkeys through the `org.freedesktop.portal.GlobalShortcuts` portal.
//!
//! The portal is the only cross-compositor way to receive global shortcuts on
//! Wayland. It is available on KDE Plasma, GNOME 48+ and Hyprland (via their
//! portal backends); desktops without it report [`HotkeyEvent::Unavailable`]
//! and users can bind `aujar launcher toggle` in their desktop settings instead
//! (see docs/HOTKEYS.md).
//!
//! Everything runs on a dedicated thread with its own tokio runtime, so the
//! daemon's runtime never depends on the portal's futures being `Send`.

use anyhow::{Context, Result};
use ashpd::{
    AppID,
    desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut},
};
use futures_util::StreamExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Application id announced to the portal. Must match the installed desktop file
/// (`packaging/desktop/dev.aujar.Aujar.desktop`). Change together with it.
pub const APP_ID: &str = "dev.aujar.Aujar";

/// A shortcut Aujar asks the desktop to bind.
#[derive(Debug, Clone)]
pub struct ShortcutSpec {
    /// Stable identifier reported back when the shortcut fires.
    pub id: &'static str,
    /// Human-readable description shown in the desktop's shortcut settings.
    pub description: &'static str,
    /// Suggested trigger in XDG shortcuts syntax, e.g. `CTRL+ALT+space`. The user can
    /// change or reject it in the desktop's UI.
    pub preferred_trigger: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// The shortcut with this id was pressed.
    Activated(String),
    /// Global shortcuts cannot be used; the reason is meant for logs.
    Unavailable(String),
}

/// Start listening for the given shortcuts on a background thread.
pub fn start(specs: Vec<ShortcutSpec>) -> UnboundedReceiver<HotkeyEvent> {
    let (tx, rx) = unbounded_channel();
    let thread_tx = tx.clone();

    let spawned = std::thread::Builder::new()
        .name("aujar-hotkeys".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = thread_tx.send(HotkeyEvent::Unavailable(error.to_string()));
                    return;
                }
            };

            if let Err(error) = runtime.block_on(listen(&specs, &thread_tx)) {
                let _ = thread_tx.send(HotkeyEvent::Unavailable(format!("{error:#}")));
            }
        });

    if let Err(error) = spawned {
        let _ = tx.send(HotkeyEvent::Unavailable(error.to_string()));
    }

    rx
}

async fn listen(specs: &[ShortcutSpec], tx: &UnboundedSender<HotkeyEvent>) -> Result<()> {
    // Host (non-Flatpak) apps have no app id unless they register one. Best effort:
    // some portal backends refuse shortcuts for unidentified apps.
    match AppID::try_from(APP_ID) {
        Ok(app_id) => {
            if let Err(error) = ashpd::register_host_app(app_id).await {
                tracing::warn!(%error, "could not register the app id with the portal");
            }
        }
        Err(error) => tracing::warn!(%error, "invalid portal app id"),
    }

    let portal = GlobalShortcuts::new()
        .await
        .context("the GlobalShortcuts portal is not available on this desktop")?;

    let session = portal
        .create_session(Default::default())
        .await
        .context("failed to create a global shortcuts session")?;

    let shortcuts: Vec<NewShortcut> = specs
        .iter()
        .map(|spec| {
            NewShortcut::new(spec.id, spec.description).preferred_trigger(spec.preferred_trigger)
        })
        .collect();

    let bound = portal
        .bind_shortcuts(&session, &shortcuts, None, BindShortcutsOptions::default())
        .await
        .context("failed to bind global shortcuts")?
        .response()
        .context("the desktop did not accept the shortcuts")?;

    for shortcut in bound.shortcuts() {
        tracing::info!(
            id = shortcut.id(),
            trigger = shortcut.trigger_description(),
            "global shortcut bound"
        );
    }

    let mut activated = portal
        .receive_activated()
        .await
        .context("failed to subscribe to shortcut activations")?;

    while let Some(event) = activated.next().await {
        if tx
            .send(HotkeyEvent::Activated(event.shortcut_id().to_owned()))
            .is_err()
        {
            break;
        }
    }

    // Keep the session alive for as long as we listen.
    drop(session);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_id_is_a_valid_portal_app_id() {
        assert!(AppID::try_from(APP_ID).is_ok());
    }
}
