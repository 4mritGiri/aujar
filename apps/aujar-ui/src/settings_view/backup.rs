//! Backup: back up the databases of this computer or of any server over SSH, to this computer.
//!
//! Nothing is typed in by port or by hand. Pick a machine (this computer, or a server added
//! once as `user@host`), press Scan, and the page lists every PostgreSQL, MySQL/MariaDB and
//! Docker database it finds, whatever its port or version. From a cluster you can add one job
//! for everything in it, or pick databases and get one job (one file) per database. Each job's
//! settings stay editable afterwards.
//!
//! All work lives in `aujar-backup` (the same code as `aujar backup ...`, and the jobs are the
//! same `backup.toml`), so a job made here runs from cron, and the other way round. Scans,
//! database listings, backups and the cron update run off the UI thread.
//!
//! Passwords are never typed here or stored: servers use key login, PostgreSQL clusters use
//! their own OS user on the local socket, and anything else points to a `password_file`.

use std::{collections::HashMap, path::PathBuf, process::Command, time::Duration};

use aujar_backup::{
    BackupFile, Config, Job, Kind, Server, cron,
    discover::{self, Cluster, Discovery, Target},
    human_bytes, iso,
};
use gpui::{AnyElement, Context, Div, SharedString, Stateful, div, prelude::*, px, rgb};

use super::{EditTarget, Page, SettingsView};

/// How often the list of backups and the cron state refresh while the page is open.
const REFRESH: Duration = Duration::from_secs(15);

/// The editable settings of a job, one row each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Field {
    Database,
    User,
    Host,
    Port,
    RunAs,
    BinDir,
    PasswordFile,
    Dest,
    Schedule,
    KeepLast,
    KeepDays,
    GpgRecipient,
    GpgPassphrase,
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Self::Database => "Database",
            Self::User => "Database user",
            Self::Host => "Host",
            Self::Port => "Port",
            Self::RunAs => "Run as OS user",
            Self::BinDir => "Client tools folder",
            Self::PasswordFile => "Password file",
            Self::Dest => "Save backups in",
            Self::Schedule => "Schedule",
            Self::KeepLast => "Keep newest",
            Self::KeepDays => "Keep for days",
            Self::GpgRecipient => "Encrypt to GnuPG key",
            Self::GpgPassphrase => "Encrypt with passphrase file",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Database => "One database name. Empty backs up every database in the server.",
            Self::User => "The database role to connect as.",
            Self::Host => "As seen from where the dump runs. Empty uses the local socket.",
            Self::Port => "The cluster's TCP port.",
            Self::RunAs => {
                "Operating-system user the dump runs as (PostgreSQL peer login), e.g. postgres."
            }
            Self::BinDir => {
                "Matching pg_dump/psql, e.g. /usr/lib/postgresql/18/bin. Needed with several versions."
            }
            Self::PasswordFile => {
                "For a cluster that asks for a password: a file on this computer (chmod 600) holding it. It is sent over SSH when the dump starts, never stored on the server or shown in a command."
            }
            Self::Dest => "A folder on this computer. Empty uses ~/aujar-backups/<name>.",
            Self::Schedule => {
                "A cron expression such as 0 2 * * *, @daily, or `daily 02:00`. Empty turns it off."
            }
            Self::KeepLast => "How many of the newest backups to keep (empty: 14).",
            Self::KeepDays => {
                "Also delete backups older than this many days. The newest is always kept."
            }
            Self::GpgRecipient => {
                "Key id, fingerprint or e-mail. Only the key owner can read the backups."
            }
            Self::GpgPassphrase => {
                "A file (mode 600) with the passphrase. Use either this or a key."
            }
        }
    }

    fn get(self, job: &Job) -> String {
        let path = |p: &Option<PathBuf>| p.as_ref().map(|p| p.display().to_string());

        let value = match self {
            Self::Database => job.database.clone(),
            Self::User => job.user.clone(),
            Self::Host => job.host.clone(),
            Self::Port => job.port.map(|p| p.to_string()),
            Self::RunAs => job.run_as.clone(),
            Self::BinDir => path(&job.bin_dir),
            Self::PasswordFile => path(&job.password_file),
            Self::Dest => path(&job.dest),
            Self::Schedule => job.schedule.clone(),
            Self::KeepLast => job.keep_last.map(|n| n.to_string()),
            Self::KeepDays => job.keep_days.map(|n| n.to_string()),
            Self::GpgRecipient => job.gpg_recipient.clone(),
            Self::GpgPassphrase => path(&job.gpg_passphrase_file),
        };

        value.unwrap_or_default()
    }

    /// What the row shows: the value, or what an empty value means.
    fn shown(self, job: &Job) -> String {
        let value = self.get(job);

        if !value.is_empty() {
            return value;
        }

        match self {
            Self::Database => "all databases".to_owned(),
            Self::Dest => job.dest_dir().display().to_string(),
            Self::Schedule => "not scheduled".to_owned(),
            Self::KeepLast => {
                if job.keep_days.is_some() {
                    "no limit".to_owned()
                } else {
                    format!("default ({})", aujar_backup::DEFAULT_KEEP_LAST)
                }
            }
            Self::KeepDays => "no limit".to_owned(),
            Self::Host => "local socket".to_owned(),
            _ => "not set".to_owned(),
        }
    }

    /// Apply the typed text to a job. Empty clears the setting. The caller validates the job.
    fn set(self, job: &mut Job, text: &str) -> Result<(), String> {
        let text = text.trim();
        let value = (!text.is_empty()).then(|| text.to_owned());
        let path = || value.clone().map(PathBuf::from);

        match self {
            Self::Database => job.database = value,
            Self::User => job.user = value,
            Self::Host => job.host = value,
            Self::Port => {
                job.port = match &value {
                    None => None,
                    Some(text) => Some(
                        text.parse::<u16>()
                            .ok()
                            .filter(|port| *port > 0)
                            .ok_or("The port is a number from 1 to 65535.")?,
                    ),
                }
            }
            Self::RunAs => job.run_as = value,
            Self::BinDir => job.bin_dir = path(),
            Self::PasswordFile => job.password_file = path(),
            Self::Dest => job.dest = path(),
            Self::Schedule => {
                job.schedule = match &value {
                    None => None,
                    Some(text) => Some(match text.strip_prefix("daily ") {
                        Some(time) => cron::daily_at(time.trim())?,
                        None => cron::normalize_schedule(text)?,
                    }),
                }
            }
            Self::KeepLast => {
                job.keep_last = match &value {
                    None => None,
                    Some(text) => Some(
                        text.parse::<usize>()
                            .ok()
                            .filter(|n| *n >= 1)
                            .ok_or("Type a whole number of 1 or more.")?,
                    ),
                }
            }
            Self::KeepDays => {
                job.keep_days = match &value {
                    None => None,
                    Some(text) => Some(
                        text.parse::<u32>()
                            .ok()
                            .filter(|n| *n >= 1)
                            .ok_or("Type a whole number of days, 1 or more.")?,
                    ),
                }
            }
            Self::GpgRecipient => job.gpg_recipient = value,
            Self::GpgPassphrase => job.gpg_passphrase_file = path(),
        }

        Ok(())
    }

    /// The settings that make sense for this job, in display order.
    fn for_job(job: &Job) -> Vec<Field> {
        let mut fields = Vec::new();

        if matches!(job.kind, Kind::Postgres | Kind::Mysql) {
            fields.extend([Self::Database, Self::User]);

            if job.container.is_none() {
                fields.extend([
                    Self::Host,
                    Self::Port,
                    Self::RunAs,
                    Self::BinDir,
                    Self::PasswordFile,
                ]);
            }
        }

        fields.extend([
            Self::Dest,
            Self::Schedule,
            Self::KeepLast,
            Self::KeepDays,
            Self::GpgRecipient,
            Self::GpgPassphrase,
        ]);

        fields
    }
}

