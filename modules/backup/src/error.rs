use std::{io, path::PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("invalid backup job `{job}`: {reason}")]
    Invalid { job: String, reason: String },

    #[error("{0}")]
    Config(String),

    #[error("`{tool}` was not found. {hint}")]
    ToolMissing { tool: String, hint: String },

    #[error("{tool} failed ({status}): {stderr}")]
    ToolFailed {
        tool: String,
        status: String,
        stderr: String,
    },

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },

    #[error("the backup contained no data, so it was discarded")]
    Empty,

    #[error("checksum mismatch for {0}: the file changed or is damaged")]
    ChecksumMismatch(PathBuf),

    #[error("scheduler: {0}")]
    Schedule(String),
}

impl BackupError {
    pub(crate) fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    pub(crate) fn invalid(job: &str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            job: job.to_owned(),
            reason: reason.into(),
        }
    }
}
