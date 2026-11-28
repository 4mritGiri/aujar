//! Finding the database clusters on a machine, so nobody has to know which ports they use.
//!
//! One short, fixed shell script is run on the machine (locally, or over SSH with key login
//! only). It prints facts as tab-separated lines: Debian's `pg_lsclusters`, the listening TCP
//! sockets with the program behind each one, where that program lives and which folder it
//! runs in (for PostgreSQL that is the data directory), and database containers from
//! Docker. [`parse`] turns those lines into [`Cluster`]s and never runs anything.
//!
//! Matching listeners by data directory rather than by port matters on real servers: a
//! stopped cluster and a running one can both claim port 5433, and a pooler such as
//! PgBouncer listens on a port that looks like a database but cannot be dumped reliably.
//!
//! Nothing here stores a secret. The script only reads; the only user input that ever reaches
//! a command line is the validated SSH target.

use std::{
    collections::{BTreeSet, HashMap},
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::{
    config::{Job, Kind, Server, read_secret},
    error::BackupError,
    runner::{Plan, failure, find_tool, push_opt, spawn_error, ssh_base_args, wrap},
    util::{is_safe_token, shell_quote},
};

/// How long a scan or a database listing may take before it is abandoned.
const TIMEOUT: Duration = Duration::from_secs(45);

/// The facts script. Fixed text: nothing from outside is ever put into it.
const SCRIPT: &str = r##"LC_ALL=C
export LC_ALL
if command -v pg_lsclusters >/dev/null 2>&1; then
  pg_lsclusters --no-header 2>/dev/null | while IFS= read -r l; do printf 'LSCLUSTER\t%s\n' "$l"; done
fi
if command -v ss >/dev/null 2>&1; then
  ss -ltnp 2>/dev/null | tail -n +2 | while IFS= read -r l; do printf 'SS\t%s\n' "$l"; done
  for p in $(ss -ltnp 2>/dev/null | grep -E '"(postgres|postmaster|mysqld|mariadbd|pgbouncer|pgpool)"' | grep -o 'pid=[0-9]*' | cut -d= -f2 | sort -u); do
    printf 'PROC\t%s\t%s\t%s\t%s\n' "$p" "$(readlink /proc/$p/exe 2>/dev/null)" "$(readlink /proc/$p/cwd 2>/dev/null)" "$(stat -c %U /proc/$p 2>/dev/null)"
  done
fi
if command -v docker >/dev/null 2>&1; then
  docker ps --format '{{.Names}}|{{.Image}}|{{.Ports}}' 2>/dev/null </dev/null | while IFS='|' read -r n i p; do
    case "$i" in
      *postgres*|*postgis*|*timescale*|*mysql*|*mariadb*|*percona*)
        printf 'DOCKER\t%s\t%s\t%s\n' "$n" "$i" "$p"
        u=$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$n" 2>/dev/null </dev/null | sed -n 's/^POSTGRES_USER=//p' | head -n 1)
        printf 'DOCKERUSER\t%s\t%s\n' "$n" "$u"
        ;;
    esac
  done
fi
exit 0
"##;

/// Where to look: this computer, or a server over SSH.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Target {
    pub ssh: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_key: Option<PathBuf>,
}

impl Target {
    pub fn local() -> Self {
        Self::default()
    }

    pub fn is_local(&self) -> bool {
        self.ssh.is_none()
    }

    pub fn describe(&self) -> String {
        self.ssh
            .clone()
            .unwrap_or_else(|| "this computer".to_owned())
    }
}

impl From<&Server> for Target {
    fn from(server: &Server) -> Self {
        Self {
            ssh: Some(server.ssh.clone()),
            ssh_port: server.ssh_port,
            ssh_key: server.ssh_key.clone(),
        }
    }
}

/// One database server found on the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster {
    pub kind: Kind,
    /// Major version (`18`), when it could be told.
    pub version: Option<String>,
    /// The cluster's name (`main`), from `pg_lsclusters` or its data directory.
    pub name: Option<String>,
    /// TCP port; `None` for a container that publishes none.
    pub port: Option<u16>,
    pub online: bool,
    /// Operating-system user that owns the server process.
    pub owner: Option<String>,
    pub data_dir: Option<String>,
    /// Folder with the client tools that match this server.
    pub bin_dir: Option<String>,
    pub container: Option<String>,
    pub image: Option<String>,
    /// Database user to connect as (containers only).
    pub db_user: Option<String>,
    /// A connection pooler such as PgBouncer: it looks like a database but is not one.
    pub pooler: Option<String>,
}

