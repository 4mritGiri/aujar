//! The color editor, laid out like PowerToys': a color bar on top (the color in the middle, a
//! lighter shade strip along its top edge and a darker one along its bottom edge), the history of
//! picked colors on the left and the selected color in every format on the right. Click a format
//! to copy it, click the color to fine-tune it, right-click a history color to remove it or
//! export the history.

use crate::{
    UiMessage,
    color_picker::{HISTORY_LIMIT, load_history, remember, save_history, send_to_ui},
    theme::Palette,
};
use aujar_color::Rgb;
use aujar_config::{ColorFormat, Settings};
use gpui::{
    App, Bounds, ClipboardItem, Context, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, TitlebarOptions, Window,
    WindowBounds, WindowHandle, WindowOptions, canvas, div, linear_color_stop, linear_gradient,
    prelude::*, px, relative, rgb, rgba, size,
};
use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Duration};

/// How many lighter and how many darker shades the color bar shows.
const SHADES: usize = 6;

const SLIDERS: usize = 6;

/// A color with the hue, saturation and value the sliders work in. They are kept apart from the
/// color because a gray has no hue: dragging the saturation up must start from the hue you had.
#[derive(Clone, Copy)]
struct Tuning {
    color: Rgb,
    hue: f64,
    saturation: f64,
    value: f64,
}

impl Tuning {
    fn new(color: Rgb) -> Self {
        let (hue, saturation, value) = color.to_hsv();

        Self {
            color,
            hue,
            saturation,
            value,
        }
    }

    fn set_hsv(&mut self, hue: f64, saturation: f64, value: f64) {
        self.hue = hue;
        self.saturation = saturation;
        self.value = value;
        self.color = Rgb::from_hsv(hue, saturation, value);
    }

    fn set_rgb(&mut self, color: Rgb) {
        let (hue, saturation, value) = color.to_hsv();

        self.color = color;
        self.value = value;

        // A gray or black has no hue or saturation of its own: keep the ones the sliders had.
        if saturation > 0.0 && value > 0.0 {
            self.hue = hue;
        }

        if value > 0.0 {
            self.saturation = saturation;
        }
    }

    /// Slider positions, `0.0..=1.0`: hue, saturation, value, red, green, blue.
    fn positions(&self) -> [f32; SLIDERS] {
        [
            (self.hue / 360.0) as f32,
            self.saturation as f32,
            self.value as f32,
            f32::from(self.color.r) / 255.0,
            f32::from(self.color.g) / 255.0,
            f32::from(self.color.b) / 255.0,
        ]
    }

    fn set_slider(&mut self, which: usize, fraction: f32) {
        let fraction = f64::from(fraction.clamp(0.0, 1.0));
        let byte = (fraction * 255.0).round() as u8;
        let Rgb { r, g, b } = self.color;

        match which {
            0 => self.set_hsv(fraction * 360.0, self.saturation, self.value),
            1 => self.set_hsv(self.hue, fraction, self.value),
            2 => self.set_hsv(self.hue, self.saturation, fraction),
            3 => self.set_rgb(Rgb::new(byte, g, b)),
            4 => self.set_rgb(Rgb::new(r, byte, b)),
            _ => self.set_rgb(Rgb::new(r, g, byte)),
        }
    }
}

struct TileMenu {
    index: usize,
    at: Point<Pixels>,
}

pub(crate) struct ColorEditorView {
    settings: Settings,
    palette: Palette,
    history: Vec<Rgb>,
    selected: usize,
    focus_handle: FocusHandle,
    /// The format whose value was just copied.
    copied: Option<String>,
    menu: Option<TileMenu>,
    tuning: Option<Tuning>,
    /// Hex digits typed in the fine-tune panel (`None` while showing the color's own).
    hex_text: Option<String>,
    /// Where each slider's track was drawn last, to turn the pointer position into a value.
    tracks: Rc<RefCell<[Bounds<Pixels>; SLIDERS]>>,
    dragging: Option<usize>,
    status: Option<String>,
}

fn contrast(color: Rgb) -> u32 {
    let luminance =
        0.299 * f32::from(color.r) + 0.587 * f32::from(color.g) + 0.114 * f32::from(color.b);

    if luminance > 150.0 {
        0x101114
    } else {
        0xffffff
    }
}

