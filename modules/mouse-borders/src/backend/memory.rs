//! In-memory backends: deterministic stand-ins for tests, demos and `--dry-run` style use.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use aujar_core::{Position, Size};
use tokio::sync::mpsc;

use super::{
    CapturedInput, Clipboard, ClipboardContent, Desktop, InputSink, InputSource, ScreenLocker,
};
use crate::{
    error::{Error, Result},
    input::Button,
};

/// Input fed from a channel; reports whether it is currently grabbed.
pub struct MemorySource {
    events: mpsc::UnboundedReceiver<CapturedInput>,
    grabbed: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct MemorySourceHandle {
    events: mpsc::UnboundedSender<CapturedInput>,
    grabbed: Arc<AtomicBool>,
}

impl MemorySource {
    pub fn new() -> (Self, MemorySourceHandle) {
        let (events_tx, events) = mpsc::unbounded_channel();
        let grabbed = Arc::new(AtomicBool::new(false));

        (
            Self {
                events,
                grabbed: Arc::clone(&grabbed),
            },
            MemorySourceHandle {
                events: events_tx,
                grabbed,
            },
        )
    }
}

impl MemorySourceHandle {
    pub fn send(&self, input: CapturedInput) {
        let _ = self.events.send(input);
    }

    pub fn is_grabbed(&self) -> bool {
        self.grabbed.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl InputSource for MemorySource {
    async fn next(&mut self) -> Result<Option<CapturedInput>> {
        Ok(self.events.recv().await)
    }

    fn set_grab(&mut self, grabbed: bool) -> Result<()> {
        self.grabbed.store(grabbed, Ordering::SeqCst);

        Ok(())
    }
}

/// One injected action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkEvent {
    PointerAbs(Position),
    PointerRel(i32, i32),
    Button(Button, bool),
    Wheel(i32, i32),
    Key(u16, bool),
    Nudge,
}

/// Shared log of everything a [`MemorySink`] injected.
#[derive(Clone, Default)]
pub struct SinkLog(Arc<Mutex<Vec<SinkEvent>>>);

impl SinkLog {
    pub fn snapshot(&self) -> Vec<SinkEvent> {
        self.0.lock().unwrap().clone()
    }

    /// Take and clear the recorded events.
    pub fn take(&self) -> Vec<SinkEvent> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

pub struct MemorySink {
    log: SinkLog,
}

impl MemorySink {
    pub fn new() -> (Self, SinkLog) {
        let log = SinkLog::default();

        (Self { log: log.clone() }, log)
    }

    fn record(&self, event: SinkEvent) -> Result<()> {
        self.log.0.lock().unwrap().push(event);

        Ok(())
    }
}

impl InputSink for MemorySink {
    fn pointer_abs(&mut self, position: Position) -> Result<()> {
        self.record(SinkEvent::PointerAbs(position))
    }

    fn pointer_rel(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.record(SinkEvent::PointerRel(dx, dy))
    }

    fn button(&mut self, button: Button, pressed: bool) -> Result<()> {
        self.record(SinkEvent::Button(button, pressed))
    }

    fn wheel(&mut self, horizontal: i32, vertical: i32) -> Result<()> {
        self.record(SinkEvent::Wheel(horizontal, vertical))
    }

    fn key(&mut self, key: u16, pressed: bool) -> Result<()> {
        self.record(SinkEvent::Key(key, pressed))
    }

    fn nudge(&mut self) -> Result<()> {
        self.record(SinkEvent::Nudge)
    }
}

/// A desktop of fixed size whose pointer the test moves.
#[derive(Clone)]
pub struct MemoryDesktop {
    size: Size,
    pointer: Arc<Mutex<Option<Position>>>,
}

impl MemoryDesktop {
    /// `tracks_pointer: false` mimics a Wayland session that cannot report the pointer.
    pub fn new(size: Size, tracks_pointer: bool) -> Self {
        Self {
            size,
            pointer: Arc::new(Mutex::new(
                tracks_pointer.then_some(Position { x: 0, y: 0 }),
            )),
        }
    }

    pub fn set_pointer(&self, position: Position) {
        let mut pointer = self.pointer.lock().unwrap();

        if pointer.is_some() {
            *pointer = Some(position);
        }
    }
}

impl Desktop for MemoryDesktop {
    fn screen_size(&self) -> Result<Size> {
        Ok(self.size)
    }

    fn pointer(&self) -> Result<Position> {
        self.pointer
            .lock()
            .unwrap()
            .ok_or_else(|| Error::unsupported("pointer tracking", "memory desktop"))
    }
}

#[derive(Clone, Default)]
pub struct MemoryClipboard(Arc<Mutex<Option<ClipboardContent>>>);

impl MemoryClipboard {
    pub fn set(&self, content: ClipboardContent) {
        *self.0.lock().unwrap() = Some(content);
    }

    pub fn get(&self) -> Option<ClipboardContent> {
        self.0.lock().unwrap().clone()
    }
}

#[async_trait]
impl Clipboard for MemoryClipboard {
    async fn read(&self) -> Result<Option<ClipboardContent>> {
        Ok(self.get())
    }

    async fn write(&self, content: &ClipboardContent) -> Result<()> {
        self.set(content.clone());

        Ok(())
    }
}

/// Counts how often the screen was locked.
#[derive(Clone, Default)]
pub struct MemoryLocker(Arc<Mutex<u32>>);

impl MemoryLocker {
    pub fn count(&self) -> u32 {
        *self.0.lock().unwrap()
    }
}

#[async_trait]
impl ScreenLocker for MemoryLocker {
    async fn lock(&self) -> Result<()> {
        *self.0.lock().unwrap() += 1;

        Ok(())
    }
}
