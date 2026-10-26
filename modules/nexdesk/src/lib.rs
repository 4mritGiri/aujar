//! NexDesk integration for Aujar.
//!
//! NexDesk (<https://github.com/4mritGiri/nexdesk>) is a separate Rust remote-access suite:
//! an RDP client with its own GPUI manager, isolated RDP engine process and password vault.
//! Aujar does not link it. This module only locates the `nexdesk` executable, starts the
//! manager as a detached process on request, and reports whether it is still running.
//! That keeps NexDesk's process boundary and secrets handling intact and avoids unifying two
//! GPUI/tokio dependency graphs. See docs/NEXDESK.md.

pub mod api;
pub mod locate;

use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use aujar_runtime::{Capability, Module, ModuleContext, ModuleMetadata, RuntimeError};

pub use api::{NexDeskRequest, NexDeskResponse, NexDeskStatus};

const CAPABILITIES: &[Capability] = &[Capability::ExecuteCommand, Capability::Network];

static METADATA: ModuleMetadata = ModuleMetadata::new(
    "nexdesk",
    "NexDesk",
    env!("CARGO_PKG_VERSION"),
    "Open the NexDesk remote-access manager (RDP, remote control) as a separate process.",
    CAPABILITIES,
);

#[derive(Debug, thiserror::Error)]
pub enum NexDeskError {
    #[error(
        "NexDesk is not installed: no `nexdesk` executable beside aujar, on PATH, or at \
         $AUJAR_NEXDESK_BIN (see docs/NEXDESK.md)"
    )]
    NotInstalled,

    #[error("failed to start {path}: {source}")]
    Spawn {
        path: String,
        source: std::io::Error,
    },
}

/// Cheap to clone; every clone shares the same record of the manager it started.
#[derive(Clone, Default)]
pub struct NexDeskModule {
    /// A fixed executable, used by tests. `None` means look it up each time.
    binary: Option<PathBuf>,
    manager: Arc<Mutex<Option<Child>>>,
}

impl NexDeskModule {
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a fixed executable instead of searching (`None` behaves as "not installed").
    pub fn with_binary(binary: Option<PathBuf>) -> Self {
        Self {
            binary: Some(binary.unwrap_or_default()),
            manager: Arc::default(),
        }
    }

    fn executable(&self) -> Option<PathBuf> {
        match &self.binary {
            Some(path) if path.as_os_str().is_empty() => None,
            Some(path) => Some(path.clone()),
            None => locate::locate(),
        }
    }

    /// Process id of the manager this module started, if it is still running. A manager that
    /// has exited is reaped here, so it never lingers as a zombie.
    fn running_pid(slot: &mut Option<Child>) -> Option<u32> {
        let alive = match slot.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        };

        if alive {
            slot.as_ref().map(Child::id)
        } else {
            *slot = None;
            None
        }
    }

    pub fn status(&self) -> NexDeskStatus {
        let path = self.executable();
        let mut slot = self
            .manager
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        NexDeskStatus {
            installed: path.is_some(),
            path: path.map(|path| path.display().to_string()),
            running_pid: Self::running_pid(&mut slot),
        }
    }

    /// Start the manager. Returns its pid and whether one was already running.
    pub fn open(&self) -> Result<(u32, bool), NexDeskError> {
        let mut slot = self
            .manager
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(pid) = Self::running_pid(&mut slot) {
            return Ok((pid, true));
        }

        let path = self.executable().ok_or(NexDeskError::NotInstalled)?;

        let mut command = Command::new(&path);

        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;

            // Own process group: stopping the daemon must not close the user's sessions.
            command.process_group(0);
        }

        let child = command.spawn().map_err(|source| NexDeskError::Spawn {
            path: path.display().to_string(),
            source,
        })?;
        let pid = child.id();

        tracing::info!(program = %path.display(), pid, "launched NexDesk");

        *slot = Some(child);

        Ok((pid, false))
    }
}

#[async_trait]
impl Module for NexDeskModule {
    fn metadata(&self) -> &'static ModuleMetadata {
        &METADATA
    }

    async fn start(&self, _context: ModuleContext) -> Result<(), RuntimeError> {
        // Not being installed is normal: the module simply stays idle.
        match self.executable() {
            Some(path) => tracing::info!(path = %path.display(), "NexDesk found"),
            None => tracing::info!("NexDesk is not installed; the module stays idle"),
        }

        Ok(())
    }

    async fn stop(&self) -> Result<(), RuntimeError> {
        // The manager is the user's window: leave it running when the daemon stops.
        Ok(())
    }
}
