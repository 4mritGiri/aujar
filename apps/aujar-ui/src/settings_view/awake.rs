//! Awake: keep the computer from sleeping, until you turn it off or for a set time.
//!
//! The daemon holds the lock and remembers the mode across restarts (`aujar awake ...` does the
//! same from a terminal). This page shows the status, refreshing while it is open, and has the
//! buttons.

use std::time::Duration;

use aujar_awake::api::describe_seconds;
use aujar_ipc::{AwakeMode, AwakeRequest, AwakeResponse, AwakeStatus, Request, Response};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px, rgb};

use super::{EditTarget, Page, SettingsView};

/// How often the status refreshes while the page is open.
const REFRESH: Duration = Duration::from_secs(2);

/// The timed choices: label and seconds.
const TIMES: [(&str, u64); 5] = [
    ("30 min", 30 * 60),
    ("1 hour", 3600),
    ("2 hours", 2 * 3600),
    ("4 hours", 4 * 3600),
    ("8 hours", 8 * 3600),
];

#[derive(Default)]
pub(super) struct State {
    status: Option<Result<AwakeStatus, String>>,
    /// The answer to the last click, when it was an error.
    error: Option<String>,
    refreshing: bool,
    /// The preset last asked for, so it can be shown as the chosen one while its timer runs.
    chosen: Option<u64>,
}

/// How the page looks for a given state of the daemon: decided here, drawn below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    /// Not known yet.
    Unknown,
    Off,
    On,
    Problem,
}