impl Cluster {
    fn empty(kind: Kind) -> Self {
        Self {
            kind,
            version: None,
            name: None,
            port: None,
            online: true,
            owner: None,
            data_dir: None,
            bin_dir: None,
            container: None,
            image: None,
            db_user: None,
            pooler: None,
        }
    }

    /// Whether a backup can be made from it now.
    pub fn backupable(&self) -> bool {
        self.online && self.pooler.is_none()
    }

    pub fn title(&self) -> String {
        if let Some(pooler) = &self.pooler {
            return pooler.clone();
        }
        if let Some(container) = &self.container {
            return format!("{container} (Docker)");
        }
        let product = match self.kind {
            Kind::Mysql => "MySQL / MariaDB".to_owned(),
            _ => match &self.version {
                Some(version) => format!("PostgreSQL {version}"),
                None => "PostgreSQL".to_owned(),
            },
        };
        match &self.name {
            Some(name) => format!("{product} \u{b7} {name}"),
            None => product,
        }
    }

    /// One line for a settings row: port, state and where it lives.
    pub fn detail(&self) -> String {
        let mut parts = Vec::new();
        parts.push(match self.port {
            Some(port) => format!("port {port}"),
            None => "no published port".to_owned(),
        });
        if self.pooler.is_some() {
            parts.push("connection pooler: back up the database behind it".to_owned());
        } else {
            parts.push(if self.online { "online" } else { "stopped" }.to_owned());
        }
        if let Some(image) = &self.image {
            parts.push(image.clone());
        }
        if let Some(owner) = &self.owner {
            parts.push(format!("owner {owner}"));
        }
        if let Some(dir) = &self.data_dir {
            parts.push(dir.clone());
        }
        parts.join(" \u{b7} ")
    }

    /// A stable name for the jobs made from this cluster. Letters, digits, `-` and `_`.
    pub fn job_name(&self, server: &str, database: Option<&str>) -> String {
        let engine = match self.kind {
            Kind::Mysql => "my",
            _ => "pg",
        };
        let mut base = server.to_owned();
        match (&self.container, self.port) {
            (Some(container), _) => base.push_str(&format!("-{container}")),
            (None, Some(port)) => {
                base.push('-');
                base.push_str(engine);
                if let Some(version) = &self.version {
                    base.push_str(version);
                }
                base.push_str(&format!("-{port}"));
            }
            (None, None) => base.push_str(&format!("-{engine}")),
        }
        base.push('-');
        base.push_str(database.unwrap_or("all"));
        slug(&base)
    }

    /// A ready-to-save job for this cluster. `database` of `None` backs up every database in it.
    pub fn job(&self, server: &str, target: &Target, database: Option<&str>) -> Job {
        let mut job = Job::new(self.job_name(server, database), self.kind);
        job.database = database.map(str::to_owned);
        job.ssh = target.ssh.clone();
        job.ssh_port = target.ssh_port;
        job.ssh_key = target.ssh_key.clone();

        if let Some(container) = &self.container {
            job.container = Some(container.clone());
            job.user = Some(match (&self.db_user, self.kind) {
                (Some(user), _) => user.clone(),
                (None, Kind::Mysql) => "root".to_owned(),
                (None, _) => "postgres".to_owned(),
            });
            return job;
        }

        job.port = self.port;
        match self.kind {
            Kind::Postgres => {
                // The cluster's own OS user logs in over the local socket ("peer" login).
                let owner = self.owner.clone().unwrap_or_else(|| "postgres".to_owned());
                job.user = Some(owner.clone());
                job.run_as = Some(owner);
                job.bin_dir = self.bin_dir.as_ref().map(PathBuf::from);
            }
            Kind::Mysql => {
                // TCP, so that `-P` picks the right one when several servers run.
                job.host = Some("127.0.0.1".to_owned());
                job.user = Some("root".to_owned());
            }
            Kind::Sqlite | Kind::Files => {}
        }
        job
    }
}