fn enabled_formats(settings: &Settings) -> Vec<ColorFormat> {
    settings
        .color_picker
        .formats
        .iter()
        .filter(|format| format.enabled)
        .cloned()
        .collect()
}

impl ColorEditorView {
    fn new(cx: &mut App) -> Self {
        let settings = Settings::load_or_default();

        Self {
            palette: Palette::from_settings(&settings),
            settings,
            history: load_history(),
            selected: 0,
            focus_handle: cx.focus_handle(),
            copied: None,
            menu: None,
            tuning: None,
            hex_text: None,
            tracks: Rc::new(RefCell::new([Bounds::default(); SLIDERS])),
            dragging: None,
            status: None,
        }
    }

    fn current(&self) -> Option<Rgb> {
        self.history.get(self.selected).copied()
    }

    /// Make a color the newest in the history and the selected one.
    fn choose(&mut self, color: Rgb, cx: &mut Context<Self>) {
        self.history = remember(color);
        self.selected = 0;
        cx.notify();
    }

    fn copy(&mut self, format: &ColorFormat, color: Rgb, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(color.format(&format.pattern)));
        self.copied = Some(format.name.clone());
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1400))
                .await;

            let _ = this.update(cx, |this, cx| {
                this.copied = None;
                cx.notify();
            });
        })
        .detach();
    }

    fn remove(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.history.len() {
            self.history.remove(index);
            save_history(&self.history);
            self.selected = self.selected.min(self.history.len().saturating_sub(1));
        }

        cx.notify();
    }

    fn export(&mut self, by_color: bool, cx: &mut Context<Self>) {
        let formats = enabled_formats(&self.settings);
        let text = export_text(&self.history, &formats, by_color);
        let name = if by_color {
            "aujar-colors-by-color.txt"
        } else {
            "aujar-colors-by-format.txt"
        };
        let directory = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
        let chosen = cx.prompt_for_new_path(&directory, Some(name));

        cx.spawn(async move |this, cx| {
            let path = match chosen.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                // No file dialog on this desktop: save next to the home folder.
                _ => directory.join(name),
            };
            let result = std::fs::write(&path, text);

            let _ = this.update(cx, |this, cx| {
                this.status = Some(match result {
                    Ok(()) => format!("Exported to {}", path.display()),
                    Err(error) => format!("Could not export: {error}"),
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn open_tuning(&mut self, cx: &mut Context<Self>) {
        if let Some(color) = self.current() {
            self.tuning = Some(Tuning::new(color));
            self.hex_text = None;
            cx.notify();
        }
    }

    fn apply_tuning(&mut self, cx: &mut Context<Self>) {
        if let Some(tuning) = self.tuning.take() {
            self.hex_text = None;
            self.choose(tuning.color, cx);
        }
    }

    fn set_slider(&mut self, which: usize, fraction: f32, cx: &mut Context<Self>) {
        if let Some(tuning) = self.tuning.as_mut() {
            tuning.set_slider(which, fraction);
            self.hex_text = None;
            cx.notify();
        }
    }

    fn fraction(&self, which: usize, x: Pixels) -> f32 {
        let track = self.tracks.borrow()[which];
        let width = f32::from(track.size.width).max(1.0);

        ((f32::from(x) - f32::from(track.origin.x)) / width).clamp(0.0, 1.0)
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let key = keystroke.key.as_str();

        if key == "escape" {
            if self.menu.take().is_some() || self.tuning.take().is_some() {
                self.hex_text = None;
                cx.notify();
            } else {
                window.remove_window();
            }

            return;
        }

        let Some(tuning) = self.tuning else {
            return;
        };

        let typed = self
            .hex_text
            .clone()
            .unwrap_or_else(|| tuning.color.to_hex()[1..].to_owned());
        let paste = keystroke.modifiers.control && key == "v";

        let next = if key == "backspace" {
            let mut text = typed;

            text.pop();
            text
        } else if paste {
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .map(|text| text.trim().trim_start_matches('#').to_owned())
                .unwrap_or_default()
        } else if key == "enter" {
            self.apply_tuning(cx);
            return;
        } else if let Some(character) = keystroke
            .key_char
            .as_deref()
            .filter(|text| text.len() == 1 && !keystroke.modifiers.control)
            .filter(|text| text.chars().all(|c| c.is_ascii_hexdigit()))
        {
            let mut text = if self.hex_text.is_some() {
                typed
            } else {
                String::new()
            };

            text.push_str(character);
            text
        } else {
            return;
        };

        let next: String = next
            .chars()
            .filter(char::is_ascii_hexdigit)
            .take(6)
            .collect();

        if let Some(color) = (next.len() == 6 || next.len() == 3)
            .then(|| Rgb::parse_hex(&next))
            .flatten()
            && let Some(tuning) = self.tuning.as_mut()
        {
            tuning.set_rgb(color);
        }

        self.hex_text = Some(next);
        cx.notify();
    }

    // ---- Pieces ---------------------------------------------------------------------------

    fn color_bar(&self, color: Rgb, cx: &mut Context<Self>) -> gpui::AnyElement {
        let pal = self.palette;
        let (lighter, darker) = color.similar(SHADES);

        let strip = |id: &'static str, shades: Vec<Rgb>, cx: &mut Context<Self>| {
            let mut row = div().flex().flex_row().w_full().h(px(18.0));

            for (index, shade) in shades.into_iter().enumerate() {
                row = row.child(
                    div()
                        .id((id, index))
                        .flex_1()
                        .h_full()
                        .bg(rgb(shade.to_u32()))
                        .cursor_pointer()
                        .hover(|segment| segment.opacity(0.8))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.choose(shade, cx);
                        })),
                );
            }

            row
        };

        div()
            .flex()
            .flex_col()
            .w_full()
            .h(px(112.0))
            .rounded_xl()
            .overflow_hidden()
            .border_1()
            .border_color(rgba(pal.border))
            .child(strip("lighter", lighter, cx))
            .child(
                div()
                    .id("color-bar-main")
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(color.to_u32()))
                    .text_color(rgb(contrast(color)))
                    .cursor_pointer()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(div().text_lg().child(color.to_hex()))
                            .when(self.settings.color_picker.show_color_name, |this| {
                                this.child(div().text_xs().child(color.name()))
                            }),
                    )
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        if this.tuning.is_some() {
                            this.tuning = None;
                            this.hex_text = None;
                            cx.notify();
                        } else {
                            this.open_tuning(cx);
                        }
                    })),
            )
            .child(strip("darker", darker, cx))
            .into_any_element()
    }

    fn history_column(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let pal = self.palette;
        let mut column = div()
            .id("color-history")
            .flex()
            .flex_col()
            .gap_1()
            .w(px(128.0))
            .h_full()
            .overflow_y_scroll();

        for (index, color) in self.history.iter().copied().enumerate() {
            let selected = index == self.selected;

            column = column.child(
                div()
                    .id(("history", index))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w_full()
                    .h(px(38.0))
                    .flex_none()
                    .rounded_lg()
                    .bg(rgb(color.to_u32()))
                    .text_xs()
                    .text_color(rgb(contrast(color)))
                    .border_2()
                    .border_color(if selected {
                        rgb(pal.accent_fg)
                    } else {
                        rgba(0x00000000)
                    })
                    .cursor_pointer()
                    .child(color.to_hex())
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.selected = index;

                        if this.tuning.is_some() {
                            this.tuning = Some(Tuning::new(color));
                            this.hex_text = None;
                        }

                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            this.selected = index;
                            this.menu = Some(TileMenu {
                                index,
                                at: event.position,
                            });
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            );
        }

        column.into_any_element()
    }

    fn formats_panel(&self, color: Rgb, cx: &mut Context<Self>) -> gpui::AnyElement {
        let pal = self.palette;
        let mut list = div()
            .id("color-formats")
            .flex()
            .flex_col()
            .gap_1()
            .flex_1()
            .h_full()
            .overflow_y_scroll();

        for (index, format) in enabled_formats(&self.settings).into_iter().enumerate() {
            let value = color.format(&format.pattern);
            let copied = self.copied.as_deref() == Some(format.name.as_str());
            let copy_format = format.clone();

            list = list.child(
                div()
                    .id(("format", index))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .flex_none()
                    .rounded_lg()
                    .bg(rgba(pal.subtle))
                    .cursor_pointer()
                    .hover(|row| row.bg(rgba(pal.hover)))
                    .child(
                        div()
                            .w(px(64.0))
                            .text_xs()
                            .text_color(rgb(pal.text_dim))
                            .child(format.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(rgb(pal.text))
                            .child(value),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if copied {
                                rgb(pal.positive)
                            } else {
                                rgb(pal.text_faint)
                            })
                            .child(if copied { "Copied" } else { "Copy" }),
                    )
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.copy(&copy_format, color, cx);
                    })),
            );
        }

        list.into_any_element()
    }

    /// One slider: a gradient track with a knob. Pressing on it starts a drag that carries on
    /// anywhere in the panel.
    fn slider(
        &self,
        which: usize,
        label: &'static str,
        shown: String,
        track: gpui::AnyElement,
        position: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let pal = self.palette;
        let tracks = self.tracks.clone();

        div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .w(px(14.0))
                    .text_xs()
                    .text_color(rgb(pal.text_dim))
                    .child(label),
            )
            .child(
                div()
                    .id(("slider", which))
                    .relative()
                    .flex_1()
                    .h(px(16.0))
                    .rounded_full()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            this.dragging = Some(which);
                            let fraction = this.fraction(which, event.position.x);

                            this.set_slider(which, fraction, cx);
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .size_full()
                            .rounded_full()
                            .overflow_hidden()
                            .child(track),
                    )
                    .child(
                        canvas(
                            move |bounds, _window, _cx| tracks.borrow_mut()[which] = bounds,
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(-1.0))
                            .left(relative(position))
                            .ml(px(-9.0))
                            .size(px(18.0))
                            .rounded_full()
                            .border_2()
                            .border_color(rgb(0xffffff))
                            .shadow_sm(),
                    ),
            )
            .child(
                div()
                    .w(px(34.0))
                    .text_xs()
                    .text_color(rgb(pal.text_dim))
                    .child(shown),
            )
            .into_any_element()
    }

    fn tuning_panel(&self, tuning: Tuning, cx: &mut Context<Self>) -> gpui::AnyElement {
        let pal = self.palette;
        let positions = tuning.positions();
        let Rgb { r, g, b } = tuning.color;
        let (hue, saturation, value) = (tuning.hue, tuning.saturation, tuning.value);

        let ramp = |from: Rgb, to: Rgb| -> gpui::AnyElement {
            div()
                .size_full()
                .bg(linear_gradient(
                    90.0,
                    linear_color_stop(rgb(from.to_u32()), 0.0),
                    linear_color_stop(rgb(to.to_u32()), 1.0),
                ))
                .into_any_element()
        };

        // The hue track is six ramps side by side (a gradient only has two stops).
        let hue_track = {
            let mut row = div().flex().size_full();

            for sector in 0..6 {
                let start = f64::from(sector) * 60.0;

                row = row.child(div().flex_1().h_full().bg(linear_gradient(
                    90.0,
                    linear_color_stop(rgb(Rgb::from_hsv(start, 1.0, 1.0).to_u32()), 0.0),
                    linear_color_stop(rgb(Rgb::from_hsv(start + 60.0, 1.0, 1.0).to_u32()), 1.0),
                )));
            }

            row.into_any_element()
        };

        let hex = self
            .hex_text
            .clone()
            .unwrap_or_else(|| tuning.color.to_hex()[1..].to_owned());

        div()
            .id("tune-panel")
            .flex()
            .flex_col()
            .gap_3()
            .flex_1()
            .h_full()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if let Some(which) = this.dragging
                    && event.pressed_button == Some(MouseButton::Left)
                {
                    let fraction = this.fraction(which, event.position.x);

                    this.set_slider(which, fraction, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, _cx| this.dragging = None),
            )
            .child(self.slider(
                0,
                "H",
                format!("{}°", hue.round() as i32),
                hue_track,
                positions[0],
                cx,
            ))
            .child(self.slider(
                1,
                "S",
                format!("{}%", (saturation * 100.0).round() as i32),
                ramp(
                    Rgb::from_hsv(hue, 0.0, value),
                    Rgb::from_hsv(hue, 1.0, value),
                ),
                positions[1],
                cx,
            ))
            .child(self.slider(
                2,
                "V",
                format!("{}%", (value * 100.0).round() as i32),
                ramp(
                    Rgb::from_hsv(hue, saturation, 0.0),
                    Rgb::from_hsv(hue, saturation, 1.0),
                ),
                positions[2],
                cx,
            ))
            .child(self.slider(
                3,
                "R",
                r.to_string(),
                ramp(Rgb::new(0, g, b), Rgb::new(255, g, b)),
                positions[3],
                cx,
            ))
            .child(self.slider(
                4,
                "G",
                g.to_string(),
                ramp(Rgb::new(r, 0, b), Rgb::new(r, 255, b)),
                positions[4],
                cx,
            ))
            .child(self.slider(
                5,
                "B",
                b.to_string(),
                ramp(Rgb::new(r, g, 0), Rgb::new(r, g, 255)),
                positions[5],
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().text_xs().text_color(rgb(pal.text_dim)).child("HEX"))
                    .child(
                        div()
                            .flex_1()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgba(pal.border))
                            .bg(rgba(pal.subtle))
                            .text_sm()
                            .text_color(rgb(pal.text))
                            .child(format!("#{hex}")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("tune-select")
                            .px_4()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(pal.accent_fg))
                            .text_sm()
                            .text_color(rgb(0x101114))
                            .cursor_pointer()
                            .child("Select")
                            .on_click(
                                cx.listener(|this, _event, _window, cx| this.apply_tuning(cx)),
                            ),
                    )
                    .child(
                        div()
                            .id("tune-cancel")
                            .px_4()
                            .py_1()
                            .rounded_md()
                            .bg(rgba(pal.subtle))
                            .text_sm()
                            .text_color(rgb(pal.text))
                            .cursor_pointer()
                            .child("Cancel")
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.tuning = None;
                                this.hex_text = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn context_menu(&self, menu: &TileMenu, cx: &mut Context<Self>) -> gpui::AnyElement {
        let pal = self.palette;
        let index = menu.index;

        let item = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_md()
                .text_sm()
                .text_color(rgb(pal.text))
                .cursor_pointer()
                .hover(|row| row.bg(rgba(pal.hover)))
                .child(label)
        };

        div()
            .absolute()
            .left(menu.at.x)
            .top(menu.at.y)
            .w(px(170.0))
            .p_1()
            .rounded_lg()
            .bg(rgba(pal.menu))
            .border_1()
            .border_color(rgba(pal.border))
            .shadow_md()
            .occlude()
            .child(item("menu-remove", "Remove").on_click(cx.listener(
                move |this, _event, _window, cx| {
                    this.menu = None;
                    this.remove(index, cx);
                },
            )))
            .child(
                item("menu-export-color", "Export, by color").on_click(cx.listener(
                    |this, _event, _window, cx| {
                        this.menu = None;
                        this.export(true, cx);
                    },
                )),
            )
            .child(
                item("menu-export-format", "Export, by format").on_click(cx.listener(
                    |this, _event, _window, cx| {
                        this.menu = None;
                        this.export(false, cx);
                    },
                )),
            )
            .into_any_element()
    }
}

