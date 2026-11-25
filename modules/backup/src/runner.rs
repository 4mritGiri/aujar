//! Running one backup job: dump -> (gzip) -> (GnuPG) -> a private file in the destination
//! folder, written under a temporary name and renamed only when everything succeeded.

use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    thread::{self, JoinHandle},
};

use flate2::{Compression, write::GzEncoder};
use sha2::{Digest, Sha256};

use crate::{
    config::{Job, Kind, expand, read_secret},
    error::BackupError,
    util::{now_secs, parse_stamp, shell_quote, stamp, which},
};

/// What will be executed, without running it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub program: String,
    pub args: Vec<String>,
    /// Environment variable that receives the contents of a secret file at run time.
    pub secret_env: Option<(&'static str, PathBuf)>,
    /// The secret is sent to the command's standard input (one line) instead of its
    /// environment: used over SSH, where an environment variable would not arrive and a
    /// command line would show the password to every user of the server.
    pub secret_stdin: bool,
    /// Extension of the finished file (`dump`, `sql.gz`, `tar.gz`), before `.gpg`.
    pub extension: &'static str,
    /// Compress the tool's output ourselves (tools that already compress set this to false).
    pub gzip: bool,
}

impl Plan {
    /// The command as a person would type it (no secrets are ever part of it).
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .map(shell_quote)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub(crate) fn push_opt(args: &mut Vec<String>, flag: &str, value: Option<impl ToString>) {
    if let Some(value) = value {
        args.push(flag.to_owned());
        args.push(value.to_string());
    }
}

/// The tool and arguments for a job; for SSH jobs the same command wrapped in `ssh`.
pub fn plan(job: &Job) -> Result<Plan, BackupError> {
    job.validate()?;

    let (tool, args, extension, gzip): (&str, Vec<String>, &'static str, bool) = match job.kind
    {
        Kind::Postgres => {
            let mut args = vec!["--no-password".to_owned()];
            push_opt(&mut args, "-h", job.host.as_ref());
            push_opt(&mut args, "-p", job.port);
            push_opt(&mut args, "-U", job.user.as_ref());
            match &job.database {
                // Custom format: compressed, and restorable table by table with pg_restore.
                Some(database) => {
                    args.extend([
                        "--format=custom".to_owned(),
                        "--dbname".to_owned(),
                        database.clone(),
                    ]);
                    ("pg_dump", args, "dump", false)
                }
                None => ("pg_dumpall", args, "sql.gz", true),
            }
        }
        Kind::Mysql => {
            let mut args = vec![
                "--single-transaction".to_owned(),
                "--routines".to_owned(),
                "--triggers".to_owned(),
                "--events".to_owned(),
            ];
            push_opt(&mut args, "-h", job.host.as_ref());
            push_opt(&mut args, "-P", job.port);
            push_opt(&mut args, "-u", job.user.as_ref());
            match &job.database {
                Some(database) => {
                    args.extend(["--databases".to_owned(), database.clone()]);
                }
                None => args.push("--all-databases".to_owned()),
            }
            ("mysqldump", args, "sql.gz", true)
        }
        Kind::Sqlite => {
            let path = target_path(job)?;
            ("sqlite3", vec![path, ".dump".to_owned()], "sql.gz", true)
        }
        Kind::Files => {
            let full = PathBuf::from(target_path(job)?);
            let (Some(parent), Some(name)) = (full.parent(), full.file_name()) else {
                return Err(BackupError::invalid(
                    &job.name,
                    "`path` must name a file or folder",
                ));
            };
            let args = vec![
                "-czf".to_owned(),
                "-".to_owned(),
                "-C".to_owned(),
                parent.display().to_string(),
                "--".to_owned(),
                name.to_string_lossy().into_owned(),
            ];
            ("tar", args, "tar.gz", false)
        }
    };

    let secret_env = match (job.kind, &job.password_file) {
        (Kind::Postgres, Some(file)) => Some(("PGPASSWORD", file.clone())),
        (Kind::Mysql, Some(file)) => Some(("MYSQL_PWD", file.clone())),
        _ => None,
    };

    wrap(job, tool, args, secret_env, extension, gzip)
}

/// Shell snippet that starts a command as another user: `runuser` when we are root,
/// `sudo -n` (never asks for a password) otherwise. `$0` is the user, `"$@"` the command.
const AS_USER: &str = r#"if [ "$(id -u)" = 0 ] && command -v runuser >/dev/null 2>&1; then exec runuser -u "$0" -- "$@"; else exec sudo -n -u "$0" -- "$@"; fi"#;

/// Inside a MySQL/MariaDB container the root password is in the container's own environment.
const IN_CONTAINER_MYSQL: &str = r#"MYSQL_PWD="${MYSQL_ROOT_PASSWORD:-$MARIADB_ROOT_PASSWORD}" exec "$0" "$@""#;

/// The options every SSH run uses: key login only, no prompts, no forwarding, keep-alives.
pub(crate) fn ssh_base_args(port: Option<u16>, key: Option<&Path>) -> Vec<String> {
    let mut args: Vec<String> = [
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=20",
        "-o",
        "ServerAliveInterval=30",
        "-o",
        "ServerAliveCountMax=6",
        "-o",
        "ClearAllForwardings=yes",
        "-T",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    push_opt(&mut args, "-p", port);
    if let Some(key) = key {
        args.extend([
            "-i".to_owned(),
            expand(key).display().to_string(),
            "-o".to_owned(),
            "IdentitiesOnly=yes".to_owned(),
        ]);
    }
    args
}

/// Put a tool and its arguments where the job says they run: inside a container, as another
/// user, from a specific client folder, locally or on a server over SSH.
pub(crate) fn wrap(
    job: &Job,
    tool: &str,
    args: Vec<String>,
    secret_env: Option<(&'static str, PathBuf)>,
    extension: &'static str,
    gzip: bool,
) -> Result<Plan, BackupError> {
    let mut argv: Vec<String> = Vec::new();

    if let Some(container) = &job.container {
        argv.extend(["docker", "exec", "-i"].map(str::to_owned));
        argv.push(container.clone());
        if job.kind == Kind::Mysql {
            argv.extend(["sh".to_owned(), "-c".to_owned(), IN_CONTAINER_MYSQL.to_owned()]);
        }
        argv.push(tool.to_owned());
    } else if let Some(dir) = &job.bin_dir {
        let dir = if job.ssh.is_some() {
            dir.clone()
        } else {
            expand(dir)
        };
        argv.push(dir.join(tool).display().to_string());
    } else {
        argv.push(tool.to_owned());
    }
    argv.extend(args);

    // Over SSH the password travels on stdin; the server side reads it into the variable the
    // tool expects, then starts the tool. It is never part of any command line.
    let secret_stdin = job.ssh.is_some() && secret_env.is_some();

    if let Some((name, _)) = secret_env.as_ref().filter(|_| secret_stdin) {
        let script = format!(r#"IFS= read -r {name}; export {name}; exec "$0" "$@""#);
        let mut wrapped = vec!["sh".to_owned(), "-c".to_owned(), script];

        wrapped.append(&mut argv);
        argv = wrapped;
    }

    if let Some(user) = &job.run_as {
        let mut wrapped = vec!["sh".to_owned(), "-c".to_owned(), AS_USER.to_owned(), user.clone()];
        wrapped.append(&mut argv);
        argv = wrapped;
    }

    let Some(target) = &job.ssh else {
        let program = argv.remove(0);
        return Ok(Plan {
            program,
            args: argv,
            secret_env,
            secret_stdin: false,
            extension,
            gzip,
        });
    };

    // Remote: ssh runs the command through the server's shell, so every word is quoted.
    let remote = argv
        .iter()
        .map(|word| shell_quote(word))
        .collect::<Vec<_>>()
        .join(" ");

    let mut ssh_args = ssh_base_args(job.ssh_port, job.ssh_key.as_deref());
    ssh_args.push(target.clone());
    ssh_args.push(remote);

    Ok(Plan {
        program: "ssh".to_owned(),
        args: ssh_args,
        secret_env,
        secret_stdin,
        extension,
        gzip,
    })
}

fn target_path(job: &Job) -> Result<String, BackupError> {
    let path = job
        .path
        .as_ref()
        .ok_or_else(|| BackupError::invalid(&job.name, "this kind needs `path`"))?;
    // Remote paths are used as given; local `~/` is expanded here.
    let path = if job.ssh.is_some() {
        path.clone()
    } else {
        expand(path)
    };
    Ok(path.display().to_string())
}

fn install_hint(tool: &str) -> String {
    match tool {
        "pg_dump" | "pg_dumpall" => "Install the PostgreSQL client tools (Debian/Ubuntu: `sudo apt install postgresql-client`, \
             macOS: `brew install libpq`). Their major version must be at least that of the server."
            .into(),
        "mysqldump" => "Install the MySQL/MariaDB client (Debian/Ubuntu: `sudo apt install mariadb-client`).".into(),
        "sqlite3" => "Install sqlite3 (Debian/Ubuntu: `sudo apt install sqlite3`).".into(),
        "ssh" => "Install an OpenSSH client.".into(),
        "gpg" => "Install GnuPG (Debian/Ubuntu: `sudo apt install gnupg`).".into(),
        "tar" => "Install tar.".into(),
        "docker" => "Install the Docker command-line client.".into(),
        "sudo" | "sh" | "runuser" => "A standard Unix shell and sudo are needed to run the dump as another user.".into(),
        _ => String::new(),
    }
}

pub(crate) fn find_tool(name: &str) -> Result<PathBuf, BackupError> {
    which(name).ok_or_else(|| BackupError::ToolMissing {
        tool: name.to_owned(),
        hint: install_hint(name),
    })
}

/// Drain a pipe on its own thread so a chatty tool can never block on a full pipe. Only the
/// start is kept: that is where tools print what went wrong.
fn drain(mut pipe: impl Read + Send + 'static) -> JoinHandle<String> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut chunk = [0_u8; 4096];
        while let Ok(n) = pipe.read(&mut chunk) {
            if n == 0 {
                break;
            }
            if kept.len() < 8192 {
                kept.extend_from_slice(&chunk[..n.min(8192 - kept.len())]);
            }
        }
        String::from_utf8_lossy(&kept).trim().to_owned()
    })
}

pub(crate) fn failure(tool: &str, status: std::process::ExitStatus, stderr: String) -> BackupError {
    BackupError::ToolFailed {
        tool: tool.to_owned(),
        status: status.to_string(),
        stderr: if stderr.is_empty() {
            "(no message)".into()
        } else {
            stderr
        },
    }
}

/// The output chain: optional gzip in front of either a file or GnuPG's input.
enum Out {
    Plain(Box<dyn Write + Send>),
    Gzip(GzEncoder<Box<dyn Write + Send>>),
}

impl Write for Out {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(w) => w.write(buf),
            Self::Gzip(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(w) => w.flush(),
            Self::Gzip(w) => w.flush(),
        }
    }
}

impl Out {
    /// Write the gzip trailer and close the sink (dropping it ends GnuPG's input).
    fn finish(self) -> io::Result<()> {
        let mut sink = match self {
            Self::Plain(w) => w,
            Self::Gzip(w) => w.finish()?,
        };
        sink.flush()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub job: String,
    pub file: PathBuf,
    pub bytes: u64,
    pub sha256: String,
    pub encrypted: bool,
    /// Old backups removed by the retention rule.
    pub pruned: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

#[cfg(unix)]
fn private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    // A folder that already existed keeps its mode, except that other users are shut out.
    let mut perms = fs::metadata(path)?.permissions();
    if perms.mode() & 0o077 != 0 {
        perms.set_mode(perms.mode() & !0o077);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

fn create_private(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    }
    #[cfg(not(unix))]
    {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
    }
}

pub(crate) fn spawn_error(tool: &str, error: io::Error) -> BackupError {
    if error.kind() == io::ErrorKind::NotFound {
        BackupError::ToolMissing {
            tool: tool.to_owned(),
            hint: install_hint(tool),
        }
    } else {
        BackupError::io(format!("cannot start {tool}"), error)
    }
}

fn start_gpg(job: &Job, output: &Path) -> Result<Child, BackupError> {
    let gpg = find_tool("gpg")?;
    let mut command = Command::new(gpg);
    command
        .args(["--batch", "--yes", "--quiet", "--no-tty", "--output"])
        .arg(output);

    if let Some(file) = &job.gpg_passphrase_file {
        // Read (and permission-checked) here, but handed over as a file, never on a command line.
        read_secret(file)?;
        command
            .args([
                "--pinentry-mode",
                "loopback",
                "--symmetric",
                "--cipher-algo",
                "AES256",
                "--passphrase-file",
            ])
            .arg(expand(file));
    } else if let Some(recipient) = &job.gpg_recipient {
        command.args([
            "--trust-model",
            "always",
            "--encrypt",
            "--recipient",
            recipient,
        ]);
    }

    command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| spawn_error("gpg", e))
}

/// Run one job now. Nothing is ever kept from a failed run, and old backups are only removed
/// after a new one succeeded.
pub fn run_job(job: &Job) -> Result<Report, BackupError> {
    run_job_at(job, now_secs())
}

pub fn run_job_at(job: &Job, now: u64) -> Result<Report, BackupError> {
    let plan = plan(job)?;
    let program = find_tool(&plan.program)?;

    let secret = plan
        .secret_env
        .as_ref()
        .map(|(name, file)| read_secret(file).map(|value| (*name, value)))
        .transpose()?;

    let dest = job.dest_dir();
    private_dir(&dest)
        .map_err(|e| BackupError::io(format!("cannot create {}", dest.display()), e))?;

    let encrypted = job.is_encrypted();
    let mut file_name = format!("{}_{}.{}", job.name, stamp(now), plan.extension);
    if encrypted {
        file_name.push_str(".gpg");
    }
    let final_path = dest.join(&file_name);
    let partial = dest.join(format!(".{file_name}.partial"));
    if final_path.exists() {
        return Err(BackupError::Config(format!(
            "{} already exists (two runs in the same second?)",
            final_path.display()
        )));
    }

    let result = dump_to(job, &plan, &program, secret, &partial, encrypted);
    if let Err(error) = result {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }

    let finish = (|| -> Result<(u64, String), BackupError> {
        let bytes = fs::metadata(&partial)
            .map_err(|e| BackupError::io("cannot read the new backup", e))?
            .len();
        if bytes == 0 {
            return Err(BackupError::Empty);
        }
        File::open(&partial)
            .and_then(|f| f.sync_all())
            .map_err(|e| BackupError::io("cannot flush the new backup to disk", e))?;
        let sha256 = sha256_of(&partial)?;
        fs::rename(&partial, &final_path).map_err(|e| {
            BackupError::io(
                format!("cannot move the backup to {}", final_path.display()),
                e,
            )
        })?;
        Ok((bytes, sha256))
    })();

    let (bytes, sha256) = match finish {
        Ok(done) => done,
        Err(error) => {
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
    };

    let mut warnings = Vec::new();
    if let Err(error) = crate::config::write_private(
        &checksum_path(&final_path),
        &format!("{sha256}  {file_name}\n"),
    ) {
        warnings.push(format!("could not write the checksum file: {error}"));
    }

    let (pruned, prune_warnings) = prune(job, now);
    warnings.extend(prune_warnings);

    Ok(Report {
        job: job.name.clone(),
        file: final_path,
        bytes,
        sha256,
        encrypted,
        pruned,
        warnings,
    })
}

fn dump_to(
    job: &Job,
    plan: &Plan,
    program: &Path,
    secret: Option<(&'static str, String)>,
    partial: &Path,
    encrypted: bool,
) -> Result<(), BackupError> {
    let mut command = Command::new(program);
    command
        .args(&plan.args)
        .stdin(if plan.secret_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let (Some((name, value)), false) = (&secret, plan.secret_stdin) {
        command.env(name, value);
    }

    // Start the encryptor first so a missing gpg or bad passphrase file fails before any dump.
    let mut gpg = if encrypted {
        Some(start_gpg(job, partial)?)
    } else {
        None
    };

    let sink: Box<dyn Write + Send> = match gpg.as_mut() {
        Some(child) => {
            let stdin: ChildStdin = child
                .stdin
                .take()
                .ok_or_else(|| BackupError::Config("cannot open gpg's input".into()))?;
            Box::new(stdin)
        }
        None => Box::new(
            create_private(partial)
                .map_err(|e| BackupError::io(format!("cannot create {}", partial.display()), e))?,
        ),
    };
    let mut out = if plan.gzip {
        Out::Gzip(GzEncoder::new(sink, Compression::default()))
    } else {
        Out::Plain(sink)
    };
    let gpg_stderr = gpg
        .as_mut()
        .and_then(|child| child.stderr.take())
        .map(drain);

    let mut dump = command.spawn().map_err(|e| spawn_error(&plan.program, e))?;
    if let (Some(mut stdin), Some((_, value))) = (dump.stdin.take(), &secret) {
        // One short line; the pipe buffer takes it without waiting for the reader.
        let _ = writeln!(stdin, "{value}");
    }
    let mut stdout = dump.stdout.take().expect("stdout is piped");
    let dump_stderr = drain(dump.stderr.take().expect("stderr is piped"));

    let copied = io::copy(&mut stdout, &mut out);
    if copied.is_err() {
        let _ = dump.kill();
    }
    drop(stdout);
    let finished = out.finish();
    let dump_status = dump
        .wait()
        .map_err(|e| BackupError::io(format!("{} did not finish", plan.program), e))?;
    let dump_message = dump_stderr.join().unwrap_or_default();

    let gpg_result = match gpg {
        Some(mut child) => {
            let status = child
                .wait()
                .map_err(|e| BackupError::io("gpg did not finish", e))?;
            let message = gpg_stderr
                .map(|h| h.join().unwrap_or_default())
                .unwrap_or_default();
            (!status.success()).then(|| failure("gpg", status, message))
        }
        None => None,
    };

    // Report the root cause: the dump tool first, then encryption, then plain I/O.
    if !dump_status.success() {
        return Err(failure(&plan.program, dump_status, dump_message));
    }
    if let Some(error) = gpg_result {
        return Err(error);
    }
    copied.map_err(|e| BackupError::io("cannot write the backup", e))?;
    finished.map_err(|e| BackupError::io("cannot finish the backup file", e))?;
    Ok(())
}

pub fn checksum_path(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".sha256");
    PathBuf::from(name)
}

fn sha256_of(path: &Path) -> Result<String, BackupError> {
    let mut file = File::open(path)
        .map_err(|e| BackupError::io(format!("cannot read {}", path.display()), e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 16];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| BackupError::io(format!("cannot read {}", path.display()), e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupFile {
    pub path: PathBuf,
    pub bytes: u64,
    /// Seconds since the Unix epoch, from the time in the file name.
    pub taken_at: u64,
}

/// This job's backups in its folder, newest first. Only files named exactly like Aujar names
/// them (`<job>_<UTC stamp>.<ext>`) are considered, so nothing else in the folder is ever
/// listed, let alone deleted.
pub fn list_backups(job: &Job) -> Vec<BackupFile> {
    let Ok(entries) = fs::read_dir(job.dest_dir()) else {
        return Vec::new();
    };
    let prefix = format!("{}_", job.name);

    let mut files: Vec<BackupFile> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let rest = name.strip_prefix(&prefix)?;
            let (stamp_text, extension) = (rest.get(..16)?, rest.get(16..)?);
            let taken_at = parse_stamp(stamp_text)?;
            if !extension.starts_with('.')
                || extension.ends_with(".sha256")
                || extension.ends_with(".partial")
            {
                return None;
            }
            let meta = entry.metadata().ok()?;
            meta.is_file().then(|| BackupFile {
                path: entry.path(),
                bytes: meta.len(),
                taken_at,
            })
        })
        .collect();

    files.sort_by(|a, b| {
        b.taken_at
            .cmp(&a.taken_at)
            .then_with(|| b.path.cmp(&a.path))
    });
    files
}

/// Which backups the retention rule would remove, given `files` newest first. The newest one
/// is never removed.
pub fn expired(
    files: &[BackupFile],
    keep_last: Option<usize>,
    keep_days: Option<u32>,
    now: u64,
) -> Vec<&BackupFile> {
    files
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(rank, file)| {
            let too_many = keep_last.is_some_and(|keep| *rank >= keep);
            let too_old = keep_days
                .is_some_and(|days| now.saturating_sub(file.taken_at) > u64::from(days) * 86_400);
            too_many || too_old
        })
        .map(|(_, file)| file)
        .collect()
}

fn prune(job: &Job, now: u64) -> (Vec<PathBuf>, Vec<String>) {
    let (keep_last, keep_days) = job.retention();
    let files = list_backups(job);
    let mut removed = Vec::new();
    let mut warnings = Vec::new();

    for file in expired(&files, keep_last, keep_days, now) {
        match fs::remove_file(&file.path) {
            Ok(()) => {
                let _ = fs::remove_file(checksum_path(&file.path));
                removed.push(file.path.clone());
            }
            Err(error) => {
                warnings.push(format!("could not remove {}: {error}", file.path.display()))
            }
        }
    }
    (removed, warnings)
}

/// Check a finished backup: the checksum file (if present) and, for PostgreSQL custom dumps,
/// that `pg_restore` can read the table of contents. Returns what was checked.
pub fn verify_file(path: &Path) -> Result<Vec<String>, BackupError> {
    let mut checked = Vec::new();
    let meta = fs::metadata(path)
        .map_err(|e| BackupError::io(format!("cannot read {}", path.display()), e))?;
    if meta.len() == 0 {
        return Err(BackupError::Empty);
    }

    let sums = checksum_path(path);
    match fs::read_to_string(&sums) {
        Ok(text) => {
            let expected = text.split_whitespace().next().unwrap_or_default();
            if !expected.eq_ignore_ascii_case(&sha256_of(path)?) {
                return Err(BackupError::ChecksumMismatch(path.to_owned()));
            }
            checked.push("SHA-256 matches the checksum file".to_owned());
        }
        Err(_) => checked.push("no checksum file next to it (skipped)".to_owned()),
    }

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name.ends_with(".gpg") {
        checked.push("encrypted: decrypt it before restoring (`gpg -d FILE > out`)".to_owned());
    } else if name.ends_with(".dump") {
        if let Some(pg_restore) = which("pg_restore") {
            let output = Command::new(pg_restore)
                .arg("--list")
                .arg(path)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()
                .map_err(|e| BackupError::io("cannot run pg_restore", e))?;
            if !output.status.success() {
                return Err(failure(
                    "pg_restore --list",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                ));
            }
            checked.push("pg_restore can read the dump".to_owned());
        } else {
            checked.push("pg_restore not installed (content check skipped)".to_owned());
        }
    } else if name.ends_with(".gz") {
        let mut decoder = flate2::read::GzDecoder::new(
            File::open(path)
                .map_err(|e| BackupError::io(format!("cannot read {}", path.display()), e))?,
        );
        io::copy(&mut decoder, &mut io::sink())
            .map_err(|e| BackupError::io("the gzip data is damaged", e))?;
        checked.push("gzip stream is intact".to_owned());
    }
    Ok(checked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(taken_at: u64) -> BackupFile {
        BackupFile {
            path: PathBuf::from(format!("/x/{taken_at}")),
            bytes: 1,
            taken_at,
        }
    }

    fn pg() -> Job {
        let mut job = Job::new("prod", Kind::Postgres);
        job.database = Some("app".into());
        job.host = Some("db.internal".into());
        job.port = Some(5433);
        job.user = Some("backup_user".into());
        job
    }

    #[test]
    fn postgres_plan_uses_custom_format_and_never_prompts() {
        let plan = plan(&pg()).unwrap();
        assert_eq!(plan.program, "pg_dump");
        assert_eq!(plan.extension, "dump");
        assert!(!plan.gzip);
        assert_eq!(
            plan.display(),
            "pg_dump --no-password -h db.internal -p 5433 -U backup_user --format=custom --dbname app"
        );
    }

    #[test]
    fn all_databases_uses_pg_dumpall_with_gzip() {
        let mut job = pg();
        job.database = None;
        let plan = plan(&job).unwrap();
        assert_eq!(plan.program, "pg_dumpall");
        assert_eq!(plan.extension, "sql.gz");
        assert!(plan.gzip);
    }

    #[test]
    fn password_goes_through_the_environment_only() {
        let mut job = pg();
        job.password_file = Some("/home/u/.pg_pass".into());
        let plan = plan(&job).unwrap();
        assert_eq!(
            plan.secret_env,
            Some(("PGPASSWORD", PathBuf::from("/home/u/.pg_pass")))
        );
        assert!(!plan.display().contains("pg_pass"));
    }

    #[test]
    fn over_ssh_the_password_travels_on_stdin_not_in_the_command() {
        let mut job = pg();
        job.ssh = Some("root@203.0.113.7".into());
        job.host = None;
        job.run_as = Some("postgres".into());
        job.password_file = Some("/home/u/.pg_pass".into());
        let plan = plan(&job).unwrap();
        assert!(plan.secret_stdin);
        assert_eq!(plan.program, "ssh");
        let remote = plan.args.last().unwrap();
        assert!(remote.contains("read -r PGPASSWORD"), "{remote}");
        assert!(!remote.contains("pg_pass"), "{remote}");

        job.ssh = None;
        assert!(!super::plan(&job).unwrap().secret_stdin);
    }

    #[test]
    fn mysql_plan() {
        let mut job = Job::new("shop", Kind::Mysql);
        job.user = Some("root".into());
        let plan = plan(&job).unwrap();
        assert_eq!(plan.program, "mysqldump");
        assert!(plan.args.contains(&"--all-databases".to_owned()));
        assert!(plan.args.contains(&"--single-transaction".to_owned()));
    }

    #[test]
    fn ssh_plan_quotes_the_remote_command_and_disables_prompts() {
        let mut job = pg();
        job.host = None;
        job.port = None;
        job.ssh = Some("deploy@203.0.113.7".into());
        job.ssh_port = Some(2222);
        job.ssh_key = Some("/home/u/.ssh/id_backup".into());
        let plan = plan(&job).unwrap();

        assert_eq!(plan.program, "ssh");
        let joined = plan.args.join(" ");
        assert!(joined.contains("BatchMode=yes"));
        assert!(joined.contains("-p 2222"));
        assert!(joined.contains("-i /home/u/.ssh/id_backup"));
        assert_eq!(plan.args[plan.args.len() - 2], "deploy@203.0.113.7");
        assert_eq!(
            plan.args.last().unwrap(),
            "pg_dump --no-password -U backup_user --format=custom --dbname app"
        );
    }

    #[test]
    fn ssh_plan_protects_odd_remote_paths() {
        let mut job = Job::new("site", Kind::Files);
        job.ssh = Some("deploy@server".into());
        job.path = Some("/var/www/my site; rm -rf ~".into());
        let plan = plan(&job).unwrap();
        let remote = plan.args.last().unwrap();
        assert!(remote.contains("'my site; rm -rf ~'"), "{remote}");
    }

    #[test]
    fn bin_dir_selects_the_matching_client_version() {
        let mut job = pg();
        job.bin_dir = Some("/usr/lib/postgresql/18/bin".into());
        let plan = plan(&job).unwrap();
        assert_eq!(plan.program, "/usr/lib/postgresql/18/bin/pg_dump");
    }

    #[test]
    fn run_as_wraps_the_tool_for_peer_login() {
        let mut job = pg();
        job.host = None;
        job.run_as = Some("postgres".into());
        job.ssh = Some("root@203.0.113.7".into());
        job.bin_dir = Some("/usr/lib/postgresql/18/bin".into());
        let plan = plan(&job).unwrap();
        assert_eq!(plan.program, "ssh");
        let remote = plan.args.last().unwrap();
        assert!(remote.starts_with("sh -c '"), "{remote}");
        assert!(remote.contains("runuser -u"), "{remote}");
        assert!(remote.contains("postgres /usr/lib/postgresql/18/bin/pg_dump --no-password"));

        job.ssh = None;
        let local = super::plan(&job).unwrap();
        assert_eq!(local.program, "sh");
        assert_eq!(local.args[0], "-c");
        assert_eq!(local.args[2], "postgres");
        assert_eq!(local.args[3], "/usr/lib/postgresql/18/bin/pg_dump");
    }

    #[test]
    fn container_jobs_use_docker_exec_without_host_or_port() {
        let mut job = Job::new("app", Kind::Postgres);
        job.container = Some("app-db-1".into());
        job.user = Some("postgres".into());
        job.database = Some("app".into());
        let plan = plan(&job).unwrap();
        assert_eq!(plan.program, "docker");
        assert_eq!(
            plan.display(),
            "docker exec -i app-db-1 pg_dump --no-password -U postgres --format=custom --dbname app"
        );

        let mut my = Job::new("shop", Kind::Mysql);
        my.container = Some("shop-db".into());
        my.user = Some("root".into());
        let plan = super::plan(&my).unwrap();
        assert!(plan.display().contains("MYSQL_ROOT_PASSWORD"));
        assert!(plan.display().contains("mysqldump"));
    }

    #[test]
    fn files_plan_archives_relative_to_the_parent() {
        let mut job = Job::new("docs", Kind::Files);
        job.path = Some("/srv/docs".into());
        let plan = plan(&job).unwrap();
        assert_eq!(plan.display(), "tar -czf - -C /srv -- docs");
        assert_eq!(plan.extension, "tar.gz");
    }

    #[test]
    fn invalid_jobs_have_no_plan() {
        let mut job = pg();
        job.host = Some("-oProxyCommand=x".into());
        assert!(plan(&job).is_err());
    }

    #[test]
    fn retention_keeps_the_newest_n() {
        let files: Vec<_> = (0..5).rev().map(|day| file(day * 86_400)).collect();
        let gone = expired(&files, Some(2), None, 10 * 86_400);
        assert_eq!(gone.len(), 3);
        assert!(gone.iter().all(|f| f.taken_at < 3 * 86_400));
    }

    #[test]
    fn retention_by_age_never_deletes_the_newest() {
        let now = 100 * 86_400;
        let files = vec![file(now - 40 * 86_400), file(now - 50 * 86_400)];
        let gone = expired(&files, None, Some(30), now);
        assert_eq!(gone.len(), 1, "the newest backup stays even if it is old");
        assert_eq!(gone[0].taken_at, now - 50 * 86_400);
    }

    #[test]
    fn retention_combines_both_rules() {
        let now = 100 * 86_400;
        let files: Vec<_> = (0..6).map(|i| file(now - i * 86_400)).collect();
        // Ages are 0..=5 days. keep_last=4 drops ranks 4 and 5; keep_days=3 drops ages > 3
        // (ranks 4 and 5 too), so a backup exactly 3 days old stays.
        let gone = expired(&files, Some(4), Some(3), now);
        assert_eq!(gone.len(), 2);
        // keep_days=2 additionally drops rank 3 (age 3 > 2).
        let gone = expired(&files, Some(4), Some(2), now);
        assert_eq!(gone.len(), 3);
    }
}
