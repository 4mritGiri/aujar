//! Text Extractor: Tesseract's status and languages, which languages to recognise, how an area
//! is selected, and the keyboard shortcut (docs/TEXT_EXTRACTOR.md).
//!
//! Tesseract runs on this computer, so its status is read here directly (off the UI thread),
//! without the daemon. Extracting goes through `aujar text`, like the shortcut does.

use aujar_text_extractor::{Backend, ocr, permission};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px, rgb, rgba};
use std::path::PathBuf;

use super::SettingsView;

#[derive(Default)]
pub(super) struct State {
    status: Option<Result<ocr::Status, String>>,
    /// Whether GNOME lets Aujar take the snip overlay's screenshot without asking.
    allowed: Option<Result<Option<bool>, String>>,
    checking: bool,
}

/// What each way of selecting an area means, for the page.
fn backend_hint(backend: Backend) -> &'static str {
    match backend {
        Backend::Auto => {
            "Pick from your desktop: grim on Sway/Hyprland, maim or scrot on X11, else the desktop's screenshot dialog"
        }
        Backend::Portal => "The desktop's screenshot dialog (GNOME, KDE)",
        Backend::Grim => "slurp to drag an area, grim to capture it (Sway, Hyprland)",
        Backend::Maim => "maim's crosshair (X11)",
        Backend::Scrot => "scrot's crosshair (X11)",
    }
}

/// The `aujar` command: beside this program, or on the PATH.
fn aujar_command() -> PathBuf {
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name("aujar"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("aujar"))
}

