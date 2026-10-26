//! Weather: `weather paris`, `weather in tokyo`, `kathmandu weather`, `forecast oslo`.
//!
//! Data comes from Open-Meteo (open-meteo.com): no account or key. The place name is sent to
//! its geocoding endpoint and the resulting coordinates to its forecast endpoint, only after
//! you type one of the trigger phrases.

use crate::{
    model::{LaunchSpec, Provider, ResultKind, SearchResult},
    web::{self, Lookup, WebCache},
};
use std::time::Duration;

const GEOCODE: &str = "https://geocoding-api.open-meteo.com/v1/search";
const FORECAST: &str = "https://api.open-meteo.com/v1/forecast";
const TTL: Duration = Duration::from_secs(10 * 60);
const FORECAST_DAYS: usize = 5;

#[derive(Debug, Clone, PartialEq)]
struct Day {
    /// `YYYY-MM-DD`.
    date: String,
    high: f64,
    low: f64,
    code: i64,
}

#[derive(Debug, Clone, PartialEq)]
struct Report {
    place: String,
    temperature: f64,
    feels_like: f64,
    humidity: f64,
    wind: f64,
    code: i64,
    is_day: bool,
    days: Vec<Day>,
    imperial: bool,
}

/// A place the geocoder found.
#[derive(Debug, Clone, PartialEq)]
struct Place {
    label: String,
    latitude: f64,
    longitude: f64,
}

/// WMO weather interpretation code -> description.
fn describe(code: i64) -> &'static str {
    match code {
        0 => "Clear sky",
        1 => "Mostly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 | 48 => "Fog",
        51 | 53 | 55 => "Drizzle",
        56 | 57 => "Freezing drizzle",
        61 => "Light rain",
        63 => "Rain",
        65 => "Heavy rain",
        66 | 67 => "Freezing rain",
        71 => "Light snow",
        73 => "Snow",
        75 => "Heavy snow",
        77 => "Snow grains",
        80..=82 => "Rain showers",
        85 | 86 => "Snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm with hail",
        _ => "Unknown",
    }
}

/// Day of the week for a `YYYY-MM-DD` date (Sakamoto's method).
fn weekday(date: &str) -> Option<&'static str> {
    let mut parts = date.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: usize = parts.next()?.parse().ok()?;
    let day: i32 = parts.next()?.parse().ok()?;

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    const OFFSETS: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let year = if month < 3 { year - 1 } else { year };
    let index =
        (year + year / 4 - year / 100 + year / 400 + OFFSETS[month - 1] + day).rem_euclid(7);

    Some(["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][index as usize])
}

/// The city in `weather in paris`, `paris weather`, `forecast paris`; `None` if not a weather query.
fn parse_query(query: &str) -> Option<String> {
    let lower = query.trim().to_lowercase();

    let mut place = None;

    for prefix in [
        "weather in ",
        "weather for ",
        "weather at ",
        "weather ",
        "forecast for ",
        "forecast in ",
        "forecast ",
    ] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            place = Some(rest);
            break;
        }
    }

    if place.is_none() {
        for suffix in [" weather", " forecast"] {
            if let Some(rest) = lower.strip_suffix(suffix) {
                place = Some(rest);
                break;
            }
        }
    }

    let place = place?.trim().trim_matches(|c: char| matches!(c, '?' | '"'));
    let count = place.chars().count();

    let plausible = (2..=60).contains(&count)
        && place
            .chars()
            .all(|c| c.is_alphabetic() || matches!(c, ' ' | ',' | '-' | '.' | '\''))
        && place.chars().any(char::is_alphabetic);

    plausible.then(|| place.to_owned())
}

fn parse_place(value: &serde_json::Value) -> Option<Place> {
    let first = value.get("results")?.as_array()?.first()?;

    let name = first.get("name")?.as_str()?;
    let region = first.get("admin1").and_then(|v| v.as_str());
    let country = first.get("country").and_then(|v| v.as_str());

    let mut parts = vec![name];

    for extra in [region, country].into_iter().flatten() {
        if !parts.contains(&extra) {
            parts.push(extra);
        }
    }

    let label = parts.join(", ");

    Some(Place {
        label,
        latitude: first.get("latitude")?.as_f64()?,
        longitude: first.get("longitude")?.as_f64()?,
    })
}

fn parse_report(value: &serde_json::Value, place: &str, imperial: bool) -> Option<Report> {
    let current = value.get("current")?;
    let number = |object: &serde_json::Value, key: &str| object.get(key)?.as_f64();

    let daily = value.get("daily")?;
    let dates = daily.get("time")?.as_array()?;
    let highs = daily.get("temperature_2m_max")?.as_array()?;
    let lows = daily.get("temperature_2m_min")?.as_array()?;
    let codes = daily.get("weather_code")?.as_array()?;

    let days = dates
        .iter()
        .enumerate()
        .filter_map(|(index, date)| {
            Some(Day {
                date: date.as_str()?.to_owned(),
                high: highs.get(index)?.as_f64()?,
                low: lows.get(index)?.as_f64()?,
                code: codes.get(index)?.as_i64()?,
            })
        })
        .take(FORECAST_DAYS)
        .collect();

    Some(Report {
        place: place.to_owned(),
        temperature: number(current, "temperature_2m")?,
        feels_like: number(current, "apparent_temperature").unwrap_or(f64::NAN),
        humidity: number(current, "relative_humidity_2m").unwrap_or(f64::NAN),
        wind: number(current, "wind_speed_10m").unwrap_or(f64::NAN),
        code: current.get("weather_code")?.as_i64()?,
        is_day: current
            .get("is_day")
            .and_then(|v| v.as_i64())
            .is_none_or(|v| v != 0),
        days,
        imperial,
    })
}

