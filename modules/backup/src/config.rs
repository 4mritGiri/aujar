//! Backup jobs and where they are stored (`backup.toml` in the Aujar config directory).
//!
//! Passwords are never stored in the file: a job points to a `password_file` that only its
//! owner may read.

use std::{
    fs,
    path::{Path, PathBuf},
};

use aujar_config::platform;
use serde::{Deserialize, Serialize};

use crate::{error::BackupError, util::is_safe_token};

/// Used when a job sets neither `keep_last` nor `keep_days`.
pub const DEFAULT_KEEP_LAST: usize = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// `pg_dump` (one database) or `pg_dumpall` (no `database` set).
    Postgres,
    /// `mysqldump` (also works for MariaDB).
    Mysql,
    /// `sqlite3 FILE .dump`.
    Sqlite,
    /// A directory or file, as a `tar.gz`.
    Files,
}

impl Kind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::Sqlite => "sqlite",
            Self::Files => "files",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    /// Letters, digits, `-` and `_`. Used in file names and in the crontab.
    pub name: String,
    pub kind: Kind,

    /// Database host as seen from where the dump runs (the server itself for SSH jobs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Database name; leave out to back up every database (needs a privileged user).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    /// File holding the database password (mode 600). Direct connections only; for SSH jobs
    /// use the server's own `~/.pgpass` / `~/.my.cnf`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_file: Option<PathBuf>,

    /// SQLite database file, or the directory/file to archive (`files`). Absolute path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,

    /// `user@server`: run the dump on that server over SSH (key login only) and store the
    /// result here. The database is then reached from the server, usually on `localhost`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_key: Option<PathBuf>,

    /// Run the tool as this operating-system user (`sudo -n -u USER`, or `runuser` when the
    /// connection is root). PostgreSQL clusters usually accept only their own OS user on the
    /// local socket ("peer" login), so a cluster owned by `postgres` needs `run_as = "postgres"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_as: Option<String>,
    /// Folder holding the client tools that match the server, e.g. `/usr/lib/postgresql/18/bin`.
    /// Needed when several PostgreSQL versions are installed: `pg_dump` 16 cannot dump a
    /// version 18 server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin_dir: Option<PathBuf>,
    /// Docker container that runs the database: the dump runs inside it (`docker exec`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,

    /// Local folder for the backups. Default: `~/aujar-backups/<name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest: Option<PathBuf>,

    /// Keep this many of the newest backups (default 14 when no retention is set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_last: Option<usize>,
    /// Also delete backups older than this many days (the newest is always kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_days: Option<u32>,

    /// Encrypt with GnuPG using the passphrase in this file (mode 600)...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpg_passphrase_file: Option<PathBuf>,
    /// ...or with this public key (so the machine never holds the secret to read backups).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpg_recipient: Option<String>,

    /// Five-field cron expression or `@daily`-style macro; installed by `schedule install`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
}

impl Job {
    pub fn new(name: impl Into<String>, kind: Kind) -> Self {
        Self {
            name: name.into(),
            kind,
            host: None,
            port: None,
            user: None,
            database: None,
            password_file: None,
            path: None,
            ssh: None,
            ssh_port: None,
            ssh_key: None,
            run_as: None,
            bin_dir: None,
            container: None,
            dest: None,
            keep_last: None,
            keep_days: None,
            gpg_passphrase_file: None,
            gpg_recipient: None,
            schedule: None,
        }
    }

    pub fn dest_dir(&self) -> PathBuf {
        match &self.dest {
            Some(dest) => expand(dest),
            None => platform::home_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("aujar-backups")
                .join(&self.name),
        }
    }

    pub fn is_encrypted(&self) -> bool {
        self.gpg_passphrase_file.is_some() || self.gpg_recipient.is_some()
    }

    /// `(keep_last, keep_days)` with the default applied.
    pub fn retention(&self) -> (Option<usize>, Option<u32>) {
        if self.keep_last.is_none() && self.keep_days.is_none() {
            (Some(DEFAULT_KEEP_LAST), None)
        } else {
            (self.keep_last, self.keep_days)
        }
    }

    /// Reject anything that could be mistaken for an option, run a command, or silently do
    /// the wrong thing. Called before saving and before every run.
    pub fn validate(&self) -> Result<(), BackupError> {
        let bad = |reason: &str| Err(BackupError::invalid(&self.name, reason));

        if self.name.is_empty()
            || self.name.len() > 64
            || !self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return bad("the name may only contain letters, digits, `-` and `_` (max 64)");
        }