/// The history as text: one block per color with every format, or one block per format with
/// every color.
fn export_text(history: &[Rgb], formats: &[ColorFormat], by_color: bool) -> String {
    let mut text = String::new();

    if by_color {
        for color in history {
            for format in formats {
                text.push_str(&format!(
                    "{}: {}\n",
                    format.name,
                    color.format(&format.pattern)
                ));
            }

            text.push('\n');
        }
    } else {
        for format in formats {
            text.push_str(&format!("{}\n", format.name));

            for color in history {
                text.push_str(&format!("{}\n", color.format(&format.pattern)));
            }

            text.push('\n');
        }
    }

    text
}

impl Focusable for ColorEditorView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ColorEditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let current = self.current();

        let pick = div()
            .id("editor-pick")
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .rounded_lg()
            .bg(rgb(pal.accent_fg))
            .text_sm()
            .text_color(rgb(0x101114))
            .cursor_pointer()
            .child("Pick a color")
            .on_click(cx.listener(|_this, _event, _window, _cx| {
                send_to_ui(UiMessage::ColorPick);
            }));

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .child(pick)
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_faint))
                    .child(format!("{} of {HISTORY_LIMIT} colors", self.history.len())),
            );

        let body = match (current, self.tuning) {
            (None, _) => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(rgb(pal.text_dim))
                .child("No colors yet. Pick one from the screen.")
                .into_any_element(),
            (Some(_), Some(tuning)) => div()
                .flex()
                .flex_1()
                .min_h_0()
                .gap_3()
                .child(self.history_column(cx))
                .child(self.tuning_panel(tuning, cx))
                .into_any_element(),
            (Some(color), None) => div()
                .flex()
                .flex_1()
                .min_h_0()
                .gap_3()
                .child(self.history_column(cx))
                .child(self.formats_panel(color, cx))
                .into_any_element(),
        };

        div()
            .id("color-editor")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    if this.menu.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .relative()
            .flex()
            .flex_col()
            .gap_3()
            .size_full()
            .p_4()
            .bg(rgb(0x14172a))
            .text_color(rgb(pal.text))
            .child(header)
            .when_some(current, |this, color| this.child(self.color_bar(color, cx)))
            .child(body)
            .when_some(self.status.clone(), |this, status| {
                this.child(div().text_xs().text_color(rgb(pal.text_dim)).child(status))
            })
            .when_some(self.menu.as_ref(), |this, menu| {
                this.child(self.context_menu(menu, cx))
            })
    }
}

