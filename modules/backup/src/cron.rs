//! Daily (or any) scheduling through the user's crontab.
//!
//! Aujar only ever touches the lines between its own two marker comments; everything else in
//! the crontab is preserved byte for byte, and the old crontab is saved before each change.

use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use crate::{
    config::{Job, write_private},
    error::BackupError,
    util::shell_quote,
};

pub const BEGIN: &str =
    "# BEGIN AUJAR-BACKUP (managed by `aujar backup schedule`; do not edit between the markers)";
pub const END: &str = "# END AUJAR-BACKUP";

const MACROS: [&str; 7] = [
    "@hourly",
    "@daily",
    "@midnight",
    "@weekly",
    "@monthly",
    "@yearly",
    "@annually",
];

fn number(text: &str) -> Option<u32> {
    if text.is_empty() || text.len() > 3 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn valid_field(field: &str, min: u32, max: u32) -> bool {
    field.split(',').all(|part| {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, Some(step)),
            None => (part, None),
        };
        if step.is_some_and(|s| !number(s).is_some_and(|n| n >= 1 && n <= max)) {
            return false;
        }
        if range == "*" {
            return true;
        }
        match range.split_once('-') {
            None => step.is_none() && number(range).is_some_and(|n| (min..=max).contains(&n)),
            Some((a, b)) => match (number(a), number(b)) {
                (Some(a), Some(b)) => {
                    (min..=max).contains(&a) && (min..=max).contains(&b) && a <= b
                }
                _ => false,
            },
        }
    })
}

/// Check a schedule and return it with single spaces. Only numbers, `*`, `,`, `-`, `/` and a
/// few `@` macros are accepted, so nothing else can reach the crontab line.
pub fn normalize_schedule(expression: &str) -> Result<String, String> {
    let fields: Vec<&str> = expression.split_whitespace().collect();

    if let [one] = fields.as_slice() {
        return if MACROS.contains(one) {
            Ok((*one).to_owned())
        } else {
            Err(format!(
                "`{one}` is not a schedule (use five cron fields like `0 2 * * *`, or one of {})",
                MACROS.join(", ")
            ))
        };
    }

    let limits = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
    if fields.len() != 5 {
        return Err(format!(
            "`{expression}` needs five fields: minute hour day-of-month month day-of-week (e.g. `0 2 * * *`)"
        ));
    }
    for (index, (field, (min, max))) in fields.iter().zip(limits).enumerate() {
        if !valid_field(field, min, max) {
            let names = ["minute", "hour", "day-of-month", "month", "day-of-week"];
            return Err(format!(
                "invalid {} field `{field}` (allowed {min}-{max}, `*`, lists `1,3`, ranges `1-5`, steps `*/2`)",
                names[index]
            ));
        }
    }
    Ok(fields.join(" "))
}

/// `02:30` -> `30 2 * * *`.
pub fn daily_at(time: &str) -> Result<String, String> {
    let bad = || format!("`{time}` is not a time of day (use HH:MM, e.g. 02:30)");
    let (hour, minute) = time.split_once(':').ok_or_else(bad)?;
    let (hour, minute) = (
        number(hour).ok_or_else(bad)?,
        number(minute).ok_or_else(bad)?,
    );
    if hour > 23 || minute > 59 {
        return Err(bad());
    }
    Ok(format!("{minute} {hour} * * *"))
}

/// Characters cron treats specially in the command part of a line.
fn cron_escape(command: &str) -> String {
    command.replace('%', "\\%")
}

/// The managed block for every job that has a schedule (empty when none has).
pub fn render_block(jobs: &[Job], exe: &Path, log: &Path) -> String {
    let lines: Vec<String> = jobs
        .iter()
        .filter_map(|job| {
            let schedule = normalize_schedule(job.schedule.as_deref()?).ok()?;
            let command = format!(
                "{} backup run {} >> {} 2>&1",
                shell_quote(&exe.display().to_string()),
                shell_quote(&job.name),
                shell_quote(&log.display().to_string()),
            );
            Some(format!("{schedule} {}", cron_escape(&command)))
        })
        .collect();

    if lines.is_empty() {
        String::new()
    } else {
        format!("{BEGIN}\n{}\n{END}\n", lines.join("\n"))
    }
}