/// What a scan found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovery {
    pub clusters: Vec<Cluster>,
    /// Things worth telling the person (hidden processes, skipped poolers).
    pub notes: Vec<String>,
}

fn slug(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    out.truncate(64);
    out
}

// ---- Parsing ----

#[derive(Debug)]
struct Listener {
    port: u16,
    name: Option<String>,
    pid: Option<u32>,
}

#[derive(Debug, Default)]
struct Proc {
    exe: String,
    cwd: String,
    owner: String,
}

/// `LISTEN 0 244 127.0.0.1:5432 0.0.0.0:* users:(("postgres",pid=1071,fd=5))`
fn parse_listener(line: &str) -> Option<Listener> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let local = fields.get(3)?;
    let (_, port) = local.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;

    let users = fields.get(5..).map(|rest| rest.join(" "));
    let name = users
        .as_deref()
        .and_then(|u| u.split_once("((\""))
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(name, _)| name.to_owned());
    let pid = users
        .as_deref()
        .and_then(|u| u.split_once("pid="))
        .and_then(|(_, rest)| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        });

    Some(Listener { port, name, pid })
}

/// `18  main  5432 online postgres /var/lib/postgresql/18/main /var/log/...`
fn parse_lscluster(line: &str) -> Option<Cluster> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let [version, name, port, status, owner, data_dir, ..] = fields.as_slice() else {
        return None;
    };
    let mut cluster = Cluster::empty(Kind::Postgres);
    cluster.version = Some((*version).to_owned());
    cluster.name = Some((*name).to_owned());
    cluster.port = port.parse().ok();
    cluster.online = status.starts_with("online");
    cluster.owner = Some((*owner).to_owned());
    cluster.data_dir = Some((*data_dir).to_owned());
    cluster.bin_dir = Some(format!("/usr/lib/postgresql/{version}/bin"));
    Some(cluster)
}

/// `18` from `/usr/lib/postgresql/18/bin/postgres`, `/usr/pgsql-16/bin/postgres` or
/// `/var/lib/postgresql/18/main`.
fn version_from_path(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        let numeric = !part.is_empty() && part.chars().all(|c| c.is_ascii_digit() || c == '.');
        if numeric && index > 0 && parts[index - 1] == "postgresql" {
            return Some((*part).to_owned());
        }
        let prefixed = part
            .strip_prefix("pgsql-")
            .or_else(|| part.strip_prefix("postgresql-"));
        if let Some(rest) = prefixed {
            if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return Some(rest.to_owned());
            }
        }
    }
    None
}

/// `16` from `postgres:16-alpine`.
fn version_from_image(image: &str) -> Option<String> {
    let (_, tag) = image.rsplit_once(':')?;
    if tag.contains('/') {
        // `registry:5000/postgres`: the colon was a port, not a tag.
        return None;
    }
    let digits: String = tag
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!digits.is_empty()).then_some(digits)
}

/// The host port of a container mapping such as
/// `0.0.0.0:5434->5432/tcp, [::]:5434->5432/tcp`. A mapping to the database's own port wins.
fn published_port(ports: &str, inner: u16) -> Option<u16> {
    let mut first = None;
    for mapping in ports.split(',') {
        let Some((host, container)) = mapping.trim().split_once("->") else {
            continue;
        };
        let Some(host_port) = host
            .rsplit_once(':')
            .and_then(|(_, p)| p.parse::<u16>().ok())
        else {
            continue;
        };
        let container_port: Option<u16> = container.split('/').next().and_then(|p| p.parse().ok());
        if container_port == Some(inner) {
            return Some(host_port);
        }
        first.get_or_insert(host_port);
    }
    first
}

