//! The settings window. Every change is applied and saved immediately to
//! `~/.config/aujar/settings.toml`; the launcher re-reads it each time it opens, and the
//! daemon reloads its plugins when the file changes.

use crate::theme::Palette;
use aujar_config::{Accent, Engine, Settings, Theme};
use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, KeyDownEvent, SharedString, Stateful,
    Window, div, prelude::*, px, rgb, rgba,
};

pub struct SettingsView {
    settings: Settings,
    palette: Palette,
    status: Option<String>,
    pub focus_handle: FocusHandle,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = Settings::load_or_default();

        Self {
            palette: Palette::from_settings(&settings),
            settings,
            status: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Apply a change, save it and refresh the colors.
    fn mutate(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Settings)) {
        change(&mut self.settings);

        self.settings = self.settings.clone().sanitized();
        self.palette = Palette::from_settings(&self.settings);
        self.status = Some(match self.settings.save() {
            Ok(()) => "Saved".to_owned(),
            Err(error) => format!("Could not save: {error:#}"),
        });

        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, _cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            window.remove_window();
        }
    }

    // ---- Building blocks ----

    fn section(&self, title: &str, rows: Vec<AnyElement>) -> AnyElement {
        let pal = self.palette;

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .px_1()
                    .text_xs()
                    .text_color(rgb(pal.text_dim))
                    .child(title.to_uppercase()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgba(pal.border))
                    .bg(rgba(pal.subtle))
                    .children(rows.into_iter().enumerate().map(|(index, row)| {
                        div()
                            .when(index > 0, |this| {
                                this.border_t_1().border_color(rgba(pal.divider))
                            })
                            .child(row)
                    })),
            )
            .into_any_element()
    }

    fn row(&self, label: &str, hint: &str, control: impl IntoElement) -> AnyElement {
        let pal = self.palette;

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_4()
            .px_4()
            .py_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(div().text_sm().text_color(rgb(pal.text)).child(label.to_owned()))
                    .child(div().text_xs().text_color(rgb(pal.text_dim)).child(hint.to_owned())),
            )
            .child(control)
            .into_any_element()
    }

    fn switch(&self, id: &'static str, on: bool) -> Stateful<gpui::Div> {
        let pal = self.palette;

        div()
            .id(id)
            .flex()
            .items_center()
            .w(px(40.))
            .h(px(22.))
            .px_1()
            .rounded_full()
            .cursor_pointer()
            .bg(rgb(if on { pal.accent } else { pal.switch_off }))
            .when(on, |this| this.justify_end())
            .child(div().size(px(16.)).rounded_full().bg(rgb(0xffffff)))
    }

    fn pill(&self, id: SharedString, label: &str, selected: bool) -> Stateful<gpui::Div> {
        let pal = self.palette;

        div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .border_1()
            .border_color(rgba(pal.keycap_border))
            .when(selected, |this| this.bg(rgb(pal.accent)).text_color(rgb(pal.on_accent)))
            .when(!selected, |this| {
                this.text_color(rgb(pal.text)).hover(|style| style.bg(rgba(pal.hover)))
            })
            .child(label.to_owned())
    }
}

/// A switch row bound to a boolean field of [`Settings`].
macro_rules! toggle_row {
    ($this:ident, $cx:ident, $id:literal, $label:expr, $hint:expr, $($field:tt)+) => {{
        let on = $this.settings.$($field)+;
        let control = $this.switch($id, on).on_click($cx.listener(|this, _event, _window, cx| {
            this.mutate(cx, |settings| settings.$($field)+ = !settings.$($field)+);
        }));

        $this.row($label, $hint, control)
    }};
}