/// `crontab` without the managed block. Fails (changing nothing) if the markers are damaged.
pub fn strip_block(existing: &str) -> Result<String, BackupError> {
    let mut kept = Vec::new();
    let mut inside = false;

    for line in existing.lines() {
        if line.starts_with("# BEGIN AUJAR-BACKUP") {
            if inside {
                return Err(damaged());
            }
            inside = true;
        } else if line.starts_with("# END AUJAR-BACKUP") {
            if !inside {
                return Err(damaged());
            }
            inside = false;
        } else if !inside {
            kept.push(line);
        }
    }
    if inside {
        return Err(damaged());
    }

    let mut text = kept.join("\n");
    while text.ends_with("\n\n") {
        text.pop();
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

fn damaged() -> BackupError {
    BackupError::Schedule(
        "the Aujar markers in your crontab are damaged (missing BEGIN/END); fix it with `crontab -e`"
            .into(),
    )
}

/// `existing` with the managed block replaced by `block` (or removed when `block` is empty).
pub fn apply_block(existing: &str, block: &str) -> Result<String, BackupError> {
    let mut text = strip_block(existing)?;
    if !block.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(block);
    }
    Ok(text)
}

fn crontab_missing(error: std::io::Error) -> BackupError {
    if error.kind() == std::io::ErrorKind::NotFound {
        BackupError::ToolMissing {
            tool: "crontab".into(),
            hint: if cfg!(windows) {
                "cron does not exist on Windows; create a Task Scheduler task that runs \
                 `aujar backup run --all` instead."
                    .into()
            } else {
                "Install cron (Debian/Ubuntu: `sudo apt install cron`, Fedora: `sudo dnf install cronie`) \
                 and make sure the service is enabled."
                    .into()
            },
        }
    } else {
        BackupError::io("cannot run crontab", error)
    }
}

/// The current crontab text (empty when the user has none yet).
pub fn read_crontab() -> Result<String, BackupError> {
    let output = Command::new("crontab")
        .arg("-l")
        .output()
        .map_err(crontab_missing)?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("no crontab") {
        Ok(String::new())
    } else {
        Err(BackupError::Schedule(format!(
            "`crontab -l` failed: {}",
            stderr.trim()
        )))
    }
}

fn write_crontab(text: &str) -> Result<(), BackupError> {
    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(crontab_missing)?;

    child
        .stdin
        .take()
        .ok_or_else(|| BackupError::Schedule("cannot open crontab's input".into()))?
        .write_all(text.as_bytes())
        .map_err(|e| BackupError::io("cannot send the crontab", e))?;

    let output = child
        .wait_with_output()
        .map_err(|e| BackupError::io("crontab did not finish", e))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(BackupError::Schedule(format!(
            "`crontab -` rejected the new schedule: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// What `install` or `remove` did.
#[derive(Debug, PartialEq, Eq)]
pub struct Installed {
    pub scheduled_jobs: usize,
}

/// Put every scheduled job into the user's crontab. The previous crontab is copied to
/// `saved_copy` first.
pub fn install(
    jobs: &[Job],
    exe: &Path,
    log: &Path,
    saved_copy: &Path,
) -> Result<Installed, BackupError> {
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| BackupError::io(format!("cannot create {}", parent.display()), e))?;
    }
    let existing = read_crontab()?;
    let block = render_block(jobs, exe, log);
    let updated = apply_block(&existing, &block)?;

    if !existing.is_empty() {
        write_private(saved_copy, &existing)?;
    }
    if updated != existing {
        write_crontab(&updated)?;
    }
    Ok(Installed {
        scheduled_jobs: jobs.iter().filter(|j| j.schedule.is_some()).count(),
    })
}

/// Remove Aujar's block (other crontab lines stay).
pub fn remove(saved_copy: &Path) -> Result<bool, BackupError> {
    let existing = read_crontab()?;
    if !existing.contains("# BEGIN AUJAR-BACKUP") {
        return Ok(false);
    }
    let updated = strip_block(&existing)?;
    write_private(saved_copy, &existing)?;
    write_crontab(&updated)?;
    Ok(true)
}

/// The managed block currently in the crontab, if any.
pub fn installed_block() -> Result<Option<String>, BackupError> {
    let existing = read_crontab()?;
    let mut inside = false;
    let mut lines = Vec::new();
    for line in existing.lines() {
        if line.starts_with("# BEGIN AUJAR-BACKUP") {
            inside = true;
        }
        if inside {
            lines.push(line);
        }
        if line.starts_with("# END AUJAR-BACKUP") {
            inside = false;
        }
    }
    Ok((!lines.is_empty()).then(|| lines.join("\n") + "\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Kind;

    #[test]
    fn accepts_common_schedules() {
        for ok in [
            "0 2 * * *",
            "*/15 * * * *",
            "30 3 * * 1-5",
            "0 0,12 1 * *",
            "  0   2  * * *  ",
            "@daily",
            "0 2 * * 7",
        ] {
            assert!(normalize_schedule(ok).is_ok(), "{ok}");
        }
        assert_eq!(normalize_schedule("  0   2  * * *").unwrap(), "0 2 * * *");
    }

    #[test]
    fn rejects_bad_schedules() {
        for bad in [
            "",
            "* * * *",
            "* * * * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "5/2 * * * *",
            "*/0 * * * *",
            "5-1 * * * *",
            "a * * * *",
            "+5 * * * *",
            "0 2 * * * ; rm -rf /",
            "$(id) * * * *",
            "@reboot",
            "@sometimes",
        ] {
            assert!(normalize_schedule(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn daily_at_builds_a_schedule() {
        assert_eq!(daily_at("02:30").unwrap(), "30 2 * * *");
        assert_eq!(daily_at("0:05").unwrap(), "5 0 * * *");
        assert!(daily_at("24:00").is_err());
        assert!(daily_at("2").is_err());
        assert!(daily_at("02:61").is_err());
    }

    fn job(name: &str, schedule: Option<&str>) -> Job {
        let mut job = Job::new(name, Kind::Postgres);
        job.database = Some("app".into());
        job.schedule = schedule.map(str::to_owned);
        job
    }

    #[test]
    fn renders_only_scheduled_jobs_with_safe_quoting() {
        let jobs = [job("a", Some("0 2 * * *")), job("b", None)];
        let block = render_block(
            &jobs,
            Path::new("/opt/my tools/aujar"),
            Path::new("/home/u/100%/log"),
        );
        assert!(block.starts_with(BEGIN));
        assert!(
            block.contains(
                "0 2 * * * '/opt/my tools/aujar' backup run a >> '/home/u/100\\%/log' 2>&1"
            )
        );
        assert!(!block.contains("run b"));
        assert!(render_block(&[job("b", None)], Path::new("/x"), Path::new("/y")).is_empty());
    }

    #[test]
    fn replaces_only_the_managed_block() {
        let mine = render_block(
            &[job("a", Some("@daily"))],
            Path::new("/bin/aujar"),
            Path::new("/l"),
        );
        let user_lines = "MAILTO=me@example.com\n0 5 * * * /usr/bin/other\n";

        let first = apply_block(user_lines, &mine).unwrap();
        assert!(first.starts_with(user_lines));
        assert!(first.contains("backup run a"));

        let newer = render_block(
            &[job("b", Some("@weekly"))],
            Path::new("/bin/aujar"),
            Path::new("/l"),
        );
        let second = apply_block(&first, &newer).unwrap();
        assert!(second.starts_with(user_lines));
        assert!(second.contains("backup run b") && !second.contains("backup run a"));
        assert_eq!(second.matches(BEGIN).count(), 1);

        assert_eq!(apply_block(&second, "").unwrap(), user_lines);
        assert_eq!(apply_block("", &mine).unwrap(), mine);
    }

    #[test]
    fn refuses_a_damaged_block() {
        assert!(strip_block("# BEGIN AUJAR-BACKUP x\n0 0 * * * y\n").is_err());
        assert!(strip_block("# END AUJAR-BACKUP\n").is_err());
        assert!(
            strip_block("# BEGIN AUJAR-BACKUP\n# BEGIN AUJAR-BACKUP\n# END AUJAR-BACKUP\n")
                .is_err()
        );
    }
}
