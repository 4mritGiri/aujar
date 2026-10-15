//! Clipboard access through `wl-clipboard` (Wayland) or `xclip` (X11).
//!
//! Commands are run directly (no shell) with fixed arguments and short timeouts. Content
//! that the source application marks as password-manager data is never read.

use std::{process::Stdio, time::Duration};

use async_trait::async_trait;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};

use crate::{
    backend::{Clipboard, ClipboardContent},
    error::{Error, Result},
    protocol::MAX_CLIPBOARD_BYTES,
};

const TOOL_TIMEOUT: Duration = Duration::from_secs(2);

/// Marker set by KeePassXC, Bitwarden and others on copied secrets.
const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    WlClipboard,
    Xclip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pick {
    Png,
    Text(&'static str),
}

/// Choose what to read from the advertised MIME types / targets. `None` for secrets or
/// formats we do not share.
fn pick_type(types: &[&str]) -> Option<Pick> {
    if types.iter().any(|t| t.eq_ignore_ascii_case(PASSWORD_HINT)) {
        return None;
    }

    let has = |wanted: &str| types.iter().any(|t| t.eq_ignore_ascii_case(wanted));

    if has("image/png") {
        Some(Pick::Png)
    } else if has("text/plain;charset=utf-8") {
        Some(Pick::Text("text/plain;charset=utf-8"))
    } else if has("UTF8_STRING") {
        Some(Pick::Text("UTF8_STRING"))
    } else if has("text/plain") {
        Some(Pick::Text("text/plain"))
    } else {
        None
    }
}

pub struct CliClipboard {
    tool: Tool,
}

impl CliClipboard {
    /// The right tool for this session, if installed.
    pub fn detect(wayland: bool) -> Result<Self> {
        let candidates: &[(&str, Tool)] = if wayland {
            &[("wl-paste", Tool::WlClipboard)]
        } else {
            &[("xclip", Tool::Xclip)]
        };

        for (program, tool) in candidates {
            if on_path(program) {
                return Ok(Self { tool: *tool });
            }
        }

        Err(Error::unsupported(
            "clipboard sharing",
            if wayland {
                "install `wl-clipboard`"
            } else {
                "install `xclip`"
            },
        ))
    }

    async fn run(&self, program: &str, args: &[&str]) -> Result<Vec<u8>> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| Error::Other(format!("cannot run {program}: {error}")))?;

        let mut stdout = child.stdout.take().expect("piped");
        let mut data = Vec::new();
        let limit = MAX_CLIPBOARD_BYTES + 1;

        let read = timeout(TOOL_TIMEOUT, async {
            (&mut stdout).take(limit).read_to_end(&mut data).await
        })
        .await;

        match read {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => return Err(Error::Other(format!("{program} timed out"))),
        }

        let _ = child.kill().await;

        if data.len() as u64 > MAX_CLIPBOARD_BYTES {
            return Err(Error::Other(
                "clipboard content is too large to share".into(),
            ));
        }

        Ok(data)
    }

    async fn types(&self) -> Result<Vec<String>> {
        let output = match self.tool {
            Tool::WlClipboard => self.run("wl-paste", &["--list-types"]).await?,
            Tool::Xclip => {
                self.run("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"])
                    .await?
            }
        };

        Ok(String::from_utf8_lossy(&output)
            .lines()
            .map(str::to_owned)
            .collect())
    }

    async fn feed(&self, program: &str, args: &[&str], data: &[u8]) -> Result<()> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(false)
            .spawn()
            .map_err(|error| Error::Other(format!("cannot run {program}: {error}")))?;

        let mut stdin = child.stdin.take().expect("piped");

        timeout(TOOL_TIMEOUT, async {
            stdin.write_all(data).await?;
            drop(stdin);

            // The tool forks to keep serving the selection; its parent exits at once.
            child.wait().await
        })
        .await
        .map_err(|_| Error::Other(format!("{program} timed out")))??;

        Ok(())
    }
}

#[async_trait]
impl Clipboard for CliClipboard {
    async fn read(&self) -> Result<Option<ClipboardContent>> {
        let types = self.types().await.unwrap_or_default();
        let refs: Vec<&str> = types.iter().map(String::as_str).collect();

        let Some(pick) = pick_type(&refs) else {
            return Ok(None);
        };

        let (mime, png) = match &pick {
            Pick::Png => ("image/png", true),
            Pick::Text(mime) => (*mime, false),
        };

        let data = match self.tool {
            Tool::WlClipboard => {
                self.run("wl-paste", &["--no-newline", "--type", mime])
                    .await?
            }
            Tool::Xclip => {
                self.run("xclip", &["-selection", "clipboard", "-t", mime, "-o"])
                    .await?
            }
        };

        if data.is_empty() {
            return Ok(None);
        }

        if png {
            return Ok(Some(ClipboardContent::Png(data)));
        }

        Ok(String::from_utf8(data).ok().map(ClipboardContent::Text))
    }

    async fn write(&self, content: &ClipboardContent) -> Result<()> {
        let (mime, data): (&str, &[u8]) = match content {
            ClipboardContent::Text(text) => ("text/plain;charset=utf-8", text.as_bytes()),
            ClipboardContent::Png(bytes) => ("image/png", bytes),
        };

        match self.tool {
            Tool::WlClipboard => self.feed("wl-copy", &["--type", mime], data).await,
            Tool::Xclip => {
                let target = if mime == "image/png" {
                    "image/png"
                } else {
                    "UTF8_STRING"
                };

                self.feed(
                    "xclip",
                    &["-selection", "clipboard", "-i", "-t", target],
                    data,
                )
                .await
            }
        }
    }
}

pub(super) fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_images_then_utf8_text() {
        assert_eq!(pick_type(&["text/plain", "image/png"]), Some(Pick::Png));
        assert_eq!(
            pick_type(&["text/plain", "text/plain;charset=utf-8"]),
            Some(Pick::Text("text/plain;charset=utf-8"))
        );
        assert_eq!(
            pick_type(&["TARGETS", "UTF8_STRING", "STRING"]),
            Some(Pick::Text("UTF8_STRING"))
        );
        assert_eq!(pick_type(&["text/plain"]), Some(Pick::Text("text/plain")));
    }

    #[test]
    fn ignores_unsupported_formats() {
        assert_eq!(pick_type(&["text/html", "application/x-custom"]), None);
        assert_eq!(pick_type(&[]), None);
    }

    #[test]
    fn never_reads_password_manager_content() {
        assert_eq!(
            pick_type(&["text/plain", "x-kde-passwordManagerHint"]),
            None
        );
        assert_eq!(pick_type(&["image/png", "X-KDE-PASSWORDMANAGERHINT"]), None);
    }
}