enum Service {
    Postgres,
    Mysql,
    Pooler(&'static str),
    Other,
}

fn classify(process: &str) -> Service {
    match process {
        "postgres" | "postmaster" => Service::Postgres,
        "mysqld" | "mariadbd" => Service::Mysql,
        "pgbouncer" => Service::Pooler("PgBouncer"),
        "pgpool" => Service::Pooler("Pgpool-II"),
        _ => Service::Other,
    }
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Turn the script's output into clusters. Pure: easy to test and safe on any input.
pub fn parse(output: &str) -> Discovery {
    let mut clusters: Vec<Cluster> = Vec::new();
    let mut listeners: Vec<Listener> = Vec::new();
    let mut procs: HashMap<u32, Proc> = HashMap::new();
    let mut containers: Vec<(String, String, String)> = Vec::new();
    let mut docker_users: HashMap<String, String> = HashMap::new();

    for line in output.lines() {
        let Some((tag, rest)) = line.split_once('\t') else {
            continue;
        };
        match tag {
            "LSCLUSTER" => clusters.extend(parse_lscluster(rest)),
            "SS" => listeners.extend(parse_listener(rest)),
            "PROC" => {
                let fields: Vec<&str> = rest.split('\t').collect();
                if let [pid, exe, cwd, owner, ..] = fields.as_slice() {
                    if let Ok(pid) = pid.parse() {
                        procs.insert(
                            pid,
                            Proc {
                                exe: (*exe).to_owned(),
                                cwd: (*cwd).to_owned(),
                                owner: (*owner).to_owned(),
                            },
                        );
                    }
                }
            }
            "DOCKER" => {
                let fields: Vec<&str> = rest.splitn(3, '\t').collect();
                if let [name, image, ports] = fields.as_slice() {
                    containers.push(((*name).to_owned(), (*image).to_owned(), (*ports).to_owned()));
                }
            }
            "DOCKERUSER" => {
                if let Some((name, user)) = rest.split_once('\t') {
                    if let Some(user) = non_empty(user) {
                        docker_users.insert(name.to_owned(), user);
                    }
                }
            }
            _ => {}
        }
    }

    let mut notes = Vec::new();
    let mut seen: BTreeSet<(u16, String)> = BTreeSet::new();

    for listener in &listeners {
        let Some(process) = &listener.name else {
            continue;
        };
        // 0.0.0.0 and [::] are the same server.
        if !seen.insert((listener.port, process.clone())) {
            continue;
        }
        let info = listener.pid.and_then(|pid| procs.get(&pid));

        match classify(process) {
            Service::Postgres => {
                let data_dir = info.and_then(|p| non_empty(&p.cwd));
                let host = |c: &Cluster| c.kind == Kind::Postgres && c.container.is_none();

                let index = data_dir
                    .as_ref()
                    .and_then(|dir| {
                        clusters
                            .iter()
                            .position(|c| host(c) && c.data_dir.as_ref() == Some(dir))
                    })
                    .or_else(|| {
                        clusters
                            .iter()
                            .position(|c| host(c) && c.online && c.port == Some(listener.port))
                    });

                let exe_dir = info
                    .and_then(|p| p.exe.rsplit_once('/'))
                    .map(|(dir, _)| dir.to_owned())
                    .filter(|dir| !dir.is_empty());

                match index {
                    Some(i) => {
                        let cluster = &mut clusters[i];
                        cluster.online = true;
                        cluster.port = Some(listener.port);
                        if let Some(dir) = exe_dir {
                            cluster.bin_dir = Some(dir);
                        }
                    }
                    None => {
                        let mut cluster = Cluster::empty(Kind::Postgres);
                        cluster.port = Some(listener.port);
                        cluster.version = info
                            .and_then(|p| version_from_path(&p.exe))
                            .or_else(|| data_dir.as_deref().and_then(version_from_path));
                        cluster.name = data_dir
                            .as_deref()
                            .and_then(|d| d.trim_end_matches('/').rsplit('/').next())
                            .map(str::to_owned);
                        cluster.owner = info.and_then(|p| non_empty(&p.owner));
                        cluster.data_dir = data_dir;
                        cluster.bin_dir = exe_dir;
                        clusters.push(cluster);
                    }
                }
            }
            Service::Mysql => {
                let mut cluster = Cluster::empty(Kind::Mysql);
                cluster.port = Some(listener.port);
                cluster.owner = info.and_then(|p| non_empty(&p.owner));
                cluster.name = Some(process.clone());
                clusters.push(cluster);
            }
            Service::Pooler(label) => {
                let mut cluster = Cluster::empty(Kind::Postgres);
                cluster.port = Some(listener.port);
                cluster.pooler = Some(label.to_owned());
                notes.push(format!(
                    "{label} listens on port {}. It is skipped: back up the PostgreSQL server behind it.",
                    listener.port
                ));
                clusters.push(cluster);
            }
            Service::Other => {}
        }
    }

    for (name, image, ports) in &containers {
        let lower = image.to_lowercase();
        let (kind, inner) = if ["mysql", "mariadb", "percona"]
            .iter()
            .any(|word| lower.contains(word))
        {
            (Kind::Mysql, 3306)
        } else {
            (Kind::Postgres, 5432)
        };
        let mut cluster = Cluster::empty(kind);
        cluster.container = Some(name.clone());
        cluster.image = Some(image.clone());
        cluster.version = version_from_image(image);
        cluster.port = published_port(ports, inner);
        cluster.db_user = docker_users.get(name).cloned();
        clusters.push(cluster);
    }

    if !listeners.is_empty() && listeners.iter().all(|l| l.name.is_none()) {
        notes.push(
            "The system hides which program owns each port. Connect as root to find clusters on \
             custom ports."
                .to_owned(),
        );
    }

    // Running first, then by engine and port.
    clusters.sort_by_key(|c| {
        (
            !c.backupable(),
            c.kind != Kind::Postgres,
            c.port.unwrap_or(u16::MAX),
            c.name.clone(),
        )
    });

    Discovery { clusters, notes }
}

// ---- Running ----

fn read_all(mut pipe: impl Read) -> String {
    let mut kept = Vec::new();
    let mut chunk = [0_u8; 8192];
    while let Ok(n) = pipe.read(&mut chunk) {
        if n == 0 {
            break;
        }
        if kept.len() < 1 << 20 {
            kept.extend_from_slice(&chunk[..n]);
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// Run a plan and return what it printed, giving up after [`TIMEOUT`].
fn capture(plan: &Plan, secret: Option<(&'static str, String)>) -> Result<String, BackupError> {
    let program = find_tool(&plan.program)?;
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

    let mut child = command.spawn().map_err(|e| spawn_error(&plan.program, e))?;
    if let (Some(mut stdin), Some((_, value))) = (child.stdin.take(), &secret) {
        let _ = writeln!(stdin, "{value}");
    }
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let out = thread::spawn(move || read_all(stdout));
    let err = thread::spawn(move || read_all(stderr));

    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BackupError::Config(format!(
                    "{} did not answer within {} seconds",
                    plan.program,
                    TIMEOUT.as_secs()
                )));
            }
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(e) => {
                return Err(BackupError::io(
                    format!("{} did not finish", plan.program),
                    e,
                ));
            }
        }
    };

    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    if !status.success() {
        return Err(failure(&plan.program, status, stderr.trim().to_owned()));
    }
    Ok(stdout)
}

/// Ask a machine which database clusters it runs.
pub fn scan(target: &Target) -> Result<Discovery, BackupError> {
    let plan = match &target.ssh {
        Some(address) => {
            if !is_safe_token(address) {
                return Err(BackupError::Config(
                    "the address must look like user@host".to_owned(),
                ));
            }
            let mut args = ssh_base_args(target.ssh_port, target.ssh_key.as_deref());
            args.push(address.clone());
            args.push(format!("sh -c {}", shell_quote(SCRIPT)));
            Plan {
                program: "ssh".to_owned(),
                args,
                secret_env: None,
                secret_stdin: false,
                extension: "txt",
                gzip: false,
            }
        }
        None => Plan {
            program: "sh".to_owned(),
            args: vec!["-c".to_owned(), SCRIPT.to_owned()],
            secret_env: None,
            secret_stdin: false,
            extension: "txt",
            gzip: false,
        },
    };

    Ok(parse(&capture(&plan, None)?))
}

const PG_LIST_SQL: &str =
    "SELECT datname FROM pg_database WHERE NOT datistemplate AND datallowconn ORDER BY 1";

/// Databases that belong to the MySQL/MariaDB server itself.
const MYSQL_SYSTEM: [&str; 4] = ["information_schema", "performance_schema", "mysql", "sys"];

/// The databases inside the server a job points to (its `database` is ignored). Uses the same
/// connection, user and tools as the backup itself, so a failure here is a failure there.
pub fn list_databases(job: &Job) -> Result<Vec<String>, BackupError> {
    job.validate()?;
    let secret_name = if job.kind == Kind::Mysql {
        "MYSQL_PWD"
    } else {
        "PGPASSWORD"
    };
    let secret_file = job.password_file.clone().map(|file| (secret_name, file));
    let secret = job
        .password_file
        .as_ref()
        .map(|file| read_secret(file).map(|value| (secret_name, value)))
        .transpose()?;

    match job.kind {
        Kind::Postgres => {
            let mut last = None;
            // `postgres` is the usual maintenance database; `template1` always exists.
            for maintenance in ["postgres", "template1"] {
                let mut args: Vec<String> = ["--no-password", "-X", "-A", "-t"]
                    .map(str::to_owned)
                    .to_vec();
                push_opt(&mut args, "-h", job.host.as_ref());
                push_opt(&mut args, "-p", job.port);
                push_opt(&mut args, "-U", job.user.as_ref());
                args.extend([
                    "-d".to_owned(),
                    maintenance.to_owned(),
                    "-c".to_owned(),
                    PG_LIST_SQL.to_owned(),
                ]);
                let plan = wrap(job, "psql", args, secret_file.clone(), "txt", false)?;
                match capture(&plan, secret.clone()) {
                    Ok(text) => return Ok(lines(&text, &[])),
                    Err(error) => last = Some(error),
                }
            }
            Err(last.expect("two attempts were made"))
        }
        Kind::Mysql => {
            let mut args: Vec<String> = ["-N", "-B"].map(str::to_owned).to_vec();
            push_opt(&mut args, "-h", job.host.as_ref());
            push_opt(&mut args, "-P", job.port);
            push_opt(&mut args, "-u", job.user.as_ref());
            args.extend(["-e".to_owned(), "SHOW DATABASES".to_owned()]);
            let plan = wrap(job, "mysql", args, secret_file, "txt", false)?;
            Ok(lines(&capture(&plan, secret)?, &MYSQL_SYSTEM))
        }
        Kind::Sqlite | Kind::Files => Ok(Vec::new()),
    }
}

/// Non-empty trimmed lines, minus `skip`, that are safe to use as a database name.
fn lines(text: &str, skip: &[&str]) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !skip.contains(line) && is_safe_token(line))
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first server from the request: two Debian clusters (16 stopped, 18 online) plus
    /// clusters under /var/lib/postgresql that pg_lsclusters does not know, and PgBouncer.
    fn server_output() -> String {
        [
            "LSCLUSTER\t16  main    5433 down   postgres /var/lib/postgresql/16/main /var/log/postgresql/postgresql-16-main.log",
            "LSCLUSTER\t18  main    5432 online postgres /var/lib/postgresql/18/main /var/log/postgresql/postgresql-18-main.log",
            "SS\tLISTEN 0 244 127.0.0.1:5432 0.0.0.0:* users:((\"postgres\",pid=1071,fd=5))",
            "SS\tLISTEN 0 244 0.0.0.0:5433 0.0.0.0:* users:((\"postgres\",pid=2001,fd=5))",
            "SS\tLISTEN 0 244 [::]:5433 [::]:* users:((\"postgres\",pid=2001,fd=6))",
            "SS\tLISTEN 0 244 127.0.0.1:5434 0.0.0.0:* users:((\"postgres\",pid=2002,fd=5))",
            "SS\tLISTEN 0 4096 0.0.0.0:6432 0.0.0.0:* users:((\"pgbouncer\",pid=3001,fd=7))",
            "SS\tLISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=900,fd=3))",
            "PROC\t1071\t/usr/lib/postgresql/18/bin/postgres\t/var/lib/postgresql/18/main\tpostgres",
            "PROC\t2001\t/usr/lib/postgresql/18/bin/postgres\t/var/lib/postgresql/18/bizpro-construction\tpostgres",
            "PROC\t2002\t/usr/lib/postgresql/16/bin/postgres\t/var/lib/postgresql/bizpro-wellness\tpostgres",
            "PROC\t3001\t/usr/sbin/pgbouncer\t/\tpostgres",
        ]
        .join("\n")
    }

