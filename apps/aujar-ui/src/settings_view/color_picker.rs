//! Color picker and editor.

use super::SettingsView;
use aujar_color::Rgb;
use gpui::{AnyElement, ClipboardItem, Context, SharedString, div, prelude::*, rgba};

impl SettingsView {
    pub(super) fn color_picker_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let color = Rgb::parse_hex(&self.color_hex).unwrap_or(Rgb::new(108, 140, 255));
        let pal = self.palette;
        let picker_settings = self.settings.color_picker.clone();

        let sample = self
            .pill("color-pick-screen".into(), "Pick from screen", true)
            .on_click(cx.listener(|this, _event, _window, _cx| {
                if this.settings.color_picker.enabled {
                    crate::color_picker::send_to_ui(crate::UiMessage::ColorPick);
                }
            }));
        let editor = self
            .pill("color-open-editor".into(), "Open editor", false)
            .on_click(cx.listener(|this, _event, _window, _cx| {
                if this.settings.color_picker.enabled {
                    crate::color_picker::send_to_ui(crate::UiMessage::ColorEditor);
                }
            }));
        let clear_history = self
            .pill("clear-color-history".into(), "Clear history", false)
            .on_click(cx.listener(|this, _event, _window, cx| {
                let _ = std::fs::remove_file(crate::color_picker::history_path());
                this.status = Some("Color history cleared".to_owned());
                cx.notify();
            }));

