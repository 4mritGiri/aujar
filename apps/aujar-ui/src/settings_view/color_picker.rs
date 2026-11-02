//! Color picker and editor.

use super::SettingsView;
use aujar_color::Rgb;
use aujar_ipc::{Request, Response};
use gpui::{AnyElement, ClipboardItem, Context, SharedString, div, prelude::*, px, rgb, rgba};
use std::time::Duration;

impl SettingsView {
    pub(super) fn color_picker_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let color = Rgb::parse_hex(&self.color_hex).unwrap_or(Rgb::new(108, 140, 255));
        let hex = color.to_hex();
        let rgb_text = color.to_rgb_string();
        let hsl_text = color.to_hsl_string();
        let pal = self.palette;
        let picker_settings = self.settings.color_picker.clone();
        let default_format_name = picker_settings.default_format.clone();
        let formats_for_pick = picker_settings.formats.clone();

        let sample = self
            .pill(
                "color-pick-screen".into(),
                if self.color_picking {
                    "Picking…"
                } else {
                    "Pick from screen"
                },
                true,
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if this.color_picking || !this.settings.color_picker.enabled {
                    return;
                }
                let formats_for_pick = formats_for_pick.clone();
                let default_format_name = default_format_name.clone();
                this.color_picking = true;
                this.status = Some("Move to a pixel and select it in the desktop picker".into());
                cx.notify();
                cx.spawn(async move |this, cx| {
                    let result = cx
                        .background_executor()
                        .spawn(async {
                            let runtime = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build();
                            match runtime {
                                Ok(runtime) => runtime.block_on(async {
                                    tokio::time::timeout(
                                        Duration::from_secs(125),
                                        aujar_ipc::send(crate::socket_path(), Request::PickColor),
                                    )
                                    .await
                                    .map_err(|_| "Color picker timed out".to_owned())?
                                    .map_err(|e| e.to_string())
                                }),
                                Err(e) => Err(e.to_string()),
                            }
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.color_picking = false;
                        match result {
                            Ok(Response::Color(color)) => {
                                this.color_hex = color.hex;
                                let picked = this.color_hex.clone();
                                this.remember_color(picked);
                                let selected =
                                    Rgb::parse_hex(&this.color_hex).unwrap_or(Rgb::new(0, 0, 0));
                                let output = formats_for_pick
                                    .iter()
                                    .find(|format| {
                                        format.enabled
                                            && format
                                                .name
                                                .eq_ignore_ascii_case(&default_format_name)
                                    })
                                    .map(|format| format_color(format, selected))
                                    .unwrap_or_else(|| this.color_hex.clone());
                                cx.write_to_clipboard(ClipboardItem::new_string(output));
                                this.status = Some("Color added to recent colors".into());
                            }
                            Ok(Response::Error(message)) => this.status = Some(message),
                            Ok(other) => {
                                this.status = Some(format!("Unexpected response: {other:?}"))
                            }
                            Err(error) => {
                                this.status = Some(format!("Could not pick color: {error}"))
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }));

        let preview = div()
            .flex()
            .items_center()
            .gap_4()
            .px_5()
            .py_4()
            .child(
                div()
                    .w(px(72.))
                    .h(px(72.))
                    .rounded_lg()
                    .border_1()
                    .border_color(rgba(pal.border))
                    .bg(rgb((u32::from(color.r) << 16)
                        | (u32::from(color.g) << 8)
                        | u32::from(color.b))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(hex.clone()),
                    )
                    .child(div().text_sm().text_color(rgb(pal.text_dim)).child(
                        if picker_settings.show_color_name {
                            color_name(color)
                        } else {
                            "Selected color"
                        },
                    )),
            );

        let copy_row = |this: &Self, label: &'static str, text: String, cx: &mut Context<Self>| {
            let copy_text = text.clone();
            let control = this
                .pill(SharedString::from(format!("copy-{label}")), "Copy", false)
                .on_click(cx.listener(move |_this, _event, _window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                }));
            this.row(label, &text, control)
        };

        let mut history = Vec::new();
        for (index, item) in self.color_history.iter().enumerate() {
            if let Some(c) = Rgb::parse_hex(item) {
                let value = item.clone();
                history.push(
                    div()
                        .id(SharedString::from(format!("color-history-{index}")))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .child(
                            div()
                                .size(px(26.))
                                .rounded_full()
                                .bg(rgb((u32::from(c.r) << 16)
                                    | (u32::from(c.g) << 8)
                                    | u32::from(c.b))),
                        )
                        .child(div().text_sm().child(item.clone()))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.color_hex = value.clone();
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        let history_controls = self
            .pill("clear-color-history".into(), "Clear history", false)
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.color_history.clear();
                this.color_hex = "#6C8CFF".to_owned();
                let _ = std::fs::remove_file(super::color_history_path());
                this.status = Some("Color history cleared".to_owned());
                cx.notify();
            }));

        let mut rows = vec![self.row(
            "Screen eyedropper",
            "Uses your desktop's secure color picker",
            sample,
        )];
        let edit_hex = self
            .pill("edit-color-hex".into(), &hex, false)
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.start_edit(super::EditTarget::ColorHex, this.color_hex.clone(), cx)
            }));
        let copy_hex_text = hex.clone();
        let copy_hex = self
            .pill("copy-hex".into(), "Copy", false)
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_hex_text.clone()));
            }));
        rows.push(
            self.row(
                "HEX",
                "Click the value to edit it",
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(edit_hex)
                    .child(copy_hex),
            ),
        );
        if self
            .editing
            .as_ref()
            .is_some_and(|e| e.target == super::EditTarget::ColorHex)
        {
            rows.extend(self.editor_row("Enter a HEX color such as #FF8800 or #F80", false, cx));
        }
        rows.push(copy_row(self, "RGB", rgb_text, cx));
        rows.push(copy_row(self, "HSL", hsl_text, cx));

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
                format_rows.extend(self.editor_row("Format: name | pattern. Tokens: {hex}, {r}, {g}, {b}, {h}, {s}, {l}, {v}, {c}, {m}, {y}, {k}", false, cx));
            }
        }
        let add_format = self
            .pill("format-add".into(), "Add new format", true)
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.start_edit(
                    super::EditTarget::ColorFormat(None),
                    "New format | {hex}".into(),
                    cx,
                );
            }));
        format_rows.insert(
            0,
            self.row(
                "Color formats",
                "Enable, edit, remove, or reorder formats",
                add_format,
            ),
        );
        if self
            .editing
            .as_ref()
            .is_some_and(|edit| edit.target == super::EditTarget::ColorFormat(None))
        {
            format_rows.extend(self.editor_row("Format: name | pattern. Tokens: {hex}, {r}, {g}, {b}, {h}, {s}, {l}, {v}, {c}, {m}, {y}, {k}", false, cx));
        }

        let history = if history.is_empty() {
            vec![
                div()
                    .px_4()
                    .py_3()
                    .text_sm()
                    .text_color(rgb(pal.text_dim))
                    .child("Picked colors will appear here.")
                    .into_any_element(),
            ]
        } else {
            history
        };
        vec![
            self.section("Shortcut", settings_rows),
            self.section("Color formats", format_rows),
            self.section("Selected color", vec![preview.into_any_element()]),
            self.section("Color values", rows),
            self.section(
                "Recent colors",
                vec![
                    self.row(
                        "History",
                        "Up to 20 colors, saved on this device",
                        history_controls,
                    ),
                    self.card(history).into_any_element(),
                ],
            ),
        ]
    }

    pub(super) fn remember_color(&mut self, value: String) {
        self.color_history
            .retain(|entry| !entry.eq_ignore_ascii_case(&value));
        self.color_history.insert(0, value);
        self.color_history.truncate(20);
        let contents = self.color_history.join("\n");
        let path = super::color_history_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, contents);
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

