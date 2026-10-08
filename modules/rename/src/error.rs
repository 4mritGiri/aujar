use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RenameError {
    #[error("invalid regular expression: {0}")]
    InvalidRegex(#[from] regex::Error),

    #[error("rename execution blocked because the plan contains validation errors")]
    ExecutionBlocked,

    #[error("failed to rename `{from}` to `{to}`: {source}")]
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
