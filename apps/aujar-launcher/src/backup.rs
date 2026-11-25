//! `aujar backup ...`: back up databases and folders (this machine or any server over SSH)
//! to this computer, and schedule it with cron.
//!
//! Like `aujar image`, this works without the daemon: a cron job must not depend on a
//! desktop session. All logic lives in `aujar-backup`; this file only turns arguments into
//! calls and prints results.

use std::path::PathBuf;

use anyhow::{Result, bail};
use aujar_backup::{Config, Job, Kind, cron};
use clap::{Args, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum KindArg {
    Postgres,
    Mysql,
    Sqlite,
    Files,
}

impl From<KindArg> for Kind {
    fn from(kind: KindArg) -> Self {
        match kind {
            KindArg::Postgres => Self::Postgres,
            KindArg::Mysql => Self::Mysql,
            KindArg::Sqlite => Self::Sqlite,
            KindArg::Files => Self::Files,
        }
    }
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// Job name (letters, digits, `-`, `_`).
    name: String,

    /// What to back up.
    #[arg(long, value_enum)]
    kind: KindArg,

    /// Database host as seen from where the dump runs (default: local socket/localhost).
    #[arg(long)]
    host: Option<String>,

    #[arg(long)]
    port: Option<u16>,

    /// Database user (use a read-only backup role).
    #[arg(long)]
    user: Option<String>,

    /// Database name. Leave out to dump every database.
    #[arg(long)]
    database: Option<String>,

    /// File containing the database password (chmod 600). Never put the password in the command.
    #[arg(long)]
    password_file: Option<PathBuf>,

    /// SQLite file, or the folder/file to archive for `--kind files`.
    #[arg(long)]
    path: Option<PathBuf>,

    /// Run the dump on a server over SSH: `user@server` (key login; no password prompts).
    #[arg(long)]
    ssh: Option<String>,

    #[arg(long)]
    ssh_port: Option<u16>,

    #[arg(long)]
    ssh_key: Option<PathBuf>,

    /// Run the dump as this OS user (PostgreSQL "peer" login), e.g. `postgres`.
    #[arg(long)]
    run_as: Option<String>,

    /// Folder with the matching client tools, e.g. /usr/lib/postgresql/18/bin.
    #[arg(long)]
    bin_dir: Option<PathBuf>,

    /// Docker container that runs the database; the dump runs inside it.
    #[arg(long)]
    container: Option<String>,

    /// Where backups are stored on this computer (default: ~/aujar-backups/<name>).
    #[arg(long)]
    dest: Option<PathBuf>,

    /// Keep the newest N backups (default 14 when no retention is given).
    #[arg(long)]
    keep_last: Option<usize>,

    /// Also delete backups older than N days (the newest is always kept).
    #[arg(long)]
    keep_days: Option<u32>,

    /// Encrypt with GnuPG using the passphrase in this file (chmod 600).
    #[arg(long, conflicts_with = "gpg_recipient")]
    gpg_passphrase_file: Option<PathBuf>,

    /// Encrypt to this GnuPG public key (id, fingerprint or e-mail).
    #[arg(long)]
    gpg_recipient: Option<String>,

    /// Cron schedule, e.g. "0 2 * * *" or "@daily".
    #[arg(long, conflicts_with = "daily_at")]
    schedule: Option<String>,

    /// Run every day at this time (HH:MM, 24-hour, this computer's local time).
    #[arg(long)]
    daily_at: Option<String>,

    /// Overwrite an existing job with the same name.
    #[arg(long)]
    replace: bool,
}

#[derive(Debug, Subcommand)]
pub enum ScheduleAction {
    /// Set or clear when one job runs (then run `schedule install`).
    Set {
        name: String,

        /// Cron expression, e.g. "30 3 * * *" or "@daily".
        #[arg(conflicts_with_all = ["daily_at", "off"])]
        schedule: Option<String>,

        /// Every day at HH:MM.
        #[arg(long, conflicts_with = "off")]
        daily_at: Option<String>,

        /// Remove the schedule from this job.
        #[arg(long)]
        off: bool,
    },

    /// Write all scheduled jobs into your crontab (only Aujar's own marked block is touched).
    Install,

    /// Remove Aujar's block from your crontab again.
    Remove,

    /// Print the crontab lines `install` would write, without changing anything.
    Show,

    /// Show what is installed in your crontab right now.
    Status,
}

#[derive(Debug, Subcommand)]
pub enum BackupAction {
    /// Add (or with --replace, change) a backup job.
    Add(Box<AddArgs>),

    /// Delete a job from the config (existing backup files are left alone).
    Remove { name: String },

    /// List the configured jobs.
    List,

    /// Run a job now (this is also what cron runs).
    Run {
        /// Job name.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        name: Option<String>,

        /// Run every job.
        #[arg(long)]
        all: bool,

        /// Show what would run, without running it.
        #[arg(long)]
        dry_run: bool,
    },

    /// List the backups a job has made.
    Backups { name: String },

    /// Check a backup file: checksum and, where possible, that it can be read.
    Verify { file: PathBuf },

    /// Schedule jobs with cron.
    Schedule {
        #[command(subcommand)]
        action: ScheduleAction,
    },
}

pub fn run(action: BackupAction) -> Result<()> {
    match action {
        BackupAction::Add(args) => add(*args),
        BackupAction::Remove { name } => {
            let mut config = Config::load()?;
            if !config.remove(&name) {
                bail!("no job named `{name}` (see `aujar backup list`)");
            }
            config.save()?;
            println!("Removed job `{name}`. Its backup files were not touched.");
            println!("If it was scheduled, refresh cron with: aujar backup schedule install");
            Ok(())
        }
        BackupAction::List => list(),
        BackupAction::Run { name, all, dry_run } => run_jobs(name, all, dry_run),
        BackupAction::Backups { name } => backups(&name),
        BackupAction::Verify { file } => {
            for line in aujar_backup::verify_file(&file)? {
                println!("ok: {line}");
            }
            Ok(())
        }
        BackupAction::Schedule { action } => schedule(action),
    }
}

fn add(args: AddArgs) -> Result<()> {
    let schedule = match (&args.schedule, &args.daily_at) {
        (Some(expression), _) => {
            Some(cron::normalize_schedule(expression).map_err(anyhow::Error::msg)?)
        }
        (None, Some(time)) => Some(cron::daily_at(time).map_err(anyhow::Error::msg)?),
        (None, None) => None,
    };

    let mut job = Job::new(&args.name, args.kind.into());
    job.host = args.host;
    job.port = args.port;
    job.user = args.user;
    job.database = args.database;
    job.password_file = args.password_file;
    job.path = args.path;
    job.ssh = args.ssh;
    job.ssh_port = args.ssh_port;
    job.ssh_key = args.ssh_key;
    job.run_as = args.run_as;
    job.bin_dir = args.bin_dir;
    job.container = args.container;
    job.dest = args.dest;
    job.keep_last = args.keep_last;
    job.keep_days = args.keep_days;
    job.gpg_passphrase_file = args.gpg_passphrase_file;
    job.gpg_recipient = args.gpg_recipient;
    job.schedule = schedule;

    let mut config = Config::load()?;
    config.upsert(job.clone(), args.replace)?;
    config.save()?;

    println!(
        "Saved job `{}` in {}",
        job.name,
        aujar_backup::config_path().display()
    );
    println!("Backups go to: {}", job.dest_dir().display());
    println!("Try it now:     aujar backup run {}", job.name);
    if job.schedule.is_some() {
        println!("Then schedule:  aujar backup schedule install");
    } else {
        println!(
            "To automate:    aujar backup schedule set {} --daily-at 02:00",
            job.name
        );
    }
    Ok(())
}

fn list() -> Result<()> {
    let config = Config::load()?;
    if config.jobs.is_empty() {
        println!(
            "No backup jobs yet ({}).",
            aujar_backup::config_path().display()
        );
        println!(
            "Example: aujar backup add prod-db --kind postgres --database mydb --user backup_user \\\n  \
             --password-file ~/.pg_backup_pass --daily-at 02:00"
        );
        return Ok(());
    }

    for job in &config.jobs {
        let source = match (&job.ssh, &job.path, &job.database) {
            (Some(ssh), Some(path), _) => format!("{ssh}:{}", path.display()),
            (Some(ssh), None, db) => {
                format!("{ssh} ({})", db.as_deref().unwrap_or("all databases"))
            }
            (None, Some(path), _) => path.display().to_string(),
            (None, None, db) => format!(
                "{}{}",
                job.host.as_deref().unwrap_or("local"),
                db.as_ref().map(|d| format!("/{d}")).unwrap_or_default()
            ),
        };
        let (keep_last, keep_days) = job.retention();
        let mut keep = Vec::new();
        keep.extend(keep_last.map(|n| format!("last {n}")));
        keep.extend(keep_days.map(|d| format!("{d} days")));

        println!("{}  [{}]  {source}", job.name, job.kind.as_str());
        println!("    to:       {}", job.dest_dir().display());
        println!(
            "    schedule: {}   encrypted: {}   keep: {}",
            job.schedule.as_deref().unwrap_or("(manual)"),
            if job.is_encrypted() { "yes" } else { "no" },
            keep.join(" + ")
        );
    }
    Ok(())
}

fn run_jobs(name: Option<String>, all: bool, dry_run: bool) -> Result<()> {
    let config = Config::load()?;
    let jobs: Vec<&Job> =
        match (&name, all) {
            (Some(name), false) => vec![config.job(name).ok_or_else(|| {
                anyhow::anyhow!("no job named `{name}` (see `aujar backup list`)")
            })?],
            _ => config.jobs.iter().collect(),
        };
    if jobs.is_empty() {
        bail!("no backup jobs are configured (see `aujar backup add --help`)");
    }

    let mut failed = Vec::new();
    for job in jobs {
        let stamp = aujar_backup::iso(aujar_backup::now_secs());

        if dry_run {
            let plan = aujar_backup::plan(job)?;
            println!("[{}] would run: {}", job.name, plan.display());
            println!("    into:      {}", job.dest_dir().display());
            println!(
                "    encrypted: {}",
                if job.is_encrypted() { "yes" } else { "no" }
            );
            continue;
        }

        match aujar_backup::run_job(job) {
            Ok(report) => {
                println!(
                    "[{stamp}] OK   {} -> {} ({}{})",
                    report.job,
                    report.file.display(),
                    aujar_backup::human_bytes(report.bytes),
                    if report.encrypted { ", encrypted" } else { "" }
                );
                for removed in &report.pruned {
                    println!("[{stamp}]      removed old backup {}", removed.display());
                }
                for warning in &report.warnings {
                    eprintln!("[{stamp}] WARN {}: {warning}", report.job);
                }
            }
            Err(error) => {
                eprintln!("[{stamp}] FAIL {}: {error}", job.name);
                failed.push(job.name.clone());
            }
        }
    }

    if failed.is_empty() {
        Ok(())
    } else {
        bail!(
            "{} backup job(s) failed: {}",
            failed.len(),
            failed.join(", ")
        )
    }
}

fn backups(name: &str) -> Result<()> {
    let config = Config::load()?;
    let job = config
        .job(name)
        .ok_or_else(|| anyhow::anyhow!("no job named `{name}` (see `aujar backup list`)"))?;

    let files = aujar_backup::list_backups(job);
    if files.is_empty() {
        println!("No backups yet in {}", job.dest_dir().display());
        return Ok(());
    }
    for file in files {
        println!(
            "{}  {:>10}  {}",
            aujar_backup::iso(file.taken_at),
            aujar_backup::human_bytes(file.bytes),
            file.path.display()
        );
    }
    Ok(())
}

fn schedule(action: ScheduleAction) -> Result<()> {
    let saved_copy = aujar_config::platform::state_dir().join("crontab.before-aujar");

    match action {
        ScheduleAction::Set {
            name,
            schedule,
            daily_at,
            off,
        } => {
            let mut config = Config::load()?;
            let value = match (schedule, daily_at, off) {
                (_, _, true) => None,
                (Some(expression), _, _) => {
                    Some(cron::normalize_schedule(&expression).map_err(anyhow::Error::msg)?)
                }
                (None, Some(time), _) => Some(cron::daily_at(&time).map_err(anyhow::Error::msg)?),
                (None, None, false) => bail!("give a schedule, --daily-at HH:MM, or --off"),
            };
            let Some(job) = config.jobs.iter_mut().find(|job| job.name == name) else {
                bail!("no job named `{name}` (see `aujar backup list`)");
            };
            job.schedule = value;
            match &job.schedule {
                Some(value) => println!("`{name}` will run on schedule `{value}`."),
                None => println!("`{name}` is no longer scheduled."),
            }
            config.save()?;
            println!("Apply it to cron with: aujar backup schedule install");
            Ok(())
        }

        ScheduleAction::Install => {
            let config = Config::load()?;
            let exe = std::env::current_exe()?;
            let log = aujar_backup::log_path();
            let installed = cron::install(&config.jobs, &exe, &log, &saved_copy)?;

            if installed.scheduled_jobs == 0 {
                println!("No job has a schedule, so nothing is in your crontab.");
                println!("Set one with: aujar backup schedule set NAME --daily-at 02:00");
            } else {
                println!(
                    "Installed {} scheduled job(s) in your crontab.",
                    installed.scheduled_jobs
                );
                println!("Log file: {}", log.display());
                println!(
                    "Your previous crontab was saved to {}",
                    saved_copy.display()
                );
            }
            if exe.components().any(|c| c.as_os_str() == "target") {
                eprintln!(
                    "note: cron will run {} (a build folder). Install aujar for real \
                     (docs/PACKAGING.md) and run `schedule install` again so the path stays valid.",
                    exe.display()
                );
            }
            Ok(())
        }

        ScheduleAction::Remove => {
            if cron::remove(&saved_copy)? {
                println!("Removed Aujar's backup schedule from your crontab.");
            } else {
                println!("Aujar had nothing in your crontab.");
            }
            Ok(())
        }

        ScheduleAction::Show => {
            let config = Config::load()?;
            let exe = std::env::current_exe()?;
            let block = cron::render_block(&config.jobs, &exe, &aujar_backup::log_path());
            if block.is_empty() {
                println!("No job has a schedule.");
            } else {
                print!("{block}");
            }
            Ok(())
        }

        ScheduleAction::Status => {
            match cron::installed_block()? {
                Some(block) => print!("{block}"),
                None => println!("Aujar has no backup schedule in your crontab."),
            }
            Ok(())
        }
    }
}
