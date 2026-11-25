# Backup

`aujar backup` copies databases and folders to **this computer** (from this machine or from any
server over SSH), keeps the last N copies, can encrypt them, and schedules itself with **cron**.

It does not need the Aujar daemon or a desktop session, so a cron job runs the same way whether
or not you are logged in graphically.

| Kind | Tool used | Result |
|---|---|---|
| `postgres` + `database` | `pg_dump --format=custom` | `NAME_<UTC stamp>.dump` (compressed; restore with `pg_restore`) |
| `postgres`, no database | `pg_dumpall` | `NAME_<stamp>.sql.gz` (all databases and roles; needs a privileged user) |
| `mysql` (MariaDB too) | `mysqldump --single-transaction` | `NAME_<stamp>.sql.gz` |
| `sqlite` | `sqlite3 FILE .dump` | `NAME_<stamp>.sql.gz` |
| `files` | `tar -czf` | `NAME_<stamp>.tar.gz` |

Encrypted backups get a `.gpg` suffix. Every backup has a `.sha256` file next to it.

## Quick start: daily PostgreSQL backup at 02:00

```bash
# 1. A read-only role on the database (once, as a Postgres superuser)
#    CREATE ROLE backup_user LOGIN PASSWORD '...';
#    GRANT pg_read_all_data TO backup_user;          -- PostgreSQL 14+

# 2. Secrets live in files only you can read, never on a command line
umask 077
printf '%s\n' 'the-db-password'          > ~/.pg_backup_pass
head -c 32 /dev/urandom | base64         > ~/.backup_gpg_pass   # keep a copy in a password manager!

# 3. Define the job, run it once, then let cron do it every day
aujar backup add prod-db --kind postgres --host localhost --user backup_user --database mydb \
    --password-file ~/.pg_backup_pass --gpg-passphrase-file ~/.backup_gpg_pass \
    --keep-last 14 --daily-at 02:00
aujar backup run prod-db            # a real run, to check it works
aujar backup schedule install       # writes the cron line
aujar backup backups prod-db        # what exists
aujar backup verify ~/aujar-backups/prod-db/prod-db_*.dump.gpg
```

The `PostgreSQL client tools` (`pg_dump`) must be installed on this computer, and their major
version must be at least the server's.

## A server somewhere else (over SSH)

```bash
aujar backup add shop --kind postgres --ssh deploy@203.0.113.7 --ssh-key ~/.ssh/id_backup \
    --user backup_user --database shop --daily-at 03:15
```

The dump runs **on the server** and streams back over SSH, so the database never has to be
reachable from the internet (keep `listen_addresses = 'localhost'`). Requirements:

* key login (`BatchMode` is always on, so a password prompt fails instead of hanging cron);
* the database password in the server user's `~/.pgpass` (`chmod 600`) or peer authentication,
  because `--password-file` is for direct connections only;
* the dump tool installed on the server.

Folders work the same way: `--kind files --ssh deploy@server --path /var/www/site`.

## The Backup page (Settings → Data → Backup)

The page does what the commands above do, but finds the databases for you, so a server with
many clusters on many ports needs no per-port set-up.

0. **Setup (once).** *Choose folder…* opens a folder dialog (needs `zenity` or `kdialog`; or use
   *Type path*) for where backups are saved on this computer; each job gets `<folder>/<job name>`.
   *Move existing jobs here* re-points jobs you already have. Also set a default schedule
   (`daily 02:00`) and how many backups to keep: every job added afterwards starts with them.
   Stored as `[defaults]` in `backup.toml`.
1. **Machine.** Pick *This computer*, or *+ Add server* and type `name | user@host | port | key`
   (only `user@host` is required; login must work with an SSH key). Servers are kept in
   `backup.toml`; passwords are never asked for or stored.
2. **Scan.** One fixed, read-only script runs on that machine and reports what is *running*:
   Debian's `pg_lsclusters`, every listening TCP port with the program behind it (with the
   program's folder and data directory), and database containers from Docker. Nothing is read
   from `.env` files, so a stale port in a config cannot mislead it.
   * Several PostgreSQL versions are fine: each cluster gets the client tools of its own
     version (`bin_dir`, e.g. `/usr/lib/postgresql/18/bin`), because `pg_dump` 16 cannot dump a
     version 18 server.
   * A stopped cluster and a running one on the same port are two entries; the stopped one is
     shown but cannot be backed up.
   * PgBouncer and Pgpool-II are listed and skipped: dump the PostgreSQL server behind them
     (for example the direct port).
   * Run the scan as **root** (or a user who can see all processes) to find clusters on
     custom ports; otherwise the page says what it could not see.
3. **Add jobs.** *Add every cluster* makes one job per running cluster in a single click. Or: *All databases* makes one job for the whole cluster (`pg_dumpall`, roles
   included). *Choose databases* lists them and makes one job (one file, restorable with
   `pg_restore`) per database, or all at once.
4. **Jobs.** *Back up now*, *Check* (checksum and a read test of the newest file), *Settings*
   (every value above, editable: database, user, host, port, OS user, client tools folder,
   destination, schedule such as `daily 02:30`, retention, encryption) and *Remove*.
