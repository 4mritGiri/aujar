//! The Linux inhibitor: the desktop's own `org.freedesktop.portal.Inhibit`.
//!
//! The lock lasts as long as the request stays open, and the desktop drops it when our
//! connection to the session bus goes away, so a crashed daemon can never leave the computer
//! unable to sleep.

use ashpd::desktop::{
    Request,
    inhibit::{InhibitFlags, InhibitOptions, InhibitProxy},
};
use async_trait::async_trait;

use crate::Inhibitor;

#[derive(Default)]
pub struct PortalInhibitor {
    /// The open request (which keeps the lock), and whether it includes the screen.
    held: Option<(bool, Request<()>)>,
}

/// What to inhibit: sleeping always, and the screen going idle (blanking, locking) when asked.
fn flags(screen: bool) -> ashpd::enumflags2::BitFlags<InhibitFlags> {
    if screen {
        InhibitFlags::Suspend | InhibitFlags::Idle
    } else {
        InhibitFlags::Suspend.into()
    }
}

#[async_trait]
impl Inhibitor for PortalInhibitor {
    async fn hold(&mut self, screen: bool) -> Result<(), String> {
        if matches!(&self.held, Some((held_screen, _)) if *held_screen == screen) {
            return Ok(());
        }

        // A new lock before the old one goes, so there is no moment when the computer may sleep.
        let proxy = InhibitProxy::new()
            .await
            .map_err(|error| format!("the desktop has no inhibit portal ({error})"))?;
        let request = proxy
            .inhibit(
                None,
                flags(screen),
                InhibitOptions::default().set_reason("Aujar Awake"),
            )
            .await
            .map_err(|error| format!("the desktop refused ({error})"))?;

        if let Some((_, old)) = self.held.replace((screen, request)) {
            let _ = old.close().await;
        }

        Ok(())
    }

    async fn release(&mut self) {
        if let Some((_, request)) = self.held.take() {
            let _ = request.close().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screen_adds_idle_to_suspend() {
        let suspend: ashpd::enumflags2::BitFlags<InhibitFlags> = InhibitFlags::Suspend.into();

        assert_eq!(flags(false), suspend);
        assert_eq!(flags(true), InhibitFlags::Suspend | InhibitFlags::Idle);
        assert!(!flags(false).contains(InhibitFlags::Idle));
    }
}