fn fetch(place_name: &str, imperial: bool) -> Result<Option<Report>, String> {
    let geocoded = web::get_json(&format!(
        "{GEOCODE}?name={}&count=1&language=en&format=json",
        web::url_encode(place_name)
    ))?;

    let Some(place) = parse_place(&geocoded) else {
        return Ok(None);
    };

    let units = if imperial {
        "&temperature_unit=fahrenheit&wind_speed_unit=mph"
    } else {
        ""
    };

    let forecast = web::get_json(&format!(
        "{FORECAST}?latitude={:.4}&longitude={:.4}\
         &current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m,is_day\
         &daily=weather_code,temperature_2m_max,temperature_2m_min\
         &timezone=auto&forecast_days={FORECAST_DAYS}{units}",
        place.latitude, place.longitude
    ))?;

    parse_report(&forecast, &place.label, imperial)
        .map(Some)
        .ok_or_else(|| "the weather service sent an unexpected answer".to_owned())
}

fn degrees(value: f64, imperial: bool) -> String {
    format!("{:.0}\u{b0}{}", value, if imperial { "F" } else { "C" })
}

fn result_for(report: &Report) -> SearchResult {
    let unit = report.imperial;
    let condition = describe(report.code);

    let mut facts = Vec::new();

    if let Some(today) = report.days.first() {
        facts.push(format!(
            "H {}  L {}",
            degrees(today.high, unit),
            degrees(today.low, unit)
        ));
    }

    if report.feels_like.is_finite() {
        facts.push(format!("feels {}", degrees(report.feels_like, unit)));
    }

    let mut lines = vec![condition.to_owned()];

    if report.humidity.is_finite() {
        lines.push(format!("Humidity {:.0}%", report.humidity));
    }

    if report.wind.is_finite() {
        lines.push(format!(
            "Wind {:.0} {}",
            report.wind,
            if unit { "mph" } else { "km/h" }
        ));
    }

    lines.push(String::new());

    for (index, day) in report.days.iter().enumerate() {
        let name = if index == 0 {
            "Today"
        } else {
            weekday(&day.date).unwrap_or("")
        };

        lines.push(format!(
            "{name}|{}|{}|{}",
            describe(day.code),
            degrees(day.high, unit),
            degrees(day.low, unit)
        ));
    }

    // The icon key travels in the id so the UI can pick a glyph without parsing prose.
    let icon_key = icon_key(report.code, report.is_day);

    SearchResult::new(
        format!("weather:{icon_key}:{}", report.place),
        format!("{}  {condition}", degrees(report.temperature, unit)),
        ResultKind::Weather,
        1850,
    )
    .with_subtitle(if facts.is_empty() {
        report.place.clone()
    } else {
        format!("{} \u{b7} {}", report.place, facts.join(" \u{b7} "))
    })
    .with_detail(lines.join("\n"))
}

/// A coarse icon name for the UI: `clear`, `clear-night`, `cloudy`, `fog`, `rain`, `snow`, `storm`.
fn icon_key(code: i64, is_day: bool) -> &'static str {
    match code {
        0 | 1 => {
            if is_day {
                "clear"
            } else {
                "clear-night"
            }
        }
        2 | 3 => "cloudy",
        45 | 48 => "fog",
        51..=67 | 80..=82 => "rain",
        71..=77 | 85 | 86 => "snow",
        95..=99 => "storm",
        _ => "cloudy",
    }
}

pub struct WeatherProvider {
    imperial: bool,
    cache: WebCache<Option<Report>>,
}

impl WeatherProvider {
    pub fn new(imperial: bool) -> Self {
        Self {
            imperial,
            cache: WebCache::new(),
        }
    }
}

impl Provider for WeatherProvider {
    fn id(&self) -> &'static str {
        "weather"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let Some(place) = parse_query(query) else {
            return Vec::new();
        };

        let what = format!("the weather in {}", title_case(&place));
        let imperial = self.imperial;
        let owned = place.clone();