    #[test]
    fn finds_every_cluster_by_what_is_running_not_by_config_files() {
        let found = parse(&server_output());

        let by_port = |port: u16| {
            found
                .clusters
                .iter()
                .filter(|c| c.port == Some(port))
                .collect::<Vec<_>>()
        };

        // Debian's 18/main: merged, not duplicated, and its client tools come from the process.
        let main = by_port(5432);
        assert_eq!(main.len(), 1);
        assert!(main[0].online);
        assert_eq!(main[0].name.as_deref(), Some("main"));
        assert_eq!(
            main[0].bin_dir.as_deref(),
            Some("/usr/lib/postgresql/18/bin")
        );

        // Port 5433: the stopped 16/main and the running bizpro-construction are two clusters.
        let on_5433 = by_port(5433);
        assert_eq!(on_5433.len(), 2);
        let running = on_5433.iter().find(|c| c.online).unwrap();
        assert_eq!(running.name.as_deref(), Some("bizpro-construction"));
        assert_eq!(running.version.as_deref(), Some("18"));
        let stopped = on_5433.iter().find(|c| !c.online).unwrap();
        assert_eq!(stopped.version.as_deref(), Some("16"));
        assert!(!stopped.backupable());

        // A cluster outside pg_lsclusters, run by a different PostgreSQL version.
        let wellness = by_port(5434);
        assert_eq!(wellness[0].version.as_deref(), Some("16"));
        assert_eq!(
            wellness[0].bin_dir.as_deref(),
            Some("/usr/lib/postgresql/16/bin")
        );

        // sshd is not a database; PgBouncer is listed but cannot be backed up.
        assert!(by_port(22).is_empty());
        let pooler = by_port(6432);
        assert_eq!(pooler[0].pooler.as_deref(), Some("PgBouncer"));
        assert!(!pooler[0].backupable());
        assert!(found.notes.iter().any(|n| n.contains("PgBouncer")));

        // Running, backup-able clusters come first.
        assert!(found.clusters[0].backupable());
        assert!(!found.clusters.last().unwrap().backupable());
    }