/// What every new job starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Setup {
    Folder,
    Schedule,
    Keep,
}

impl Setup {
    fn hint(self) -> &'static str {
        match self {
            Self::Folder => {
                "A folder on this computer, e.g. /srv/backups or ~/Backups. Each job gets its own sub-folder. Empty goes back to ~/aujar-backups."
            }
            Self::Schedule => {
                "A cron expression such as 0 2 * * *, @daily, or `daily 02:00`. Empty means new jobs are not scheduled."
            }
            Self::Keep => "How many of the newest backups to keep for each new job (empty: 14).",
        }
    }
}

/// Ask the desktop for a folder. `Ok(None)` means the person cancelled.
fn pick_folder(start: &std::path::Path) -> Result<Option<PathBuf>, String> {
    let choices: [(&str, Vec<String>); 2] = [
        (
            "zenity",
            vec![
                "--file-selection".to_owned(),
                "--directory".to_owned(),
                "--title=Backup folder".to_owned(),
                format!("--filename={}/", start.display()),
            ],
        ),
        (
            "kdialog",
            vec![
                "--getexistingdirectory".to_owned(),
                start.display().to_string(),
            ],
        ),
    ];

    for (program, args) in choices {
        let Some(exe) = aujar_backup::which(program) else {
            continue;
        };
        let output = Command::new(exe)
            .args(args)
            .output()
            .map_err(|error| format!("Could not open the folder dialog: {error}"))?;

        if !output.status.success() {
            return Ok(None);
        }

        let chosen = String::from_utf8_lossy(&output.stdout).trim().to_owned();

        return Ok((!chosen.is_empty()).then(|| PathBuf::from(chosen)));
    }

    Err("No folder dialog is installed (zenity or kdialog). Use \"Type path\" instead.".to_owned())
}

/// The newest backup of a job and how many there are.
#[derive(Clone)]
struct Latest {
    count: usize,
    newest: Option<BackupFile>,
}

/// Whether cron runs what the jobs ask for.
#[derive(Clone, PartialEq, Eq)]
enum CronState {
    Unknown,
    /// No job has a schedule and cron has nothing of ours.
    Idle,
    NotInstalled,
    UpToDate,
    OutOfDate,
    Error(String),
}

#[derive(Clone)]
struct Snapshot {
    config: Result<Config, String>,
    cron: CronState,
    latest: HashMap<String, Latest>,
}

enum Scan {
    Running,
    Done(Discovery),
    Failed(String),
}

enum Dbs {
    Loading,
    Ready(Vec<String>),
    Failed(String),
}

#[derive(Default)]
pub(super) struct State {
    snapshot: Option<Snapshot>,
    /// The machine shown: `None` is this computer, `Some(name)` a saved server.
    server: Option<String>,
    /// Scan results per machine (`""` is this computer).
    scans: HashMap<String, Scan>,
    /// Database lists per cluster, keyed by the name of its "all databases" job.
    dbs: HashMap<String, Dbs>,
    /// The job whose settings are open.
    selected: Option<String>,
    /// What is running right now (one thing at a time).
    busy: Option<String>,
    /// The result of the last action and whether it is an error.
    note: Option<(String, bool)>,
    refreshing: bool,
    /// Bumped on every change to backup.toml so an older refresh cannot overwrite it.
    generation: u64,
}

fn server_key(server: &Option<String>) -> String {
    server.clone().unwrap_or_default()
}

/// The `aujar` command that cron should run: next to this program, or on the PATH.
fn cli_path() -> Option<PathBuf> {
    let beside = std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name("aujar"))
        .filter(|path| path.is_file());

    beside.or_else(|| aujar_backup::which("aujar"))
}

fn saved_crontab_copy() -> PathBuf {
    aujar_config::platform::state_dir().join("crontab.before-aujar")
}

fn cron_state(config: &Config) -> CronState {
    let installed = match cron::installed_block() {
        Ok(block) => block,
        Err(error) => return CronState::Error(error.to_string()),
    };
    let scheduled = config.jobs.iter().any(|job| job.schedule.is_some());

    match (installed, scheduled) {
        (None, false) => CronState::Idle,
        (None, true) => CronState::NotInstalled,
        (Some(block), _) => {
            let wanted = cli_path()
                .map(|exe| cron::render_block(&config.jobs, &exe, &aujar_backup::log_path()))
                .unwrap_or_default();

            if block == wanted {
                CronState::UpToDate
            } else {
                CronState::OutOfDate
            }
        }
    }
}

/// Everything the page shows that needs the disk or `crontab`. Blocking.
fn load_snapshot() -> Snapshot {
    let config = Config::load().map_err(|error| error.to_string());

    let (latest, cron) = match &config {
        Ok(config) => (
            config
                .jobs
                .iter()
                .map(|job| {
                    let files = aujar_backup::list_backups(job);

                    (
                        job.name.clone(),
                        Latest {
                            count: files.len(),
                            newest: files.first().cloned(),
                        },
                    )
                })
                .collect(),
            cron_state(config),
        ),
        Err(_) => (HashMap::new(), CronState::Unknown),
    };

    Snapshot {
        config,
        cron,
        latest,
    }
}

/// `name | user@host | port | key`, or just `user@host` (the name is then made from the host).
fn parse_server(text: &str) -> Result<Server, String> {
    let parts: Vec<&str> = text.split('|').map(str::trim).collect();

    let none: &[&str] = &[];

    let (name, ssh, rest): (String, &str, &[&str]) = match parts.as_slice() {
        [ssh] if ssh.contains('@') => {
            let ssh: &str = ssh;
            let host = ssh.rsplit('@').next().unwrap_or(ssh);
            let name: String = host
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();

            (name.trim_matches('-').chars().take(32).collect(), ssh, none)
        }
        [name, ssh, rest @ ..] if rest.len() <= 2 => ((*name).to_owned(), *ssh, rest),
        _ => {
            return Err(
                "Write it as: name | user@host | port | key (port and key are optional)."
                    .to_owned(),
            );
        }
    };

    let ssh_port = match rest.first().filter(|port| !port.is_empty()) {
        None => None,
        Some(port) => Some(
            port.parse::<u16>()
                .ok()
                .filter(|port| *port > 0)
                .ok_or("The SSH port is a number from 1 to 65535.")?,
        ),
    };
    let ssh_key = rest
        .get(1)
        .filter(|key| !key.is_empty())
        .map(|key| PathBuf::from(*key));

    let server = Server {
        name,
        ssh: ssh.to_owned(),
        ssh_port,
        ssh_key,
    };

    server.validate().map_err(|error| error.to_string())?;

    Ok(server)
}

