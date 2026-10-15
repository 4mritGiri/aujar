//! File transfer: receiving into a download directory, and streaming files out.
//!
//! A peer is authenticated but not trusted with the filesystem. Incoming names are reduced to
//! a safe single path component, nothing is ever overwritten, data goes to a `.part` file that
//! only becomes the final name after the size and SHA-256 check out, and partial files are
//! removed on any failure.

use std::{
    collections::BTreeMap,
    fs::OpenOptions as StdOpenOptions,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};

use crate::{
    error::{Error, Result},
    layout::MachineName,
    protocol::{CHUNK_BYTES, FileMessage, MAX_FILE_BYTES, Message},
};

/// Transfers in flight at once, across all peers.
pub const MAX_CONCURRENT: usize = 8;

/// A transfer with no data for this long is abandoned.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

const MAX_NAME_BYTES: usize = 200;

/// Reduce a peer-supplied name to one safe path component.
///
/// Directories are stripped (both `/` and `\`), control characters rejected, a leading dot
/// replaced (no hidden files appear unannounced) and the length bounded.
pub fn sanitize_file_name(name: &str) -> Result<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();

    if base.is_empty() || base == "." || base == ".." {
        return Err(Error::Transfer(format!(
            "`{name}` is not a usable file name"
        )));
    }

    if base.chars().any(char::is_control) {
        return Err(Error::Transfer(
            "file name contains control characters".into(),
        ));
    }

    let mut cleaned = String::with_capacity(base.len());

    for (index, c) in base.chars().enumerate() {
        cleaned.push(if index == 0 && c == '.' { '_' } else { c });
    }

    // Truncate on a character boundary, keeping the extension if there is one.
    if cleaned.len() > MAX_NAME_BYTES {
        let extension = Path::new(&cleaned)
            .extension()
            .and_then(|e| e.to_str())
            .filter(|e| e.len() <= 16)
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        let budget = MAX_NAME_BYTES - extension.len();
        let stem: String = cleaned
            .chars()
            .scan(0usize, |used, c| {
                *used += c.len_utf8();
                (*used <= budget).then_some(c)
            })
            .collect();

        cleaned = format!("{stem}{extension}");
    }

    Ok(cleaned)
}

struct Incoming {
    part: PathBuf,
    final_path: PathBuf,
    file: File,
    expected: u64,
    received: u64,
    hasher: Sha256,
    last_activity: Instant,
}

/// Accepts files from peers into one directory.
pub struct Receiver {
    dir: PathBuf,
    max_bytes: u64,
    incoming: BTreeMap<(MachineName, u64), Incoming>,
}