/// Which choice is highlighted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chosen {
    Nothing,
    Off,
    Forever,
    Timed(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct View {
    tone: Tone,
    headline: String,
    detail: Option<String>,
    /// The big button: its label and what it asks for. `None` when nothing can be changed.
    action: Option<(&'static str, AwakeRequest)>,
    chosen: Chosen,
    /// Whether the choices can be used (the daemon is there).
    can_change: bool,
}

fn view(status: &Option<Result<AwakeStatus, String>>, chosen_preset: Option<u64>) -> View {
    let (status, can_change) = match status {
        None => {
            return View {
                tone: Tone::Unknown,
                headline: "Checking\u{2026}".into(),
                detail: None,
                action: None,
                chosen: Chosen::Nothing,
                can_change: false,
            };
        }
        Some(Err(message)) => {
            return View {
                tone: Tone::Problem,
                headline: "Awake is not available".into(),
                detail: Some(message.clone()),
                action: None,
                chosen: Chosen::Nothing,
                can_change: false,
            };
        }
        Some(Ok(status)) => (status, true),
    };

    let screen = if status.keep_screen_on {
        " The screen stays on too."
    } else {
        ""
    };

    if let Some(error) = &status.error {
        return View {
            tone: Tone::Problem,
            headline: "Awake cannot keep the computer awake".into(),
            detail: Some(error.clone()),
            action: Some(("Turn off", AwakeRequest::Off)),
            chosen: Chosen::Nothing,
            can_change,
        };
    }

    match status.mode {
        AwakeMode::Off => View {
            tone: Tone::Off,
            headline: "Awake is off".into(),
            detail: Some("The computer sleeps as usual.".into()),
            action: Some(("Keep awake", AwakeRequest::Indefinite)),
            chosen: Chosen::Off,
            can_change,
        },
        AwakeMode::Indefinite => View {
            tone: Tone::On,
            headline: "Keeping your computer awake".into(),
            detail: Some(format!("Until you turn it off.{screen}")),
            action: Some(("Turn off", AwakeRequest::Off)),
            chosen: Chosen::Forever,
            can_change,
        },
        AwakeMode::Timed => {
            let left = status.remaining_seconds.unwrap_or(0);

            View {
                tone: Tone::On,
                headline: "Keeping your computer awake".into(),
                detail: Some(format!("{} left.{screen}", describe_seconds(left))),
                action: Some(("Turn off", AwakeRequest::Off)),
                // The chosen preset only while it is plausible: some of it has run, not more
                // than all of it.
                chosen: chosen_preset
                    .filter(|preset| left <= *preset)
                    .map_or(Chosen::Nothing, Chosen::Timed),
                can_change,
            }
        }
    }
}

/// Ask the daemon. Blocking: run it off the UI thread.
fn ask(request: AwakeRequest) -> Result<AwakeStatus, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;

    runtime.block_on(async {
        let answer = tokio::time::timeout(
            Duration::from_secs(2),
            aujar_ipc::send(crate::socket_path(), Request::Awake(request)),
        )
        .await;

        match answer {
            Err(_) => Err("The Aujar daemon did not answer.".to_owned()),
            Ok(Err(_)) => Err(
                "The Aujar daemon is not running. Start it with `systemctl --user start aujar`."
                    .to_owned(),
            ),
            Ok(Ok(Response::Awake(AwakeResponse::Status(status)))) => Ok(status),
            Ok(Ok(Response::Error(message))) => Err(message),
            Ok(Ok(_)) => Err("The daemon sent an unexpected answer.".to_owned()),
        }
    })
}

impl SettingsView {
    pub(super) fn refresh_awake(&mut self, cx: &mut Context<Self>) {
        if self.awake.refreshing {
            return;
        }

        self.awake.refreshing = true;

        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async { ask(AwakeRequest::Status) })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.awake.refreshing = false;
                this.awake.status = Some(status);
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-ask the daemon every couple of seconds while this page is showing; stops when the
    /// window closes.
    pub(super) fn keep_awake_status_fresh(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let alive = this.update(cx, |this, cx| {
                    if this.page == Page::Awake {
                        this.refresh_awake(cx);
                    }
                });

                if alive.is_err() {
                    break;
                }

                cx.background_executor().timer(REFRESH).await;
            }
        })
        .detach();
    }

    pub(super) fn set_awake(&mut self, request: AwakeRequest, cx: &mut Context<Self>) {
        self.awake.error = None;
        self.awake.chosen = match request {
            AwakeRequest::For { seconds } => Some(seconds),
            _ => None,
        };

        cx.spawn(async move |this, cx| {
            let answer = cx
                .background_executor()
                .spawn(async move { ask(request) })
                .await;

            let _ = this.update(cx, |this, cx| {
                match answer {
                    Ok(status) => this.awake.status = Some(Ok(status)),
                    Err(message) => this.awake.error = Some(message),
                }

                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn awake_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let pal = self.palette;
        let view = view(&self.awake.status, self.awake.chosen);

        let mut choices = vec![self.awake_stacked_row(
            "Keep awake for",
            "Pick how long. A timer ends by itself.",
            self.awake_presets(&view, cx),
        )];

        if self
            .editing
            .as_ref()
            .is_some_and(|edit| edit.target == EditTarget::AwakeTime)
        {
            choices.extend(self.editor_row(
                "Type a time such as 45m, 3h or 1h30m (1 minute to 7 days), then press Enter.",
                false,
                cx,
            ));
        }

        vec![
            self.section("Status", vec![self.awake_status_row(&view, cx)]),
            self.section("Duration", choices),
            self.section(
                "Options",
                vec![toggle_row!(
                    self,
                    cx,
                    "awake-screen",
                    "Keep the screen on too",
                    "Also stop the screen from turning off and the session from locking. Without this only sleep is prevented. Takes effect at once, even while Awake is running.",
                    awake.keep_screen_on
                )],
            ),
            div()
                .flex_shrink_0()
                .px_1()
                .text_xs()
                .text_color(rgb(pal.text_faint))
                .child(
                    "Aujar asks the desktop not to sleep. Your choice is remembered when Aujar restarts, a timer ends by itself, and if Aujar stops the computer may sleep again. From a terminal: aujar awake on, off, for 2h.",
                )
                .into_any_element(),
        ]
    }

    /// A row with the text on top and the control below it, for controls that need the width.
    fn awake_stacked_row(&self, label: &str, hint: &str, control: impl IntoElement) -> AnyElement {
        let pal = self.palette;

        div()
            .flex()
            .flex_col()
            .gap_3()
            .px_4()
            .py_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(pal.text))
                            .child(label.to_owned()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_dim))
                            .child(hint.to_owned()),
                    ),
            )
            .child(control)
            .into_any_element()
    }

    /// The state, in words, with the one button that matters.
    fn awake_status_row(&self, view: &View, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;
        let dot = match view.tone {
            Tone::Unknown => pal.text_faint,
            Tone::Off => pal.text_faint,
            Tone::On => pal.positive,
            Tone::Problem => pal.danger,
        };

        let mut text = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .gap_1()
            .child(
                div()
                    .text_color(rgb(if view.tone == Tone::Problem {
                        pal.danger
                    } else {
                        pal.text
                    }))
                    .child(view.headline.clone()),
            );

        if let Some(detail) = &view.detail {
            text = text.child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_dim))
                    .child(detail.clone()),
            );
        }

        if let Some(error) = &self.awake.error {
            text = text.child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.danger))
                    .child(error.clone()),
            );
        }

        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_4()
            .py_3()
            .child(
                div()
                    .size(px(12.))
                    .rounded_full()
                    .flex_shrink_0()
                    .bg(rgb(dot)),
            )
            .child(text);

        if let Some((label, request)) = view.action.clone() {
            row = row.child(
                self.pill("awake-action".into(), label, true)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.set_awake(request.clone(), cx);
                    })),
            );
        }

        row.into_any_element()
    }

    /// Off, the timers, until turned off, and a time of your own, on a row that wraps.
    fn awake_presets(&self, view: &View, cx: &mut Context<Self>) -> AnyElement {
        let editing_time = self
            .editing
            .as_ref()
            .is_some_and(|edit| edit.target == EditTarget::AwakeTime);
        let usable = view.can_change;

        let mut row = div().flex().flex_row().flex_wrap().gap_2();

        let off = self.pill("awake-off".into(), "Off", view.chosen == Chosen::Off);

        row = row.child(if usable {
            off.on_click(cx.listener(|this, _event, _window, cx| {
                this.set_awake(AwakeRequest::Off, cx);
            }))
            .into_any_element()
        } else {
            off.opacity(0.45).into_any_element()
        });

        for (label, seconds) in TIMES {
            let pill = self.pill(
                SharedString::from(format!("awake-{seconds}")),
                label,
                view.chosen == Chosen::Timed(seconds),
            );

            row = row.child(if usable {
                pill.on_click(cx.listener(move |this, _event, _window, cx| {
                    this.set_awake(AwakeRequest::For { seconds }, cx);
                }))
                .into_any_element()
            } else {
                pill.opacity(0.45).into_any_element()
            });
        }

        let forever = self.pill(
            "awake-forever".into(),
            "Until I turn it off",
            view.chosen == Chosen::Forever,
        );

        row = row.child(if usable {
            forever
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.set_awake(AwakeRequest::Indefinite, cx);
                }))
                .into_any_element()
        } else {
            forever.opacity(0.45).into_any_element()
        });

        let custom = self.pill("awake-custom".into(), "Custom\u{2026}", editing_time);

        row = row.child(if usable {
            custom
                .on_click(cx.listener(|this, _event, _window, cx| {
                    if this
                        .editing
                        .as_ref()
                        .is_some_and(|edit| edit.target == EditTarget::AwakeTime)
                    {
                        this.editing = None;
                        cx.notify();
                    } else {
                        this.start_edit(EditTarget::AwakeTime, String::new(), cx);
                    }
                }))
                .into_any_element()
        } else {
            custom.opacity(0.45).into_any_element()
        });

        row.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aujar_awake::api::{MAX_SECONDS, MIN_SECONDS};

    #[test]
    fn the_timed_choices_are_valid_requests_in_increasing_order() {
        let seconds: Vec<u64> = TIMES.iter().map(|(_, seconds)| *seconds).collect();

        assert!(
            seconds
                .iter()
                .all(|seconds| (MIN_SECONDS..=MAX_SECONDS).contains(seconds)),
            "the daemon would refuse one of them: {seconds:?}"
        );
        assert!(seconds.windows(2).all(|pair| pair[0] < pair[1]));

        let mut labels: Vec<_> = TIMES.iter().map(|(label, _)| *label).collect();

        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), TIMES.len(), "labels must be distinct");
    }

    fn status(mode: AwakeMode, left: Option<u64>) -> AwakeStatus {
        AwakeStatus {
            mode,
            active: mode != AwakeMode::Off,
            remaining_seconds: left,
            keep_screen_on: false,
            error: None,
        }
    }

    fn view_of(status: AwakeStatus, preset: Option<u64>) -> View {
        view(&Some(Ok(status)), preset)
    }

    #[test]
    fn before_the_first_answer_nothing_can_be_changed() {
        let view = view(&None, None);

        assert_eq!(view.tone, Tone::Unknown);
        assert!(!view.can_change);
        assert_eq!(view.action, None);
    }

    #[test]
    fn a_missing_daemon_is_explained_and_disables_the_choices() {
        let view = view(&Some(Err("The Aujar daemon is not running.".into())), None);

        assert_eq!(view.tone, Tone::Problem);
        assert_eq!(view.headline, "Awake is not available");
        assert_eq!(
            view.detail.as_deref(),
            Some("The Aujar daemon is not running.")
        );
        assert!(!view.can_change);
        assert_eq!(view.action, None);
    }

    #[test]
    fn off_offers_to_turn_on_and_highlights_off() {
        let view = view_of(status(AwakeMode::Off, None), None);

        assert_eq!(view.tone, Tone::Off);
        assert_eq!(view.chosen, Chosen::Off);
        assert_eq!(view.action, Some(("Keep awake", AwakeRequest::Indefinite)));
        assert!(view.can_change);
    }

    #[test]
    fn indefinite_offers_to_turn_off_and_highlights_until_i_turn_it_off() {
        let view = view_of(status(AwakeMode::Indefinite, None), None);

        assert_eq!(view.tone, Tone::On);
        assert_eq!(view.chosen, Chosen::Forever);
        assert_eq!(view.action, Some(("Turn off", AwakeRequest::Off)));
        assert_eq!(view.detail.as_deref(), Some("Until you turn it off."));
    }

    #[test]
    fn a_timer_shows_the_time_left_and_the_preset_that_was_asked_for() {
        let view = view_of(status(AwakeMode::Timed, Some(5100)), Some(7200));

        assert_eq!(view.chosen, Chosen::Timed(7200));
        assert_eq!(view.detail.as_deref(), Some("1h 25m left."));
        assert_eq!(view.action, Some(("Turn off", AwakeRequest::Off)));
    }

    #[test]
    fn a_remembered_preset_that_does_not_fit_the_timer_is_not_shown_as_chosen() {
        // More time left than the preset gave: a timer set from somewhere else (the terminal).
        let view = view_of(status(AwakeMode::Timed, Some(9000)), Some(3600));

        assert_eq!(view.chosen, Chosen::Nothing);

        // And with no preset remembered (the window was closed and opened again).
        let view = view_of(status(AwakeMode::Timed, Some(900)), None);

        assert_eq!(view.chosen, Chosen::Nothing);
        assert_eq!(view.detail.as_deref(), Some("15m left."));
    }

    #[test]
    fn the_screen_setting_is_mentioned_while_it_applies() {
        let mut on = status(AwakeMode::Indefinite, None);

        on.keep_screen_on = true;

        assert!(
            view_of(on, None)
                .detail
                .is_some_and(|detail| detail.contains("The screen stays on too."))
        );
    }

    #[test]
    fn a_refusal_by_the_desktop_is_a_problem_with_a_way_out() {
        let mut failing = status(AwakeMode::Indefinite, None);

        failing.error = Some("the desktop refused".into());

        let view = view_of(failing, None);

        assert_eq!(view.tone, Tone::Problem);
        assert_eq!(view.detail.as_deref(), Some("the desktop refused"));
        assert_eq!(view.action, Some(("Turn off", AwakeRequest::Off)));
        assert!(view.can_change);
    }

    #[test]
    fn the_page_is_in_the_sidebar_and_found_by_the_words_people_use() {
        assert!(Page::MAIN.contains(&Page::Awake));
        assert!(Page::ALL.contains(&Page::Awake));

        for word in ["awake", "sleep", "caffeine", "suspend", "screen"] {
            assert!(Page::Awake.matches(word), "{word}");
        }

        assert!(!Page::Awake.matches("clipboard"));
    }
}