fn server_line(server: &Server) -> String {
    format!(
        "{} | {} | {} | {}",
        server.name,
        server.ssh,
        server.ssh_port.map(|p| p.to_string()).unwrap_or_default(),
        server
            .ssh_key
            .as_ref()
            .map(|k| k.display().to_string())
            .unwrap_or_default()
    )
}

/// `PostgreSQL "app" on root@host :5433`, for the job list.
fn source_text(job: &Job) -> String {
    let engine = match job.kind {
        Kind::Postgres => "PostgreSQL",
        Kind::Mysql => "MySQL/MariaDB",
        Kind::Sqlite => "SQLite",
        Kind::Files => "Folder",
    };
    let what = match (&job.path, &job.database) {
        (Some(path), _) => path.display().to_string(),
        (None, Some(database)) => format!("\"{database}\""),
        (None, None) => "all databases".to_owned(),
    };
    let place = match (&job.ssh, &job.container) {
        (Some(ssh), Some(container)) => format!(" in {container} on {ssh}"),
        (Some(ssh), None) => format!(" on {ssh}"),
        (None, Some(container)) => format!(" in {container}"),
        (None, None) => String::new(),
    };
    let port = job.port.map(|p| format!(" :{p}")).unwrap_or_default();

    format!("{engine} {what}{place}{port}")
}

fn retention_text(job: &Job) -> String {
    match job.retention() {
        (Some(last), Some(days)) => format!("keep {last}, {days} days"),
        (Some(last), None) => format!("keep {last}"),
        (None, Some(days)) => format!("keep {days} days"),
        (None, None) => "keep all".to_owned(),
    }
}

fn latest_text(latest: Option<&Latest>) -> String {
    match latest.and_then(|latest| latest.newest.as_ref().map(|file| (latest.count, file))) {
        Some((count, file)) => format!(
            "last {} ({}), {count} kept",
            iso(file.taken_at),
            human_bytes(file.bytes)
        ),
        None => "no backup yet".to_owned(),
    }
}

impl SettingsView {
    // ---- Loading ----