        let activation = self.choices(
            aujar_config::ColorPickerActivation::ALL
                .into_iter()
                .map(|behavior| {
                    self.pill(
                        SharedString::from(format!("picker-behavior-{behavior:?}")),
                        behavior.label(),
                        picker_settings.activation_behavior == behavior,
                    )
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.mutate(cx, |settings| {
                            settings.color_picker.activation_behavior = behavior
                        });
                    }))
                })
                .collect(),
        );
        let enable_picker = self
            .switch("picker-enabled", picker_settings.enabled)
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.mutate(cx, |settings| {
                    settings.color_picker.enabled = !settings.color_picker.enabled
                });
            }));
        let show_name = self
            .switch("picker-show-name", picker_settings.show_color_name)
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.mutate(cx, |settings| {
                    settings.color_picker.show_color_name = !settings.color_picker.show_color_name
                });
            }));
        let shortcut_text = picker_settings.shortcut.clone();
        let shortcut_label = shortcut_text.replace('+', " + ");
        let shortcut = self
            .pill("assign-color-shortcut".into(), "Assign shortcut", false)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.status = Some(match install_gnome_color_shortcut(&shortcut_text) {
                    Ok(()) => format!(
                        "{} now runs `aujar color pick`",
                        shortcut_text.replace('+', " + ")
                    ),
                    Err(error) => error,
                });
                cx.notify();
            }));
        let mut settings_rows = vec![
            self.row(
                "Enable Color Picker",
                "Screen eyedropper and color editor",
                enable_picker,
            ),
            self.row(
                "Activation shortcut",
                "Uses GNOME custom shortcuts; change it in Keyboard Settings",
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
                            .child(shortcut_label),
                    )
                    .child(shortcut),
            ),
            self.row(
                "Activation behavior",
                "What happens when you start the picker",
                activation,
            ),
        ];
        let picker_rows = vec![
            self.row(
                "Pick from screen",
                "Uses your desktop's color picker, then opens the editor",
                sample,
            ),
            self.row(
                "Color editor",
                "History, every format, fine-tuning and export",
                editor,
            ),
            self.row(
                "History",
                "Up to 20 colors, saved on this device",
                clear_history,
            ),
        ];

        let formats_available = self.choices(
            picker_settings
                .formats
                .iter()
                .filter(|format| format.enabled)
                .map(|format| {
                    let selected = picker_settings
                        .default_format
                        .eq_ignore_ascii_case(&format.name);
                    let name = format.name.clone();
                    self.pill(
                        SharedString::from(format!("default-format-{}", format.name)),
                        &format.name,
                        selected,
                    )
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.mutate(cx, |settings| {
                            settings.color_picker.default_format = name.clone()
                        });
                    }))
                })
                .collect(),
        );
        settings_rows.push(self.row(
            "Default color format",
            "Format copied when you pick a color",
            formats_available,
        ));
        settings_rows.push(self.row(
            "Show color name",
            "Show a readable color name in the picker",
            show_name,
        ));

        let mut format_rows = Vec::new();
        for (index, format) in picker_settings.formats.iter().enumerate() {
            let preview_value = format_color(format, color);
            let enabled = self
                .switch(
                    SharedString::from(format!("format-enabled-{index}")),
                    format.enabled,
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.mutate(cx, |settings| {
                        if let Some(format) = settings.color_picker.formats.get_mut(index) {
                            format.enabled = !format.enabled;
                        }
                    });
                }));
            let up = self
                .pill(SharedString::from(format!("format-up-{index}")), "↑", false)
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.mutate(cx, |settings| {
                        if index > 0 {
                            settings.color_picker.formats.swap(index, index - 1);
                        }
                    });
                }));
            let down = self
                .pill(
                    SharedString::from(format!("format-down-{index}")),
                    "↓",
                    false,
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.mutate(cx, |settings| {
                        if index + 1 < settings.color_picker.formats.len() {
                            settings.color_picker.formats.swap(index, index + 1);
                        }
                    });
                }));
            let edit_line = format!("{} | {}", format.name, format.pattern);
            let edit = self
                .pill(
                    SharedString::from(format!("format-edit-{index}")),
                    "Edit",
                    false,
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.start_edit(
                        super::EditTarget::ColorFormat(Some(index)),
                        edit_line.clone(),
                        cx,
                    );
                }));
            let copy_value = preview_value.clone();
            let copy = self
                .pill(
                    SharedString::from(format!("format-copy-{index}")),
                    "Copy",
                    false,
                )
                .on_click(cx.listener(move |_this, _event, _window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_value.clone()));
                }));
            let remove = self
                .pill(
                    SharedString::from(format!("format-remove-{index}")),
                    "Delete",
                    false,
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.mutate(cx, |settings| {
                        if index < settings.color_picker.formats.len() {
                            let removed = settings.color_picker.formats.remove(index);
                            if settings
                                .color_picker
                                .default_format
                                .eq_ignore_ascii_case(&removed.name)
                            {
                                settings.color_picker.default_format = settings
                                    .color_picker
                                    .formats
                                    .iter()
                                    .find(|f| f.enabled)
                                    .map(|f| f.name.clone())
                                    .unwrap_or_else(|| "HEX".into());
                            }
                        }
                    });
                }));
            format_rows.push(
                self.row(
                    &format.name,
                    &preview_value,
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(enabled)
                        .child(up)
                        .child(down)
                        .child(edit)
                        .child(copy)
                        .child(remove),
                ),
            );
            if self
                .editing
                .as_ref()
                .is_some_and(|edit| edit.target == super::EditTarget::ColorFormat(Some(index)))
            {
                format_rows.extend(self.editor_row("Format: name | pattern. Tokens: %Re %Gr %Bl (add X for hex: %ReX), %Hu %Sl %Ll %Sb %Va %Cy %Ma %Ye %Bk %Wh %Bn %Na ...", false, cx));
            }
        }
        let add_format = self
            .pill("format-add".into(), "Add new format", true)
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.start_edit(
                    super::EditTarget::ColorFormat(None),
                    "New format | %ReX%GrX%BlX".into(),
                    cx,
                );
            }));
        format_rows.insert(
            0,
            self.row(
                "Color formats",
                "Enable, edit, remove or reorder. Patterns use PowerToys tokens such as %ReX%GrX%BlX",
                add_format,
            ),
        );
        if self
            .editing
            .as_ref()
            .is_some_and(|edit| edit.target == super::EditTarget::ColorFormat(None))
        {
            format_rows.extend(self.editor_row("Format: name | pattern. Tokens: %Re %Gr %Bl (add X for hex: %ReX), %Hu %Sl %Ll %Sb %Va %Cy %Ma %Ye %Bk %Wh %Bn %Na ...", false, cx));
        }

        vec![
            self.section("Shortcut", settings_rows),
            self.section("Picker", picker_rows),
            self.section("Color formats", format_rows),
        ]
    }
}

fn install_gnome_color_shortcut(shortcut: &str) -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not find Aujar's install folder: {error}"))?
        .with_file_name("aujar");
    if !executable.is_file() {
        return Err(format!(
            "Aujar command not found beside the UI: {}",
            executable.display()
        ));
    }
    let output = std::process::Command::new(executable)
        .args(["color", "shortcut-install", "--keys", shortcut])
        .output()
        .map_err(|error| format!("Could not run shortcut installer: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if message.is_empty() {
            "Shortcut installation failed".into()
        } else {
            message
        })
    }
}

fn format_color(format: &aujar_config::ColorFormat, color: Rgb) -> String {
    color.format(&format.pattern)
}