        for (label, value) in [
            ("host", &self.host),
            ("user", &self.user),
            ("database", &self.database),
            ("ssh", &self.ssh),
            ("run_as", &self.run_as),
            ("container", &self.container),
        ] {
            if value.as_deref().is_some_and(|v| !is_safe_token(v)) {
                return bad(&format!(
                    "`{label}` may only contain letters, digits and . _ @ : - (and must not start with `-`)"
                ));
            }
        }

        let bad_recipient = |r: &str| {
            r.is_empty()
                || r.starts_with('-')
                || r.chars()
                    .any(|c| !(c.is_ascii_alphanumeric() || "._@:-+ <>".contains(c)))
        };
        if self.gpg_recipient.as_deref().is_some_and(bad_recipient) {
            return bad("`gpg_recipient` must be a key id, fingerprint or e-mail address");
        }

        if self.gpg_recipient.is_some() && self.gpg_passphrase_file.is_some() {
            return bad("use either `gpg_recipient` or `gpg_passphrase_file`, not both");
        }

        match self.kind {
            Kind::Postgres | Kind::Mysql => {
                if self.path.is_some() {
                    return bad("`path` is only for the sqlite and files kinds");
                }
            }
            Kind::Sqlite | Kind::Files => {
                if self.path.is_none() {
                    return bad("this kind needs `path`");
                }
                if self.database.is_some() || self.host.is_some() || self.user.is_some() {
                    return bad("`host`, `user` and `database` do not apply to this kind");
                }
            }
        }

        if let Some(path) = &self.path {
            let shown = path.display().to_string();
            if shown.contains('\0') {
                return bad("`path` contains an invalid character");
            }
            let resolved = if self.ssh.is_some() {
                path.clone()
            } else {
                expand(path)
            };
            if !resolved.is_absolute() {
                return bad("`path` must be absolute (start with `/`, or `~/` for local jobs)");
            }
            if resolved.file_name().is_none() {
                return bad("`path` must name a file or folder, not the filesystem root");
            }
        }

        if let Some(dir) = &self.bin_dir {
            if !matches!(self.kind, Kind::Postgres | Kind::Mysql) {
                return bad("`bin_dir` is only for the postgres and mysql kinds");
            }
            let shown = dir.display().to_string();
            if shown.contains(['\0', '\n', '\r']) {
                return bad("`bin_dir` contains an invalid character");
            }
            let resolved = if self.ssh.is_some() {
                dir.clone()
            } else {
                expand(dir)
            };
            if !resolved.is_absolute() {
                return bad("`bin_dir` must be absolute (start with `/`)");
            }
        }

        if self.container.is_some() {
            if !matches!(self.kind, Kind::Postgres | Kind::Mysql) {
                return bad("`container` is only for the postgres and mysql kinds");
            }
            if self.host.is_some()
                || self.port.is_some()
                || self.password_file.is_some()
                || self.bin_dir.is_some()
                || self.run_as.is_some()
            {
                return bad(
                    "with `container` the tool runs inside it: leave out `host`, `port`, \
                     `password_file`, `bin_dir` and `run_as`",
                );
            }
        }

        if self.ssh.is_none() && (self.ssh_port.is_some() || self.ssh_key.is_some()) {
            return bad("`ssh_port` and `ssh_key` need `ssh`");
        }
        if self.keep_last == Some(0) {
            return bad("`keep_last` must be at least 1");
        }
        if self.keep_days == Some(0) {
            return bad("`keep_days` must be at least 1");
        }

        if let Some(schedule) = &self.schedule {
            crate::cron::normalize_schedule(schedule)
                .map_err(|reason| BackupError::invalid(&self.name, reason))?;
        }

        Ok(())
    }
}

/// A server the Backup page can scan for database clusters. Only how to reach it is stored;
/// jobs made from it carry their own copy, so cron never needs this list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    /// Letters, digits, `-` and `_`. Used to name the jobs made from this server.
    pub name: String,
    /// `user@host` (key login only).
    pub ssh: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_key: Option<PathBuf>,
}

impl Server {
    pub fn validate(&self) -> Result<(), BackupError> {
        let bad = |reason: &str| Err(BackupError::invalid(&self.name, reason));

        if self.name.is_empty()
            || self.name.len() > 32
            || !self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return bad("the server name may only contain letters, digits, `-` and `_` (max 32)");
        }
        if self.ssh.is_empty() || !is_safe_token(&self.ssh) {
            return bad("the address must look like user@host (letters, digits and . _ @ : -)");
        }
        if self.ssh_port == Some(0) {
            return bad("the SSH port must be 1 or more");
        }
        if let Some(key) = &self.ssh_key {
            if key.display().to_string().contains(['\0', '\n', '\r']) {
                return bad("the key path contains an invalid character");
            }
        }
        Ok(())
    }
}

