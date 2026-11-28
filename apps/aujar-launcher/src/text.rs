//! `aujar text`: Text Extractor. Select part of the screen, get its text on the clipboard.
//!
//! The screen capture and recognition happen in the daemon (policy-gated `screen_capture`);
//! this command runs in the user's session, so it copies the result and, when started from a
//! keyboard shortcut (no terminal), reports through a desktop notification.

use anyhow::{Result, bail};
use aujar_ipc::{ExtractTextRequest, Request, Response, send};
use clap::{Args, Subcommand};
use std::{io::IsTerminal, path::PathBuf};

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct TextArgs {
    #[command(subcommand)]
    action: Option<TextAction>,

    #[command(flatten)]
    extract: ExtractArgs,
}

#[derive(Debug, Args)]
struct ExtractArgs {
    /// Tesseract language(s) to recognise, e.g. `nep` or `eng+nep` (default: the settings,
    /// `[text_extractor] languages`).
    #[arg(long = "lang", short = 'l')]
    languages: Vec<String>,

    /// Join the text into one line.
    #[arg(long)]
    single_line: bool,

    /// Print the text only; leave the clipboard alone.
    #[arg(long)]
    no_copy: bool,

    /// Read the text from this picture instead of selecting an area of the screen.
    #[arg(long, value_name = "PICTURE")]
    file: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum TextAction {
    /// List the installed Tesseract languages.
    Languages,

    /// Register the Text Extractor shortcut in GNOME Keyboard Shortcuts.
    ShortcutInstall {
        /// Modifier names followed by one key (default: the settings, SUPER+SHIFT+T).
        #[arg(long)]
        keys: Option<String>,
    },
}

/// Started from a shortcut: nobody sees standard output.
fn headless() -> bool {
    !std::io::stdout().is_terminal() && !std::io::stderr().is_terminal()
}

/// A desktop notification, when `notify-send` is installed. Best effort.
fn notify(summary: &str, body: &str) {
    let _ = std::process::Command::new("notify-send")
        .args(["--app-name=Aujar", "--icon=edit-copy", "--", summary, body])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// The first characters of `text` on one line, for a notification.
pub fn preview(text: &str, max_chars: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");

    if flat.chars().count() <= max_chars {
        flat
    } else {
        let cut: String = flat.chars().take(max_chars).collect();
        // End on a whole word when there is one.
        let cut = match cut.rfind(' ') {
            Some(space) if space > 0 => &cut[..space],
            _ => cut.as_str(),
        };

        format!("{}…", cut.trim_end())
    }
}

pub async fn run(args: TextArgs, socket: PathBuf) -> Result<()> {
    match args.action {
        Some(TextAction::Languages) => {
            for language in
                tokio::task::spawn_blocking(aujar_text_extractor::ocr::languages).await??
            {
                println!("{language}");
            }

            Ok(())
        }
        Some(TextAction::ShortcutInstall { keys }) => {
            let keys = keys.unwrap_or_else(|| {
                aujar_config::Settings::load_or_default()
                    .text_extractor
                    .shortcut
            });

            crate::shortcut::install_text_extractor(&keys)
        }
        None => {
            let headless = headless();
            let result = extract(args.extract, socket).await;

            if headless && let Err(error) = &result {
                notify("Text Extractor failed", &format!("{error:#}"));
            }

            result
        }
    }
}

/// Whether the overlay's silent screenshot will work: always without the portal (wlroots,
/// X11), and on GNOME once `aujar snip allow` was run. Otherwise the desktop's own selection is
/// used, which needs no permission.
fn overlay_can_capture() -> bool {
    use aujar_text_extractor::{Backend, capture, permission};

    let backend = aujar_config::Settings::load_or_default()
        .text_extractor
        .backend
        .parse::<Backend>()
        .unwrap_or_default();

    if !matches!(
        capture::resolve_full(backend, &capture::Session::detect()),
        Ok(Backend::Portal)
    ) {
        return true;
    }

    match permission::status() {
        Ok(Some(true)) => true,
        // No GNOME permission store (KDE, ...): try the overlay.
        Err(_) => true,
        Ok(_) => {
            eprintln!(
                "Using the desktop's selection. For Aujar's own overlay, allow screenshots once: \
                 aujar snip allow"
            );
            false
        }
    }
}

async fn extract(args: ExtractArgs, socket: PathBuf) -> Result<()> {
    let (text, languages) = match &args.file {
        Some(path) => {
            let settings = aujar_config::Settings::load_or_default().text_extractor;
            let options = aujar_text_extractor::Options {
                languages: if args.languages.is_empty() {
                    settings.languages
                } else {
                    args.languages.clone()
                },
                single_line: args.single_line || settings.single_line,
                backend: aujar_text_extractor::Backend::Auto,
                engine: settings.engine.parse().unwrap_or_default(),
            };
            let path = path.clone();
            let extracted = tokio::task::spawn_blocking(move || {
                aujar_text_extractor::extract_from_file(&path, &options)
            })
            .await??;

            (extracted.text, extracted.languages)
        }
        None => {
            // Plain `aujar text` (the shortcut): Aujar's own overlay, when the UI is running.
            // The UI copies the text and reports it. Options still go through the daemon.
            let plain = args.languages.is_empty() && !args.single_line && !args.no_copy;

            if plain
                && aujar_config::Settings::load_or_default()
                    .snip
                    .text_extractor
                && overlay_can_capture()
            {
                let overlay = send(
                    &socket,
                    Request::Launcher(aujar_ipc::LauncherCommand::SnipText),
                )
                .await;

                if matches!(overlay, Ok(Response::Delivered { subscribers }) if subscribers > 0) {
                    eprintln!("Select an area; its text is copied.");
                    return Ok(());
                }
            }

            let request = Request::ExtractText(ExtractTextRequest {
                languages: args.languages.clone(),
                single_line: args.single_line,
            });

            match send(&socket, request).await? {
                Response::Text(response) if response.cancelled => {
                    eprintln!("Cancelled.");
                    return Ok(());
                }
                Response::Text(response) => (response.text, response.languages),
                Response::Error(message) => bail!(message),
                other => bail!("unexpected answer from the daemon: {other:?}"),
            }
        }
    };

    if text.is_empty() {
        let message = format!(
            "No text found (languages: {}). Try a larger area, or another language with --lang.",
            languages.join("+")
        );

        if headless() {
            notify("No text found", &message);
        }

        bail!(message);
    }

    println!("{text}");

    if !args.no_copy {
        match crate::copy_to_clipboard(&text) {
            Ok(()) if headless() => notify("Text copied", &preview(&text, 120)),
            Ok(()) => eprintln!("Copied to the clipboard."),
            Err(error) => {
                if headless() {
                    notify("Text Extractor: could not copy", &error);
                }

                eprintln!("Could not copy to the clipboard: {error}");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_are_one_short_line() {
        assert_eq!(preview("a\n\nb   c", 10), "a b c");
        assert_eq!(preview("one two three four", 9), "one two…");
        assert_eq!(preview("नमस्ते संसार", 6), "नमस्ते…");
    }
}
