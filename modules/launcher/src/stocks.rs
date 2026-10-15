//! Stock quotes: `stock aapl`, `quote tsla`, `$nvda`, `msft stock`, `msft share price`.
//!
//! Quotes come from Yahoo Finance's public chart endpoint. It is unofficial and keyless, so it
//! can change or rate-limit without notice; failures are reported plainly instead of hidden.
//! Prices are delayed and for information only. The ticker symbol is sent only after you type
//! one of the trigger forms.

use crate::{
    model::{LaunchSpec, Provider, ResultKind, SearchResult},
    web::{self, Lookup, WebCache},
};
use std::time::Duration;

const API: &str = "https://query1.finance.yahoo.com/v8/finance/chart/";
const TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq)]
struct Quote {
    symbol: String,
    name: Option<String>,
    currency: String,
    price: f64,
    previous_close: Option<f64>,
    day_high: Option<f64>,
    day_low: Option<f64>,
    volume: Option<f64>,
    year_high: Option<f64>,
    year_low: Option<f64>,
    exchange: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum Found {
    Quote(Box<Quote>),
    Unknown,
}

/// Ticker syntax: letters/digits with `.`, `-`, `=`, `^` (BRK-B, RELIANCE.NS, ^GSPC, EURUSD=X).
fn valid_symbol(symbol: &str) -> bool {
    (1..=12).contains(&symbol.len())
        && symbol
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '=' | '^'))
        && symbol.chars().any(|c| c.is_ascii_alphabetic())
}

/// `stock aapl`, `$aapl`, `aapl stock price` -> `AAPL`.
fn parse_query(query: &str) -> Option<String> {
    let lower = query.trim().to_lowercase();

    let mut symbol = None;

    for prefix in ["stock price ", "stocks ", "stock ", "quote ", "ticker ", "share price "] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            symbol = Some(rest);
            break;
        }
    }

    if symbol.is_none() {
        // `$aapl` (but never `$50`, which is money).
        symbol = lower
            .strip_prefix('$')
            .filter(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic()));
    }

    if symbol.is_none() {
        for suffix in [" stock price", " share price", " stock", " quote"] {
            if let Some(rest) = lower.strip_suffix(suffix) {
                symbol = Some(rest);
                break;
            }
        }
    }

    let symbol = symbol?.trim().to_uppercase();

    valid_symbol(&symbol).then_some(symbol)
}

fn parse_quote(value: &serde_json::Value, requested: &str) -> Result<Found, String> {
    let chart = value
        .get("chart")
        .ok_or_else(|| "the quote service sent an unexpected answer".to_owned())?;

    if let Some(error) = chart.get("error").filter(|error| !error.is_null()) {
        let code = error.get("code").and_then(|code| code.as_str()).unwrap_or("");

        return if code.eq_ignore_ascii_case("Not Found") {
            Ok(Found::Unknown)
        } else {
            Err("the quote service reported an error".to_owned())
        };
    }

    let meta = chart
        .get("result")
        .and_then(|result| result.as_array())
        .and_then(|result| result.first())
        .and_then(|first| first.get("meta"))
        .ok_or_else(|| "the quote service sent an unexpected answer".to_owned())?;

    let number = |key: &str| meta.get(key).and_then(|v| v.as_f64());
    let text = |key: &str| {
        meta.get(key)
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };

    let Some(price) = number("regularMarketPrice") else {
        return Ok(Found::Unknown);
    };

    Ok(Found::Quote(Box::new(Quote {
        symbol: text("symbol").unwrap_or_else(|| requested.to_owned()),
        name: text("longName").or_else(|| text("shortName")),
        currency: text("currency").unwrap_or_default(),
        price,
        previous_close: number("chartPreviousClose").or_else(|| number("previousClose")),
        day_high: number("regularMarketDayHigh"),
        day_low: number("regularMarketDayLow"),
        volume: number("regularMarketVolume"),
        year_high: number("fiftyTwoWeekHigh"),
        year_low: number("fiftyTwoWeekLow"),
        exchange: text("fullExchangeName").or_else(|| text("exchangeName")),
    })))
}

fn fetch(symbol: &str) -> Result<Found, String> {
    let response = web::get(&format!(
        "{API}{}?range=1d&interval=1d",
        web::url_encode(symbol)
    ))?;

    match response.status {
        // Yahoo answers an unknown ticker with 404 and a JSON error body.
        200 | 404 => {
            let value: serde_json::Value = serde_json::from_str(&response.body)
                .map_err(|_| "the quote service sent unreadable data".to_owned())?;

            parse_quote(&value, symbol)
        }
        429 => Err("the quote service is rate limiting requests; try again in a minute".to_owned()),
        status => Err(format!("the quote service answered HTTP {status}")),
    }
}

