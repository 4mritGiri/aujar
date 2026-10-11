//! Currency conversion: `100 usd to eur`, `$50 in npr`.
//!
//! Rates come from the free open.er-api.com service (rates by Exchange Rate API,
//! https://www.exchangerate-api.com), fetched with `curl` on a background thread and cached
//! in `~/.cache/aujar/rates.json`. Offline, the cached rates are used. Opt-in.

use crate::model::{Provider, ResultKind, SearchResult};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    process::Command,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const RATES_URL: &str = "https://open.er-api.com/v6/latest/USD";
const MAX_AGE_SECS: u64 = 12 * 3600;

#[derive(Debug, Clone, PartialEq)]
struct Rates {
    /// Units of each currency per 1 USD.
    per_usd: HashMap<String, f64>,
    /// Unix seconds.
    fetched: u64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn cache_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    base.join("aujar").join("rates.json")
}

fn parse_rates(json: &str, fetched: u64) -> Option<Rates> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let table = value.get("rates")?.as_object()?;

    let per_usd: HashMap<String, f64> = table
        .iter()
        .filter_map(|(code, rate)| Some((code.to_uppercase(), rate.as_f64()?)))
        .filter(|(_, rate)| *rate > 0.0)
        .collect();

    // A sane response has USD = 1 and plenty of currencies.
    (per_usd.get("USD") == Some(&1.0) && per_usd.len() > 20).then_some(Rates { per_usd, fetched })
}

fn load_cache() -> Option<Rates> {
    let content = fs::read_to_string(cache_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    let fetched = value.get("fetched")?.as_u64()?;

    parse_rates(&content, fetched)
}

fn fetch() -> Option<Rates> {
    let output = Command::new("curl")
        .args(["-fsS", "--max-time", "10", RATES_URL])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let body = String::from_utf8(output.stdout).ok()?;
    let rates = parse_rates(&body, now())?;

    // Cache as {"fetched": N, "rates": {...}} so offline starts still work.
    let cached = serde_json::json!({ "fetched": rates.fetched, "rates": rates.per_usd });

    if let Some(parent) = cache_path().parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(cache_path(), cached.to_string());

    Some(rates)
}

/// Currency symbols people type instead of codes.
fn symbol_code(symbol: char) -> Option<&'static str> {
    match symbol {
        '$' => Some("USD"),
        '\u{20ac}' => Some("EUR"),
        '\u{a3}' => Some("GBP"),
        '\u{a5}' => Some("JPY"),
        '\u{20b9}' => Some("INR"),
        _ => None,
    }
}

fn code_of(text: &str) -> Option<String> {
    let text = text.trim();

    if text.len() == 3 && text.chars().all(|c| c.is_ascii_alphabetic()) {
        return Some(text.to_uppercase());
    }

    let mut chars = text.chars();

    match (chars.next(), chars.next()) {
        (Some(symbol), None) => symbol_code(symbol).map(str::to_owned),
        _ => None,
    }
}

/// `"100 usd to eur"` -> `(100.0, "USD", "EUR")`; also `"$100 to eur"`.
fn parse_query(query: &str) -> Option<(f64, String, String)> {
    let lower = query.trim().to_lowercase();

    let (left, right) = [" to ", " in ", " as ", "->", "=>"]
        .iter()
        .find_map(|separator| lower.split_once(separator))?;

    let left = left.trim();
    let to = code_of(right)?;

    // Symbol prefix: "$100".
    if let Some(symbol) = left.chars().next().filter(|c| symbol_code(*c).is_some()) {
        let number: f64 = left[symbol.len_utf8()..].trim().replace(',', "").parse().ok()?;

        return Some((number, symbol_code(symbol)?.to_owned(), to));
    }

    let split = left
        .char_indices()
        .find(|(index, c)| !(c.is_ascii_digit() || matches!(c, '.' | ',') || (*index == 0 && *c == '-')))
        .map_or(left.len(), |(index, _)| index);

    let number: f64 = left[..split].replace(',', "").parse().ok()?;
    let from = code_of(&left[split..])?;

    Some((number, from, to))
}

fn convert(value: f64, from: &str, to: &str, rates: &Rates) -> Option<f64> {
    let from_rate = rates.per_usd.get(from)?;
    let to_rate = rates.per_usd.get(to)?;

    Some(value / from_rate * to_rate)
}