/// Open the editor, or replace the one that is open (so it shows the current history).
pub(crate) fn open_window(cx: &mut App) -> Option<WindowHandle<ColorEditorView>> {
    let bounds = Bounds::centered(None, size(px(600.0), px(640.0)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Aujar Color Picker".into()),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        window_min_size: Some(size(px(520.0), px(520.0))),
        app_id: Some("aujar".into()),
        ..Default::default()
    };

    cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| ColorEditorView::new(cx));
        let focus_handle = view.read(cx).focus_handle.clone();

        window.focus(&focus_handle, cx);

        view
    })
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formats() -> Vec<ColorFormat> {
        vec![
            ColorFormat {
                name: "HEX".into(),
                pattern: "%ReX%GrX%BlX".into(),
                enabled: true,
            },
            ColorFormat {
                name: "RGB".into(),
                pattern: "rgb(%Re, %Gr, %Bl)".into(),
                enabled: true,
            },
        ]
    }

    #[test]
    fn exports_by_color_and_by_format() {
        let history = [Rgb::new(255, 136, 0), Rgb::new(0, 0, 255)];

        assert_eq!(
            export_text(&history, &formats(), true),
            "HEX: FF8800\nRGB: rgb(255, 136, 0)\n\nHEX: 0000FF\nRGB: rgb(0, 0, 255)\n\n"
        );
        assert_eq!(
            export_text(&history, &formats(), false),
            "HEX\nFF8800\n0000FF\n\nRGB\nrgb(255, 136, 0)\nrgb(0, 0, 255)\n\n"
        );
    }

    #[test]
    fn sliders_keep_the_hue_of_a_gray() {
        let mut tuning = Tuning::new(Rgb::new(255, 0, 0));

        tuning.set_slider(1, 0.0);
        assert_eq!(tuning.color, Rgb::new(255, 255, 255));

        // The hue was red; raising the saturation again brings red back, not a default hue.
        tuning.set_slider(1, 1.0);
        assert_eq!(tuning.color, Rgb::new(255, 0, 0));
    }

    #[test]
    fn rgb_sliders_move_one_channel() {
        let mut tuning = Tuning::new(Rgb::new(10, 20, 30));

        tuning.set_slider(3, 1.0);
        assert_eq!(tuning.color, Rgb::new(255, 20, 30));
        tuning.set_slider(5, 0.0);
        assert_eq!(tuning.color, Rgb::new(255, 20, 0));
    }

    #[test]
    fn text_on_a_color_is_readable() {
        assert_eq!(contrast(Rgb::new(255, 255, 255)), 0x101114);
        assert_eq!(contrast(Rgb::new(0, 0, 0)), 0xffffff);
    }
}