/// `- value +` stepper bound to a `u32` field, moving by `$step` within the clamp in `sanitized`.
macro_rules! stepper_row {
    ($this:ident, $cx:ident, $id:literal, $label:expr, $hint:expr, $step:expr, $suffix:expr, $($field:tt)+) => {{
        let value = $this.settings.$($field)+;
        let pal = $this.palette;

        let minus = $this
            .pill(SharedString::from(concat!($id, "-minus")), "\u{2212}", false)
            .on_click($cx.listener(|this, _event, _window, cx| {
                this.mutate(cx, |settings| {
                    settings.$($field)+ = settings.$($field)+.saturating_sub($step)
                });
            }));
        let plus = $this
            .pill(SharedString::from(concat!($id, "-plus")), "+", false)
            .on_click($cx.listener(|this, _event, _window, cx| {
                this.mutate(cx, |settings| {
                    settings.$($field)+ = settings.$($field)+.saturating_add($step)
                });
            }));

        let control = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(minus)
            .child(
                div()
                    .min_w(px(64.))
                    .flex()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(pal.text))
                    .child(format!("{value}{}", $suffix)),
            )
            .child(plus);

        $this.row($label, $hint, control)
    }};
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;

        // ---- Appearance ----
        let theme_control = div()
            .flex()
            .flex_row()
            .gap_2()
            .child(
                self.pill("theme-dark".into(), "Dark", self.settings.appearance.theme == Theme::Dark)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.mutate(cx, |settings| settings.appearance.theme = Theme::Dark)
                    })),
            )
            .child(
                self.pill("theme-light".into(), "Light", self.settings.appearance.theme == Theme::Light)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.mutate(cx, |settings| settings.appearance.theme = Theme::Light)
                    })),
            );

        let accent_control = div().flex().flex_row().gap_2().children(Accent::ALL.map(|accent| {
            let selected = self.settings.appearance.accent == accent;

            div()
                .id(SharedString::from(format!("accent-{accent:?}")))
                .size(px(24.))
                .rounded_full()
                .cursor_pointer()
                .bg(rgb(accent.rgb()))
                .border_2()
                .border_color(if selected { rgb(pal.text) } else { rgb(accent.rgb()) })
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.mutate(cx, |settings| settings.appearance.accent = accent)
                }))
        }));

        let appearance = self.section(
            "Appearance",
            vec![
                self.row("Theme", "Launcher and settings colors", theme_control),
                self.row("Accent color", "Selected rows and switches", accent_control),
                stepper_row!(self, cx, "width", "Width", "Launcher width in pixels", 40, " px", appearance.width),
                stepper_row!(self, cx, "max", "Results per section", "Applications shown (other sections show fewer)", 1, "", appearance.max_results),
                toggle_row!(self, cx, "icons", "App icons", "Show application icons next to results", appearance.show_icons),
                toggle_row!(self, cx, "footer", "Keyboard hints", "Show the shortcut bar at the bottom", appearance.show_footer),
                toggle_row!(self, cx, "preview", "Preview pane", "Details of the selected result on the right", appearance.show_preview),
                toggle_row!(self, cx, "recents", "Recent & frequent", "Show what you use most when the search box is empty", appearance.show_recents),
                toggle_row!(self, cx, "blur", "Close when clicking away", "Hide the launcher when it loses focus", appearance.close_on_blur),
            ],
        );

        // ---- Plugins ----
        let plugins = self.section(
            "Search plugins",
            vec![
                toggle_row!(self, cx, "p-apps", "Applications", "Installed desktop applications", plugins.applications),
                toggle_row!(self, cx, "p-calc", "Calculator", "Math: 2*(3+4), 2^10, 10 % 3", plugins.calculator),
                toggle_row!(self, cx, "p-units", "Unit converter", "10 km to mi, 100 f to c, 2 gb to mb", plugins.units),
                toggle_row!(self, cx, "p-currency", "Currency converter", "100 usd to eur. Downloads exchange rates (curl) about twice a day", plugins.currency),
                toggle_row!(self, cx, "p-clip", "Clipboard history", "Type clip or cb. Records what you copy (in memory only)", plugins.clipboard),
                toggle_row!(self, cx, "p-files", "Files & folders", "Search file names under your home folder", plugins.files),
                toggle_row!(self, cx, "p-system", "System commands", "Lock, sleep, restart, shut down, log out, settings", plugins.system),
                toggle_row!(self, cx, "p-dict", "Dictionary", "define word, meaning of word, Ctrl+L. Sends only that word to dictionaryapi.dev", plugins.dictionary),
                toggle_row!(self, cx, "p-weather", "Weather", "weather paris. Sends only the place to Open-Meteo", plugins.weather),
                toggle_row!(self, cx, "p-imperial", "Fahrenheit and mph", "Weather in imperial units", weather.imperial),
                toggle_row!(self, cx, "p-stocks", "Stocks", "stock aapl, $tsla. Sends only the ticker to Yahoo Finance (delayed quotes)", plugins.stocks),
                toggle_row!(self, cx, "p-sports", "Sports scores", "nba, premier league, nfl lakers. Sends only the league to ESPN", plugins.sports),
                toggle_row!(self, cx, "p-windows", "Window switching", "< name lists open windows. GNOME needs the extension: aujar gnome-extension install", plugins.windows),
                toggle_row!(self, cx, "p-shell", "Shell commands", "> command runs it in a shell. Runs arbitrary commands: off by default", plugins.shell),
            ],
        );

        // ---- Web ----
        let engine_control = div().flex().flex_row().flex_wrap().gap_2().children(Engine::ALL.map(|engine| {
            let selected = self.settings.web.engine == engine;

            self.pill(SharedString::from(format!("engine-{}", engine.name())), engine.name(), selected)
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.mutate(cx, |settings| settings.web.engine = engine)
                }))
        }));

        let shortcut_list = self
            .settings
            .web
            .shortcuts
            .iter()
            .map(|shortcut| format!("{} = {}", shortcut.keyword, shortcut.name))
            .collect::<Vec<_>>()
            .join(", ");

        let web = self.section(
            "Web search",
            vec![
                self.row("Search engine", "Used by the \"Search the web\" row and `? query`", engine_control),
                self.row(
                    "Shortcuts",
                    "Type a keyword and a query, e.g. `g rust lifetimes`. Add more in the config file.",
                    div().max_w(px(260.)).text_xs().text_color(rgb(pal.text_dim)).child(shortcut_list),
                ),
            ],
        );

        // ---- About ----
        let open_config = self
            .pill("open-config".into(), "Open config file", false)
            .on_click(cx.listener(|this, _event, _window, cx| {
                // Make sure the file exists so the editor has something to open.
                this.mutate(cx, |_| {});

                let _ = std::process::Command::new("xdg-open")
                    .arg(Settings::path())
                    .spawn();
            }));

        let clear_history = self
            .pill("clear-history".into(), "Clear history", false)
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.status = Some(match aujar_config::History::default().save() {
                    Ok(()) => "History cleared".to_owned(),
                    Err(error) => format!("Could not clear: {error:#}"),
                });
                cx.notify();
            }));

        let about = self.section(
            "Shortcut & config",
            vec![
                self.row(
                    "Launcher shortcut",
                    "Set with `aujar shortcut install --keys '<Super>space'` (GNOME), or in your desktop's keyboard settings: `aujar launcher toggle`.",
                    div(),
                ),
                self.row("Config file", &Settings::path().display().to_string(), open_config),
                self.row("Launch history", "Recent and frequent items (stored in ~/.local/state/aujar)", clear_history),
            ],
        );

        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .flex_col()
            .size_full()
            .bg(rgba(pal.card))
            .text_color(rgb(pal.text))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_6()
                    .py_4()
                    .border_b_1()
                    .border_color(rgba(pal.divider))
                    .child(div().text_xl().child("Aujar Settings"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(pal.text_dim))
                            .child(self.status.clone().unwrap_or_else(|| "Changes save automatically".to_owned())),
                    ),
            )
            .child(
                div()
                    .id("settings-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_5()
                    .p_6()
                    .overflow_y_scroll()
                    .child(appearance)
                    .child(plugins)
                    .child(web)
                    .child(about),
            )
    }
}