/// Settings every new job starts with, so adding a database is one click.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Defaults {
    /// Parent folder for backups: a job named `x` saves to `<backup_dir>/x`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_dir: Option<PathBuf>,
    /// Cron expression such as `0 2 * * *`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_last: Option<usize>,
}

impl Defaults {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    pub fn validate(&self) -> Result<(), BackupError> {
        let bad = |reason: &str| Err(BackupError::invalid("defaults", reason));

        if let Some(dir) = &self.backup_dir {
            if dir.display().to_string().contains(['\0', '\n', '\r']) {
                return bad("the backup folder contains an invalid character");
            }
            if !expand(dir).is_absolute() {
                return bad("the backup folder must be absolute (start with `/` or `~`)");
            }
        }
        if self.keep_last == Some(0) {
            return bad("keep_last must be 1 or more");
        }
        if let Some(schedule) = &self.schedule {
            if crate::cron::normalize_schedule(schedule).is_err() {
                return bad("the schedule is not a valid cron expression");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Defaults::is_empty")]
    pub defaults: Defaults,
    #[serde(default)]
    pub jobs: Vec<Job>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<Server>,
}

/// `~` and `~/rest` expanded to the home directory.
pub fn expand(path: &Path) -> PathBuf {
    let home = platform::home_dir();
    platform::expand_home(path, home.as_deref()).unwrap_or_else(|| path.to_owned())
}

/// Where the jobs are stored. `AUJAR_BACKUP_CONFIG` overrides it (tests, several setups).
pub fn config_path() -> PathBuf {
    std::env::var_os("AUJAR_BACKUP_CONFIG")
        .filter(|p| !p.is_empty())
        .map_or_else(|| platform::config_dir().join("backup.toml"), PathBuf::from)
}

/// The log file cron jobs append to.
pub fn log_path() -> PathBuf {
    platform::state_dir().join("backup.log")
}

impl Config {
    pub fn load() -> Result<Self, BackupError> {
        Self::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> Result<Self, BackupError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(BackupError::io(
                    format!("cannot read {}", path.display()),
                    error,
                ));
            }
        };

        let config: Self = toml::from_str(&text)
            .map_err(|error| BackupError::Config(format!("{}: {error}", path.display())))?;

        config.defaults.validate()?;

        let mut servers = std::collections::BTreeSet::new();
        for server in &config.servers {
            server.validate()?;
            if !servers.insert(server.name.clone()) {
                return Err(BackupError::Config(format!(
                    "{}: two servers are named `{}`",
                    path.display(),
                    server.name
                )));
            }
        }

        let mut seen = std::collections::BTreeSet::new();
        for job in &config.jobs {
            job.validate()?;
            if !seen.insert(job.name.clone()) {
                return Err(BackupError::Config(format!(
                    "{}: two jobs are named `{}`",
                    path.display(),
                    job.name
                )));
            }
        }
        Ok(config)
    }

    pub fn save(&self) -> Result<(), BackupError> {
        self.save_to(&config_path())
    }

    /// Written atomically with mode 600, so a crash never leaves half a file.
    pub fn save_to(&self, path: &Path) -> Result<(), BackupError> {
        let text = toml::to_string_pretty(self)
            .map_err(|error| BackupError::Config(format!("cannot serialize config: {error}")))?;
        write_private(
            path,
            &format!("# Managed by `aujar backup`. No passwords belong in this file.\n\n{text}"),
        )
    }

    pub fn job(&self, name: &str) -> Option<&Job> {
        self.jobs.iter().find(|job| job.name == name)
    }

    pub fn upsert(&mut self, job: Job, replace: bool) -> Result<(), BackupError> {
        job.validate()?;
        match self
            .jobs
            .iter()
            .position(|existing| existing.name == job.name)
        {
            Some(_) if !replace => Err(BackupError::Config(format!(
                "a job named `{}` already exists (use --replace to overwrite it)",
                job.name
            ))),
            Some(index) => {
                self.jobs[index] = job;
                Ok(())
            }
            None => {
                self.jobs.push(job);
                Ok(())
            }
        }
    }

