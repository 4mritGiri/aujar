//! Awake request and response types, shared by the daemon, the CLI and the UI, plus the helpers
//! that turn what a person types into seconds and what Awake knows into a sentence.

use aujar_config::AwakeMode;
use aujar_runtime::Capability;
use serde::{Deserialize, Serialize};

use crate::{AwakeModule, AwakeStatus};

/// The shortest timed mode.
pub const MIN_SECONDS: u64 = 60;

/// The longest timed mode (for longer, keep awake until turned off).
pub const MAX_SECONDS: u64 = 7 * 24 * 3600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AwakeRequest {
    Status,
    /// Let the computer sleep again.
    Off,
    /// Keep it awake until told otherwise.
    Indefinite,
    /// Keep it awake for this many seconds (between [`MIN_SECONDS`] and [`MAX_SECONDS`]).
    For {
        seconds: u64,
    },
}

impl AwakeRequest {
    /// Capability the daemon must grant before serving this request.
    pub fn required_capability(&self) -> Option<Capability> {
        match self {
            Self::Status => None,
            Self::Off | Self::Indefinite | Self::For { .. } => Some(Capability::InhibitSleep),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AwakeResponse {
    Status(AwakeStatus),
}

/// A time in seconds, or a message saying how long a timed Awake may be.
pub fn check_seconds(seconds: u64) -> Result<u64, String> {
    if (MIN_SECONDS..=MAX_SECONDS).contains(&seconds) {
        Ok(seconds)
    } else {
        Err(format!(
            "a timed Awake lasts from {} to {} (use `aujar awake on` for longer)",
            describe_seconds(MIN_SECONDS),
            describe_seconds(MAX_SECONDS)
        ))
    }
}

/// Serve one request. Errors are user-facing messages.
pub async fn handle(request: AwakeRequest, module: &AwakeModule) -> Result<AwakeResponse, String> {
    match request {
        AwakeRequest::Status => {}
        AwakeRequest::Off => module.set_off().await,
        AwakeRequest::Indefinite => module.set_indefinite().await,
        AwakeRequest::For { seconds } => {
            module.set_for(check_seconds(seconds)?).await;
        }
    }

    Ok(AwakeResponse::Status(module.status().await))
}

/// `30m`, `2h`, `1h30m`, `1d`, `90s`; a bare number is minutes. Returns seconds.
pub fn parse_duration(text: &str) -> Result<u64, String> {
    let hint = "give a time such as 30m, 2h, 1h30m or 1d (a bare number is minutes)";
    let text: String = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();

    if text.is_empty() {
        return Err(hint.into());
    }

    let mut total: u64 = 0;
    let mut rest = text.as_str();
    let mut first = true;

    while !rest.is_empty() {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();

        if digits == 0 {
            return Err(format!("cannot read \"{text}\": {hint}"));
        }

        let amount: u64 = rest[..digits]
            .parse()
            .map_err(|_| format!("\"{text}\" is too long a time"))?;

        rest = &rest[digits..];

        let unit_length = rest.chars().take_while(char::is_ascii_alphabetic).count();
        let unit = &rest[..unit_length];

        rest = &rest[unit_length..];

        let seconds_per = match unit {
            "" if first && rest.is_empty() => 60,
            "" => return Err(format!("cannot read \"{text}\": {hint}")),
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            "d" | "day" | "days" => 86_400,
            _ => return Err(format!("cannot read \"{text}\": {hint}")),
        };

        total = amount
            .checked_mul(seconds_per)
            .and_then(|seconds| total.checked_add(seconds))
            .ok_or_else(|| format!("\"{text}\" is too long a time"))?;
        first = false;
    }

    Ok(total)
}

/// `1h 25m`, `45m`, `30s`.
pub fn describe_seconds(seconds: u64) -> String {
    let (days, hours, minutes, rest) = (
        seconds / 86_400,
        seconds % 86_400 / 3600,
        seconds % 3600 / 60,
        seconds % 60,
    );

    let mut parts = Vec::new();

    if days > 0 {
        parts.push(format!("{days}d"));
    }

    if hours > 0 {
        parts.push(format!("{hours}h"));
    }

    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }

    // Seconds only matter when that is all there is, or in the last minute of a countdown.
    if rest > 0 && (parts.is_empty() || seconds < 120) {
        parts.push(format!("{rest}s"));
    }

    if parts.is_empty() {
        "0s".into()
    } else {
        parts.join(" ")
    }
}

/// One sentence for a person.
pub fn describe(status: &AwakeStatus) -> String {
    let screen = if status.keep_screen_on {
        " The screen stays on too."
    } else {
        ""
    };

    if let Some(error) = &status.error {
        return format!("Awake is on but cannot keep the computer awake: {error}");
    }

    match status.mode {
        AwakeMode::Off => "Awake is off: the computer sleeps as usual.".into(),
        AwakeMode::Indefinite => {
            format!("Keeping the computer awake until you turn it off.{screen}")
        }
        AwakeMode::Timed => format!(
            "Keeping the computer awake for {} more.{screen}",
            describe_seconds(status.remaining_seconds.unwrap_or(0))
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_read_the_way_people_write_them() {
        assert_eq!(parse_duration("30m"), Ok(1800));
        assert_eq!(parse_duration("2h"), Ok(7200));
        assert_eq!(parse_duration("1h30m"), Ok(5400));
        assert_eq!(parse_duration("1d"), Ok(86_400));
        assert_eq!(parse_duration("90s"), Ok(90));
        assert_eq!(parse_duration("45"), Ok(2700), "a bare number is minutes");
        assert_eq!(parse_duration(" 1 h 15 m "), Ok(4500));
        assert_eq!(parse_duration("2H"), Ok(7200));
        assert_eq!(parse_duration("2 hours"), Ok(7200));
        assert_eq!(parse_duration("1d2h3m4s"), Ok(86_400 + 7200 + 180 + 4));
    }

    #[test]
    fn nonsense_is_an_explained_error_not_a_guess() {
        for text in ["", "   ", "h", "abc", "1x", "1h30", "-5m", "1.5h", "m30"] {
            let error = parse_duration(text).unwrap_err();

            assert!(error.contains("30m"), "{text:?}: {error}");
        }
    }

    #[test]
    fn huge_numbers_do_not_overflow() {
        assert!(parse_duration("99999999999999999999h").is_err());
        assert!(parse_duration("18446744073709551615d").is_err());
    }

    #[test]
    fn time_left_is_short_and_readable() {
        assert_eq!(describe_seconds(5100), "1h 25m");
        assert_eq!(describe_seconds(2700), "45m");
        assert_eq!(describe_seconds(30), "30s");
        assert_eq!(
            describe_seconds(90),
            "1m 30s",
            "the last minutes count seconds"
        );
        assert_eq!(describe_seconds(3600), "1h");
        assert_eq!(describe_seconds(90_000), "1d 1h");
        assert_eq!(describe_seconds(0), "0s");
        // Not counting seconds when there are plenty of minutes.
        assert_eq!(describe_seconds(3 * 3600 + 5 * 60 + 9), "3h 5m");
    }

    #[test]
    fn the_limits_are_checked_in_one_place_with_a_helpful_message() {
        assert_eq!(check_seconds(MIN_SECONDS), Ok(MIN_SECONDS));
        assert_eq!(check_seconds(MAX_SECONDS), Ok(MAX_SECONDS));

        for seconds in [0, MIN_SECONDS - 1, MAX_SECONDS + 1] {
            let error = check_seconds(seconds).unwrap_err();

            assert!(error.contains("1m") && error.contains("7d"), "{error}");
        }
    }

    #[test]
    fn only_changing_requests_need_the_capability() {
        assert_eq!(AwakeRequest::Status.required_capability(), None);

        for request in [
            AwakeRequest::Off,
            AwakeRequest::Indefinite,
            AwakeRequest::For { seconds: 60 },
        ] {
            assert_eq!(
                request.required_capability(),
                Some(Capability::InhibitSleep)
            );
        }
    }

    fn status(mode: AwakeMode) -> AwakeStatus {
        AwakeStatus {
            mode,
            active: mode != AwakeMode::Off,
            remaining_seconds: (mode == AwakeMode::Timed).then_some(5100),
            keep_screen_on: false,
            error: None,
        }
    }

    #[test]
    fn the_status_reads_as_a_sentence() {
        assert!(describe(&status(AwakeMode::Off)).contains("off"));
        assert!(describe(&status(AwakeMode::Indefinite)).contains("until you turn it off"));
        assert!(describe(&status(AwakeMode::Timed)).contains("1h 25m more"));

        let mut screen = status(AwakeMode::Indefinite);

        screen.keep_screen_on = true;
        assert!(describe(&screen).contains("screen stays on"));

        let mut failing = status(AwakeMode::Indefinite);

        failing.error = Some("no portal".into());
        assert!(describe(&failing).contains("cannot keep the computer awake: no portal"));
    }

    #[test]
    fn requests_travel_as_json() {
        for request in [
            AwakeRequest::Status,
            AwakeRequest::Off,
            AwakeRequest::Indefinite,
            AwakeRequest::For { seconds: 1800 },
        ] {
            let text = serde_json::to_string(&request).unwrap();

            assert_eq!(
                serde_json::from_str::<AwakeRequest>(&text).unwrap(),
                request
            );
        }
    }
}