    #[test]
    fn dual_stack_listeners_are_one_cluster() {
        let found = parse(&server_output());
        let running_5433 = found
            .clusters
            .iter()
            .filter(|c| c.port == Some(5433) && c.online)
            .count();
        assert_eq!(running_5433, 1);
    }

    #[test]
    fn clusters_beside_the_debian_ones_get_their_own_job_settings() {
        let found = parse(&server_output());
        let cluster = found
            .clusters
            .iter()
            .find(|c| c.name.as_deref() == Some("bizpro-construction"))
            .unwrap();
        let target = Target {
            ssh: Some("root@203.0.113.7".into()),
            ssh_port: None,
            ssh_key: None,
        };

        let job = cluster.job("prod", &target, Some("app"));
        job.validate().unwrap();
        assert_eq!(job.name, "prod-pg18-5433-app");
        assert_eq!(job.port, Some(5433));
        assert_eq!(job.run_as.as_deref(), Some("postgres"));
        assert_eq!(job.user.as_deref(), Some("postgres"));
        assert_eq!(
            job.bin_dir.as_deref().map(|p| p.display().to_string()),
            Some("/usr/lib/postgresql/18/bin".to_owned())
        );
        assert_eq!(job.database.as_deref(), Some("app"));

        let all = cluster.job("prod", &target, None);
        all.validate().unwrap();
        assert_eq!(all.name, "prod-pg18-5433-all");
        assert_eq!(all.database, None);
    }

