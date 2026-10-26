//! An audit trail of security-relevant events, as JSON lines.
//!
//! Mouse Borders lets one machine type on another, so administrators want to know when that
//! happened. Each line records *what* occurred (a machine connected or was refused, control
//! moved, a file arrived, a screen was locked remotely, the key changed) and *when*.
//! It never records clipboard contents, keystrokes, file contents or the security key: only
//! sizes, names and counts.
//!
//! The log is append-only, created private to the user, and rotated by size so it cannot fill
//! a disk.

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::fsutil;

/// Rotate when the log reaches this size.
pub const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// How many rotated files to keep besides the current one.
pub const DEFAULT_KEEP: usize = 3;

/// What happened. Serialised with an `event` tag so each line reads naturally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AuditEvent {
    ServiceStarted {
        listen: String,
    },
    ServiceStopped,
    PeerConnected {
        peer: String,
        address: Option<String>,
    },
    PeerDisconnected {
        peer: String,
        reason: String,
    },
    /// A connection was turned away before or during the handshake.
    PeerRefused {
        address: Option<String>,
        reason: String,
    },
    /// The keyboard and mouse moved to another machine (`to`), or back (`to` is this machine).
    ControlMoved {
        from: String,
        to: String,
    },
    /// Another machine took control of this one, or let go.
    ControlledBy {
        peer: Option<String>,
    },
    BroadcastChanged {
        on: bool,
    },
    ClipboardSent {
        peer: String,
        kind: String,
        bytes: u64,
    },
    ClipboardReceived {
        peer: String,
        kind: String,
        bytes: u64,
    },
    FileOffered {
        peer: String,
        name: String,
        bytes: u64,
    },
    FileReceived {
        peer: String,
        name: String,
        bytes: u64,
    },
    FileRefused {
        peer: String,
        name: String,
        reason: String,
    },
    LockRequested {
        peer: Option<String>,
        /// Whether the lock was carried out (a machine can refuse remote locks).
        allowed: bool,
    },
    KeyChanged {
        how: String,
    },
}

/// One line of the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    /// RFC 3339, UTC.
    pub time: String,
    pub machine: String,
    #[serde(flatten)]
    pub event: AuditEvent,
}

impl AuditRecord {
    /// One readable line: `time machine event key=value ...`.
    pub fn summary(&self) -> String {
        let mut line = format!("{} {}", self.time, self.machine);

        let Ok(serde_json::Value::Object(fields)) = serde_json::to_value(&self.event) else {
            return line;
        };

        if let Some(event) = fields.get("event").and_then(|value| value.as_str()) {
            line.push(' ');
            line.push_str(event);
        }

        for (name, value) in &fields {
            match value {
                serde_json::Value::Null => {}
                _ if name == "event" => {}
                serde_json::Value::String(text) => line.push_str(&format!(" {name}={text}")),
                other => line.push_str(&format!(" {name}={other}")),
            }
        }

        line
    }
}

/// Somewhere audit events go.
pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent);
}

/// Discards everything (auditing switched off).
pub struct NullAudit;

impl AuditSink for NullAudit {
    fn record(&self, _event: AuditEvent) {}
}

/// Keeps events in memory, for tests.
#[derive(Clone, Default)]
pub struct MemoryAudit(Arc<Mutex<Vec<AuditEvent>>>);

impl MemoryAudit {
    pub fn events(&self) -> Vec<AuditEvent> {
        self.0.lock().unwrap().clone()
    }
}

