//! Display geometry, pointer position and screen locking on macOS.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use aujar_core::{Position, Size};
use core_graphics::{
    display::{CGDisplay, CGRect},
    event::CGEvent,
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::{CGPoint, CGSize},
};

use crate::{
    backend::{Desktop, ScreenLocker},
    error::{Error, Result},
};

/// How long a measurement of the displays is reused. Pointer queries happen at mouse rate;
/// monitors do not come and go that fast.
const CACHE: Duration = Duration::from_secs(1);

/// The rectangle spanning every active display, in points. Its origin is negative when a
/// display sits left of or above the main one.
pub fn virtual_bounds() -> CGRect {
    let displays = CGDisplay::active_displays().unwrap_or_default();
    let mut bounds: Option<CGRect> = None;

    for id in displays {
        let rect = CGDisplay::new(id).bounds();

        bounds = Some(match bounds {
            None => rect,
            Some(total) => {
                let left = total.origin.x.min(rect.origin.x);
                let top = total.origin.y.min(rect.origin.y);
                let right =
                    (total.origin.x + total.size.width).max(rect.origin.x + rect.size.width);
                let bottom =
                    (total.origin.y + total.size.height).max(rect.origin.y + rect.size.height);

                CGRect::new(
                    &CGPoint::new(left, top),
                    &CGSize::new(right - left, bottom - top),
                )
            }
        });
    }

    bounds.unwrap_or_else(|| CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(1.0, 1.0)))
}

pub struct MacDesktop {
    cache: Mutex<Option<(Instant, CGRect)>>,
}

impl MacDesktop {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(None),
        }
    }

    fn bounds(&self) -> CGRect {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        match *cache {
            Some((at, rect)) if at.elapsed() < CACHE => rect,
            _ => {
                let rect = virtual_bounds();

                *cache = Some((Instant::now(), rect));

                rect
            }
        }
    }
}

impl Default for MacDesktop {
    fn default() -> Self {
        Self::new()
    }
}

impl Desktop for MacDesktop {
    fn screen_size(&self) -> Result<Size> {
        let bounds = self.bounds();

        Ok(Size {
            width: bounds.size.width.round().max(1.0) as u32,
            height: bounds.size.height.round().max(1.0) as u32,
        })
    }

    fn pointer(&self) -> Result<Position> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| Error::Other("cannot create an event source".into()))?;
        let location = CGEvent::new(source)
            .map_err(|()| Error::Other("cannot read the pointer position".into()))?
            .location();
        let bounds = self.bounds();

        Ok(Position {
            x: (location.x - bounds.origin.x).round() as i32,
            y: (location.y - bounds.origin.y).round() as i32,
        })
    }
}

pub struct MacLocker;

const CG_SESSION: &str =
    "/System/Library/CoreServices/Menu Extras/User.menu/Contents/Resources/CGSession";

#[async_trait]
impl ScreenLocker for MacLocker {
    async fn lock(&self) -> Result<()> {
        // `CGSession -suspend` shows the login window (a real lock) where it still exists;
        // otherwise put the display to sleep, which locks if "require password" is set.
        let status = if std::path::Path::new(CG_SESSION).exists() {
            tokio::process::Command::new(CG_SESSION)
                .arg("-suspend")
                .status()
                .await
        } else {
            tokio::process::Command::new("pmset")
                .arg("displaysleepnow")
                .status()
                .await
        }
        .map_err(|error| Error::Other(format!("cannot lock the screen: {error}")))?;

        status
            .success()
            .then_some(())
            .ok_or_else(|| Error::Other(format!("locking the screen failed ({status})")))
    }
}
