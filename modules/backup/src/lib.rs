//! Backups of databases and folders to the local machine, with optional SSH, encryption,
//! retention and cron scheduling. The CLI (`aujar backup ...`) only translates arguments;
//! everything is implemented here.
//!
//! Design rules:
//!
//! * Tools are started directly, never through a shell; hosts, users and names are validated
//!   so they cannot be mistaken for options.
//! * Passwords are not stored in the config and never appear on a command line.
//! * A backup is written under a temporary name and renamed only when it is complete, so a
//!   failed run leaves nothing behind and old backups are pruned only after a good one.
//! * Servers are explored, not configured by hand: `discover` asks a machine (this one or one
//!   over SSH) which PostgreSQL, MySQL/MariaDB and Docker databases are running, on which
//!   ports, so a server with many clusters on many ports needs no per-port set-up.
//! * Backups are private (mode 600 in a 700 folder) and can be encrypted with GnuPG.

mod config;
pub mod cron;
pub mod discover;
mod error;
mod runner;
mod util;

pub use config::{
    Config, DEFAULT_KEEP_LAST, Defaults, Job, Kind, Server, config_path, expand, log_path,
};
pub use error::BackupError;
pub use runner::{
    BackupFile, Plan, Report, checksum_path, expired, list_backups, plan, run_job, run_job_at,
    verify_file,
};
pub use util::{human_bytes, iso, now_secs, shell_quote, which};
