//! The color editor: a big swatch of the color, its hex value, a color model (OKLCH, RGB, HSL or
//! HSB) with one slider per channel and an alpha slider, and the recent colors below. The toolbar
//! has the eyedropper (pick from the screen), and three tabs: Edit, Formats (every format, click
//! to copy) and Shades (lighter and darker versions of the color).

use crate::{
    UiMessage,
    color_picker::{default_text_with_alpha, load_history, remember, save_history, send_to_ui},
    theme::{Palette, rounded_px},
};
use aujar_color::Rgb;
use aujar_config::{ColorFormat, Settings};
use gpui::{
    App, Bounds, ClipboardItem, Context, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Stateful, TitlebarOptions, Window,
    WindowBounds, WindowDecorations, WindowHandle, WindowOptions, canvas, deferred, div,
    linear_color_stop, linear_gradient, prelude::*, px, rgb, rgba, size, svg,
};
use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Duration};

const WINDOW_W: f32 = 340.0;
const WINDOW_H: f32 = 620.0;
const TITLEBAR_H: f32 = 48.0;
const SWATCH_H: f32 = 196.0;
const PAD: f32 = 20.0;
const KNOB: f32 = 24.0;
/// The sliders: three for the color model and one for alpha.
const SLIDERS: usize = 4;
const ALPHA: usize = 3;
/// OKLCH chroma that the chroma slider calls 100.
const CHROMA_FULL: f64 = 0.4;
const SHADES: usize = 6;
/// Six tiles and five 10px gaps fit the 300px the window leaves for them.
const TILE: f32 = 41.0;

// ---- Color models -----------------------------------------------------------------------

/// The channels the three sliders edit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Model {
    Oklch,
    Rgb,
    Hsl,
    Hsb,
}

impl Model {
    const ALL: [Self; 4] = [Self::Rgb, Self::Hsl, Self::Hsb, Self::Oklch];

    const fn label(self) -> &'static str {
        match self {
            Self::Oklch => "OKLCH",
            Self::Rgb => "RGB",
            Self::Hsl => "HSL",
            Self::Hsb => "HSB",
        }
    }

    /// What each slider reaches at its far end.
    const fn max(self) -> [f64; 3] {
        match self {
            Self::Rgb => [255.0; 3],
            Self::Hsl | Self::Hsb => [360.0, 100.0, 100.0],
            Self::Oklch => [100.0, 100.0, 360.0],
        }
    }

    /// The channel values of a color. A gray has no hue (and black or white no saturation): for
    /// those the value the sliders already had stays, so a slider does not jump to zero.
    fn read(self, color: Rgb, previous: [f64; 3]) -> [f64; 3] {
        match self {
            Self::Rgb => [f64::from(color.r), f64::from(color.g), f64::from(color.b)],
            Self::Hsl => {
                let (hue, saturation, lightness) = color.to_hsl_f64();
                let plain = saturation == 0.0 || lightness == 0.0 || lightness == 1.0;

                [
                    if plain { previous[0] } else { hue },
                    if lightness == 0.0 || lightness == 1.0 {
                        previous[1]
                    } else {
                        saturation * 100.0
                    },
                    lightness * 100.0,
                ]
            }
            Self::Hsb => {
                let (hue, saturation, value) = color.to_hsv();

                [
                    if saturation == 0.0 || value == 0.0 {
                        previous[0]
                    } else {
                        hue
                    },
                    if value == 0.0 {
                        previous[1]
                    } else {
                        saturation * 100.0
                    },
                    value * 100.0,
                ]
            }
            Self::Oklch => {
                let (lightness, chroma, hue) = color.to_oklch();
                let plain = chroma < 0.002 || lightness < 0.001 || lightness > 0.999;

                [
                    lightness * 100.0,
                    if lightness < 0.001 || lightness > 0.999 {
                        previous[1]
                    } else {
                        (chroma / CHROMA_FULL * 100.0).min(100.0)
                    },
                    if plain { previous[2] } else { hue },
                ]
            }
        }
    }

    fn write(self, values: [f64; 3]) -> Rgb {
        match self {
            Self::Rgb => Rgb::new(
                values[0].round().clamp(0.0, 255.0) as u8,
                values[1].round().clamp(0.0, 255.0) as u8,
                values[2].round().clamp(0.0, 255.0) as u8,
            ),
            Self::Hsl => Rgb::from_hsl(values[0], values[1] / 100.0, values[2] / 100.0),
            Self::Hsb => Rgb::from_hsv(values[0], values[1] / 100.0, values[2] / 100.0),
            Self::Oklch => Rgb::from_oklch(
                values[0] / 100.0,
                values[1] / 100.0 * CHROMA_FULL,
                values[2],
            ),
        }
    }

    /// The colors along slider `which` with the other channels as they are: the stops of its
    /// gradient.
    fn track(self, values: [f64; 3], which: usize, steps: usize) -> Vec<Rgb> {
        let max = self.max()[which];

        (0..=steps)
            .map(|step| {
                let mut at = values;

                at[which] = max * step as f64 / steps as f64;

                self.write(at)
            })
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Edit,
    Formats,
    Shades,
}