    /// Fill what a new job leaves empty from the defaults.
    pub fn apply_defaults(&self, job: &mut Job) {
        let defaults = &self.defaults;

        if job.dest.is_none() {
            job.dest = defaults
                .backup_dir
                .as_ref()
                .map(|dir| dir.join(&job.name));
        }
        if job.schedule.is_none() {
            job.schedule.clone_from(&defaults.schedule);
        }
        if job.keep_last.is_none() && job.keep_days.is_none() {
            job.keep_last = defaults.keep_last;
        }
    }

    pub fn server(&self, name: &str) -> Option<&Server> {
        self.servers.iter().find(|server| server.name == name)
    }

    /// Add a server, or replace the one with the same name.
    pub fn upsert_server(&mut self, server: Server) -> Result<(), BackupError> {
        server.validate()?;
        match self.servers.iter().position(|s| s.name == server.name) {
            Some(index) => self.servers[index] = server,
            None => self.servers.push(server),
        }
        Ok(())
    }

    pub fn remove_server(&mut self, name: &str) -> bool {
        let before = self.servers.len();
        self.servers.retain(|server| server.name != name);
        self.servers.len() != before
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.jobs.len();
        self.jobs.retain(|job| job.name != name);
        self.jobs.len() != before
    }
}

/// Create parent folders, write to a temporary file with mode 600, then rename into place.
pub(crate) fn write_private(path: &Path, text: &str) -> Result<(), BackupError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| BackupError::io(format!("cannot create {}", parent.display()), e))?;
    }
    let tmp = path.with_extension("tmp");

    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
    };
    #[cfg(not(unix))]
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp);

    let mut file =
        file.map_err(|e| BackupError::io(format!("cannot write {}", tmp.display()), e))?;
    std::io::Write::write_all(&mut file, text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| BackupError::io(format!("cannot write {}", tmp.display()), e))?;
    fs::rename(&tmp, path)
        .map_err(|e| BackupError::io(format!("cannot replace {}", path.display()), e))
}

