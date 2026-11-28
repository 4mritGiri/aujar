//! Native global hotkeys.
//!
//! | Platform | How | Needs |
//! |---|---|---|
//! | Linux | `org.freedesktop.portal.GlobalShortcuts` (below) | a desktop with the portal |
//! | Windows | `RegisterHotKey` on a thread with its own message loop | nothing; a key another program owns is reported |
//! | macOS | a Quartz event tap on a thread with its own run loop | Accessibility permission |
//!
//! Triggers are written as in the settings (`CTRL+ALT+space`, [`Trigger`]).
//!
//! The portal is the only cross-compositor way to receive global shortcuts on
//! Wayland. It is available on KDE Plasma, GNOME 48+ and Hyprland (via their
//! portal backends); desktops without it report [`HotkeyEvent::Unavailable`]
//! and users can bind `aujar launcher toggle` in their desktop settings instead
//! (see docs/HOTKEYS.md).
//!
//! Everything runs on a dedicated thread with its own tokio runtime, so the
//! daemon's runtime never depends on the portal's futures being `Send`.

#[cfg(target_os = "macos")]
mod macos;
mod trigger;
#[cfg(windows)]
mod windows;

pub use trigger::{Key, Trigger};

#[cfg(target_os = "linux")]
use anyhow::{Context, Result};
#[cfg(target_os = "linux")]
use ashpd::{
    AppID,
    desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut},
};
#[cfg(target_os = "linux")]
use futures_util::StreamExt;
#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// Application id announced to the portal. Must match the installed desktop file
/// (`packaging/desktop/dev.aujar.Aujar.desktop`). Change together with it.
pub const APP_ID: &str = "dev.aujar.Aujar";

/// A shortcut Aujar asks the desktop to bind.
#[derive(Debug, Clone)]
pub struct ShortcutSpec {
    /// Stable identifier reported back when the shortcut fires.
    pub id: String,
    /// Human-readable description shown in the desktop's shortcut settings.
    pub description: String,
    /// The trigger as written in the settings (`CTRL+ALT+space`, `SUPER+SHIFT+S`). On Linux the
    /// portal only takes it as a suggestion; the user can change it in the desktop's UI.
    pub trigger: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// The desktop bound a shortcut; `trigger` is its human-readable key combination.
    Bound { id: String, trigger: String },
    /// The shortcut with this id was pressed.
    Activated(String),
    /// This one shortcut could not be bound (taken by another program, or not a valid key).
    Failed { id: String, reason: String },
    /// Global shortcuts cannot be used; the reason is meant for logs.
    Unavailable(String),
}

/// Start listening for the given shortcuts on a background thread.
#[cfg(any(windows, target_os = "macos"))]
pub fn start(specs: Vec<ShortcutSpec>) -> UnboundedReceiver<HotkeyEvent> {
    let (tx, rx) = unbounded_channel();
    let parsed = parse_all(specs, &tx);

    #[cfg(windows)]
    windows::start(parsed, tx);
    #[cfg(target_os = "macos")]
    macos::start(parsed, tx);

    rx
}

/// Start listening for the given shortcuts on a background thread.
#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
pub fn start(_specs: Vec<ShortcutSpec>) -> UnboundedReceiver<HotkeyEvent> {
    let (tx, rx) = unbounded_channel();
    let _ = tx.send(HotkeyEvent::Unavailable(
        "native global hotkeys are not implemented for this platform yet".to_owned(),
    ));
    rx
}

/// Start listening for the given shortcuts on a background thread.
#[cfg(target_os = "linux")]
pub fn start(specs: Vec<ShortcutSpec>) -> UnboundedReceiver<HotkeyEvent> {
    {
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
}

/// The specs whose triggers parse; the others are reported as [`HotkeyEvent::Failed`].
#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
fn parse_all(
    specs: Vec<ShortcutSpec>,
    tx: &UnboundedSender<HotkeyEvent>,
) -> Vec<(ShortcutSpec, Trigger)> {
    specs
        .into_iter()
        .filter_map(|spec| match spec.trigger.parse::<Trigger>() {
            Ok(trigger) => Some((spec, trigger)),
            Err(reason) => {
                let _ = tx.send(HotkeyEvent::Failed {
                    id: spec.id,
                    reason,
                });
                None
            }
        })
        .collect()
}

#[cfg(target_os = "linux")]
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

    let shortcuts: Vec<NewShortcut> = parse_all(specs.to_vec(), tx)
        .iter()
        .map(|(spec, trigger)| {
            NewShortcut::new(spec.id.as_str(), spec.description.as_str())
                .preferred_trigger(trigger.xdg().as_str())
        })
        .collect();

    let bound = portal
        .bind_shortcuts(&session, &shortcuts, None, BindShortcutsOptions::default())
        .await
        .context("failed to bind global shortcuts")?
        .response()
        .context("the desktop did not accept the shortcuts")?;

    if bound.shortcuts().is_empty() {
        anyhow::bail!(
            "the desktop did not bind any shortcut (was the dialog dismissed, or is the key already taken?)"
        );
    }

    for shortcut in bound.shortcuts() {
        let _ = tx.send(HotkeyEvent::Bound {
            id: shortcut.id().to_string(),
            trigger: shortcut.trigger_description().to_string(),
        });
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
    #[cfg(target_os = "linux")]
    fn app_id_is_a_valid_portal_app_id() {
        assert!(AppID::try_from(APP_ID).is_ok());
    }

    #[test]
    fn invalid_triggers_are_reported_per_shortcut() {
        let (tx, mut rx) = unbounded_channel();
        let parsed = parse_all(
            vec![
                ShortcutSpec {
                    id: "good".into(),
                    description: String::new(),
                    trigger: "CTRL+ALT+space".into(),
                },
                ShortcutSpec {
                    id: "bad".into(),
                    description: String::new(),
                    trigger: "S".into(),
                },
            ],
            &tx,
        );

        assert_eq!(parsed.len(), 1);
        assert!(matches!(rx.try_recv(), Ok(HotkeyEvent::Failed { id, .. }) if id == "bad"));
    }
}