/// `1234.5` -> `1,234.50`; small prices keep more decimals.
pub(crate) fn price_text(value: f64) -> String {
    let decimals = if value.abs() < 1.0 { 4 } else { 2 };
    let text = format!("{:.*}", decimals, value.abs());
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));

    let mut grouped = String::new();

    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(',');
        }

        grouped.push(digit);
    }

    let sign = if value < 0.0 { "-" } else { "" };

    if fraction.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{fraction}")
    }
}

/// `1_250_000` -> `1.25M`.
pub(crate) fn compact(value: f64) -> String {
    match value.abs() {
        v if v >= 1e9 => format!("{:.2}B", value / 1e9),
        v if v >= 1e6 => format!("{:.2}M", value / 1e6),
        v if v >= 1e3 => format!("{:.1}K", value / 1e3),
        _ => format!("{value:.0}"),
    }
}

fn result_for(quote: &Quote) -> SearchResult {
    let title = if quote.currency.is_empty() {
        format!("{}  {}", quote.symbol, price_text(quote.price))
    } else {
        format!("{}  {} {}", quote.symbol, price_text(quote.price), quote.currency)
    };

    // The arrow is what the UI colours green or red.
    let change = quote.previous_close.filter(|previous| *previous != 0.0).map(|previous| {
        let delta = quote.price - previous;
        let percent = delta / previous * 100.0;
        let arrow = if delta >= 0.0 { '\u{25b2}' } else { '\u{25bc}' };

        format!("{arrow} {} ({:.2}%)", price_text(delta.abs()), percent.abs())
    });

    let mut subtitle = change.unwrap_or_else(|| "No change data".to_owned());

    if let Some(name) = &quote.name {
        subtitle.push_str(" \u{b7} ");
        subtitle.push_str(name);
    }

    let mut lines = Vec::new();
    let mut row = |label: &str, value: Option<String>| {
        if let Some(value) = value {
            lines.push(format!("{label}|{value}"));
        }
    };

    row("Previous close", quote.previous_close.map(price_text));
    row("Day range", quote.day_low.zip(quote.day_high).map(|(low, high)| {
        format!("{} \u{2013} {}", price_text(low), price_text(high))
    }));
    row("52-week range", quote.year_low.zip(quote.year_high).map(|(low, high)| {
        format!("{} \u{2013} {}", price_text(low), price_text(high))
    }));
    row("Volume", quote.volume.map(compact));
    row("Exchange", quote.exchange.clone());

    SearchResult::new(
        format!("stock:{}", quote.symbol),
        title,
        ResultKind::Stock,
        1850,
    )
    .with_subtitle(subtitle)
    .with_detail(lines.join("\n"))
}

pub struct StocksProvider {
    cache: WebCache<Found>,
}

impl StocksProvider {
    pub fn new() -> Self {
        Self {
            cache: WebCache::new(),
        }
    }
}

impl Default for StocksProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for StocksProvider {
    fn id(&self) -> &'static str {
        "stock"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let Some(symbol) = parse_query(query) else {
            return Vec::new();
        };

        let what = format!("{symbol} quote");
        let owned = symbol.clone();