/// Read a secret file (password or passphrase). It must not be readable by other users.
pub fn read_secret(path: &Path) -> Result<String, BackupError> {
    let path = expand(path);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::metadata(&path).map_err(|e| {
            BackupError::io(format!("cannot read secret file {}", path.display()), e)
        })?;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(BackupError::Config(format!(
                "{} is readable by other users; run: chmod 600 {}",
                path.display(),
                path.display()
            )));
        }
    }

    let text = fs::read_to_string(&path)
        .map_err(|e| BackupError::io(format!("cannot read secret file {}", path.display()), e))?;
    let secret = text.trim_end_matches(['\n', '\r']).to_owned();
    if secret.is_empty() {
        return Err(BackupError::Config(format!("{} is empty", path.display())));
    }
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pg(name: &str) -> Job {
        let mut job = Job::new(name, Kind::Postgres);
        job.database = Some("app".into());
        job
    }

    #[test]
    fn accepts_a_plain_postgres_job() {
        pg("prod-db_1").validate().unwrap();
    }

    #[test]
    fn rejects_bad_names_and_option_injection() {
        assert!(pg("a b").validate().is_err());
        assert!(pg("../x").validate().is_err());
        assert!(pg("").validate().is_err());

        let mut job = pg("x");
        job.host = Some("-oProxyCommand=evil".into());
        assert!(job.validate().is_err());

        let mut job = pg("x");
        job.database = Some("a;rm".into());
        assert!(job.validate().is_err());
    }

    #[test]
    fn kind_specific_rules() {
        let mut files = Job::new("f", Kind::Files);
        assert!(files.validate().is_err(), "needs a path");
        files.path = Some("relative/dir".into());
        assert!(files.validate().is_err(), "must be absolute");
        files.path = Some("/srv/data".into());
        files.validate().unwrap();
        files.path = Some("/".into());
        assert!(files.validate().is_err(), "root is not a backup target");
    }

    #[test]
    fn ssh_jobs_can_send_a_local_password_file() {
        let mut job = pg("x");
        job.ssh = Some("deploy@server".into());
        job.password_file = Some("/tmp/pw".into());
        job.validate().unwrap();
    }

    #[test]
    fn encryption_choices_are_exclusive() {
        let mut job = pg("x");
        job.gpg_recipient = Some("me@example.com".into());
        job.gpg_passphrase_file = Some("/tmp/pass".into());
        assert!(job.validate().is_err());
    }

    #[test]
    fn schedule_is_checked() {
        let mut job = pg("x");
        job.schedule = Some("0 2 * * *".into());
        job.validate().unwrap();
        job.schedule = Some("0 2 * * * ; rm -rf /".into());
        assert!(job.validate().is_err());
    }

    #[test]
    fn retention_defaults() {
        assert_eq!(pg("x").retention(), (Some(DEFAULT_KEEP_LAST), None));
        let mut job = pg("x");
        job.keep_days = Some(30);
        assert_eq!(job.retention(), (None, Some(30)));
    }

    #[test]
    fn config_round_trips_and_rejects_duplicates() {
        let dir = std::env::temp_dir().join(format!("aujar-backup-cfg-{}", std::process::id()));
        let path = dir.join("backup.toml");

        let mut config = Config::default();
        let mut job = pg("one");
        job.keep_last = Some(7);
        job.schedule = Some("@daily".into());
        config.upsert(job, false).unwrap();
        assert!(config.upsert(pg("one"), false).is_err());
        config.upsert(pg("one"), true).unwrap();
        config.upsert(pg("two"), false).unwrap();
        config.save_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded, config);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        fs::write(
            &path,
            "[[jobs]]\nname='a'\nkind='postgres'\n[[jobs]]\nname='a'\nkind='postgres'\n",
        )
        .unwrap();
        assert!(Config::load_from(&path).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn run_as_bin_dir_and_container_rules() {
        let mut job = pg("x");
        job.run_as = Some("postgres".into());
        job.bin_dir = Some("/usr/lib/postgresql/18/bin".into());
        job.validate().unwrap();

        job.bin_dir = Some("bin".into());
        assert!(job.validate().is_err(), "bin_dir must be absolute");

        let mut job = pg("x");
        job.container = Some("db-1".into());
        job.validate().unwrap();
        job.port = Some(5432);
        assert!(job.validate().is_err(), "container excludes host and port");

        let mut job = Job::new("f", Kind::Files);
        job.path = Some("/srv".into());
        job.bin_dir = Some("/usr/bin".into());
        assert!(job.validate().is_err());

        let mut job = pg("x");
        job.run_as = Some("-evil".into());
        assert!(job.validate().is_err());
    }

    #[test]
    fn defaults_fill_new_jobs_and_round_trip() {
        let dir = std::env::temp_dir().join(format!("aujar-backup-def-{}", std::process::id()));
        let path = dir.join("backup.toml");

        let mut config = Config::default();
        config.defaults.backup_dir = Some("/srv/backups".into());
        config.defaults.schedule = Some("0 2 * * *".into());
        config.defaults.keep_last = Some(7);

        let mut job = pg("a");
        config.apply_defaults(&mut job);
        assert_eq!(job.dest.as_deref(), Some(Path::new("/srv/backups/a")));
        assert_eq!(job.schedule.as_deref(), Some("0 2 * * *"));
        assert_eq!(job.keep_last, Some(7));

        let mut own = pg("b");
        own.dest = Some("/elsewhere".into());
        own.keep_days = Some(3);
        config.apply_defaults(&mut own);
        assert_eq!(own.dest.as_deref(), Some(Path::new("/elsewhere")));
        assert_eq!(own.keep_last, None);

        config.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap(), config);

        config.defaults.backup_dir = Some("relative".into());
        assert!(config.defaults.validate().is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn servers_round_trip_and_are_validated() {
        let dir = std::env::temp_dir().join(format!("aujar-backup-srv-{}", std::process::id()));
        let path = dir.join("backup.toml");

        let mut config = Config::default();
        config
            .upsert_server(Server {
                name: "prod".into(),
                ssh: "root@203.0.113.7".into(),
                ssh_port: Some(2222),
                ssh_key: None,
            })
            .unwrap();
        assert!(
            config
                .upsert_server(Server {
                    name: "bad name".into(),
                    ssh: "root@h".into(),
                    ssh_port: None,
                    ssh_key: None,
                })
                .is_err()
        );
        assert!(
            config
                .upsert_server(Server {
                    name: "x".into(),
                    ssh: "-oProxyCommand=evil".into(),
                    ssh_port: None,
                    ssh_key: None,
                })
                .is_err()
        );
        config.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap(), config);
        assert!(config.remove_server("prod"));
        assert!(!config.remove_server("prod"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_config_is_empty() {
        let config = Config::load_from(Path::new("/nonexistent/aujar/backup.toml")).unwrap();
        assert!(config.jobs.is_empty());
    }
}