        match self.cache.get(&place, TTL, move || fetch(&owned, imperial)) {
            Lookup::Loading => vec![web::loading_result(&place, &what)],
            Lookup::Failed(reason) => vec![web::failure_result(&place, &what, &reason)],
            Lookup::Ready(None) => vec![
                SearchResult::new(
                    format!("notice:{place}"),
                    format!("No place called \u{201c}{}\u{201d}", title_case(&place)),
                    ResultKind::Notice,
                    1850,
                )
                .with_subtitle("Check the spelling or add a country"),
            ],
            Lookup::Ready(Some(report)) => vec![result_for(&report)],
        }
    }

    /// Enter opens a web forecast for the place.
    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let rest = id.strip_prefix("weather:")?;
        let (_, place) = rest.split_once(':')?;

        if place.is_empty() || place.chars().count() > 120 || place.chars().any(char::is_control) {
            return Some(Err("not a place".to_owned()));
        }

        Some(Ok(LaunchSpec {
            title: place.to_owned(),
            program: "xdg-open".to_owned(),
            args: vec![format!(
                "https://duckduckgo.com/?q=weather+{}",
                web::url_encode(place).replace("%20", "+")
            )],
        }))
    }
}

fn title_case(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let mut chars = word.chars();

            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEOCODE_SAMPLE: &str = r#"{"results":[{"name":"Paris","latitude":48.85341,"longitude":2.3488,"country":"France","admin1":"Île-de-France"}]}"#;

    const FORECAST_SAMPLE: &str = r#"{
        "current": {"temperature_2m": 18.4, "apparent_temperature": 17.1, "relative_humidity_2m": 62,
                    "weather_code": 2, "wind_speed_10m": 11.5, "is_day": 1},
        "daily": {"time": ["2026-10-11","2026-10-12","2026-10-13"],
                  "weather_code": [2, 61, 3],
                  "temperature_2m_max": [20.1, 17.0, 16.2],
                  "temperature_2m_min": [11.0, 10.4, 9.8]}
    }"#;

    #[test]
    fn recognises_weather_queries() {
        assert_eq!(parse_query("weather paris").as_deref(), Some("paris"));
        assert_eq!(
            parse_query("Weather in New York").as_deref(),
            Some("new york")
        );
        assert_eq!(
            parse_query("kathmandu weather").as_deref(),
            Some("kathmandu")
        );
        assert_eq!(parse_query("forecast oslo").as_deref(), Some("oslo"));
        assert_eq!(
            parse_query("weather in São Paulo").as_deref(),
            Some("são paulo")
        );
    }

    #[test]
    fn ignores_everything_else() {
        assert_eq!(parse_query("weather"), None);
        assert_eq!(parse_query("weather 12345"), None);
        assert_eq!(parse_query("firefox"), None);
        assert_eq!(parse_query("weather $(rm -rf)"), None);
    }

    #[test]
    fn finds_weekdays() {
        // 2026-10-11 is a Sunday.
        assert_eq!(weekday("2026-10-11"), Some("Sun"));
        assert_eq!(weekday("2000-01-01"), Some("Sat"));
        assert_eq!(weekday("2024-02-29"), Some("Thu"));
        assert_eq!(weekday("nonsense"), None);
    }

    #[test]
    fn labels_places_without_repeating_names() {
        let place = parse_place(&serde_json::from_str(GEOCODE_SAMPLE).unwrap()).unwrap();

        assert_eq!(place.label, "Paris, Île-de-France, France");
        assert!((place.latitude - 48.85341).abs() < 1e-9);
        assert_eq!(parse_place(&serde_json::json!({})), None);
    }

    #[test]
    fn builds_a_result_with_forecast_lines() {
        let report = parse_report(
            &serde_json::from_str(FORECAST_SAMPLE).unwrap(),
            "Paris, France",
            false,
        )
        .unwrap();

        let result = result_for(&report);

        assert_eq!(result.title, "18\u{b0}C  Partly cloudy");
        assert!(
            result
                .subtitle
                .as_deref()
                .unwrap()
                .starts_with("Paris, France")
        );
        assert!(result.subtitle.as_deref().unwrap().contains("H 20\u{b0}C"));

        let detail = result.detail.unwrap();

        assert!(detail.contains("Today|Partly cloudy|20\u{b0}C|11\u{b0}C"));
        assert!(detail.contains("Mon|Light rain|17\u{b0}C|10\u{b0}C"));
        assert!(result.id.starts_with("weather:cloudy:"));
    }

    #[test]
    fn imperial_units_are_labelled() {
        let report =
            parse_report(&serde_json::from_str(FORECAST_SAMPLE).unwrap(), "X", true).unwrap();

        assert!(result_for(&report).title.contains("\u{b0}F"));
        assert!(result_for(&report).detail.unwrap().contains("mph"));
    }

    #[test]
    fn serves_cache_and_resolves_safely() {
        let provider = WeatherProvider::new(false);
        let report = parse_report(
            &serde_json::from_str(FORECAST_SAMPLE).unwrap(),
            "Paris",
            false,
        );

        provider.cache.insert("paris", report);

        let results = provider.search("weather paris");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, ResultKind::Weather);

        let spec = provider.resolve(&results[0].id).unwrap().unwrap();

        assert_eq!(spec.program, "xdg-open");
        assert_eq!(spec.args, ["https://duckduckgo.com/?q=weather+Paris"]);
        assert!(provider.resolve("weather:clear:").unwrap().is_err());
    }
}
