//! `aujar nexdesk ...`: open the NexDesk manager through the Aujar daemon.
//!
//! NexDesk is a separate program (see docs/NEXDESK.md). Connection profiles and passwords stay
//! in NexDesk; this command only asks the daemon to start it and reports what is installed.

use std::path::PathBuf;

use anyhow::{Result, bail};
use aujar_ipc::{NexDeskRequest, NexDeskResponse, Request, Response, send};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum NexDeskAction {
    /// Open the NexDesk manager window.
    Open,

    /// Say whether NexDesk is installed and whether the manager Aujar started is running.
    Status,
}

pub async fn run(action: NexDeskAction, socket: PathBuf) -> Result<()> {
    let request = match action {
        NexDeskAction::Open => NexDeskRequest::Open,
        NexDeskAction::Status => NexDeskRequest::Status,
    };

    match send(&socket, Request::NexDesk(request)).await? {
        Response::NexDesk(NexDeskResponse::Opened {
            pid,
            already_running: true,
        }) => println!("NexDesk is already open (pid {pid})."),
        Response::NexDesk(NexDeskResponse::Opened { pid, .. }) => {
            println!("Opened NexDesk (pid {pid}).");
        }
        Response::NexDesk(NexDeskResponse::Status(status)) => {
            match (&status.path, status.installed) {
                (Some(path), true) => println!("installed:  {path}"),
                _ => println!(
                    "installed:  no (put `nexdesk` next to `aujar` or on PATH, or set \
                     AUJAR_NEXDESK_BIN; see docs/NEXDESK.md)"
                ),
            }

            match status.running_pid {
                Some(pid) => println!("running:    yes (pid {pid})"),
                None => println!("running:    no"),
            }
        }
        Response::Error(message) => bail!(message),
        other => bail!("unexpected answer: {other:?}"),
    }

    Ok(())
}
