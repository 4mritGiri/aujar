//! Locking the screen through systemd-logind.

use std::process::Stdio;

use async_trait::async_trait;
use tokio::process::Command;

use crate::{
    backend::ScreenLocker,
    error::{Error, Result},
};

pub struct LogindLocker;

#[async_trait]
impl ScreenLocker for LogindLocker {
    async fn lock(&self) -> Result<()> {
        // Inside a login session lock just that one; as a system service (no session) lock
        // them all.
        let verb = if std::env::var_os("XDG_SESSION_ID").is_some() {
            "lock-session"
        } else {
            "lock-sessions"
        };

        let status = Command::new("loginctl")
            .arg(verb)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|error| Error::Other(format!("cannot run loginctl: {error}")))?;

        status
            .success()
            .then_some(())
            .ok_or_else(|| Error::Other(format!("`loginctl {verb}` failed ({status})")))
    }
}