struct TileMenu {
    index: usize,
    at: Point<Pixels>,
}

pub(crate) struct ColorEditorView {
    settings: Settings,
    palette: Palette,
    history: Vec<Rgb>,
    color: Rgb,
    /// The sliders' values in the current model; kept so a gray remembers its hue.
    values: [f64; 3],
    /// `0.0..=1.0`.
    alpha: f64,
    model: Model,
    tab: Tab,
    recents_open: bool,
    model_menu: bool,
    more_menu: bool,
    menu: Option<TileMenu>,
    focus_handle: FocusHandle,
    /// What was just copied, shown for a moment.
    copied: Option<String>,
    status: Option<String>,
    /// Where each slider's track was drawn last, to turn the pointer position into a value.
    tracks: Rc<RefCell<[Bounds<Pixels>; SLIDERS]>>,
    dragging: Option<usize>,
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

fn alpha_byte(alpha: f64) -> u8 {
    (alpha.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// `#RRGGBB`, or `#RRGGBBAA` when the color is see-through.
fn hex_text(color: Rgb, alpha: f64) -> String {
    if alpha_byte(alpha) == 255 {
        color.to_hex()
    } else {
        format!("{}{:02X}", color.to_hex(), alpha_byte(alpha))
    }
}

impl ColorEditorView {
    fn new(cx: &mut App) -> Self {
        let settings = Settings::load_or_default();
        let history = load_history();
        let color = history
            .first()
            .copied()
            .unwrap_or(Rgb::new(0x6c, 0x8c, 0xff));
        let model = Model::Oklch;

        Self {
            palette: Palette::from_settings(&settings),
            settings,
            values: model.read(color, [0.0, 0.0, 0.0]),
            history,
            color,
            alpha: 1.0,
            model,
            tab: Tab::Edit,
            recents_open: true,
            model_menu: false,
            more_menu: false,
            menu: None,
            focus_handle: cx.focus_handle(),
            copied: None,
            status: None,
            tracks: Rc::new(RefCell::new([Bounds::default(); SLIDERS])),
            dragging: None,
        }
    }

    fn close_menus(&mut self) -> bool {
        let open = self.model_menu || self.more_menu || self.menu.is_some();

        self.model_menu = false;
        self.more_menu = false;
        self.menu = None;

        open
    }

    /// Show another color (from the history, a shade or the clipboard).
    fn load(&mut self, color: Rgb, cx: &mut Context<Self>) {
        self.color = color;
        self.values = self.model.read(color, self.values);
        self.alpha = 1.0;
        cx.notify();
    }

    fn set_model(&mut self, model: Model, cx: &mut Context<Self>) {
        self.model = model;
        self.values = model.read(self.color, self.values);
        cx.notify();
    }

    fn set_slider(&mut self, which: usize, fraction: f32, cx: &mut Context<Self>) {
        let fraction = f64::from(fraction.clamp(0.0, 1.0));

        if which == ALPHA {
            self.alpha = fraction;
        } else {
            self.values[which] = fraction * self.model.max()[which];
            self.color = self.model.write(self.values);
        }

        cx.notify();
    }

    /// Put the current color at the top of the recent colors.
    fn save(&mut self, cx: &mut Context<Self>) {
        self.history = remember(self.color);
        cx.notify();
    }

    fn flash(&mut self, what: &str, cx: &mut Context<Self>) {
        self.copied = Some(what.to_owned());
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

    /// Copy the color in the default format, and keep it in the recent colors.
    fn copy_default(&mut self, cx: &mut Context<Self>) {
        let text = default_text_with_alpha(&self.settings, self.color, alpha_byte(self.alpha));

        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.save(cx);
        self.flash("Copied", cx);
    }

    fn copy_format(&mut self, format: &ColorFormat, cx: &mut Context<Self>) {
        let text = self
            .color
            .format_with_alpha(&format.pattern, alpha_byte(self.alpha));

        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.save(cx);
        self.flash(&format.name, cx);
    }

    fn remove(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.history.len() {
            self.history.remove(index);
            save_history(&self.history);
        }

        cx.notify();
    }

    fn clear_history(&mut self, cx: &mut Context<Self>) {
        self.history.clear();
        save_history(&self.history);
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
                // No file dialog on this desktop: save in the home folder.
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

    fn fraction(&self, which: usize, x: Pixels) -> f32 {
        let track = self.tracks.borrow()[which];
        let room = (f32::from(track.size.width) - KNOB).max(1.0);

        ((f32::from(x) - f32::from(track.origin.x) - KNOB / 2.0) / room).clamp(0.0, 1.0)
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;

        if keystroke.key == "escape" {
            if self.close_menus() {
                cx.notify();
            } else {
                window.remove_window();
            }
        } else if keystroke.modifiers.control && keystroke.key == "v" {
            // A color copied anywhere (`#5600D8`, `5600d8`) shows up here.
            if let Some(color) = cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .and_then(|text| Rgb::parse_hex(&text))
            {
                self.load(color, cx);
            }
        }
    }

    // ---- Pieces ---------------------------------------------------------------------------

    fn glyph(&self, path: &'static str, size: f32, color: u32) -> impl IntoElement {
        svg()
            .path(path)
            .size(px(size))
            .flex_shrink_0()
            .text_color(rgb(color))
    }

    fn icon_button(
        &self,
        id: &'static str,
        glyph: &'static str,
        active: bool,
    ) -> Stateful<gpui::Div> {
        let pal = self.palette;

        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(38.0))
            .h(px(30.0))
            .flex_shrink_0()
            .rounded_md()
            .cursor_pointer()
            .when(active, |this| this.bg(rgba(pal.pill)))
            .hover(|style| style.bg(rgba(pal.hover)))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation()
            })
            .child(self.glyph(glyph, 18.0, if active { pal.text } else { pal.text_dim }))
    }

    fn traffic_light(&self, id: &'static str, color: u32, active: bool) -> Stateful<gpui::Div> {
        let pal = self.palette;

        div()
            .id(id)
            .size(px(13.0))
            .flex_shrink_0()
            .rounded_full()
            .cursor_pointer()
            .bg(rgb(if active { color } else { pal.switch_off }))
            .hover(|style| style.opacity(0.8))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation()
            })
    }