    #[test]
    fn mysql_and_mariadb_are_found() {
        let found = parse(
            "SS\tLISTEN 0 80 127.0.0.1:3306 0.0.0.0:* users:((\"mariadbd\",pid=77,fd=20))\n\
             PROC\t77\t/usr/sbin/mariadbd\t/var/lib/mysql\tmysql\n",
        );
        assert_eq!(found.clusters.len(), 1);
        let cluster = &found.clusters[0];
        assert_eq!(cluster.kind, Kind::Mysql);
        let job = cluster.job("db", &Target::local(), None);
        job.validate().unwrap();
        assert_eq!(job.host.as_deref(), Some("127.0.0.1"));
        assert_eq!(job.port, Some(3306));
    }

    #[test]
    fn docker_databases_are_found_with_their_published_port() {
        let found = parse(
            "DOCKER\tshop-db-1\tpostgres:16-alpine\t0.0.0.0:5438->5432/tcp, [::]:5438->5432/tcp\n\
             DOCKERUSER\tshop-db-1\tshop\n\
             DOCKER\tcache\tpostgres:15\t\n\
             DOCKERUSER\tcache\t\n",
        );
        assert_eq!(found.clusters.len(), 2);

        let shop = found
            .clusters
            .iter()
            .find(|c| c.container.as_deref() == Some("shop-db-1"))
            .unwrap();
        assert_eq!(shop.port, Some(5438));
        assert_eq!(shop.version.as_deref(), Some("16"));
        assert_eq!(shop.title(), "shop-db-1 (Docker)");

        let job = shop.job("srv", &Target::local(), Some("app"));
        job.validate().unwrap();
        assert_eq!(job.container.as_deref(), Some("shop-db-1"));
        assert_eq!(job.user.as_deref(), Some("shop"));
        assert_eq!(job.port, None);

        let cache = found
            .clusters
            .iter()
            .find(|c| c.container.as_deref() == Some("cache"))
            .unwrap();
        assert_eq!(cache.port, None);
        assert_eq!(
            cache.job("srv", &Target::local(), None).user.as_deref(),
            Some("postgres")
        );
    }

