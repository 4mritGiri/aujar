//! Small helpers: shell quoting, UTC timestamps (no time-zone database needed), tool lookup.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Quote one word for a POSIX shell. Used for the remote command of SSH jobs and for crontab
/// lines; local tools are started without any shell.
pub fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./=:@,+".contains(c));

    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn parts(secs: u64) -> (i64, i64, i64, u64, u64, u64) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// `20261124-021500Z`: sorts in time order and is safe in file names.
pub fn stamp(secs: u64) -> String {
    let (y, m, d, hh, mm, ss) = parts(secs);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}Z")
}

/// `2026-11-24T02:15:00Z`, for log lines.
pub fn iso(secs: u64) -> String {
    let (y, m, d, hh, mm, ss) = parts(secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Inverse of [`stamp`]. `None` unless the text is exactly a valid stamp.
pub fn parse_stamp(text: &str) -> Option<u64> {
    let b = text.as_bytes();
    if b.len() != 16 || b[8] != b'-' || b[15] != b'Z' {
        return None;
    }
    if !b[..8].iter().chain(&b[9..15]).all(u8::is_ascii_digit) {
        return None;
    }
    let n = |r: std::ops::Range<usize>| text[r].parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(4..6)?, n(6..8)?);
    let (hh, mm, ss) = (n(9..11)?, n(11..13)?, n(13..15)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let days = days_from_civil(y, m, d);
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Directories searched for the dump tools. Cron starts programs with a minimal PATH, so the
/// usual install locations are appended (including Debian/Ubuntu's versioned PostgreSQL dirs).
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();

    let mut extra: Vec<PathBuf> = [
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/opt/homebrew/bin",
        "/opt/homebrew/opt/libpq/bin",
        "/usr/local/opt/libpq/bin",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();

    if let Ok(entries) = fs::read_dir("/usr/lib/postgresql") {
        let mut versions: Vec<PathBuf> = entries.flatten().map(|e| e.path().join("bin")).collect();
        versions.sort();
        versions.reverse();
        extra.extend(versions);
    }

    for dir in extra {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Absolute path of a tool, searched in [`search_dirs`].
pub fn which(name: &str) -> Option<PathBuf> {
    search_dirs().into_iter().find_map(|dir| {
        let plain = dir.join(name);
        if is_executable(&plain) {
            return Some(plain);
        }
        let exe = dir.join(format!("{name}.exe"));
        is_executable(&exe).then_some(exe)
    })
}

/// A token that is safe to pass as a host, user or database name: it cannot start an option
/// and contains nothing a shell or a tool would treat specially.
pub fn is_safe_token(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 253
        && !text.starts_with('-')
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._@:-".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_round_trips() {
        for secs in [0, 86_399, 1_700_000_000, 1_795_567_200, 4_102_444_799] {
            assert_eq!(parse_stamp(&stamp(secs)), Some(secs), "{secs}");
        }
        assert_eq!(stamp(1_700_000_000), "20231114-221320Z");
        assert_eq!(iso(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn stamp_handles_leap_days() {
        assert_eq!(stamp(951_782_400), "20000229-000000Z");
        assert_eq!(parse_stamp("20000229-000000Z"), Some(951_782_400));
    }

    #[test]
    fn rejects_malformed_stamps() {
        for bad in [
            "",
            "2023111-221320Z",
            "20231314-221320Z",
            "20231114-221320",
            "2023111a-221320Z",
        ] {
            assert_eq!(parse_stamp(bad), None, "{bad}");
        }
    }

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(shell_quote("plain-1.2/x"), "plain-1.2/x");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("$(rm -rf /)"), "'$(rm -rf /)'");
    }

    #[test]
    fn safe_tokens() {
        assert!(is_safe_token("db.example.com"));
        assert!(is_safe_token("deploy@10.0.0.5"));
        assert!(!is_safe_token("-oProxyCommand=x"));
        assert!(!is_safe_token("a;b"));
        assert!(!is_safe_token("a b"));
        assert!(!is_safe_token(""));
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
    }
}