    fn titlebar(&self, active: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let pal = self.palette;

        let lights = self.settings.appearance.traffic_lights.then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .pr_2()
                .child(
                    self.traffic_light("light-close", 0xff5f57, active)
                        .on_click(cx.listener(|_this, _event, window, _cx| window.remove_window())),
                )
                .child(
                    self.traffic_light("light-min", 0xfebc2e, active).on_click(
                        cx.listener(|_this, _event, window, _cx| window.minimize_window()),
                    ),
                )
                // The window has a fixed size, so the green one has nothing to zoom.
                .child(self.traffic_light("light-zoom", 0x28c840, false))
        });

        let tab = |this: &Self,
                   id: &'static str,
                   glyph: &'static str,
                   which: Tab,
                   cx: &mut Context<Self>| {
            this.icon_button(id, glyph, this.tab == which)
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.tab = which;
                    this.close_menus();
                    cx.notify();
                }))
        };

        div()
            .flex()
            .items_center()
            .gap_2()
            .flex_shrink_0()
            .h(px(TITLEBAR_H))
            .px_4()
            .border_b_1()
            .border_color(rgba(pal.divider))
            // The empty parts of the title bar move the window.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _event: &MouseDownEvent, window, _cx| {
                    window.start_window_move();
                }),
            )
            .children(lights)
            .child(
                self.icon_button("tb-pick", "icons/eyedropper.svg", false)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.close_menus();
                        send_to_ui(UiMessage::ColorPick);
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .child(tab(self, "tb-edit", "icons/sliders.svg", Tab::Edit, cx))
            .child(tab(self, "tb-formats", "icons/code.svg", Tab::Formats, cx))
            .child(tab(self, "tb-shades", "icons/layers.svg", Tab::Shades, cx))
            .child(div().w(px(1.0)).h(px(18.0)).bg(rgba(pal.divider)))
            .child(
                self.icon_button("tb-more", "icons/chevron-down.svg", self.more_menu)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        let open = !this.more_menu;

                        this.close_menus();
                        this.more_menu = open;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// Alternating squares behind a see-through color.
    fn checker(&self, columns: usize, rows: usize, side: f32) -> gpui::AnyElement {
        let mut grid = div().flex().flex_col().absolute().size_full();

        for row in 0..rows {
            let mut line = div().flex().flex_row();

            for column in 0..columns {
                let light = (row + column) % 2 == 0;

                line = line.child(div().size(px(side)).flex_shrink_0().bg(rgb(if light {
                    0x6b6b6b
                } else {
                    0x4a4a4a
                })));
            }

            grid = grid.child(line);
        }

        grid.into_any_element()
    }

    fn swatch(&self) -> gpui::AnyElement {
        let pal = self.palette;
        let see_through = alpha_byte(self.alpha) < 255;

        div()
            .relative()
            .flex_shrink_0()
            .w_full()
            .h(px(SWATCH_H))
            .overflow_hidden()
            .when(see_through, |this| this.child(self.checker(13, 8, 28.0)))
            .child(div().absolute().size_full().bg(rgba(
                self.color.to_u32() << 8 | u32::from(alpha_byte(self.alpha)),
            )))
            .child(
                div()
                    .absolute()
                    .top(px(12.0))
                    .left(px(12.0))
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(rgba(0x00000040))
                    .text_xs()
                    .text_color(rgba(0xffffffd9))
                    .child("sRGB"),
            )
            .child(
                // The color's name, bottom left, like the badge.
                div()
                    .absolute()
                    .bottom(px(10.0))
                    .left(px(14.0))
                    .text_xs()
                    .text_color(rgba(0xffffffb3))
                    .when(self.settings.color_picker.show_color_name, |this| {
                        this.child(self.color.name())
                    }),
            )
            .border_b_1()
            .border_color(rgba(pal.divider))
            .into_any_element()
    }

    fn info_row(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let pal = self.palette;
        let copied = self.copied.clone();

        let hex = div()
            .id("hex")
            .text_color(match &copied {
                Some(_) => rgb(pal.positive),
                None => rgb(pal.text_dim),
            })
            .cursor_pointer()
            .child(copied.unwrap_or_else(|| hex_text(self.color, self.alpha)))
            .on_click(cx.listener(|this, _event, _window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(hex_text(this.color, this.alpha)));
                this.flash("Copied", cx);
            }));

        let model = div()
            .id("model")
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_color(rgb(pal.text_dim))
            .hover(|style| style.bg(rgba(pal.hover)))
            .child(self.model.label())
            .child(self.glyph("icons/chevron-down.svg", 14.0, pal.text_dim))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation()
            })
            .on_click(cx.listener(|this, _event, _window, cx| {
                let open = !this.model_menu;

                this.close_menus();
                this.model_menu = open;
                cx.notify();
            }));

        let save = self
            .icon_button("save", "icons/palette.svg", false)
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.save(cx);
                this.flash("Saved", cx);
            }));
        let copy = self
            .icon_button("copy", "icons/copy.svg", false)
            .on_click(cx.listener(|this, _event, _window, cx| this.copy_default(cx)));

        let menu = self.model_menu.then(|| {
            let mut list = div()
                .absolute()
                .top(px(40.0))
                .right(px(PAD + 78.0))
                .w(px(120.0))
                .p_1()
                .rounded_lg()
                .bg(rgba(pal.menu))
                .border_1()
                .border_color(rgba(pal.border))
                .shadow_md()
                .occlude();

            for option in Model::ALL {
                list = list.child(
                    div()
                        .id(option.label())
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .text_sm()
                        .cursor_pointer()
                        .text_color(rgb(if option == self.model {
                            pal.accent_fg
                        } else {
                            pal.text
                        }))
                        .hover(|style| style.bg(rgba(pal.hover)))
                        .child(option.label())
                        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                            cx.stop_propagation()
                        })
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.model_menu = false;
                            this.set_model(option, cx);
                        })),
                );
            }

            deferred(list).with_priority(10)
        });

        div()
            .relative()
            .flex()
            .items_center()
            .gap_1()
            .flex_shrink_0()
            .px(px(PAD))
            .pt_3()
            .pb_2()
            .text_sm()
            .child(hex)
            .child(div().flex_1())
            .child(model)
            .child(save)
            .child(copy)
            .children(menu)
            .into_any_element()
    }

    /// One slider: a gradient track, a knob that is a ring, and the value.
    fn slider(
        &self,
        which: usize,
        track: gpui::AnyElement,
        fraction: f32,
        shown: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let pal = self.palette;
        let tracks = self.tracks.clone();
        // The knob stays inside the track: its middle runs from half a knob in to half a knob in.
        let width = f32::from(self.tracks.borrow()[which].size.width);
        let room = if width > KNOB {
            width - KNOB
        } else {
            250.0 - KNOB
        };

        div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .id(("slider", which))
                    .relative()
                    .flex_1()
                    .h(px(KNOB))
                    .rounded_full()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            this.dragging = Some(which);
                            let fraction = this.fraction(which, event.position.x);

                            this.set_slider(which, fraction, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .size_full()
                            .rounded_full()
                            .overflow_hidden()
                            .border_1()
                            .border_color(rgba(pal.border))
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
                            .top(px(0.0))
                            .left(px(fraction * room))
                            .size(px(KNOB))
                            .rounded_full()
                            .border_3()
                            .border_color(rgb(0xffffff))
                            .shadow_sm(),
                    ),
            )
            .child(
                div()
                    .w(px(40.0))
                    .flex()
                    .justify_end()
                    .text_sm()
                    .text_color(rgb(pal.text_dim))
                    .child(shown),
            )
            .into_any_element()
    }

    fn gradient(colors: &[Rgb]) -> gpui::AnyElement {
        let mut row = div().flex().size_full();

        for pair in colors.windows(2) {
            row = row.child(div().flex_1().h_full().bg(linear_gradient(
                90.0,
                linear_color_stop(rgb(pair[0].to_u32()), 0.0),
                linear_color_stop(rgb(pair[1].to_u32()), 1.0),
            )));
        }

        row.into_any_element()
    }

    fn sliders(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let max = self.model.max();
        let mut column = div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .flex_shrink_0()
            .px(px(PAD))
            .py_2();

        for which in 0..3 {
            // Hue goes all the way round: more stops keep the gradient faithful.
            let hue = matches!(
                (self.model, which),
                (Model::Oklch, 2) | (Model::Hsl | Model::Hsb, 0)
            );
            let stops = self
                .model
                .track(self.values, which, if hue { 12 } else { 6 });

            column = column.child(self.slider(
                which,
                Self::gradient(&stops),
                (self.values[which] / max[which]) as f32,
                format!("{}", self.values[which].round() as i32),
                cx,
            ));
        }

        // Alpha: the color itself fading in over a checkerboard.
        let solid = self.color.to_u32() << 8;
        let alpha_track = div()
            .relative()
            .size_full()
            .child(self.checker(40, 2, KNOB / 2.0))
            .child(div().absolute().size_full().bg(linear_gradient(
                90.0,
                linear_color_stop(rgba(solid), 0.0),
                linear_color_stop(rgba(solid | 0xff), 1.0),
            )))
            .into_any_element();

        column
            .child(self.slider(
                ALPHA,
                alpha_track,
                self.alpha as f32,
                format!("{}", (self.alpha * 100.0).round() as i32),
                cx,
            ))
            .into_any_element()
    }

    fn tile(&self, id: (&'static str, usize), color: Rgb, selected: bool) -> Stateful<gpui::Div> {
        let pal = self.palette;

        rounded_px(div(), 11.0)
            .id(id)
            .size(px(TILE))
            .flex_shrink_0()
            .bg(rgb(color.to_u32()))
            .border_1()
            .border_color(rgba(pal.border))
            .when(selected, |this| {
                this.border_2().border_color(rgb(pal.accent_fg))
            })
            .cursor_pointer()
            .hover(|style| style.opacity(0.85))
    }

    fn recents(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let pal = self.palette;

        let header = div()
            .id("recents-header")
            .flex()
            .items_center()
            .gap_2()
            .flex_shrink_0()
            .px(px(PAD))
            .pt_3()
            .pb_2()
            .cursor_pointer()
            .child(
                div()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Recent Colors"),
            )
            .child(self.glyph(
                if self.recents_open {
                    "icons/chevron-down.svg"
                } else {
                    "icons/chevron-right.svg"
                },
                15.0,
                pal.text_dim,
            ))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.recents_open = !this.recents_open;
                cx.notify();
            }));

        let mut grid = div()
            .id("recent-colors")
            .flex()
            .flex_row()
            .flex_wrap()
            .content_start()
            .gap(px(10.0))
            .px(px(PAD))
            .pb(px(PAD))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll();

        for (index, color) in self.history.iter().copied().enumerate() {
            grid = grid.child(
                self.tile(("recent", index), color, color == self.color)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.load(color, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            this.close_menus();
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

        if self.history.is_empty() {
            grid = grid.child(
                div()
                    .text_sm()
                    .text_color(rgb(pal.text_dim))
                    .child("Nothing yet. Pick a color with the eyedropper."),
            );
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .child(header)
            .when(self.recents_open, |this| this.child(grid))
            .into_any_element()
    }

    fn edit_tab(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .child(self.swatch())
            .child(self.info_row(cx))
            .child(self.sliders(cx))
            .child(self.recents(cx))
            .into_any_element()
    }

    fn formats_tab(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let pal = self.palette;
        let mut list = div()
            .id("color-formats")
            .flex()
            .flex_col()
            .gap_1()
            .flex_1()
            .min_h(px(0.0))
            .px(px(PAD))
            .py_3()
            .overflow_y_scroll();

        for (index, format) in enabled_formats(&self.settings).into_iter().enumerate() {
            let value = self
                .color
                .format_with_alpha(&format.pattern, alpha_byte(self.alpha));
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
                    .flex_shrink_0()
                    .rounded_lg()
                    .bg(rgba(pal.subtle))
                    .cursor_pointer()
                    .hover(|row| row.bg(rgba(pal.hover)))
                    .child(
                        div()
                            .w(px(60.0))
                            .text_xs()
                            .text_color(rgb(pal.text_dim))
                            .child(format.name.clone()),
                    )
                    .child(div().flex_1().text_sm().child(value))
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
                        this.copy_format(&copy_format, cx);
                    })),
            );
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .child(self.swatch_strip())
            .child(list)
            .into_any_element()
    }

    /// A slim version of the swatch for the tabs that are not editing.
    fn swatch_strip(&self) -> gpui::AnyElement {
        let pal = self.palette;

        div()
            .flex()
            .items_center()
            .justify_between()
            .flex_shrink_0()
            .h(px(64.0))
            .px(px(PAD))
            .bg(rgba(
                self.color.to_u32() << 8 | u32::from(alpha_byte(self.alpha)),
            ))
            .border_b_1()
            .border_color(rgba(pal.divider))
            .text_sm()
            .text_color(rgba(0xffffffd9))
            .child(hex_text(self.color, self.alpha))
            .child(div().text_xs().child(self.color.name()))
            .into_any_element()
    }

    fn shades_tab(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let pal = self.palette;
        let (lighter, darker) = self.color.similar(SHADES);

        let section = |this: &Self,
                       title: &'static str,
                       id: &'static str,
                       shades: Vec<Rgb>,
                       cx: &mut Context<Self>| {
            let mut row = div().flex().flex_row().flex_wrap().gap(px(10.0));

            for (index, shade) in shades.into_iter().enumerate() {
                row = row.child(this.tile((id, index), shade, false).on_click(cx.listener(
                    move |this, _event, _window, cx| {
                        this.load(shade, cx);
                        this.save(cx);
                    },
                )));
            }

            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(title),
                )
                .child(row)
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .child(self.swatch_strip())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .px(px(PAD))
                    .py_4()
                    .child(section(self, "Lighter", "lighter", lighter, cx))
                    .child(section(self, "Darker", "darker", darker, cx))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_faint))
                            .child("Click a shade to use it"),
                    ),
            )
            .into_any_element()
    }

    fn popup(&self, id: &'static str, label: &'static str) -> Stateful<gpui::Div> {
        let pal = self.palette;

        div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_md()
            .text_sm()
            .text_color(rgb(pal.text))
            .cursor_pointer()
            .hover(|row| row.bg(rgba(pal.hover)))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation()
            })
            .child(label)
    }

    fn menu_box(&self) -> gpui::Div {
        let pal = self.palette;

        div()
            .absolute()
            .w(px(190.0))
            .p_1()
            .rounded_lg()
            .bg(rgba(pal.menu))
            .border_1()
            .border_color(rgba(pal.border))
            .shadow_md()
            .occlude()
    }

    /// The menu behind the chevron in the title bar.
    fn more_menu(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.menu_box()
            .top(px(TITLEBAR_H - 6.0))
            .right(px(12.0))
            .child(
                self.popup("more-settings", "Color picker settings")
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.more_menu = false;
                        send_to_ui(UiMessage::OpenSettings);
                        cx.notify();
                    })),
            )
            .child(
                self.popup("more-export-color", "Export, by color")
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.more_menu = false;
                        this.export(true, cx);
                    })),
            )
            .child(
                self.popup("more-export-format", "Export, by format")
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.more_menu = false;
                        this.export(false, cx);
                    })),
            )
            .child(
                self.popup("more-clear", "Clear recent colors")
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.more_menu = false;
                        this.clear_history(cx);
                    })),
            )
            .into_any_element()
    }

    /// The menu of a recent color (right-click).
    fn tile_menu(&self, menu: &TileMenu, cx: &mut Context<Self>) -> gpui::AnyElement {
        let index = menu.index;

        self.menu_box()
            .left(menu.at.x)
            .top(menu.at.y)
            .w(px(170.0))
            .child(self.popup("menu-remove", "Remove").on_click(cx.listener(
                move |this, _event, _window, cx| {
                    this.menu = None;
                    this.remove(index, cx);
                },
            )))
            .child(
                self.popup("menu-export-color", "Export, by color")
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.menu = None;
                        this.export(true, cx);
                    })),
            )
            .child(
                self.popup("menu-export-format", "Export, by format")
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.menu = None;
                        this.export(false, cx);
                    })),
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let active = window.is_window_active();
        let filled = window.is_maximized() || window.is_fullscreen();

        let body = match self.tab {
            Tab::Edit => self.edit_tab(cx),
            Tab::Formats => self.formats_tab(cx),
            Tab::Shades => self.shades_tab(cx),
        };

        let frame = rounded_px(div(), if filled { 0.0 } else { pal.radius })
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(rgba(pal.card))
            .when(!filled, |this| {
                this.border_1().border_color(rgba(pal.border))
            })
            .text_color(rgb(pal.text))
            .child(self.titlebar(active, cx))
            .child(body)
            .when_some(self.status.clone(), |this, status| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .px(px(PAD))
                        .py_1()
                        .border_t_1()
                        .border_color(rgba(pal.divider))
                        .text_xs()
                        .text_color(rgb(pal.text_dim))
                        .child(status),
                )
            });

        div()
            .id("color-editor")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
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
            // A click anywhere else closes an open menu.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    if this.close_menus() {
                        cx.notify();
                    }
                }),
            )
            .relative()
            .size_full()
            .child(frame)
            .when(self.more_menu, |this| this.child(self.more_menu(cx)))
            .when_some(self.menu.as_ref(), |this, menu| {
                this.child(self.tile_menu(menu, cx))
            })
    }
}