impl AuditSink for MemoryAudit {
    fn record(&self, event: AuditEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// Appends to a rotating file.
pub struct FileAudit {
    machine: String,
    path: PathBuf,
    max_bytes: u64,
    keep: usize,
    file: Mutex<Option<(File, u64)>>,
}

impl FileAudit {
    pub fn new(machine: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self::with_limits(machine, path, DEFAULT_MAX_BYTES, DEFAULT_KEEP)
    }

    pub fn with_limits(
        machine: impl Into<String>,
        path: impl Into<PathBuf>,
        max_bytes: u64,
        keep: usize,
    ) -> Self {
        Self {
            machine: machine.into(),
            path: path.into(),
            max_bytes,
            keep,
            file: Mutex::new(None),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn rotated(&self, index: usize) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();

        name.push(format!(".{index}"));

        PathBuf::from(name)
    }

    fn open(&self) -> std::io::Result<(File, u64)> {
        if let Some(parent) = self.path.parent() {
            fsutil::create_private_dir_all(parent)?;
        }

        let size = fs::metadata(&self.path).map(|meta| meta.len()).unwrap_or(0);
        let mut options = OpenOptions::new();

        options.create(true).append(true);

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            options.mode(0o600);
        }

        Ok((options.open(&self.path)?, size))
    }

    /// Shift `log` to `log.1`, `log.1` to `log.2`, ... dropping the oldest.
    fn rotate(&self) -> std::io::Result<()> {
        if self.keep == 0 {
            return fs::remove_file(&self.path);
        }

        let _ = fs::remove_file(self.rotated(self.keep));

        for index in (1..self.keep).rev() {
            let from = self.rotated(index);

            if from.exists() {
                fs::rename(&from, self.rotated(index + 1))?;
            }
        }

        fs::rename(&self.path, self.rotated(1))
    }

    fn write(&self, line: &[u8]) -> std::io::Result<()> {
        let mut guard = self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if guard.is_none() {
            *guard = Some(self.open()?);
        }

        if let Some((_, size)) = guard.as_ref()
            && *size > 0
            && *size + line.len() as u64 > self.max_bytes
        {
            // Close, rotate, reopen: never rename a file we still hold open for writing.
            *guard = None;
            self.rotate()?;
            *guard = Some(self.open()?);
        }

        let (file, size) = guard.as_mut().expect("opened above");

        file.write_all(line)?;
        file.flush()?;
        *size += line.len() as u64;

        Ok(())
    }
}

impl AuditSink for FileAudit {
    fn record(&self, event: AuditEvent) {
        let record = AuditRecord {
            time: format_utc(now_secs()),
            machine: self.machine.clone(),
            event,
        };

        let Ok(mut line) = serde_json::to_vec(&record) else {
            return;
        };

        line.push(b'\n');

        // Auditing must never take the service down; say so once per failure and move on.
        if let Err(error) = self.write(&line) {
            tracing::warn!(%error, path = %self.path.display(), "cannot write the audit log");
        }
    }
}

/// Record an event in the current user's audit log, if the settings ask for auditing.
/// For things that happen outside a running service (joining a pairing code, key changes).
pub fn record_user(settings: &aujar_config::MouseBorders, event: AuditEvent) {
    if settings.audit_log {
        FileAudit::new(
            crate::settings_edit::local_name(settings).to_string(),
            user_log_path(),
        )
        .record(event);
    }
}

/// Where the current user's audit log lives.
pub fn user_log_path() -> PathBuf {
    aujar_config::platform::state_dir().join("mouse-borders-audit.log")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `1970-01-01T00:00:00Z` style timestamp for seconds since the epoch.
pub fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;

    // Civil-from-days (Howard Hinnant's algorithm), valid for the whole Gregorian calendar.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_correct_utc() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(86_399), "1970-01-01T23:59:59Z");
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z"); // a leap day
        assert_eq!(format_utc(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(format_utc(4_102_444_800), "2100-01-01T00:00:00Z"); // 2100 is not a leap year
    }

    fn read(path: &Path) -> Vec<AuditRecord> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn events_become_readable_json_lines() {
        let dir = tempfile::tempdir().unwrap();
        let audit = FileAudit::new("desk", dir.path().join("audit.log"));

        audit.record(AuditEvent::PeerConnected {
            peer: "laptop".into(),
            address: Some("192.168.1.20:15101".into()),
        });
        audit.record(AuditEvent::PeerRefused {
            address: None,
            reason: "wrong key".into(),
        });

        let text = fs::read_to_string(audit.path()).unwrap();
        let first = text.lines().next().unwrap();

        assert!(first.contains("\"event\":\"peer_connected\""), "{first}");
        assert!(first.contains("\"machine\":\"desk\""));
        assert!(
            first.contains("\"time\":\"20"),
            "an RFC 3339 timestamp: {first}"
        );

        let records = read(audit.path());

        assert_eq!(records.len(), 2);
        assert!(matches!(records[1].event, AuditEvent::PeerRefused { .. }));
    }

    #[test]
    fn a_new_file_is_created_private_and_appended_to_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("audit.log");

        FileAudit::new("a", &path).record(AuditEvent::ServiceStopped);
        FileAudit::new("a", &path).record(AuditEvent::ServiceStopped);

        assert_eq!(read(&path).len(), 2);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn the_log_rotates_by_size_and_keeps_a_bounded_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let audit = FileAudit::with_limits("m", &path, 400, 2);

        for index in 0..60 {
            audit.record(AuditEvent::KeyChanged {
                how: format!("change {index}"),
            });
        }

        let files: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();

        assert!(files.contains(&"audit.log".to_owned()), "{files:?}");
        assert!(files.contains(&"audit.log.1".to_owned()), "{files:?}");
        assert!(files.contains(&"audit.log.2".to_owned()), "{files:?}");
        assert!(
            !files.contains(&"audit.log.3".to_owned()),
            "old history is dropped: {files:?}"
        );

        // Every file stays near the limit and the newest event is in the current file.
        for name in files {
            assert!(fs::metadata(dir.path().join(name)).unwrap().len() <= 600);
        }
        let newest = read(&path).pop().unwrap();

        assert_eq!(
            newest.event,
            AuditEvent::KeyChanged {
                how: "change 59".into()
            }
        );
    }

    #[test]
    fn an_unwritable_location_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");

        fs::write(&blocker, "x").unwrap();

        // A "directory" that is really a file: opening fails, recording must not.
        FileAudit::new("m", blocker.join("audit.log")).record(AuditEvent::ServiceStopped);
    }

    #[test]
    fn the_memory_sink_records_in_order_and_null_discards() {
        let memory = MemoryAudit::default();

        memory.record(AuditEvent::ServiceStopped);
        memory.record(AuditEvent::BroadcastChanged { on: true });
        NullAudit.record(AuditEvent::ServiceStopped);

        assert_eq!(
            memory.events(),
            [
                AuditEvent::ServiceStopped,
                AuditEvent::BroadcastChanged { on: true }
            ]
        );
    }

    #[test]
    fn summaries_read_as_one_line_and_skip_empty_fields() {
        let record = AuditRecord {
            time: "2026-01-01T00:00:00Z".into(),
            machine: "desk".into(),
            event: AuditEvent::PeerRefused {
                address: None,
                reason: "wrong key".into(),
            },
        };

        assert_eq!(
            record.summary(),
            "2026-01-01T00:00:00Z desk peer_refused reason=wrong key"
        );

        let transfer = AuditRecord {
            time: "t".into(),
            machine: "m".into(),
            event: AuditEvent::FileReceived {
                peer: "laptop".into(),
                name: "a.pdf".into(),
                bytes: 12,
            },
        };

        assert!(transfer.summary().starts_with("t m file_received "));
        assert!(transfer.summary().contains("bytes=12"));
        assert!(transfer.summary().contains("peer=laptop"));
    }

    #[test]
    fn records_round_trip() {
        let record = AuditRecord {
            time: "2026-01-01T00:00:00Z".into(),
            machine: "desk".into(),
            event: AuditEvent::FileReceived {
                peer: "laptop".into(),
                name: "a.pdf".into(),
                bytes: 12,
            },
        };
        let text = serde_json::to_string(&record).unwrap();

        assert_eq!(serde_json::from_str::<AuditRecord>(&text).unwrap(), record);
    }
}