5. **Automatic backups.** *Apply* writes the cron lines; the page says when cron is out of date.

How the dump authenticates: a PostgreSQL cluster is dumped as its own OS user over the local
socket (`run_as = "postgres"`, `runuser` when the SSH user is root, `sudo -n` otherwise), so no
password is involved. A database in Docker is dumped with `docker exec` inside its container.
A cluster whose `pg_hba.conf` demands a password even on the local socket ("no password
supplied") needs a **Password file**: a file on this computer (`chmod 600`) containing the
password. Over SSH it is sent on the connection's standard input and read into `PGPASSWORD`
by a tiny wrapper on the server, so it is never stored there or visible in `ps`. Alternatively
use `~/.pgpass` / `~/.my.cnf` on the server.

New job settings (also available as `aujar backup add --run-as/--bin-dir/--container`):

```toml
[defaults]                   # what new jobs start with (all optional)
backup_dir = "/srv/backups"
schedule = "0 2 * * *"
keep_last = 14

[[servers]]                  # a machine the page can scan; jobs keep their own copy of the SSH details
name = "prod"
ssh = "root@203.0.113.7"

[[jobs]]
name = "prod-pg18-5433-app"
kind = "postgres"
ssh = "root@203.0.113.7"
port = 5433
user = "postgres"
run_as = "postgres"                          # OS user for peer login
bin_dir = "/usr/lib/postgresql/18/bin"       # matching pg_dump
database = "app"

[[jobs]]
name = "prod-shop-db-1-all"
kind = "postgres"
ssh = "root@203.0.113.7"
container = "shop-db-1"                      # dump runs inside this Docker container
user = "postgres"
```

## Encryption

* `--gpg-passphrase-file FILE`: symmetric AES-256. Simple; the passphrase must be kept safely
  (without it the backups are unreadable).
* `--gpg-recipient KEY`: public-key encryption. This computer can write backups but cannot read
  them, which is the safer choice when the backup machine could be compromised.

Restore: `gpg -d NAME.dump.gpg > NAME.dump`, then `pg_restore -d newdb NAME.dump`
(`gunzip -c NAME.sql.gz | psql newdb` for `.sql.gz`). **Test a restore now and then.**

## Retention

`--keep-last N` keeps the newest N backups (default 14 when nothing is set) and `--keep-days D`
removes older ones. Both can be combined. Old backups are removed only **after** a new backup
succeeded, the newest is never removed, and only files named exactly like Aujar's are touched.

## Scheduling

`aujar backup schedule set NAME --daily-at 02:30` (local time), or a cron expression such as
`"0 */6 * * *"` or `@daily`. `schedule install` puts one line per scheduled job between two
marker comments in your crontab; the rest of your crontab is never changed, and the previous
one is saved to `<state dir>/crontab.before-aujar`. `schedule show` previews, `schedule status`
shows what is installed, `schedule remove` takes it out again. Output goes to
`<state dir>/backup.log` (`~/.local/state/aujar/backup.log` on Linux). Cron starts with a very
small `PATH`; Aujar searches the usual places (including `/usr/lib/postgresql/*/bin`) itself.

Put `MAILTO=you@example.com` at the top of your crontab if you want an e-mail when a job fails
(a failed run exits non-zero and writes `FAIL ...` to the log). On Windows, cron does not exist:
create a Task Scheduler task that runs `aujar backup run --all`.

If the computer is off at the scheduled time, cron does not catch up. For laptops, schedule a
time when it is usually on, or run `aujar backup run --all` from a systemd user timer with
`Persistent=true`.

## Security notes

* Jobs are stored in `backup.toml` (mode 600) in the Aujar config folder; it holds no secrets.
* Secret files must not be readable by group/other: Aujar refuses to use them otherwise.
* Passwords reach the tool through an environment variable of that one process, never in `argv`
  (where other users could see them in `ps`).
* Tools are started without a shell. Host, user, database and job names are validated (letters,
  digits, `. _ @ : -`, not starting with `-`), schedules only accept numbers and `* , - /`, and
  everything sent to a server's shell is quoted.
* Backups are created with mode 600 in a mode-700 folder, under a temporary name, and renamed
  only when complete: a failed or interrupted run leaves no partial file.
* **Backups on the same disk are not enough.** Ransomware or an intruder can delete them too.
  Keep a second copy off the machine (external drive, or `rclone` to storage with versioning or
  object lock), and keep the encryption key separate from the backups.

## Configuration reference

`~/.config/aujar/backup.toml` (override with `AUJAR_BACKUP_CONFIG`):

```toml
[[jobs]]
name = "prod-db"
kind = "postgres"            # postgres | mysql | sqlite | files
host = "localhost"
user = "backup_user"
database = "mydb"            # omit for every database
password_file = "~/.pg_backup_pass"
dest = "~/aujar-backups/prod-db"
keep_last = 14
keep_days = 60
gpg_passphrase_file = "~/.backup_gpg_pass"
schedule = "0 2 * * *"
```

SSH jobs add `ssh`, `ssh_port`, `ssh_key`; `sqlite` and `files` use `path` (absolute).