/// Open the editor, or replace the one that is open (so it shows the current history).
pub(crate) fn open_window(cx: &mut App) -> Option<WindowHandle<ColorEditorView>> {
    let settings = Settings::load_or_default();
    let bounds = Bounds::centered(None, size(px(WINDOW_W), px(WINDOW_H)), cx);

    // Like the settings window, this one draws its own title bar and frame.
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Aujar Color Picker".into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        window_decorations: Some(WindowDecorations::Client),
        window_background: crate::background_for(&settings),
        window_min_size: Some(size(px(WINDOW_W), px(WINDOW_H))),
        is_resizable: false,
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
    fn every_model_gives_the_color_back() {
        for model in Model::ALL {
            for color in [
                Rgb::new(86, 0, 216),
                Rgb::new(255, 136, 0),
                Rgb::new(12, 200, 99),
                Rgb::new(128, 128, 128),
                Rgb::new(255, 255, 255),
                Rgb::new(0, 0, 0),
            ] {
                let values = model.read(color, [0.0; 3]);
                let back = model.write(values);

                // Sliders show whole numbers, but the values behind them are exact.
                assert_eq!(back, color, "{model:?} {color:?} {values:?}");
            }
        }
    }

    #[test]
    fn a_gray_keeps_the_hue_it_had() {
        let violet = Model::Oklch.read(Rgb::new(86, 0, 216), [0.0; 3]);
        let gray = Model::Oklch.read(Rgb::new(128, 128, 128), violet);

        assert_eq!(gray[2], violet[2], "hue stays");

        // Same in HSL and HSB, and for black and white.
        let red = Model::Hsb.read(Rgb::new(255, 0, 0), [0.0; 3]);
        let white = Model::Hsb.read(Rgb::new(255, 255, 255), red);

        assert_eq!(white[0], red[0]);
        assert_eq!(white[1], 0.0);
    }

    #[test]
    fn slider_ends_are_the_extremes() {
        let values = Model::Rgb.read(Rgb::new(10, 20, 30), [0.0; 3]);
        let track = Model::Rgb.track(values, 0, 4);

        assert_eq!(track.len(), 5);
        assert_eq!(track[0], Rgb::new(0, 20, 30));
        assert_eq!(track[4], Rgb::new(255, 20, 30));

        // Chroma at zero is a gray, whatever the hue.
        let violet = Model::Oklch.read(Rgb::new(86, 0, 216), [0.0; 3]);
        let chroma = Model::Oklch.track(violet, 1, 4);

        assert_eq!(chroma[0].r, chroma[0].b);
    }

    #[test]
    fn hex_shows_alpha_only_when_see_through() {
        let color = Rgb::new(0x56, 0x00, 0xd8);

        assert_eq!(hex_text(color, 1.0), "#5600D8");
        assert_eq!(hex_text(color, 0.5), "#5600D880");
        assert_eq!(hex_text(color, 0.0), "#5600D800");
    }
}