/// `12345.678` -> `"12,345.68"`.
fn format_money(value: f64) -> String {
    let text = format!("{:.2}", value.abs());
    let (whole, cents) = text.split_once('.').unwrap_or((&text, "00"));

    let mut grouped = String::new();

    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }

    let sign = if value < 0.0 { "-" } else { "" };

    format!("{sign}{grouped}.{cents}")
}

pub struct CurrencyProvider {
    rates: Arc<RwLock<Option<Rates>>>,
}

impl CurrencyProvider {
    pub fn new() -> Self {
        let rates = Arc::new(RwLock::new(load_cache()));
        let shared = Arc::clone(&rates);

        let _ = std::thread::Builder::new()
            .name("aujar-currency".into())
            .spawn(move || {
                loop {
                    let fresh = shared
                        .read()
                        .ok()
                        .and_then(|guard| guard.as_ref().map(|r| now().saturating_sub(r.fetched)))
                        .is_some_and(|age| age < MAX_AGE_SECS);

                    if !fresh {
                        if let Some(latest) = fetch() {
                            if let Ok(mut guard) = shared.write() {
                                *guard = Some(latest);
                            }
                        }
                    }

                    // Re-check hourly; exit when the provider is gone (we hold the only reference).
                    for _ in 0..720 {
                        std::thread::sleep(Duration::from_secs(5));

                        if Arc::strong_count(&shared) == 1 {
                            return;
                        }
                    }
                }
            });

        Self { rates }
    }

    #[cfg(test)]
    fn with_rates(per_usd: &[(&str, f64)]) -> Self {
        let per_usd = per_usd.iter().map(|(code, rate)| (code.to_string(), *rate)).collect();

        Self {
            rates: Arc::new(RwLock::new(Some(Rates { per_usd, fetched: now() }))),
        }
    }
}

impl Default for CurrencyProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for CurrencyProvider {
    fn id(&self) -> &'static str {
        "currency"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let Some((value, from, to)) = parse_query(query) else {
            return Vec::new();
        };

        let Ok(guard) = self.rates.read() else {
            return Vec::new();
        };

        let Some(rates) = guard.as_ref() else {
            return Vec::new();
        };

        let Some(answer) = convert(value, &from, &to, rates) else {
            return Vec::new();
        };

        let hours = now().saturating_sub(rates.fetched) / 3600;
        let age = if hours == 0 { "rates updated just now".to_owned() } else { format!("rates {hours}h old") };

        vec![SearchResult {
            id: format!("currency:{to}"),
            title: format!("{} {to}", format_money(answer)),
            subtitle: Some(format!("= {} {from} \u{b7} {age}", format_money(value))),
            kind: ResultKind::Calculation,
            score: 1950,
            icon: None,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> CurrencyProvider {
        CurrencyProvider::with_rates(&[("USD", 1.0), ("EUR", 0.9), ("NPR", 133.0)])
    }

    #[test]
    fn parses_codes_and_symbols() {
        assert_eq!(parse_query("100 usd to eur"), Some((100.0, "USD".into(), "EUR".into())));
        assert_eq!(parse_query("$50 in npr"), Some((50.0, "USD".into(), "NPR".into())));
        assert_eq!(parse_query("1,000 eur to usd"), Some((1000.0, "EUR".into(), "USD".into())));
        assert_eq!(parse_query("10 km to mi"), None);
    }

    #[test]
    fn converts_through_usd() {
        let results = provider().search("90 eur to usd");

        assert_eq!(results[0].title, "100.00 USD");
    }

    #[test]
    fn unknown_currency_gives_nothing() {
        assert!(provider().search("5 usd to xyz").is_empty());
    }

    #[test]
    fn formats_money_with_grouping() {
        assert_eq!(format_money(1234567.891), "1,234,567.89");
        assert_eq!(format_money(5.0), "5.00");
        assert_eq!(format_money(-1000.5), "-1,000.50");
    }

    #[test]
    fn rejects_implausible_rate_tables() {
        assert!(parse_rates(r#"{"rates":{"USD":1.0}}"#, 0).is_none());
        assert!(parse_rates("not json", 0).is_none());
    }
}