    #[test]
    fn without_root_the_missing_process_names_are_explained() {
        let found = parse(
            "LSCLUSTER\t18  main 5432 online postgres /var/lib/postgresql/18/main /var/log/x.log\n\
             SS\tLISTEN 0 244 127.0.0.1:5432 0.0.0.0:*\n",
        );
        assert_eq!(found.clusters.len(), 1);
        assert!(found.clusters[0].online);
        assert!(found.notes.iter().any(|n| n.contains("root")));
    }

    #[test]
    fn garbage_is_ignored() {
        let found = parse("hello\nSS\tnot a socket line\nPROC\tx\nDOCKER\tonly-one\n\n");
        assert!(found.clusters.is_empty());
    }

    #[test]
    fn versions_are_read_from_paths_and_images() {
        assert_eq!(
            version_from_path("/usr/lib/postgresql/18/bin/postgres").as_deref(),
            Some("18")
        );
        assert_eq!(
            version_from_path("/usr/pgsql-16/bin/postgres").as_deref(),
            Some("16")
        );
        assert_eq!(
            version_from_path("/var/lib/postgresql/17/main").as_deref(),
            Some("17")
        );
        assert_eq!(version_from_path("/opt/pg/bin/postgres"), None);
        assert_eq!(
            version_from_image("postgres:16-alpine").as_deref(),
            Some("16")
        );
        assert_eq!(version_from_image("postgres"), None);
    }

    #[test]
    fn published_port_prefers_the_databases_own_port() {
        assert_eq!(
            published_port("0.0.0.0:8080->80/tcp, 0.0.0.0:5438->5432/tcp", 5432),
            Some(5438)
        );
        assert_eq!(published_port("0.0.0.0:8080->80/tcp", 5432), Some(8080));
        assert_eq!(published_port("5432/tcp", 5432), None);
        assert_eq!(published_port("", 5432), None);
    }

    #[test]
    fn job_names_are_always_valid() {
        let mut cluster = Cluster::empty(Kind::Postgres);
        cluster.port = Some(5432);
        let name = cluster.job_name("my server!", Some("db.name"));
        let job = Job::new(&name, Kind::Postgres);
        job.validate().unwrap();
    }

    #[test]
    fn database_names_from_psql_are_filtered() {
        let text = "app\n\n  billing \n-oops\nwith space\n";
        assert_eq!(lines(text, &[]), vec!["app", "billing"]);
        assert_eq!(
            lines("mysql\nshop\nsys\n", &MYSQL_SYSTEM),
            vec!["shop".to_owned()]
        );
    }

    #[test]
    fn the_script_is_plain_and_free_of_shell_traps() {
        // It is sent through `sh -c '...'`, possibly to a fish or csh login shell.
        assert!(!SCRIPT.contains("\\\\"));
        assert!(!SCRIPT.contains("\\'"));
        assert!(SCRIPT.contains("pg_lsclusters"));
        assert!(SCRIPT.ends_with("exit 0\n"));
    }
}