    pub(super) fn refresh_backup(&mut self, cx: &mut Context<Self>) {
        if self.backup.refreshing {
            return;
        }

        self.backup.refreshing = true;
        let generation = self.backup.generation;

        cx.spawn(async move |this, cx| {
            let snapshot = cx
                .background_executor()
                .spawn(async { load_snapshot() })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.backup.refreshing = false;

                if this.backup.generation == generation {
                    this.backup.snapshot = Some(snapshot);
                    cx.notify();
                } else {
                    // The file changed while this was loading: load it again.
                    this.refresh_backup(cx);
                }
            });
        })
        .detach();
    }

    /// Reload now and then while this page is showing, so backups made by cron appear.
    pub(super) fn keep_backup_fresh(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH).await;

                let alive = this.update(cx, |this, cx| {
                    if this.page == Page::Backup && this.backup.busy.is_none() {
                        this.refresh_backup(cx);
                    }
                });

                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn backup_config(&self) -> Result<&Config, String> {
        match &self.backup.snapshot {
            Some(snapshot) => snapshot.config.as_ref().map_err(|error| error.clone()),
            None => Err("Still loading the backup settings.".to_owned()),
        }
    }

    /// The name used in job names and how to reach the machine that is shown.
    fn backup_target(&self) -> Result<(String, Target), String> {
        match &self.backup.server {
            None => Ok(("local".to_owned(), Target::local())),
            Some(name) => self
                .backup_config()?
                .server(name)
                .map(|server| (server.name.clone(), Target::from(server)))
                .ok_or_else(|| format!("There is no server named {name}.")),
        }
    }

    fn report_backup(&mut self, text: String, error: bool, cx: &mut Context<Self>) {
        self.backup.note = Some((text, error));
        cx.notify();
    }

    // ---- Changing backup.toml ----

    /// Load backup.toml, let `change` edit it, save it, and show the result at once.
    fn try_change_backup_config(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Config) -> Result<String, String>,
    ) -> Result<(), String> {
        let mut config = Config::load().map_err(|error| error.to_string())?;
        let message = change(&mut config)?;

        config.save().map_err(|error| error.to_string())?;

        self.backup.generation += 1;

        if let Some(snapshot) = self.backup.snapshot.as_mut() {
            snapshot.config = Ok(config);
        }

        self.status = Some(message);
        self.refresh_backup(cx);
        cx.notify();

        Ok(())
    }

    /// Like [`Self::try_change_backup_config`]; a problem is shown on the page.
    fn change_backup_config(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Config) -> Result<String, String>,
    ) {
        if let Err(error) = self.try_change_backup_config(cx, change) {
            self.report_backup(error, true, cx);
        }
    }

    fn add_backup_jobs(&mut self, jobs: Vec<Job>, cx: &mut Context<Self>) {
        self.change_backup_config(cx, move |config| {
            let (mut added, mut existing) = (0, 0);

            for job in jobs {
                if config.job(&job.name).is_some() {
                    existing += 1;
                } else {
                    let mut job = job;

                    config.apply_defaults(&mut job);
                    config
                        .upsert(job, false)
                        .map_err(|error| error.to_string())?;
                    added += 1;
                }
            }

            Ok(match (added, existing) {
                (0, _) => "Those jobs already exist".to_owned(),
                (1, _) => "Added 1 backup job".to_owned(),
                (n, _) => format!("Added {n} backup jobs"),
            })
        });
    }

    fn remove_backup_job(&mut self, name: String, cx: &mut Context<Self>) {
        if self.backup.selected.as_deref() == Some(name.as_str()) {
            self.backup.selected = None;
            self.editing = None;
        }

        self.change_backup_config(cx, move |config| {
            if config.remove(&name) {
                Ok(format!(
                    "Removed {name}. Its backup files were not touched."
                ))
            } else {
                Err(format!("There is no job named {name}."))
            }
        });
    }

    fn remove_backup_server(&mut self, name: String, cx: &mut Context<Self>) {
        if self.backup.server.as_deref() == Some(name.as_str()) {
            self.backup.server = None;
        }

        self.backup.scans.remove(&name);
        self.editing = None;

        self.change_backup_config(cx, move |config| {
            if config.remove_server(&name) {
                Ok(format!("Removed server {name}. Its jobs were kept."))
            } else {
                Err(format!("There is no server named {name}."))
            }
        });
    }

    /// Called when Enter is pressed in the inline editor for a backup setting.
    pub(super) fn commit_backup(
        &mut self,
        target: &EditTarget,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        match target {
            EditTarget::BackupServer => {
                let server = parse_server(text)?;
                let name = server.name.clone();
                let announced = name.clone();

                self.try_change_backup_config(cx, move |config| {
                    config
                        .upsert_server(server)
                        .map_err(|error| error.to_string())?;

                    Ok(format!("Saved server {announced}"))
                })?;

                // Look at it straight away: that is why it was added.
                self.backup.server = Some(name);
                self.scan_backup_server(cx);

                Ok(())
            }
            EditTarget::BackupField(field) => {
                let field = *field;
                let name = self
                    .backup
                    .selected
                    .clone()
                    .ok_or_else(|| "No job is open.".to_owned())?;
                let text = text.to_owned();

                self.try_change_backup_config(cx, move |config| {
                    let job = config
                        .jobs
                        .iter_mut()
                        .find(|job| job.name == name)
                        .ok_or("That job no longer exists.")?;

                    let mut changed = job.clone();

                    field.set(&mut changed, &text)?;
                    changed.validate().map_err(|error| error.to_string())?;
                    *job = changed;

                    Ok(format!("{} saved", field.label()))
                })
            }
            EditTarget::BackupDefault(setup) => {
                let setup = *setup;
                let text = text.trim().to_owned();

                self.try_change_backup_config(cx, move |config| {
                    let mut probe = Job::new("probe", Kind::Files);

                    match setup {
                        Setup::Folder => {
                            config.defaults.backup_dir =
                                (!text.is_empty()).then(|| PathBuf::from(&text));
                        }
                        Setup::Schedule => {
                            Field::Schedule.set(&mut probe, &text)?;
                            config.defaults.schedule = probe.schedule;
                        }
                        Setup::Keep => {
                            Field::KeepLast.set(&mut probe, &text)?;
                            config.defaults.keep_last = probe.keep_last;
                        }
                    }

                    config
                        .defaults
                        .validate()
                        .map_err(|error| error.to_string())?;

                    Ok("Default saved. It applies to jobs you add from now on.".to_owned())
                })
            }
            _ => Ok(()),
        }
    }

    /// Open a folder dialog and make the choice the default backup folder.
    fn choose_backup_folder(&mut self, cx: &mut Context<Self>) {
        let start = self
            .backup_config()
            .ok()
            .and_then(|config| config.defaults.backup_dir.clone())
            .map(|dir| aujar_backup::expand(&dir))
            .or_else(aujar_config::platform::home_dir)
            .unwrap_or_else(std::env::temp_dir);

        self.run_backup_work(
            "Waiting for you to choose a folder\u{2026}".to_owned(),
            cx,
            move || {
                let Some(folder) = pick_folder(&start)? else {
                    return Ok("No folder chosen. Nothing changed.".to_owned());
                };

                let mut config = Config::load().map_err(|error| error.to_string())?;

                config.defaults.backup_dir = Some(folder.clone());
                config
                    .defaults
                    .validate()
                    .map_err(|error| error.to_string())?;
                config.save().map_err(|error| error.to_string())?;

                Ok(format!(
                    "New backups go to {}/<job name>. Use \"Move existing jobs\" to change the ones you already have.",
                    folder.display()
                ))
            },
        );
    }

    /// Point every job at `<backup folder>/<job name>`. Old files stay where they are.
    fn move_jobs_to_backup_folder(&mut self, cx: &mut Context<Self>) {
        self.change_backup_config(cx, |config| {
            let Some(dir) = config.defaults.backup_dir.clone() else {
                return Err("Choose a backup folder first.".to_owned());
            };

            for job in &mut config.jobs {
                job.dest = Some(dir.join(&job.name));
            }

            Ok(format!(
                "{} job(s) now save under {}. Backups already made stay where they were.",
                config.jobs.len(),
                dir.display()
            ))
        });
    }

    /// One job for every running cluster on the machine shown.
    fn add_every_cluster(&mut self, cx: &mut Context<Self>) {
        let (server, target) = match self.backup_target() {
            Ok(found) => found,
            Err(error) => return self.report_backup(error, true, cx),
        };
        let jobs: Vec<Job> = match self.backup.scans.get(&server_key(&self.backup.server)) {
            Some(Scan::Done(found)) => found
                .clusters
                .iter()
                .filter(|cluster| cluster.backupable())
                .map(|cluster| cluster.job(&server, &target, None))
                .collect(),
            _ => Vec::new(),
        };

        if jobs.is_empty() {
            return self.report_backup(
                "Scan first: there is nothing to add yet.".to_owned(),
                true,
                cx,
            );
        }

        self.add_backup_jobs(jobs, cx);
    }

    // ---- Background work ----

    fn select_backup_server(&mut self, server: Option<String>, cx: &mut Context<Self>) {
        self.backup.server = server;
        self.editing = None;
        cx.notify();

        // The first look at a machine scans it.
        if !self
            .backup
            .scans
            .contains_key(&server_key(&self.backup.server))
        {
            self.scan_backup_server(cx);
        }
    }

    fn scan_backup_server(&mut self, cx: &mut Context<Self>) {
        let target = match self.backup_target() {
            Ok((_, target)) => target,
            Err(error) => return self.report_backup(error, true, cx),
        };
        let key = server_key(&self.backup.server);

        self.backup.scans.insert(key.clone(), Scan::Running);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { discover::scan(&target).map_err(|error| error.to_string()) })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.backup.scans.insert(
                    key,
                    match found {
                        Ok(found) => Scan::Done(found),
                        Err(error) => Scan::Failed(error),
                    },
                );
                cx.notify();
            });
        })
        .detach();
    }

    fn load_backup_databases(&mut self, cluster: Cluster, cx: &mut Context<Self>) {
        let (server, target) = match self.backup_target() {
            Ok(found) => found,
            Err(error) => return self.report_backup(error, true, cx),
        };
        let template = cluster.job(&server, &target, None);
        let id = template.name.clone();

        self.backup.dbs.insert(id.clone(), Dbs::Loading);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let list = cx
                .background_executor()
                .spawn(async move {
                    discover::list_databases(&template).map_err(|error| error.to_string())
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.backup.dbs.insert(
                    id,
                    match list {
                        Ok(list) => Dbs::Ready(list),
                        Err(error) => Dbs::Failed(error),
                    },
                );
                cx.notify();
            });
        })
        .detach();
    }

    /// Run `work` off the UI thread with a label on the page, then show its text.
    fn run_backup_work(
        &mut self,
        label: String,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) {
        if self.backup.busy.is_some() {
            return;
        }

        self.backup.busy = Some(label);
        self.backup.note = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work() }).await;

            let _ = this.update(cx, |this, cx| {
                this.backup.busy = None;

                match result {
                    Ok(text) => {
                        this.status = Some("Done".to_owned());
                        this.backup.note = Some((text, false));
                    }
                    Err(error) => {
                        this.status = Some("Something went wrong".to_owned());
                        this.backup.note = Some((error, true));
                    }
                }

                this.refresh_backup(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn find_backup_job(&self, name: &str) -> Result<Job, String> {
        self.backup_config()?
            .job(name)
            .cloned()
            .ok_or_else(|| format!("There is no job named {name}."))
    }

    fn run_backup_now(&mut self, name: String, cx: &mut Context<Self>) {
        let job = match self.find_backup_job(&name) {
            Ok(job) => job,
            Err(error) => return self.report_backup(error, true, cx),
        };

        self.run_backup_work(format!("Backing up {name}\u{2026}"), cx, move || {
            run_one(&job)
        });
    }

    fn run_all_backups(&mut self, cx: &mut Context<Self>) {
        let jobs = self.backup_config().map(|config| config.jobs.clone());
        let jobs = match jobs {
            Ok(jobs) => jobs,
            Err(error) => return self.report_backup(error, true, cx),
        };

        if jobs.is_empty() {
            return self.report_backup("There are no backup jobs yet.".to_owned(), true, cx);
        }

        self.run_backup_work("Backing up every job\u{2026}".to_owned(), cx, move || {
            let mut lines = Vec::new();
            let mut failed = 0;

            for job in &jobs {
                match run_one(job) {
                    Ok(line) => lines.push(line),
                    Err(error) => {
                        failed += 1;
                        lines.push(format!("{}: {error}", job.name));
                    }
                }
            }

            let text = lines.join("\n");

            if failed == 0 {
                Ok(text)
            } else {
                Err(format!("{failed} of {} failed.\n{text}", jobs.len()))
            }
        });
    }

    fn verify_backup(&mut self, name: String, cx: &mut Context<Self>) {
        let newest = self
            .backup
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.latest.get(&name))
            .and_then(|latest| latest.newest.clone());

        let Some(file) = newest else {
            return self.report_backup(format!("{name} has no backup to check yet."), true, cx);
        };

        self.run_backup_work(format!("Checking {name}\u{2026}"), cx, move || {
            aujar_backup::verify_file(&file.path)
                .map(|checked| format!("{}: {}", file.path.display(), checked.join("; ")))
                .map_err(|error| error.to_string())
        });
    }

    fn apply_backup_schedule(&mut self, cx: &mut Context<Self>) {
        let jobs = self.backup_config().map(|config| config.jobs.clone());
        let jobs = match jobs {
            Ok(jobs) => jobs,
            Err(error) => return self.report_backup(error, true, cx),
        };
        let Some(exe) = cli_path() else {
            return self.report_backup(
                "The `aujar` command was not found, and cron needs it. Install Aujar first \
                 (docs/PACKAGING.md)."
                    .to_owned(),
                true,
                cx,
            );
        };

        self.run_backup_work("Updating cron\u{2026}".to_owned(), cx, move || {
            let installed = cron::install(
                &jobs,
                &exe,
                &aujar_backup::log_path(),
                &saved_crontab_copy(),
            )
            .map_err(|error| error.to_string())?;

            Ok(if installed.scheduled_jobs == 0 {
                "No job has a schedule, so cron has nothing of ours.".to_owned()
            } else {
                format!(
                    "{} scheduled job(s) are in your crontab. Log: {}",
                    installed.scheduled_jobs,
                    aujar_backup::log_path().display()
                )
            })
        });
    }

    fn remove_backup_schedule(&mut self, cx: &mut Context<Self>) {
        self.run_backup_work("Updating cron\u{2026}".to_owned(), cx, move || {
            cron::remove(&saved_crontab_copy())
                .map(|removed| {
                    if removed {
                        "Removed Aujar's schedule from your crontab.".to_owned()
                    } else {
                        "Aujar had nothing in your crontab.".to_owned()
                    }
                })
                .map_err(|error| error.to_string())
        });
    }

    // ---- Page ----

    pub(super) fn backup_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let config = match &self.backup.snapshot {
            None => {
                return vec![self.section(
                    "Backup",
                    vec![self.message("Loading the backup settings\u{2026}", false)],
                )];
            }
            Some(Snapshot {
                config: Err(error), ..
            }) => {
                return vec![self.section(
                    "Backup settings",
                    vec![
                        self.message(error, true),
                        self.message(
                            &format!(
                                "Fix or remove {} and this page works again.",
                                aujar_backup::config_path().display()
                            ),
                            false,
                        ),
                    ],
                )];
            }
            Some(Snapshot {
                config: Ok(config), ..
            }) => config,
        };

        let mut page = Vec::new();

        if let Some(label) = &self.backup.busy {
            page.push(self.section("Working", vec![self.message(label, false)]));
        } else if let Some((text, error)) = &self.backup.note {
            page.push(self.section(
                if *error { "Problem" } else { "Result" },
                vec![self.message(text, *error)],
            ));
        }

        page.push(self.backup_setup_section(config, cx));
        page.push(self.backup_machine_section(config, cx));
        page.push(self.backup_clusters_section(config, cx));
        page.push(self.backup_jobs_section(config, cx));

        if let Some(job) = self
            .backup
            .selected
            .as_ref()
            .and_then(|name| config.job(name))
        {
            page.push(self.backup_job_section(job, cx));
        }

        page.push(self.backup_cron_section(config, cx));

        page
    }

    /// A wrapped line of text inside a card.
    fn message(&self, text: &str, error: bool) -> AnyElement {
        let pal = self.palette;

        div()
            .px_4()
            .py_3()
            .text_xs()
            .text_color(rgb(if error { pal.danger } else { pal.text_dim }))
            .child(text.to_owned())
            .into_any_element()
    }

    /// A value shown beside a row's button, cut off with an ellipsis when long.
    fn value_label(&self, text: &str) -> AnyElement {
        div()
            .max_w(px(240.))
            .truncate()
            .text_sm()
            .text_color(rgb(self.palette.text_dim))
            .child(text.to_owned())
            .into_any_element()
    }

    /// Where backups go and what every new job starts with.
    fn backup_setup_section(&self, config: &Config, cx: &mut Context<Self>) -> AnyElement {
        let defaults = &config.defaults;
        let editing = self.editing.as_ref().map(|edit| edit.target.clone());

        let folder_text = defaults.backup_dir.as_ref().map_or_else(
            || {
                aujar_config::platform::home_dir()
                    .unwrap_or_default()
                    .join("aujar-backups")
                    .display()
                    .to_string()
                    + "  (default)"
            },
            |dir| dir.display().to_string(),
        );
        let folder_edit = defaults
            .backup_dir
            .as_ref()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();

        let folder_controls = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_end()
            .gap_3()
            .child(self.value_label(&folder_text))
            .child(self.choices(vec![
                self.pill(SharedString::from("bk-folder-pick"), "Choose folder\u{2026}", true)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.choose_backup_folder(cx);
                    })),
                self.pill(SharedString::from("bk-folder-type"), "Type path", false)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.start_edit(
                            EditTarget::BackupDefault(Setup::Folder),
                            folder_edit.clone(),
                            cx,
                        );
                    })),
                self.pill(SharedString::from("bk-folder-move"), "Move existing jobs here", false)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.move_jobs_to_backup_folder(cx);
                    })),
            ]));

        let mut rows = vec![self.row(
            "Backup folder",
            "Where backups are saved on this computer. Each job gets its own sub-folder.",
            folder_controls,
        )];

        if editing == Some(EditTarget::BackupDefault(Setup::Folder)) {
            rows.extend(self.editor_row(Setup::Folder.hint(), false, cx));
        }

        let schedule_text = defaults
            .schedule
            .clone()
            .unwrap_or_else(|| "not scheduled".to_owned());
        let schedule_edit = defaults.schedule.clone().unwrap_or_default();

        rows.push(
            self.row(
                "Schedule for new jobs",
                "Set once, e.g. `daily 02:00`, and every database you add is scheduled.",
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(self.value_label(&schedule_text))
                    .child(
                        self.pill(SharedString::from("bk-def-schedule"), "Edit", false)
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.start_edit(
                                    EditTarget::BackupDefault(Setup::Schedule),
                                    schedule_edit.clone(),
                                    cx,
                                );
                            })),
                    ),
            ),
        );

        if editing == Some(EditTarget::BackupDefault(Setup::Schedule)) {
            rows.extend(self.editor_row(Setup::Schedule.hint(), false, cx));
        }

        let keep_text = defaults.keep_last.map_or_else(
            || format!("default ({})", aujar_backup::DEFAULT_KEEP_LAST),
            |n| n.to_string(),
        );
        let keep_edit = defaults
            .keep_last
            .map(|n| n.to_string())
            .unwrap_or_default();

        rows.push(
            self.row(
                "Keep newest for new jobs",
                "Older backups are removed after a new one succeeds.",
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(self.value_label(&keep_text))
                    .child(
                        self.pill(SharedString::from("bk-def-keep"), "Edit", false)
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.start_edit(
                                    EditTarget::BackupDefault(Setup::Keep),
                                    keep_edit.clone(),
                                    cx,
                                );
                            })),
                    ),
            ),
        );

        if editing == Some(EditTarget::BackupDefault(Setup::Keep)) {
            rows.extend(self.editor_row(Setup::Keep.hint(), false, cx));
        }

        self.section("Setup", rows)
    }

    fn backup_machine_section(&self, config: &Config, cx: &mut Context<Self>) -> AnyElement {
        let chosen = &self.backup.server;

        let mut pills = vec![
            self.pill(
                SharedString::from("bk-srv-local"),
                "This computer",
                chosen.is_none(),
            )
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.select_backup_server(None, cx);
            })),
        ];

        for server in &config.servers {
            let name = server.name.clone();

            pills.push(
                self.pill(
                    SharedString::from(format!("bk-srv-{}", server.name)),
                    &server.name,
                    chosen.as_deref() == Some(server.name.as_str()),
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.select_backup_server(Some(name.clone()), cx);
                })),
            );
        }

        pills.push(
            self.pill(SharedString::from("bk-srv-add"), "+ Add server", false)
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.start_edit(EditTarget::BackupServer, String::new(), cx);
                })),
        );

        let mut rows = vec![self.row(
            "Back up databases from",
            "Databases are found and dumped on that machine. Backups are stored on this computer.",
            self.choices(pills),
        )];

        let adding = self
            .editing
            .as_ref()
            .is_some_and(|edit| edit.target == EditTarget::BackupServer);

        if let Some(server) = chosen.as_ref().and_then(|name| config.server(name)) {
            let line = server_line(server);
            let name = server.name.clone();

            let controls = self.choices(vec![
                self.pill(SharedString::from("bk-srv-edit"), "Edit", false)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.start_edit(EditTarget::BackupServer, line.clone(), cx);
                    })),
                self.pill(SharedString::from("bk-srv-del"), "Remove", false)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.remove_backup_server(name.clone(), cx);
                    })),
            ]);

            let how = format!(
                "{}{}{}",
                server.ssh,
                server
                    .ssh_port
                    .map(|port| format!(" \u{b7} port {port}"))
                    .unwrap_or_default(),
                server
                    .ssh_key
                    .as_ref()
                    .map(|key| format!(" \u{b7} key {}", key.display()))
                    .unwrap_or_default()
            );

            rows.push(self.row(&server.name, &how, controls));
        }

        if adding {
            rows.extend(self.editor_row(
                "Format: name | user@host | SSH port | key file. Only the first two are needed; \
                 login must work with a key (no password prompts).",
                false,
                cx,
            ));
        }

        self.section("Machine", rows)
    }

    fn backup_clusters_section(&self, config: &Config, cx: &mut Context<Self>) -> AnyElement {
        let key = server_key(&self.backup.server);
        let title = match &self.backup.server {
            Some(name) => format!("Databases on {name}"),
            None => "Databases on this computer".to_owned(),
        };

        let (server_name, target) = match self.backup_target() {
            Ok(found) => found,
            Err(error) => return self.section(&title, vec![self.message(&error, true)]),
        };

        let mut rows: Vec<AnyElement> = Vec::new();

        match self.backup.scans.get(&key) {
            None => rows.push(self.row(
                "Not scanned yet",
                "Finds PostgreSQL, MySQL/MariaDB and Docker databases on every port.",
                self.backup_scan_pill("Scan", cx),
            )),
            Some(Scan::Running) => rows.push(self.message("Scanning\u{2026}", false)),
            Some(Scan::Failed(error)) => {
                rows.push(self.message(error, true));
                rows.push(self.row(
                    "Try again",
                    "Check that key login works from a terminal.",
                    self.backup_scan_pill("Scan", cx),
                ));
            }
            Some(Scan::Done(found)) => {
                if found.clusters.is_empty() {
                    rows.push(self.row(
                        "No databases found",
                        "No PostgreSQL, MySQL/MariaDB or Docker database is running here.",
                        self.backup_scan_pill("Scan again", cx),
                    ));
                }

                for cluster in &found.clusters {
                    rows.extend(self.backup_cluster_rows(
                        config,
                        cluster,
                        &server_name,
                        &target,
                        cx,
                    ));
                }

                for note in &found.notes {
                    rows.push(self.message(note, false));
                }

                if found.clusters.iter().any(Cluster::backupable) {
                    rows.push(self.row(
                        "Set up everything",
                        "One job for every running cluster above, using your folder and schedule.",
                        self.pill(SharedString::from("bk-add-every"), "Add every cluster", true)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.add_every_cluster(cx);
                            })),
                    ));
                }

                if !found.clusters.is_empty() {
                    rows.push(self.row(
                        "Look again",
                        "After you start, stop or add a database.",
                        self.backup_scan_pill("Scan again", cx),
                    ));
                }
            }
        }

        self.section(&title, rows)
    }

    fn backup_scan_pill(&self, label: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        self.pill(SharedString::from("bk-scan"), label, false)
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.scan_backup_server(cx);
            }))
    }

    fn backup_cluster_rows(
        &self,
        config: &Config,
        cluster: &Cluster,
        server_name: &str,
        target: &Target,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let all = cluster.job(server_name, target, None);
        let id = all.name.clone();

        if !cluster.backupable() {
            return vec![self.row(&cluster.title(), &cluster.detail(), div())];
        }

        let all_added = config.job(&all.name).is_some();
        let all_job = all.clone();
        let loading_cluster = cluster.clone();

        let controls = self.choices(vec![
            self.pill(
                SharedString::from(format!("bk-all-{id}")),
                if all_added {
                    "All databases \u{2713}"
                } else {
                    "All databases"
                },
                all_added,
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.add_backup_jobs(vec![all_job.clone()], cx);
            })),
            self.pill(
                SharedString::from(format!("bk-pick-{id}")),
                "Choose databases",
                false,
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.load_backup_databases(loading_cluster.clone(), cx);
            })),
        ]);

        let mut rows = vec![self.row(&cluster.title(), &cluster.detail(), controls)];

        match self.backup.dbs.get(&id) {
            None => {}
            Some(Dbs::Loading) => rows.push(self.message("Listing databases\u{2026}", false)),
            Some(Dbs::Failed(error)) => rows.push(self.message(error, true)),
            Some(Dbs::Ready(names)) if names.is_empty() => {
                rows.push(self.message("No databases besides the system ones.", false));
            }
            Some(Dbs::Ready(names)) => {
                let jobs: Vec<Job> = names
                    .iter()
                    .map(|name| cluster.job(server_name, target, Some(name.as_str())))
                    .collect();

                for (name, job) in names.iter().zip(&jobs) {
                    let added = config.job(&job.name).is_some();
                    let job = job.clone();

                    rows.push(
                        self.row(
                            &format!("    {name}"),
                            "Its own backup file",
                            self.pill(
                                SharedString::from(format!("bk-db-{}", job.name)),
                                if added { "Added \u{2713}" } else { "Add" },
                                added,
                            )
                            .on_click(cx.listener(
                                move |this, _event, _window, cx| {
                                    this.add_backup_jobs(vec![job.clone()], cx);
                                },
                            )),
                        ),
                    );
                }

                if jobs.iter().any(|job| config.job(&job.name).is_none()) {
                    rows.push(
                        self.row(
                            &format!("    Every database above ({})", jobs.len()),
                            "One job and one file per database",
                            self.pill(
                                SharedString::from(format!("bk-dball-{id}")),
                                "Add all",
                                true,
                            )
                            .on_click(cx.listener(
                                move |this, _event, _window, cx| {
                                    this.add_backup_jobs(jobs.clone(), cx);
                                },
                            )),
                        ),
                    );
                }
            }
        }

        rows
    }

    fn backup_jobs_section(&self, config: &Config, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = self.backup.snapshot.as_ref();
        let mut rows: Vec<AnyElement> = Vec::new();

        if config.jobs.is_empty() {
            rows.push(self.message(
                "No jobs yet. Scan a machine above and add its databases.",
                false,
            ));
        }

        for job in &config.jobs {
            let name = job.name.clone();
            let latest = snapshot.and_then(|snapshot| snapshot.latest.get(&job.name));
            let schedule = job
                .schedule
                .as_ref()
                .map_or_else(|| "not scheduled".to_owned(), |when| format!("cron {when}"));
            let encrypted = if job.is_encrypted() {
                " \u{b7} encrypted"
            } else {
                ""
            };

            let hint = format!(
                "{} \u{b7} {schedule} \u{b7} {}{encrypted} \u{b7} {}",
                source_text(job),
                retention_text(job),
                latest_text(latest)
            );

            let (run, check, open, remove) =
                (name.clone(), name.clone(), name.clone(), name.clone());
            let open_selected = self.backup.selected.as_deref() == Some(job.name.as_str());

            let controls = self.choices(vec![
                self.pill(
                    SharedString::from(format!("bk-run-{name}")),
                    "Back up now",
                    true,
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.run_backup_now(run.clone(), cx);
                })),
                self.pill(
                    SharedString::from(format!("bk-check-{name}")),
                    "Check",
                    false,
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.verify_backup(check.clone(), cx);
                })),
                self.pill(
                    SharedString::from(format!("bk-open-{name}")),
                    "Settings",
                    open_selected,
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.editing = None;
                    this.backup.selected = if this.backup.selected.as_deref() == Some(open.as_str())
                    {
                        None
                    } else {
                        Some(open.clone())
                    };
                    cx.notify();
                })),
                self.pill(
                    SharedString::from(format!("bk-del-{name}")),
                    "Remove",
                    false,
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.remove_backup_job(remove.clone(), cx);
                })),
            ]);

            rows.push(self.row(&job.name, &hint, controls));
        }

        if !config.jobs.is_empty() {
            let folder = config
                .jobs
                .first()
                .map(|job| job.dest_dir())
                .and_then(|dir| dir.parent().map(PathBuf::from));

            let mut controls = vec![
                self.pill(SharedString::from("bk-run-all"), "Back up all now", false)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.run_all_backups(cx);
                    })),
            ];

            if let Some(folder) = folder {
                controls.push(
                    self.pill(SharedString::from("bk-folder"), "Open folder", false)
                        .on_click(cx.listener(move |_this, _event, _window, _cx| {
                            let _ = Command::new("xdg-open").arg(&folder).spawn();
                        })),
                );
            }

            rows.push(self.row(
                "Every job",
                "Runs one after another. Old backups are removed only after a new one succeeded.",
                self.choices(controls),
            ));
        }

        self.section("Backup jobs", rows)
    }

    fn backup_job_section(&self, job: &Job, cx: &mut Context<Self>) -> AnyElement {
        let editing = self.editing.as_ref().map(|edit| edit.target.clone());
        let mut rows: Vec<AnyElement> = Vec::new();

        rows.push(self.row("What it runs", &source_text(job), div()));

        for field in Field::for_job(job) {
            let current = field.get(job);

            let control = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .child(self.value_label(&field.shown(job)))
                .child(
                    self.pill(
                        SharedString::from(format!("bk-field-{field:?}")),
                        "Edit",
                        false,
                    )
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.start_edit(EditTarget::BackupField(field), current.clone(), cx);
                    })),
                );

            rows.push(self.row(field.label(), field.hint(), control));

            if editing == Some(EditTarget::BackupField(field)) {
                rows.extend(self.editor_row(field.hint(), false, cx));
            }
        }

        self.section(&format!("Settings for {}", job.name), rows)
    }

    fn backup_cron_section(&self, config: &Config, cx: &mut Context<Self>) -> AnyElement {
        let state = self
            .backup
            .snapshot
            .as_ref()
            .map_or(CronState::Unknown, |snapshot| snapshot.cron.clone());
        let scheduled = config
            .jobs
            .iter()
            .filter(|job| job.schedule.is_some())
            .count();

        let (headline, hint) = match &state {
            CronState::Unknown => ("Checking cron\u{2026}".to_owned(), String::new()),
            CronState::Idle => (
                "Nothing scheduled".to_owned(),
                "Give a job a schedule in its Settings, for example `daily 02:00`.".to_owned(),
            ),
            CronState::NotInstalled => (
                "Not installed in cron yet".to_owned(),
                format!("{scheduled} job(s) have a schedule. Apply it so cron runs them."),
            ),
            CronState::UpToDate => (
                "Cron is up to date".to_owned(),
                format!(
                    "{scheduled} scheduled job(s). Output goes to {}",
                    aujar_backup::log_path().display()
                ),
            ),
            CronState::OutOfDate => (
                "Cron is out of date".to_owned(),
                "Jobs or schedules changed since cron was set. Apply to update it.".to_owned(),
            ),
            CronState::Error(error) => ("Cron is not available".to_owned(), error.clone()),
        };

        let controls = self.choices(vec![
            self.pill(
                SharedString::from("bk-cron-apply"),
                "Apply",
                matches!(state, CronState::NotInstalled | CronState::OutOfDate),
            )
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.apply_backup_schedule(cx);
            })),
            self.pill(SharedString::from("bk-cron-off"), "Remove from cron", false)
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.remove_backup_schedule(cx);
                })),
        ]);

        self.section(
            "Automatic backups",
            vec![
                self.row(&headline, &hint, controls),
                self.row(
                    "Config file",
                    &aujar_backup::config_path().display().to_string(),
                    div(),
                ),
            ],
        )
    }
}

