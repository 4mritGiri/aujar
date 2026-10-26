//! Stand-ins used when a platform's real backend cannot start (no permission, no device).
//!
//! They report the reason on every use, so the engine can say *why* a machine cannot capture
//! or inject input instead of failing silently.

use async_trait::async_trait;
use aujar_core::{Position, Size};

use super::{CapturedInput, Desktop, InputSink, InputSource, ScreenLocker};
use crate::{
    error::{Error, Result},
    input::Button,
};

/// An input source that explains why there is none.
pub struct UnavailableSource(String);

impl UnavailableSource {
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

#[async_trait]
impl InputSource for UnavailableSource {
    async fn next(&mut self) -> Result<Option<CapturedInput>> {
        Err(Error::unsupported("input capture", &self.0))
    }

    fn set_grab(&mut self, _grabbed: bool) -> Result<()> {
        Err(Error::unsupported("input capture", &self.0))
    }
}

/// An input sink that explains why there is none.
pub struct UnavailableSink(String);

impl UnavailableSink {
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

impl InputSink for UnavailableSink {
    fn pointer_abs(&mut self, _: Position) -> Result<()> {
        Err(Error::unsupported("input injection", &self.0))
    }

    fn pointer_rel(&mut self, _: i32, _: i32) -> Result<()> {
        Err(Error::unsupported("input injection", &self.0))
    }

    fn button(&mut self, _: Button, _: bool) -> Result<()> {
        Err(Error::unsupported("input injection", &self.0))
    }

    fn wheel(&mut self, _: i32, _: i32) -> Result<()> {
        Err(Error::unsupported("input injection", &self.0))
    }

    fn key(&mut self, _: u16, _: bool) -> Result<()> {
        Err(Error::unsupported("input injection", &self.0))
    }

    fn nudge(&mut self) -> Result<()> {
        Err(Error::unsupported("input injection", &self.0))
    }
}

/// A desktop that cannot be queried at all (an operating system without a backend).
pub struct UnavailableDesktop(String);

impl UnavailableDesktop {
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

impl Desktop for UnavailableDesktop {
    fn screen_size(&self) -> Result<Size> {
        Err(Error::unsupported("reading the screen size", &self.0))
    }

    fn pointer(&self) -> Result<Position> {
        Err(Error::unsupported("pointer tracking", &self.0))
    }
}

/// A screen locker for platforms where we cannot lock.
pub struct UnavailableLocker(String);

impl UnavailableLocker {
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

#[async_trait]
impl ScreenLocker for UnavailableLocker {
    async fn lock(&self) -> Result<()> {
        Err(Error::unsupported("locking the screen", &self.0))
    }
}