fn install_shortcut(keys: &str) -> Result<(), String> {
    let output = std::process::Command::new(aujar_command())
        .args(["text", "shortcut-install", "--keys", keys])
        .output()
        .map_err(|error| format!("Could not run `aujar`: {error}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();

        Err(if message.is_empty() {
            "The shortcut could not be installed".to_owned()
        } else {
            message
        })
    }
}

impl SettingsView {
    pub(super) fn refresh_text_extractor(&mut self, cx: &mut Context<Self>) {
        if self.text_extractor.checking {
            return;
        }

        self.text_extractor.checking = true;

        cx.spawn(async move |this, cx| {
            let (status, allowed) = cx
                .background_executor()
                .spawn(async {
                    (
                        ocr::status().map_err(|error| format!("{error:#}")),
                        permission::status().map_err(|error| format!("{error:#}")),
                    )
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.text_extractor.checking = false;
                this.text_extractor.status = Some(status);
                this.text_extractor.allowed = Some(allowed);
                cx.notify();
            });
        })
        .detach();
    }

    /// Grant or remove the screenshot permission, then show the new state. Only on a click.
    fn set_screenshot_permission(&mut self, allow: bool, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { permission::set(allow).map_err(|error| format!("{error:#}")) })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.status = Some(match result {
                    Ok(()) if allow => "Aujar may now take screenshots without asking".to_owned(),
                    Ok(()) => "Aujar must ask again before taking screenshots".to_owned(),
                    Err(error) => error,
                });
                this.refresh_text_extractor(cx);
            });
        })
        .detach();
    }

    fn toggle_language(&mut self, code: String, cx: &mut Context<Self>) {
        let chosen = &self.settings.text_extractor.languages;

        if chosen.len() == 1 && chosen[0] == code {
            self.status = Some("Keep at least one language.".to_owned());
            cx.notify();
            return;
        }

        self.mutate(cx, move |settings| {
            let languages = &mut settings.text_extractor.languages;

            if let Some(position) = languages.iter().position(|chosen| *chosen == code) {
                languages.remove(position);
            } else {
                languages.push(code);
            }
        });
    }

    pub(super) fn text_extractor_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let pal = self.palette;
        let settings = &self.settings.text_extractor;

        let recheck = self
            .pill("ocr-recheck".into(), "Check again", false)
            .on_click(cx.listener(|this, _event, _window, cx| this.refresh_text_extractor(cx)));

        let (engine_label, engine_hint, installed) = match &self.text_extractor.status {
            None => ("Checking Tesseract\u{2026}".to_owned(), String::new(), None),
            Some(Ok(status)) => (
                status.version.clone(),
                "Recognises text on this computer; nothing is sent anywhere".to_owned(),
                Some(status.languages.clone()),
            ),
            Some(Err(error)) => ("Tesseract is not available".to_owned(), error.clone(), None),
        };

        let mut engine = vec![self.row(&engine_label, &engine_hint, recheck)];

        // Languages: a pill per installed language; the chosen ones are highlighted.
        let mut language_rows = Vec::new();

        match &installed {
            Some(installed) if !installed.is_empty() => {
                let pills = installed.iter().map(|code| {
                    let on = settings.languages.contains(code);
                    let code = code.clone();

                    self.pill(SharedString::from(format!("ocr-lang-{code}")), &code, on)
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.toggle_language(code.clone(), cx);
                        }))
                });

                language_rows.push(
                    self.row(
                        "Languages",
                        "Recognise these. More is slower and can mix up similar scripts",
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_2()
                            .max_w(px(360.))
                            .children(pills),
                    ),
                );
            }
            Some(_) => engine.push(self.row(
                "No languages installed",
                "Install one, e.g. `sudo apt install tesseract-ocr-eng`",
                div(),
            )),
            None => language_rows.push(self.row(
                "Languages",
                &format!("{} (from settings.toml)", settings.languages.join(", ")),
                div(),
            )),
        }

        if let Some(installed) = &installed {
            let missing: Vec<&str> = settings
                .languages
                .iter()
                .filter(|code| !installed.contains(code))
                .map(String::as_str)
                .collect();

            if !missing.is_empty() {
                language_rows.push(self.row(
                    "Not installed",
                    &format!(
                        "{} is configured but Tesseract does not have it. Debian/Ubuntu: `sudo apt install {}`",
                        missing.join(", "),
                        missing
                            .iter()
                            .map(|code| format!("tesseract-ocr-{}", code.replace('_', "-").to_lowercase()))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ),
                    div().text_color(rgb(pal.danger)).child("!"),
                ));
            }
        }

        language_rows.push(toggle_row!(
            self,
            cx,
            "ocr-single-line",
            "Join into one line",
            "Wrapped lines become one; words split by a hyphen at a line end are joined",
            text_extractor.single_line
        ));

        // How the area is selected.
        let current: Backend = settings.backend.parse().unwrap_or_default();
        let backends = Backend::ALL.into_iter().map(|backend| {
            self.pill(
                SharedString::from(format!("ocr-backend-{}", backend.as_str())),
                backend.as_str(),
                backend == current,
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.mutate(cx, |settings| {
                    settings.text_extractor.backend = backend.as_str().to_owned()
                });
            }))
        });

        let selection = vec![
            self.row(
                "Area selection",
                backend_hint(current),
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .children(backends),
            ),
        ];

        // Shortcut and a way to try it.
        let keys = settings.shortcut.clone();
        let assign = self
            .pill("ocr-shortcut".into(), "Assign shortcut", false)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.status = Some(match install_shortcut(&keys) {
                    Ok(()) => format!("{} now runs `aujar text`", keys.replace('+', " + ")),
                    Err(error) => error,
                });
                cx.notify();
            }));
        let try_now = self
            .pill("ocr-try".into(), "Extract text now", true)
            .on_click(cx.listener(|this, _event, _window, cx| {
                // Detached: the result arrives as a notification, like from the shortcut.
                this.status = Some(
                    match std::process::Command::new(aujar_command())
                        .arg("text")
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                    {
                        Ok(_) => "Select an area; the text is copied and shown in a notification"
                            .to_owned(),
                        Err(error) => format!("Could not run `aujar text`: {error}"),
                    },
                );
                cx.notify();
            }));

        let allowed = match &self.text_extractor.allowed {
            Some(Ok(Some(true))) => Some(true),
            Some(Ok(_)) => Some(false),
            // Not GNOME, or no permission store: nothing to set.
            Some(Err(_)) | None => None,
        };
        let permission_row = allowed.map(|allowed| {
            let button = self
                .pill(
                    "snip-allow".into(),
                    if allowed { "Revoke" } else { "Allow" },
                    !allowed,
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.set_screenshot_permission(!allowed, cx);
                }));

            self.row(
                "Screenshots without asking",
                if allowed {
                    "Allowed: the snip overlay can freeze the screen"
                } else {
                    "Needed by the snip overlay (Super+Shift+S, and T here). GNOME's own dialog cannot open for the background daemon"
                },
                button,
            )
        });

        let mut shortcut = vec![];
        shortcut.extend(permission_row);
        shortcut.extend([
            self.row(
                "Activation shortcut",
                "Uses GNOME custom shortcuts; on other desktops bind `aujar text` yourself",
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgba(pal.hover))
                            .text_sm()
                            .child(settings.shortcut.replace('+', " + ")),
                    )
                    .child(assign),
            ),
            self.row(
                "Try it",
                "Drag over some text on screen; it lands on the clipboard",
                try_now,
            ),
        ]);

        vec![
            self.section("Tesseract", engine),
            self.section("Recognition", language_rows),
            self.section("Selecting an area", selection),
            self.section("Shortcut", shortcut),
        ]
    }
}