impl Receiver {
    pub fn new(dir: impl Into<PathBuf>, max_bytes: u64) -> Self {
        Self {
            dir: dir.into(),
            max_bytes: max_bytes.min(MAX_FILE_BYTES),
            incoming: BTreeMap::new(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn active(&self) -> usize {
        self.incoming.len()
    }

    /// Start receiving. `Err` carries the reason to send back in a `Reject`.
    pub async fn offer(
        &mut self,
        peer: &MachineName,
        id: u64,
        name: &str,
        size: u64,
        now: Instant,
    ) -> Result<()> {
        if size > self.max_bytes {
            return Err(Error::Transfer(format!(
                "{size} bytes is over the {} byte limit",
                self.max_bytes
            )));
        }

        if self.incoming.len() >= MAX_CONCURRENT {
            return Err(Error::Transfer("too many transfers in progress".into()));
        }

        if self.incoming.contains_key(&(peer.clone(), id)) {
            return Err(Error::Transfer("transfer id already in use".into()));
        }

        let name = sanitize_file_name(name)?;

        tokio::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .await?;

        let (final_path, part, file) = self.claim(&name).await?;

        self.incoming.insert(
            (peer.clone(), id),
            Incoming {
                part,
                final_path,
                file,
                expected: size,
                received: 0,
                hasher: Sha256::new(),
                last_activity: now,
            },
        );

        Ok(())
    }

    /// Find an unused name and create its `.part` file exclusively.
    async fn claim(&self, name: &str) -> Result<(PathBuf, PathBuf, File)> {
        let path = Path::new(name);
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
        let extension = path.extension().and_then(|e| e.to_str());

        for attempt in 0..1000u32 {
            let candidate = match (attempt, extension) {
                (0, _) => name.to_owned(),
                (n, Some(ext)) => format!("{stem} ({n}).{ext}"),
                (n, None) => format!("{stem} ({n})"),
            };
            let final_path = self.dir.join(&candidate);
            let part = self.dir.join(format!("{candidate}.part"));

            if tokio::fs::try_exists(&final_path).await? {
                continue;
            }

            let opened = tokio::task::spawn_blocking({
                let part = part.clone();

                move || {
                    StdOpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(part)
                }
            })
            .await
            .map_err(|error| Error::Other(error.to_string()))?;

            match opened {
                Ok(file) => return Ok((final_path, part, File::from_std(file))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }

        Err(Error::Transfer("too many files with that name".into()))
    }

    /// Append the next chunk. Chunks must arrive in order and not exceed the offered size.
    pub async fn chunk(
        &mut self,
        peer: &MachineName,
        id: u64,
        offset: u64,
        data: &[u8],
        now: Instant,
    ) -> Result<()> {
        let key = (peer.clone(), id);
        let result = match self.incoming.get_mut(&key) {
            None => return Err(Error::Transfer("no such transfer".into())),
            Some(transfer) => {
                if offset != transfer.received {
                    Err(Error::Transfer("chunk out of order".into()))
                } else if transfer.received + data.len() as u64 > transfer.expected {
                    Err(Error::Transfer("more data than offered".into()))
                } else {
                    transfer.hasher.update(data);
                    transfer.received += data.len() as u64;
                    transfer.last_activity = now;

                    transfer.file.write_all(data).await.map_err(Error::from)
                }
            }
        };

        if result.is_err() {
            self.abort(peer, id).await;
        }

        result
    }

    /// Finish: verify size and hash, then give the file its final name. Returns that path.
    pub async fn done(&mut self, peer: &MachineName, id: u64, sha256: [u8; 32]) -> Result<PathBuf> {
        let Some(transfer) = self.incoming.remove(&(peer.clone(), id)) else {
            return Err(Error::Transfer("no such transfer".into()));
        };

        let Incoming {
            part,
            final_path,
            mut file,
            expected,
            received,
            hasher,
            ..
        } = transfer;

        let verdict = if received != expected {
            Err(Error::Transfer(format!(
                "received {received} of {expected} bytes"
            )))
        } else if <[u8; 32]>::from(hasher.finalize()) != sha256 {
            Err(Error::Transfer("checksum mismatch".into()))
        } else {
            file.flush().await.map_err(Error::from)
        };

        drop(file);

        if let Err(error) = verdict {
            let _ = tokio::fs::remove_file(&part).await;

            return Err(error);
        }

        // `hard_link` fails if the name appeared meanwhile, so nothing is ever overwritten.
        let published = tokio::fs::hard_link(&part, &final_path).await;
        let _ = tokio::fs::remove_file(&part).await;

        published.map_err(|error| {
            Error::Transfer(format!("cannot store {}: {error}", final_path.display()))
        })?;

        Ok(final_path)
    }

    pub async fn abort(&mut self, peer: &MachineName, id: u64) {
        if let Some(transfer) = self.incoming.remove(&(peer.clone(), id)) {
            let Incoming { part, file, .. } = transfer;

            drop(file);
            let _ = tokio::fs::remove_file(part).await;
        }
    }

    /// Abandon every transfer from `peer` (it disconnected).
    pub async fn drop_peer(&mut self, peer: &MachineName) {
        let ids: Vec<u64> = self
            .incoming
            .keys()
            .filter(|(name, _)| name == peer)
            .map(|(_, id)| *id)
            .collect();

        for id in ids {
            self.abort(peer, id).await;
        }
    }

    /// Abandon transfers that stalled. Returns the peers and ids that were dropped.
    pub async fn expire(&mut self, now: Instant) -> Vec<(MachineName, u64)> {
        let stale: Vec<(MachineName, u64)> = self
            .incoming
            .iter()
            .filter(|(_, t)| now.saturating_duration_since(t.last_activity) > IDLE_TIMEOUT)
            .map(|(key, _)| key.clone())
            .collect();

        for (peer, id) in &stale {
            self.abort(peer, *id).await;
        }

        stale
    }
}

/// A file ready to be offered to a peer.
#[derive(Debug, Clone)]
pub struct Outgoing {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
}

impl Outgoing {
    /// Check that `path` is a regular file the peer will accept.
    pub async fn prepare(path: &Path, max_bytes: u64) -> Result<Self> {
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|error| Error::Transfer(format!("cannot read {}: {error}", path.display())))?;

        if metadata.is_dir() {
            return Err(Error::Transfer(format!(
                "{} is a folder; send files (archive the folder first)",
                path.display()
            )));
        }

        if !metadata.is_file() {
            return Err(Error::Transfer(format!(
                "{} is not a regular file",
                path.display()
            )));
        }

        let limit = max_bytes.min(MAX_FILE_BYTES);

        if metadata.len() > limit {
            return Err(Error::Transfer(format!(
                "{} is {} bytes; the limit is {limit}",
                path.display(),
                metadata.len()
            )));
        }

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::Transfer("file name is not valid UTF-8".into()))?
            .to_owned();

        Ok(Self {
            path: path.to_owned(),
            name,
            size: metadata.len(),
        })
    }

    /// Send the file's contents as chunks followed by `Done`. Stops quietly if the receiver
    /// went away (the channel closed).
    pub async fn stream(&self, id: u64, out: &mpsc::Sender<Message>) -> Result<()> {
        let mut file = File::open(&self.path).await?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; CHUNK_BYTES];
        let mut offset = 0u64;

        loop {
            let read = file.read(&mut buffer).await?;

            if read == 0 {
                break;
            }

            if offset + read as u64 > self.size {
                return Err(Error::Transfer(format!(
                    "{} grew while it was being sent",
                    self.name
                )));
            }

            hasher.update(&buffer[..read]);

            let chunk = Message::File(FileMessage::Chunk {
                id,
                offset,
                data: buffer[..read].to_vec(),
            });

            if out.send(chunk).await.is_err() {
                return Ok(());
            }

            offset += read as u64;
        }

        if offset != self.size {
            return Err(Error::Transfer(format!(
                "{} shrank while it was being sent",
                self.name
            )));
        }

        let _ = out
            .send(Message::File(FileMessage::Done {
                id,
                sha256: hasher.finalize().into(),
            }))
            .await;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> MachineName {
        MachineName::new("laptop").unwrap()
    }

    fn sha(data: &[u8]) -> [u8; 32] {
        Sha256::digest(data).into()
    }

    fn receiver(dir: &Path) -> Receiver {
        Receiver::new(dir.join("in"), 1024)
    }

    async fn receive(rx: &mut Receiver, id: u64, name: &str, data: &[u8]) -> Result<PathBuf> {
        let now = Instant::now();

        rx.offer(&peer(), id, name, data.len() as u64, now).await?;

        for (index, chunk) in data.chunks(10).enumerate() {
            rx.chunk(&peer(), id, (index * 10) as u64, chunk, now)
                .await?;
        }

        rx.done(&peer(), id, sha(data)).await
    }

    #[test]
    fn names_are_reduced_to_a_safe_component() {
        let ok = |name: &str| sanitize_file_name(name).unwrap();

        assert_eq!(ok("report.pdf"), "report.pdf");
        assert_eq!(ok("../../etc/passwd"), "passwd");
        assert_eq!(ok("C:\\Users\\me\\notes.txt"), "notes.txt");
        assert_eq!(ok("/abs/path/file"), "file");
        assert_eq!(ok(".bashrc"), "_bashrc");
        assert_eq!(ok("  spaced.txt "), "spaced.txt");
        assert_eq!(ok("ünïcode ✓.txt"), "ünïcode ✓.txt");

        for bad in [
            "",
            ".",
            "..",
            "a/..",
            "dir/",
            "x\0y",
            "tab\there",
            "new\nline",
        ] {
            assert!(
                sanitize_file_name(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn long_names_are_truncated_on_a_char_boundary_keeping_the_extension() {
        let name = format!("{}.tar.gz", "é".repeat(300));
        let cleaned = sanitize_file_name(&name).unwrap();

        assert!(cleaned.len() <= MAX_NAME_BYTES);
        assert!(cleaned.ends_with(".gz"));
        assert!(cleaned.is_char_boundary(cleaned.len()));
    }

    #[tokio::test]
    async fn receives_a_file_and_publishes_it_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let mut rx = receiver(dir.path());
        let data = b"hello across the desk, twenty-five bytes+".to_vec();

        let path = receive(&mut rx, 1, "note.txt", &data).await.unwrap();

        assert_eq!(path, dir.path().join("in/note.txt"));
        assert_eq!(std::fs::read(&path).unwrap(), data);
        assert_eq!(rx.active(), 0);
        assert!(!dir.path().join("in/note.txt.part").exists());

        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0, "received files must not be executable");
    }

    #[tokio::test]
    async fn never_overwrites_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut rx = receiver(dir.path());

        let first = receive(&mut rx, 1, "a.txt", b"one").await.unwrap();
        let second = receive(&mut rx, 2, "a.txt", b"two").await.unwrap();
        let third = receive(&mut rx, 3, "noext", b"3").await.unwrap();
        let fourth = receive(&mut rx, 4, "noext", b"4").await.unwrap();

        assert_eq!(std::fs::read(first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
        assert!(second.ends_with("a (1).txt"));
        assert!(third.ends_with("noext") && fourth.ends_with("noext (1)"));
    }

    #[tokio::test]
    async fn traversal_names_stay_inside_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut rx = receiver(dir.path());

        let path = receive(&mut rx, 1, "../../escape.txt", b"x").await.unwrap();

        assert_eq!(path, dir.path().join("in/escape.txt"));
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[tokio::test]
    async fn rejects_oversized_offers_and_overruns() {
        let dir = tempfile::tempdir().unwrap();
        let mut rx = receiver(dir.path());
        let now = Instant::now();

        assert!(rx.offer(&peer(), 1, "big", 1025, now).await.is_err());

        rx.offer(&peer(), 2, "small", 4, now).await.unwrap();
        // Sending more than offered aborts and cleans up.
        assert!(rx.chunk(&peer(), 2, 0, b"12345", now).await.is_err());
        assert_eq!(rx.active(), 0);
        assert!(!dir.path().join("in/small.part").exists());
    }

    #[tokio::test]
    async fn out_of_order_chunks_abort_the_transfer() {
        let dir = tempfile::tempdir().unwrap();
        let mut rx = receiver(dir.path());
        let now = Instant::now();

        rx.offer(&peer(), 1, "f", 10, now).await.unwrap();
        rx.chunk(&peer(), 1, 0, b"abcde", now).await.unwrap();

        assert!(rx.chunk(&peer(), 1, 7, b"fgh", now).await.is_err());
        assert_eq!(rx.active(), 0);
    }

    #[tokio::test]
    async fn wrong_checksum_or_short_data_is_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let mut rx = receiver(dir.path());
        let now = Instant::now();

        rx.offer(&peer(), 1, "f", 3, now).await.unwrap();
        rx.chunk(&peer(), 1, 0, b"abc", now).await.unwrap();
        assert!(rx.done(&peer(), 1, sha(b"abd")).await.is_err());

        rx.offer(&peer(), 2, "g", 3, now).await.unwrap();
        rx.chunk(&peer(), 2, 0, b"ab", now).await.unwrap();
        assert!(rx.done(&peer(), 2, sha(b"ab")).await.is_err());

        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("in")).unwrap().collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[tokio::test]
    async fn limits_concurrency_and_cleans_up_on_disconnect_and_stall() {
        let dir = tempfile::tempdir().unwrap();
        let mut rx = receiver(dir.path());
        let now = Instant::now();

        for id in 0..MAX_CONCURRENT as u64 {
            rx.offer(&peer(), id, &format!("f{id}"), 1, now)
                .await
                .unwrap();
        }
        assert!(rx.offer(&peer(), 99, "extra", 1, now).await.is_err());

        // Duplicate ids are refused.
        rx.abort(&peer(), 0).await;
        rx.offer(&peer(), 1, "dup", 1, now).await.unwrap_err();

        let stale = rx.expire(now + IDLE_TIMEOUT + Duration::from_secs(1)).await;
        assert_eq!(stale.len(), MAX_CONCURRENT - 1);
        assert_eq!(rx.active(), 0);

        rx.offer(&peer(), 5, "again", 1, now).await.unwrap();
        rx.drop_peer(&peer()).await;
        assert_eq!(rx.active(), 0);
        assert_eq!(std::fs::read_dir(dir.path().join("in")).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn sender_streams_to_receiver_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("big.bin");
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&source, &data).unwrap();

        let outgoing = Outgoing::prepare(&source, 1_000_000).await.unwrap();
        assert_eq!(
            (outgoing.name.as_str(), outgoing.size),
            ("big.bin", 100_000)
        );

        let (tx, mut rx_channel) = mpsc::channel(8);
        let sender = tokio::spawn(async move { outgoing.stream(7, &tx).await });

        let mut receiver = Receiver::new(dir.path().join("in"), 1_000_000);
        let now = Instant::now();
        receiver
            .offer(&peer(), 7, "big.bin", 100_000, now)
            .await
            .unwrap();

        let mut stored = None;

        while let Some(message) = rx_channel.recv().await {
            match message {
                Message::File(FileMessage::Chunk { id, offset, data }) => {
                    receiver
                        .chunk(&peer(), id, offset, &data, now)
                        .await
                        .unwrap();
                }
                Message::File(FileMessage::Done { id, sha256 }) => {
                    stored = Some(receiver.done(&peer(), id, sha256).await.unwrap());
                }
                other => panic!("unexpected {other:?}"),
            }
        }

        sender.await.unwrap().unwrap();
        assert_eq!(std::fs::read(stored.unwrap()).unwrap(), data);
    }

    #[tokio::test]
    async fn prepare_rejects_folders_missing_and_oversized_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, vec![0u8; 50]).unwrap();

        assert!(
            Outgoing::prepare(dir.path(), 100)
                .await
                .unwrap_err()
                .to_string()
                .contains("folder")
        );
        assert!(
            Outgoing::prepare(&dir.path().join("nope"), 100)
                .await
                .is_err()
        );
        assert!(
            Outgoing::prepare(&file, 10)
                .await
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        assert!(Outgoing::prepare(&file, 100).await.is_ok());
        // The protocol's hard cap wins over a larger configured limit.
        assert!(Outgoing::prepare(&file, u64::MAX).await.is_ok());
    }
}