/// Run one job and describe the result in a line.
fn run_one(job: &Job) -> Result<String, String> {
    let report = aujar_backup::run_job(job).map_err(|error| {
        let text = error.to_string();

        if text.contains("no password supplied") || text.contains("password authentication failed")
        {
            format!(
                "{text}\nThis database wants a password. Open this job's Settings and set \
                 \"Password file\" (a file on this computer, chmod 600, with the password)."
            )
        } else {
            text
        }
    })?;

    let mut line = format!(
        "{}: {} ({}{})",
        report.job,
        report.file.display(),
        human_bytes(report.bytes),
        if report.encrypted { ", encrypted" } else { "" }
    );

    if !report.pruned.is_empty() {
        line.push_str(&format!(". Removed {} old backup(s)", report.pruned.len()));
    }

    for warning in &report.warnings {
        line.push_str(&format!(". Warning: {warning}"));
    }

    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_line_with_everything() {
        let server = parse_server("prod | root@203.0.113.7 | 2222 | ~/.ssh/id_ed25519").unwrap();

        assert_eq!(server.name, "prod");
        assert_eq!(server.ssh, "root@203.0.113.7");
        assert_eq!(server.ssh_port, Some(2222));
        assert_eq!(
            server.ssh_key.as_deref().map(|k| k.display().to_string()),
            Some("~/.ssh/id_ed25519".to_owned())
        );
    }

    #[test]
    fn only_an_address_is_enough() {
        let server = parse_server("deploy@db.example.com").unwrap();

        assert_eq!(server.name, "db-example-com");
        assert_eq!(server.ssh_port, None);

        let server = parse_server("fintech | root@10.0.0.5").unwrap();

        assert_eq!(server.name, "fintech");
    }

    #[test]
    fn bad_server_lines_are_explained() {
        assert!(parse_server("").is_err());
        assert!(parse_server("just words").is_err());
        assert!(parse_server("a b | root@h").is_err());
        assert!(parse_server("x | -oProxyCommand=evil").is_err());
        assert!(parse_server("x | root@h | 99999").is_err());
        assert!(parse_server("x | root@h | 22 | key | extra").is_err());
    }

    #[test]
    fn a_server_line_round_trips() {
        let server = parse_server("prod | root@h | 2222 | /k").unwrap();

        assert_eq!(parse_server(&server_line(&server)).unwrap(), server);

        let plain = parse_server("prod | root@h").unwrap();

        assert_eq!(parse_server(&server_line(&plain)).unwrap(), plain);
    }

    fn pg() -> Job {
        let mut job = Job::new("j", Kind::Postgres);
        job.database = Some("app".into());
        job
    }

    #[test]
    fn fields_set_and_clear() {
        let mut job = pg();

        Field::Port.set(&mut job, "5433").unwrap();
        assert_eq!(job.port, Some(5433));
        Field::Port.set(&mut job, "").unwrap();
        assert_eq!(job.port, None);

        assert!(Field::Port.set(&mut job, "0").is_err());
        assert!(Field::Port.set(&mut job, "abc").is_err());
        assert!(Field::KeepLast.set(&mut job, "0").is_err());

        Field::KeepLast.set(&mut job, "30").unwrap();
        assert_eq!(job.keep_last, Some(30));
        Field::Database.set(&mut job, "").unwrap();
        assert_eq!(job.database, None);
        Field::BinDir
            .set(&mut job, "/usr/lib/postgresql/18/bin")
            .unwrap();
        assert!(job.bin_dir.is_some());
    }

    #[test]
    fn schedules_accept_cron_and_daily_times() {
        let mut job = pg();

        Field::Schedule.set(&mut job, "daily 02:30").unwrap();
        assert_eq!(job.schedule.as_deref(), Some("30 2 * * *"));
        Field::Schedule.set(&mut job, "@daily").unwrap();
        assert!(job.schedule.is_some());
        assert!(
            Field::Schedule
                .set(&mut job, "0 2 * * * ; rm -rf /")
                .is_err()
        );
        Field::Schedule.set(&mut job, "").unwrap();
        assert_eq!(job.schedule, None);
    }

    #[test]
    fn a_job_only_shows_settings_that_apply() {
        let mut container = Job::new("c", Kind::Postgres);
        container.container = Some("db-1".into());

        let fields = Field::for_job(&container);

        assert!(fields.contains(&Field::User));
        assert!(!fields.contains(&Field::Port));
        assert!(!fields.contains(&Field::RunAs));

        let mut remote = pg();
        remote.ssh = Some("root@h".into());

        let fields = Field::for_job(&remote);

        assert!(fields.contains(&Field::RunAs));
        assert!(fields.contains(&Field::PasswordFile));

        let mut files = Job::new("f", Kind::Files);
        files.path = Some("/srv".into());

        assert!(!Field::for_job(&files).contains(&Field::Database));
        assert!(Field::for_job(&files).contains(&Field::Schedule));
    }

    #[test]
    fn rows_describe_the_job() {
        let mut job = pg();
        job.ssh = Some("root@h".into());
        job.port = Some(5433);

        assert_eq!(source_text(&job), "PostgreSQL \"app\" on root@h :5433");
        assert_eq!(retention_text(&job), "keep 14");
        assert_eq!(latest_text(None), "no backup yet");
    }
}
