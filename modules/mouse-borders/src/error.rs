use std::fmt;

/// Everything that can go wrong in this crate. Variants carry text meant for the user:
/// the CLI prints them as-is.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid layout: {0}")]
    Layout(String),

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("security key: {0}")]
    Key(String),

    #[error("handshake failed: {0}")]
    Handshake(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("machine `{0}` is not connected")]
    NotConnected(String),

    #[error("{feature} is not available: {reason}")]
    Unsupported {
        feature: &'static str,
        reason: String,
    },

    #[error("file transfer: {0}")]
    Transfer(String),

    #[error("{0}")]
    Other(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    /// The cause alone, without the "<feature> is not available:" prefix, for wrapping an
    /// error in a new message without repeating itself.
    pub fn reason(&self) -> String {
        match self {
            Self::Unsupported { reason, .. } => reason.clone(),
            other => other.to_string(),
        }
    }

    pub fn unsupported(feature: &'static str, reason: impl fmt::Display) -> Self {
        Self::Unsupported {
            feature,
            reason: reason.to_string(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
