//! `aujar awake ...`: keep the computer from sleeping, through the Aujar daemon.
//!
//! The daemon holds the lock (so it outlives this command), remembers the mode across restarts
//! and ends a timed mode by itself.

use std::path::PathBuf;

use anyhow::{Result, bail};
use aujar_awake::api::{describe, parse_duration};
use aujar_ipc::{AwakeRequest, AwakeResponse, Request, Response, send};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum AwakeAction {
    /// Say whether the computer is being kept awake, and for how long.
    Status,

    /// Keep the computer awake until you turn it off.
    On,

    /// Let the computer sleep again.
    Off,

    /// Keep the computer awake for a while, e.g. `aujar awake for 2h` or `... for 1h30m`.
    For {
        /// How long: `30m`, `2h`, `1h30m`, `1d` (a bare number is minutes; 1 minute to 7 days).
        time: String,
    },
}

/// The request for an action, or a message saying what is wrong with it.
fn request_for(action: AwakeAction) -> Result<AwakeRequest, String> {
    Ok(match action {
        AwakeAction::Status => AwakeRequest::Status,
        AwakeAction::On => AwakeRequest::Indefinite,
        AwakeAction::Off => AwakeRequest::Off,
        AwakeAction::For { time } => AwakeRequest::For {
            seconds: parse_duration(&time)?,
        },
    })
}

pub async fn run(action: AwakeAction, socket: PathBuf) -> Result<()> {
    let request = request_for(action).map_err(anyhow::Error::msg)?;

    match send(&socket, Request::Awake(request)).await? {
        Response::Awake(AwakeResponse::Status(status)) => println!("{}", describe(&status)),
        Response::Error(message) => bail!(message),
        other => bail!("unexpected answer: {other:?}"),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_action_is_the_request_it_says() {
        assert_eq!(request_for(AwakeAction::Status), Ok(AwakeRequest::Status));
        assert_eq!(request_for(AwakeAction::On), Ok(AwakeRequest::Indefinite));
        assert_eq!(request_for(AwakeAction::Off), Ok(AwakeRequest::Off));
        assert_eq!(
            request_for(AwakeAction::For {
                time: "1h30m".into()
            }),
            Ok(AwakeRequest::For { seconds: 5400 })
        );
    }

    #[test]
    fn a_time_that_makes_no_sense_never_reaches_the_daemon() {
        let error = request_for(AwakeAction::For {
            time: "soon".into(),
        })
        .unwrap_err();

        assert!(error.contains("30m"), "{error}");
    }
}