fn color_name(color: Rgb) -> &'static str {
    let (h, s, l) = color.to_hsl();
    if s < 8 {
        return if l < 18 {
            "Black"
        } else if l > 88 {
            "White"
        } else {
            "Gray"
        };
    }
    let hue_name = match h {
        15..=44 => "Orange",
        45..=74 => "Yellow",
        75..=104 => "Lime",
        105..=149 => "Green",
        150..=179 => "Mint",
        180..=204 => "Cyan",
        205..=224 => "Azure",
        225..=254 => "Blue",
        255..=284 => "Violet",
        285..=314 => "Magenta",
        315..=344 => "Rose",
        _ => "Red",
    };
    if l < 24 {
        match hue_name {
            "Orange" | "Yellow" | "Lime" => "Dark Brown",
            "Green" | "Mint" => "Dark Green",
            "Cyan" | "Azure" => "Dark Teal",
            "Blue" => "Dark Blue",
            "Violet" | "Magenta" => "Dark Purple",
            "Rose" | "Red" => "Dark Red",
            _ => "Dark Gray",
        }
    } else if l > 82 {
        match hue_name {
            "Orange" | "Yellow" => "Light Yellow",
            "Lime" | "Green" => "Light Green",
            "Mint" | "Cyan" => "Light Cyan",
            "Azure" | "Blue" => "Light Blue",
            "Violet" | "Magenta" => "Light Purple",
            "Rose" | "Red" => "Light Pink",
            _ => "Light Gray",
        }
    } else {
        hue_name
    }
}