        match self.cache.get(&symbol, TTL, move || fetch(&owned)) {
            Lookup::Loading => vec![web::loading_result(&symbol, &what)],
            Lookup::Failed(reason) => vec![web::failure_result(&symbol, &what, &reason)],
            Lookup::Ready(Found::Unknown) => vec![
                SearchResult::new(
                    format!("notice:{symbol}"),
                    format!("No ticker called {symbol}"),
                    ResultKind::Notice,
                    1850,
                )
                .with_subtitle("Use the exchange suffix for non-US listings, e.g. RELIANCE.NS"),
            ],
            Lookup::Ready(Found::Quote(quote)) => vec![result_for(&quote)],
        }
    }

    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let symbol = id.strip_prefix("stock:")?;

        if !valid_symbol(symbol) {
            return Some(Err("not a ticker symbol".to_owned()));
        }

        Some(Ok(LaunchSpec {
            title: symbol.to_owned(),
            program: "xdg-open".to_owned(),
            args: vec![format!("https://finance.yahoo.com/quote/{}", web::url_encode(symbol))],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"chart":{"result":[{"meta":{
        "currency":"USD","symbol":"AAPL","fullExchangeName":"NasdaqGS",
        "longName":"Apple Inc.","regularMarketPrice":227.48,"chartPreviousClose":225.0,
        "regularMarketDayHigh":228.1,"regularMarketDayLow":224.9,"regularMarketVolume":48123456,
        "fiftyTwoWeekHigh":260.1,"fiftyTwoWeekLow":164.08}}],"error":null}}"#;

    #[test]
    fn recognises_trigger_forms() {
        assert_eq!(parse_query("stock aapl").as_deref(), Some("AAPL"));
        assert_eq!(parse_query("$tsla").as_deref(), Some("TSLA"));
        assert_eq!(parse_query("msft stock").as_deref(), Some("MSFT"));
        assert_eq!(parse_query("quote brk-b").as_deref(), Some("BRK-B"));
        assert_eq!(parse_query("stock reliance.ns").as_deref(), Some("RELIANCE.NS"));
        assert_eq!(parse_query("nvda share price").as_deref(), Some("NVDA"));
    }

    #[test]
    fn money_and_plain_words_are_not_tickers() {
        assert_eq!(parse_query("$50"), None);
        assert_eq!(parse_query("$50 in npr"), None);
        assert_eq!(parse_query("firefox"), None);
        assert_eq!(parse_query("stock"), None);
        assert_eq!(parse_query("stock 123"), None);
        assert_eq!(parse_query("stock a b"), None);
        assert_eq!(parse_query("stock ../x"), None);
    }

    #[test]
    fn builds_a_quote_with_a_coloured_arrow() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let Ok(Found::Quote(quote)) = parse_quote(&value, "AAPL") else {
            panic!("expected a quote");
        };

        let result = result_for(&quote);

        assert_eq!(result.title, "AAPL  227.48 USD");
        assert!(result.subtitle.as_deref().unwrap().starts_with("\u{25b2} 2.48 (1.10%)"));
        assert!(result.subtitle.as_deref().unwrap().ends_with("Apple Inc."));

        let detail = result.detail.unwrap();

        assert!(detail.contains("Volume|48.12M"));
        assert!(detail.contains("Day range|224.90 \u{2013} 228.10"));
    }

    #[test]
    fn falling_prices_get_a_down_arrow() {
        let mut quote = Quote {
            symbol: "X".into(),
            name: None,
            currency: String::new(),
            price: 90.0,
            previous_close: Some(100.0),
            day_high: None,
            day_low: None,
            volume: None,
            year_high: None,
            year_low: None,
            exchange: None,
        };

        assert!(result_for(&quote).subtitle.unwrap().starts_with("\u{25bc} 10.00 (10.00%)"));

        quote.previous_close = None;

        assert_eq!(result_for(&quote).subtitle.as_deref(), Some("No change data"));
    }

    #[test]
    fn unknown_tickers_and_service_errors_differ() {
        let unknown: serde_json::Value = serde_json::from_str(
            r#"{"chart":{"result":null,"error":{"code":"Not Found","description":"No data"}}}"#,
        )
        .unwrap();
        let broken: serde_json::Value =
            serde_json::from_str(r#"{"chart":{"result":null,"error":{"code":"Internal"}}}"#).unwrap();

        assert_eq!(parse_quote(&unknown, "ZZZZ"), Ok(Found::Unknown));
        assert!(parse_quote(&broken, "ZZZZ").is_err());
        assert!(parse_quote(&serde_json::json!({}), "ZZZZ").is_err());
    }

    #[test]
    fn formats_numbers() {
        assert_eq!(price_text(1234567.891), "1,234,567.89");
        assert_eq!(price_text(0.12345), "0.1235");
        assert_eq!(compact(1_250_000.0), "1.25M");
        assert_eq!(compact(512.0), "512");
    }

    #[test]
    fn serves_cache_and_resolves_safely() {
        let provider = StocksProvider::new();
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();

        provider.cache.insert("AAPL", parse_quote(&value, "AAPL").unwrap());

        assert_eq!(provider.search("stock aapl").len(), 1);

        let spec = provider.resolve("stock:AAPL").unwrap().unwrap();

        assert_eq!(spec.args, ["https://finance.yahoo.com/quote/AAPL"]);
        assert!(provider.resolve("stock:../../x").unwrap().is_err());
    }
}
