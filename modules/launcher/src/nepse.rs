//! NEPSE (Nepal Stock Exchange) lookups: `nepse nabil`, `nabil nepse`, `nepse nic asia` (any
//! part of a symbol or company name), `nepse` (the index), `nepse gainers`, `nepse losers`,
//! `nepse open`, `nepse sectors`, `nepse holidays`, `nepse depth nabil`, `nepse news nabil`.
//! An exact symbol also shows company details (sector, EPS, P/E, ...).
//!
//! Data comes from the Fintech Nepal warehouse API (`warehouse.fintechnepal.com.np`), which
//! needs an API key: set `AUJAR_NEPSE_KEY`, or put the key alone in
//! `~/.config/aujar/nepse.key` (the daemon runs under systemd, where a file is easier than an
//! environment variable). The key is sent only to that host, and the symbol you typed is only
//! sent after one of the trigger forms above.
//!
//! The API's OpenAPI file lists its paths but not the shape of the answers, so the parsing here
//! is deliberately tolerant: it finds the list of records wherever it sits in the JSON, and
//! reads each field by a few likely names (case and punctuation ignored). If a field cannot
//! be found it is simply left out; if no price can be found at all, the launcher says so.
//! Prices are for information only.

mod alerts;

pub use alerts::spawn_watcher as spawn_alert_watcher;

use crate::{
    model::{LaunchSpec, Provider, ResultKind, SearchResult},
    stocks::{compact, price_text},
    web::{self, Lookup, WebCache},
};
use serde_json::{Map, Value};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const API: &str = "https://warehouse.fintechnepal.com.np/api/nepse";
const TTL: Duration = Duration::from_secs(60);
/// Company details change slowly.
const DETAILS_TTL: Duration = Duration::from_secs(900);
const MAX_MATCHES: usize = 8;
const KEY_ENV: &str = "AUJAR_NEPSE_KEY";
/// The official NEPSE site. A company page is `{SITE}/company/detail/<securityId>`.
const SITE: &str = "https://nepalstock.com.np";

// ---- API key -------------------------------------------------------------------------------

/// `~/.config/aujar`, next to settings.toml.
fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;

    Some(base.join("aujar"))
}

fn key_path() -> Option<PathBuf> {
    Some(config_dir()?.join("nepse.key"))
}

/// Keys are letters, digits and a few separators, with at most one space (a scheme prefix such
/// as `Api-Key`). Anything else (quotes, line breaks) is rejected before it reaches curl.
fn valid_key(key: &str) -> bool {
    (8..=200).contains(&key.len())
        && key.matches(' ').count() <= 1
        && key.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '.' | '_' | '-' | '~' | '+' | '/' | '=' | ' ' | ':')
        })
}

fn api_key() -> Option<String> {
    let raw = std::env::var(KEY_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| fs::read_to_string(key_path()?).ok())?;
    let key = raw.trim().to_owned();

    valid_key(&key).then_some(key)
}

fn no_key_result() -> SearchResult {
    SearchResult::new(
        "notice:nepse-key",
        "NEPSE needs an API key",
        ResultKind::Notice,
        1850,
    )
    .with_subtitle("Set AUJAR_NEPSE_KEY, or save the key in ~/.config/aujar/nepse.key")
}

// ---- Query ---------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PageKind {
    Index,
    Open,
    Gainers,
    Losers,
    Sectors,
    Holidays,
}

/// Pages about one company.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ExtraKind {
    Depth,
    News,
}

impl ExtraKind {
    fn word(self) -> &'static str {
        match self {
            Self::Depth => "depth",
            Self::News => "news",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Depth => "/market-depth/",
            Self::News => "/company/news/symbol/",
        }
    }

    fn from_word(word: &str) -> Option<Self> {
        match word {
            "depth" | "orderbook" | "book" => Some(Self::Depth),
            "news" => Some(Self::News),
            _ => None,
        }
    }
}

impl PageKind {
    fn key(self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::Open => "open",
            Self::Gainers => "gainers",
            Self::Losers => "losers",
            Self::Sectors => "sectors",
            Self::Holidays => "holidays",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Index => "/index/",
            Self::Open => "/is-open/",
            Self::Gainers => "/top-gainers/",
            Self::Losers => "/top-losers/",
            Self::Sectors => "/subindices/",
            Self::Holidays => "/holidays/",
        }
    }

    fn from_word(word: &str) -> Option<Self> {
        match word {
            "index" | "nepse" => Some(Self::Index),
            "open" | "status" | "market" => Some(Self::Open),
            "gainers" | "gainer" | "gain" => Some(Self::Gainers),
            "losers" | "loser" | "loss" => Some(Self::Losers),
            "sectors" | "sector" | "subindices" => Some(Self::Sectors),
            "holidays" | "holiday" => Some(Self::Holidays),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Intent {
    /// Part of a symbol or company name, lower-case.
    Find(String),
    Page(PageKind),
    /// `nepse depth nabil`, `nepse news nabil`.
    Extra(ExtraKind, String),
    /// `nepse depth` without a symbol: show an example.
    Usage(&'static str),
    /// `nepse alert nabil above 600`; `above` is `None` when the direction is left to us.
    Alert {
        symbol: String,
        above: Option<bool>,
        price: f64,
    },
    /// `nepse alerts`: the saved alerts.
    Alerts,
}

fn valid_security_id(id: &str) -> bool {
    (1..=10).contains(&id.len()) && id.chars().all(|c| c.is_ascii_digit())
}

fn valid_symbol(symbol: &str) -> bool {
    (2..=12).contains(&symbol.len())
        && symbol.chars().all(|c| c.is_ascii_alphanumeric())
        && symbol.chars().any(|c| c.is_ascii_alphabetic())
}

fn find_intent(words: &[&str]) -> Option<Intent> {
    let text = words.join(" ");

    ((2..=40).contains(&text.len())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '&' | '-' | '.'))
        && text.chars().any(|c| c.is_ascii_alphabetic()))
    .then_some(Intent::Find(text))
}

fn alert_price(text: &str) -> Option<f64> {
    let cleaned = text
        .trim_start_matches("rs")
        .trim_start_matches("npr")
        .replace(',', "");

    cleaned
        .parse::<f64>()
        .ok()
        .filter(|price| price.is_finite() && *price > 0.0 && *price < 10_000_000.0)
}

const ALERT_EXAMPLE: &str = "nepse alert nabil above 600";

/// `nabil above 600`, `nabil below 500`, `nabil 600` (direction picked from the current price).
fn alert_intent(rest: &[&str]) -> Intent {
    let (symbol, above, price) = match rest {
        [symbol, price] => (*symbol, None, *price),
        [symbol, direction, price] => {
            let above = match *direction {
                "above" | "over" | "up" | ">" | ">=" => Some(true),
                "below" | "under" | "down" | "<" | "<=" => Some(false),
                _ => return Intent::Usage(ALERT_EXAMPLE),
            };

            (*symbol, above, *price)
        }
        _ => return Intent::Usage(ALERT_EXAMPLE),
    };

    let symbol = symbol.to_uppercase();

    match alert_price(price) {
        Some(price) if valid_symbol(&symbol) => Intent::Alert {
            symbol,
            above,
            price,
        },
        _ => Intent::Usage(ALERT_EXAMPLE),
    }
}

fn parse_query(query: &str) -> Option<Intent> {
    let lower = query.trim().to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    match words.as_slice() {
        ["nepse"] => Some(Intent::Page(PageKind::Index)),
        ["nepse", word] if PageKind::from_word(word).is_some() => {
            PageKind::from_word(word).map(Intent::Page)
        }
        ["nepse", "alerts"] => Some(Intent::Alerts),
        ["nepse", "alert", rest @ ..] => Some(alert_intent(rest)),
        ["nepse", "depth"] => Some(Intent::Usage("nepse depth nabil")),
        ["nepse", "news"] => Some(Intent::Usage("nepse news nabil")),
        ["nepse", kind, symbol] if ExtraKind::from_word(kind).is_some() => {
            let kind = ExtraKind::from_word(kind)?;
            let symbol = symbol.to_uppercase();

            valid_symbol(&symbol).then_some(Intent::Extra(kind, symbol))
        }
        ["nepse", rest @ ..] => find_intent(rest),
        [rest @ .., "nepse"] if !rest.is_empty() => find_intent(rest),
        _ => None,
    }
}

/// Companies whose symbol or name matches `text`, best first: exact symbol, symbol prefix, all
/// words starting a word of the name, all words inside the name, then symbol contains.
fn find_matches<'a>(quotes: &'a [Quote], text: &str) -> Vec<(&'a Quote, u32)> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let joined = text.replace(' ', "");
    let mut hits = Vec::new();

    for quote in quotes {
        let symbol = quote.symbol.to_lowercase();
        let name = quote.name.as_deref().unwrap_or("").to_lowercase();

        let score = if symbol == joined {
            1850
        } else if symbol.starts_with(&joined) {
            1800
        } else if tokens.iter().all(|token| {
            name.split(|c: char| !c.is_alphanumeric())
                .any(|word| word.starts_with(token))
        }) {
            1700
        } else if tokens.iter().all(|token| name.contains(token)) {
            1600
        } else if joined.len() >= 3 && symbol.contains(&joined) {
            1500
        } else {
            continue;
        };

        hits.push((quote, score));
    }

    hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.symbol.cmp(&b.0.symbol)));
    hits.truncate(MAX_MATCHES);
    hits
}

// ---- Tolerant JSON reading -----------------------------------------------------------------

/// Lower-case letters and digits only: `lastTradedPrice`, `last_traded_price` and
/// `Last Traded Price` all become `lasttradedprice`.
fn norm(key: &str) -> String {
    key.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The first non-null field whose normalised name is one of `aliases` (tried in that order).
fn field<'a>(row: &'a Map<String, Value>, aliases: &[&str]) -> Option<&'a Value> {
    aliases.iter().find_map(|alias| {
        row.iter()
            .find(|(key, value)| !value.is_null() && norm(key) == *alias)
            .map(|(_, value)| value)
    })
}

fn num(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().replace(',', "").parse().ok(),
        _ => None,
    };

    number.filter(|number| number.is_finite())
}

fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_owned()),
        _ => None,
    }
}

fn number_of(row: &Map<String, Value>, aliases: &[&str]) -> Option<f64> {
    field(row, aliases).and_then(num)
}

/// The first list of records in `value`, wherever it is nested (`[..]`, `{"data": [..]}`, ...).
fn find_rows(value: &Value) -> Vec<&Map<String, Value>> {
    match value {
        Value::Array(items) => {
            let rows: Vec<_> = items.iter().filter_map(Value::as_object).collect();

            if !rows.is_empty() {
                return rows;
            }

            items
                .iter()
                .map(find_rows)
                .find(|rows| !rows.is_empty())
                .unwrap_or_default()
        }
        Value::Object(map) => map
            .values()
            .filter(|inner| inner.is_array() || inner.is_object())
            .map(find_rows)
            .find(|rows| !rows.is_empty())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Records for an answer that is a list, or an object keyed by name (`{"data": {"NEPSE Index":
/// {..}, "Sensitive Index": {..}}}`): every object that carries numbers.
fn records(value: &Value) -> Vec<&Map<String, Value>> {
    fn walk<'a>(value: &'a Value, out: &mut Vec<&'a Map<String, Value>>) {
        match value {
            Value::Object(map) => {
                if map.values().any(|inner| num(inner).is_some()) {
                    out.push(map);
                } else {
                    map.values().for_each(|inner| walk(inner, out));
                }
            }
            Value::Array(items) => items.iter().for_each(|inner| walk(inner, out)),
            _ => {}
        }
    }

    let rows = find_rows(value);

    if !rows.is_empty() {
        return rows;
    }

    let mut out = Vec::new();

    walk(value, &mut out);

    out
}

// ---- Quotes --------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Quote {
    symbol: String,
    /// NEPSE's own id for the company (`securityId`), used in its company page URL.
    security_id: Option<String>,
    name: Option<String>,
    price: f64,
    previous_close: Option<f64>,
    change_percent: Option<f64>,
    point_change: Option<f64>,
    open: Option<f64>,
    high: Option<f64>,
    low: Option<f64>,
    volume: Option<f64>,
    turnover: Option<f64>,
}

fn quote_from(row: &Map<String, Value>) -> Option<Quote> {
    let symbol = field(
        row,
        &[
            "symbol",
            "stocksymbol",
            "securitysymbol",
            "scripsymbol",
            "ticker",
        ],
    )
    .and_then(text)?
    .to_uppercase();

    let price = number_of(
        row,
        &[
            "lasttradedprice",
            "ltp",
            "closeprice",
            "lastprice",
            "price",
            "close",
            "closingprice",
        ],
    )?;

    // `"397"` in one answer, `8098` in another.
    let security_id = field(row, &["securityid", "companyid"]).and_then(|value| match value {
        Value::String(id) => Some(id.trim().to_owned()),
        Value::Number(id) => id.as_u64().map(|id| id.to_string()),
        _ => None,
    });

    Some(Quote {
        symbol,
        security_id: security_id.filter(|id| valid_security_id(id)),
        name: field(
            row,
            &[
                "securityname",
                "companyname",
                "name",
                "fullname",
                "companyfullname",
            ],
        )
        .and_then(text),
        price,
        previous_close: number_of(
            row,
            &[
                "previousclose",
                "previousdayclose",
                "previousdaycloseprice",
                "prevclose",
                "previousclosingprice",
            ],
        ),
        change_percent: number_of(
            row,
            &[
                "percentagechange",
                "percentchange",
                "perchange",
                "changepercent",
                "pricechangepercent",
                "changepct",
            ],
        ),
        point_change: number_of(
            row,
            &["pointchange", "pricechange", "change", "diff", "difference"],
        ),
        open: number_of(row, &["openprice", "open"]),
        high: number_of(row, &["highprice", "high", "dayhigh", "maxprice"]),
        low: number_of(row, &["lowprice", "low", "daylow", "minprice"]),
        volume: number_of(
            row,
            &[
                "totaltradequantity",
                "totaltradedquantity",
                "totaltradedvolume",
                "volume",
                "sharestraded",
                "totalquantity",
                "lasttradedvolume",
            ],
        ),
        turnover: number_of(
            row,
            &[
                "totaltradevalue",
                "totaltradedvalue",
                "turnover",
                "totalturnover",
                "amount",
            ],
        ),
    })
}

/// Rise or fall as `(amount, percent)`, from the previous close when the API gives one,
/// else from the percentage or the point change.
fn change_of(quote: &Quote) -> Option<(f64, f64)> {
    if let Some(previous) = quote.previous_close.filter(|previous| *previous != 0.0) {
        let delta = quote.price - previous;

        return Some((delta, delta / previous * 100.0));
    }

    if let Some(percent) = quote.change_percent.filter(|percent| *percent > -100.0) {
        let previous = quote.price / (1.0 + percent / 100.0);

        return Some((quote.price - previous, percent));
    }

    quote.point_change.map(|delta| {
        let previous = quote.price - delta;
        let percent = if previous != 0.0 {
            delta / previous * 100.0
        } else {
            0.0
        };

        (delta, percent)
    })
}

fn arrow_text(delta: f64, percent: f64) -> String {
    let arrow = if delta >= 0.0 { '\u{25b2}' } else { '\u{25bc}' };

    format!(
        "{arrow} {} ({:.2}%)",
        price_text(delta.abs()),
        percent.abs()
    )
}

fn rs(value: f64) -> String {
    format!("Rs {}", price_text(value))
}

fn quote_result(quote: &Quote, score: u32, extra: &[String]) -> SearchResult {
    let mut subtitle = change_of(quote)
        .map(|(delta, percent)| arrow_text(delta, percent))
        .unwrap_or_else(|| "No change data".to_owned());

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

    row("Previous close", quote.previous_close.map(rs));
    row("Open", quote.open.map(rs));
    row(
        "Day range",
        quote
            .low
            .zip(quote.high)
            .map(|(low, high)| format!("{} \u{2013} {}", price_text(low), price_text(high))),
    );
    row("Volume", quote.volume.map(compact));
    row(
        "Turnover",
        quote.turnover.map(|value| format!("Rs {}", compact(value))),
    );

    // Details may repeat something the price list already gave: keep the first.
    for line in extra {
        let label = line.split('|').next().unwrap_or_default();

        if !lines
            .iter()
            .any(|have| have.split('|').next() == Some(label))
        {
            lines.push(line.clone());
        }
    }

    lines.push("Exchange|NEPSE".to_owned());

    // `nepse:NABIL:397` when the company id is known, so Enter can open its official page.
    let id = match &quote.security_id {
        Some(security_id) => format!("nepse:{}:{security_id}", quote.symbol),
        None => format!("nepse:{}", quote.symbol),
    };

    SearchResult::new(
        id,
        format!("{}  {}", quote.symbol, rs(quote.price)),
        ResultKind::Stock,
        score,
    )
    .with_subtitle(subtitle)
    .with_detail(lines.join("\n"))
}

// ---- Pages (index, market status, movers) ----------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Page {
    title: String,
    subtitle: String,
    lines: Vec<String>,
}

fn parse_index(value: &Value) -> Result<Page, String> {
    let rows = records(value);

    // The answer may list several indices; prefer the main one.
    let record = rows
        .iter()
        .find(|row| {
            row.values()
                .filter_map(text)
                .any(|name| norm(&name) == "nepse" || norm(&name) == "nepseindex")
        })
        .copied()
        .or_else(|| rows.first().copied())
        .ok_or_else(|| "the NEPSE service sent an unexpected answer".to_owned())?;

    // `close` is the latest value; `currentValue` can lag behind it.
    let current = number_of(
        record,
        &[
            "close",
            "closeprice",
            "currentvalue",
            "indexvalue",
            "currentindex",
            "index",
            "value",
            "lastvalue",
        ],
    )
    .ok_or_else(|| "the NEPSE service sent no index value".to_owned())?;

    // The sign comes from the previous close: the API's own `change` is an absolute amount.
    let previous = number_of(record, &["previousclose", "previousdayclose"]);

    let moved = match previous.filter(|previous| *previous != 0.0) {
        Some(previous) => {
            let delta = current - previous;

            Some((delta, delta / previous * 100.0))
        }
        None => match (
            number_of(record, &["change", "pointchange", "indexchange"]),
            number_of(
                record,
                &[
                    "percentagechange",
                    "perchange",
                    "percentchange",
                    "changepercent",
                ],
            ),
        ) {
            (Some(delta), Some(percent)) => Some((delta, percent)),
            (Some(delta), None) => {
                let base = current - delta;

                Some((
                    delta,
                    if base != 0.0 {
                        delta / base * 100.0
                    } else {
                        0.0
                    },
                ))
            }
            (None, Some(percent)) => Some((percent, percent)),
            (None, None) => None,
        },
    };

    let subtitle = moved
        .map(|(delta, percent)| arrow_text(delta, percent))
        .unwrap_or_else(|| "Nepal Stock Exchange".to_owned());

    let mut lines = Vec::new();
    let mut row = |label: &str, value: Option<String>| {
        if let Some(value) = value {
            lines.push(format!("{label}|{value}"));
        }
    };

    row("Previous close", previous.map(price_text));
    row(
        "Day range",
        number_of(record, &["low", "lowvalue", "daylow"])
            .zip(number_of(record, &["high", "highvalue", "dayhigh"]))
            .map(|(low, high)| format!("{} \u{2013} {}", price_text(low), price_text(high))),
    );
    row(
        "52-week range",
        number_of(record, &["fiftytwoweeklow"])
            .zip(number_of(record, &["fiftytwoweekhigh"]))
            .map(|(low, high)| format!("{} \u{2013} {}", price_text(low), price_text(high))),
    );
    row(
        "Turnover",
        number_of(record, &["turnover", "totalturnover"]).map(|v| format!("Rs {}", compact(v))),
    );
    row(
        "Volume",
        number_of(record, &["volume", "totalvolume"]).map(compact),
    );

    Ok(Page {
        title: format!("NEPSE Index  {}", price_text(current)),
        subtitle,
        lines,
    })
}

fn parse_open(value: &Value) -> Result<Page, String> {
    fn open_in(value: &Value) -> Option<bool> {
        match value {
            Value::Bool(open) => Some(*open),
            Value::String(status) => {
                let status = status.to_lowercase();

                if status.contains("close") {
                    Some(false)
                } else if status.contains("open") {
                    Some(true)
                } else {
                    None
                }
            }
            Value::Object(map) => [
                "isopen",
                "open",
                "marketopen",
                "isnepseopen",
                "status",
                "marketstatus",
            ]
            .iter()
            .find_map(|alias| {
                map.iter()
                    .find(|(key, _)| norm(key) == *alias)
                    .and_then(|(_, inner)| open_in(inner))
            })
            .or_else(|| map.values().find_map(open_in)),
            _ => None,
        }
    }

    // `"asOf": "2026-10-14T15:00:00"` -> `2026-10-14 15:00`.
    fn as_of(value: &Value) -> Option<String> {
        match value {
            Value::Object(map) => map
                .iter()
                .find(|(key, inner)| norm(key) == "asof" && inner.is_string())
                .and_then(|(_, inner)| inner.as_str().map(str::to_owned))
                .or_else(|| map.values().find_map(as_of)),
            _ => None,
        }
    }

    let open =
        open_in(value).ok_or_else(|| "the NEPSE service sent an unexpected answer".to_owned())?;

    let subtitle = match as_of(value) {
        Some(time) => {
            let time = time.replace('T', " ");

            format!("As of {}", time.get(..16).unwrap_or(&time))
        }
        None => "Nepal Stock Exchange".to_owned(),
    };

    Ok(Page {
        title: if open {
            "NEPSE market is open"
        } else {
            "NEPSE market is closed"
        }
        .to_owned(),
        subtitle,
        lines: Vec::new(),
    })
}

fn parse_movers(value: &Value, rising: bool) -> Result<Page, String> {
    let quotes: Vec<Quote> = find_rows(value)
        .into_iter()
        .filter_map(quote_from)
        .take(10)
        .collect();

    if quotes.is_empty() {
        return Err("the NEPSE service sent no price rows".to_owned());
    }

    let lines = quotes
        .iter()
        .map(|quote| {
            // Direction comes from which list this is, not from the sign the API happens to use.
            let change = change_of(quote)
                .map(|(delta, percent)| {
                    let (delta, percent) = (delta.abs(), percent.abs());
                    let (delta, percent) = if rising {
                        (delta, percent)
                    } else {
                        (-delta, -percent)
                    };

                    format!("  {}", arrow_text(delta, percent))
                })
                .unwrap_or_default();

            format!("{}|{}{change}", quote.symbol, rs(quote.price))
        })
        .collect::<Vec<_>>();

    let (arrow, word) = if rising {
        ('\u{25b2}', "gainers")
    } else {
        ('\u{25bc}', "losers")
    };

    Ok(Page {
        title: format!("NEPSE top {word}"),
        subtitle: format!(
            "{arrow} Top {} by price {}",
            quotes.len(),
            if rising { "gain" } else { "loss" }
        ),
        lines,
    })
}

fn parse_page(kind: PageKind, value: &Value, today: &str) -> Result<Page, String> {
    match kind {
        PageKind::Index => parse_index(value),
        PageKind::Open => parse_open(value),
        PageKind::Gainers => parse_movers(value, true),
        PageKind::Losers => parse_movers(value, false),
        PageKind::Sectors => parse_sectors(value),
        PageKind::Holidays => parse_holidays(value, today),
    }
}

fn page_result(id: String, page: &Page) -> SearchResult {
    let mut result = SearchResult::new(id, page.title.clone(), ResultKind::Stock, 1850)
        .with_subtitle(page.subtitle.clone());

    if !page.lines.is_empty() {
        result = result.with_detail(page.lines.join("\n"));
    }

    result
}

// ---- Sectors, holidays, depth, news ---------------------------------------------------------

fn first_text(row: &Map<String, Value>, aliases: &[&str]) -> Option<String> {
    field(row, aliases).and_then(text)
}

/// A date as `YYYY-MM-DD` when the text starts with one (`2026-10-14T15:00:00` too).
fn date_part(value: &str) -> Option<String> {
    let head = value.get(..10)?;
    let bytes = head.as_bytes();
    let shaped = bytes.iter().enumerate().all(|(index, byte)| match index {
        4 | 7 => *byte == b'-',
        _ => byte.is_ascii_digit(),
    });

    shaped.then(|| head.to_owned())
}

/// `YYYY-MM-DD` for a count of days since 1970-01-01 (proleptic Gregorian).
fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}")
}

/// Today's date in Nepal (UTC+5:45).
fn today_npt() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
        + (5 * 3600 + 45 * 60);

    civil_date(seconds.div_euclid(86_400))
}

fn parse_sectors(value: &Value) -> Result<Page, String> {
    let mut sectors: Vec<(String, f64, Option<f64>)> = Vec::new();

    for record in records(value) {
        let Some(name) = first_text(
            record,
            &["index", "indexname", "name", "sectorname", "sector"],
        ) else {
            continue;
        };
        let Some(current) = number_of(
            record,
            &[
                "close",
                "closeprice",
                "currentvalue",
                "indexvalue",
                "currentindex",
                "value",
                "lastvalue",
            ],
        ) else {
            continue;
        };

        let percent =
            match number_of(record, &["previousclose", "previousdayclose"]).filter(|p| *p != 0.0) {
                Some(previous) => Some((current - previous) / previous * 100.0),
                None => number_of(
                    record,
                    &[
                        "percentagechange",
                        "perchange",
                        "percentchange",
                        "changepercent",
                    ],
                ),
            };

        sectors.push((name, current, percent));
    }

    if sectors.is_empty() {
        return Err("the NEPSE service sent no sector indices".to_owned());
    }

    // Best performers first.
    sectors.sort_by(|a, b| {
        b.2.unwrap_or(f64::NEG_INFINITY)
            .total_cmp(&a.2.unwrap_or(f64::NEG_INFINITY))
            .then_with(|| a.0.cmp(&b.0))
    });

    let total = sectors.len();
    let lines = sectors
        .iter()
        .take(16)
        .map(|(name, current, percent)| match percent {
            Some(percent) => format!(
                "{name}|{}  {}",
                price_text(*current),
                arrow_text(*percent, *percent)
            ),
            None => format!("{name}|{}", price_text(*current)),
        })
        .collect();

    Ok(Page {
        title: "NEPSE sectors".to_owned(),
        subtitle: format!("{total} sub-indices, best first"),
        lines,
    })
}

fn parse_holidays(value: &Value, today: &str) -> Result<Page, String> {
    let mut holidays: Vec<(String, String)> = Vec::new();

    for row in find_rows(value) {
        let Some(date) = first_text(
            row,
            &[
                "holidaydate",
                "date",
                "day",
                "eventdate",
                "holidayday",
                "fromdate",
            ],
        ) else {
            continue;
        };
        let name = first_text(
            row,
            &[
                "description",
                "holidaydescription",
                "holiday",
                "name",
                "title",
                "occasion",
                "remarks",
                "reason",
            ],
        )
        .unwrap_or_else(|| "Holiday".to_owned());

        holidays.push((date_part(&date).unwrap_or(date), name));
    }

    if holidays.is_empty() {
        return Err("the NEPSE service sent no holidays".to_owned());
    }

    holidays.sort();

    let upcoming: Vec<_> = holidays
        .iter()
        .filter(|(date, _)| date.as_str() >= today)
        .collect();

    if upcoming.is_empty() {
        return Ok(Page {
            title: "No more NEPSE holidays listed".to_owned(),
            subtitle: format!("{} holidays this year, all before today", holidays.len()),
            lines: Vec::new(),
        });
    }

    let (next_date, next_name) = upcoming[0];

    Ok(Page {
        title: "NEPSE holidays".to_owned(),
        subtitle: format!("Next: {next_date} \u{b7} {next_name}"),
        lines: upcoming
            .iter()
            .take(10)
            .map(|(date, name)| format!("{date}|{name}"))
            .collect(),
    })
}

/// A list of records under a key that names one side of the book.
fn depth_side<'a>(
    value: &'a Value,
    matches: &dyn Fn(&str) -> bool,
    depth: usize,
) -> Option<&'a Vec<Value>> {
    if depth > 4 {
        return None;
    }

    match value {
        Value::Object(map) => map
            .iter()
            .find_map(|(key, inner)| match inner {
                Value::Array(items) if matches(&norm(key)) && !items.is_empty() => Some(items),
                _ => None,
            })
            .or_else(|| {
                map.values()
                    .filter(|inner| inner.is_object() || inner.is_array())
                    .find_map(|inner| depth_side(inner, matches, depth + 1))
            }),
        Value::Array(items) => items
            .first()
            .and_then(|first| depth_side(first, matches, depth + 1)),
        _ => None,
    }
}

/// `(price, quantity, orders)` for each order-book row.
fn depth_rows(items: &[Value]) -> Vec<(f64, f64, Option<f64>)> {
    items
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|row| {
            let price = number_of(
                row,
                &[
                    "orderbookorderprice",
                    "orderprice",
                    "price",
                    "rate",
                    "bidprice",
                    "askprice",
                ],
            )?;
            let quantity = number_of(
                row,
                &[
                    "quantity",
                    "qty",
                    "orderquantity",
                    "totalquantity",
                    "volume",
                ],
            )?;

            Some((
                price,
                quantity,
                number_of(
                    row,
                    &["ordercount", "orders", "noofordersnumber", "numberoforders"],
                ),
            ))
        })
        .collect()
}

fn parse_depth(value: &Value, symbol: &str) -> Result<Page, String> {
    let mut buys = depth_side(
        value,
        &|key: &str| key.contains("buy") || key.contains("bid"),
        0,
    )
    .map(|items| depth_rows(items))
    .unwrap_or_default();
    let mut sells = depth_side(
        value,
        &|key: &str| key.contains("sell") || key.starts_with("ask"),
        0,
    )
    .map(|items| depth_rows(items))
    .unwrap_or_default();

    if buys.is_empty() && sells.is_empty() {
        return Ok(Page {
            title: format!("No market depth for {symbol}"),
            subtitle: "The order book is empty (the market may be closed)".to_owned(),
            lines: Vec::new(),
        });
    }

    buys.sort_by(|a, b| b.0.total_cmp(&a.0));
    sells.sort_by(|a, b| a.0.total_cmp(&b.0));

    let side = |label: &str, rows: &[(f64, f64, Option<f64>)]| -> Vec<String> {
        rows.iter()
            .take(5)
            .enumerate()
            .map(|(index, (price, quantity, orders))| {
                let orders = orders
                    .map(|n| format!(" ({} orders)", n as u64))
                    .unwrap_or_default();

                format!(
                    "{label} {}|{} \u{d7} {}{orders}",
                    index + 1,
                    rs(*price),
                    compact(*quantity)
                )
            })
            .collect()
    };

    let mut lines = side("Buy", &buys);
    lines.extend(side("Sell", &sells));

    let mut flat = Vec::new();

    flatten(value, &mut flat, 0);

    for (label, alias) in [
        ("Total buy qty", "totalbuyquantity"),
        ("Total sell qty", "totalsellquantity"),
    ] {
        if let Some(total) = flat
            .iter()
            .find(|(key, _)| key == alias)
            .and_then(|(_, found)| num(found))
        {
            lines.push(format!("{label}|{}", compact(total)));
        }
    }

    let bid = buys.first().map(|row| format!("Best bid {}", rs(row.0)));
    let ask = sells.first().map(|row| format!("best ask {}", rs(row.0)));
    let subtitle = [bid, ask]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" \u{b7} ");

    Ok(Page {
        title: format!("{symbol} market depth"),
        subtitle,
        lines,
    })
}

fn parse_news(value: &Value, symbol: &str) -> Result<Page, String> {
    // The real answer wraps each item: `{"companyNews": {"newsHeadline": ..., "security": {..}}}`.
    // `(date, label, headline)`; the label is the date, else the kind of news.
    let items: Vec<(String, String, String)> = find_rows(value)
        .into_iter()
        .filter_map(|row| {
            let headline = deep_field(
                row,
                &[
                    "newsheadline",
                    "headline",
                    "newstitle",
                    "title",
                    "subject",
                    "newsdescription",
                ],
            )
            .and_then(text)?;

            let date = deep_field(
                row,
                &[
                    "addeddate",
                    "publisheddate",
                    "newsdate",
                    "createddate",
                    "createdat",
                    "modifieddate",
                    "applicationdate",
                    "submitteddate",
                    "date",
                ],
            )
            .and_then(text)
            .map(|date| date_part(&date).unwrap_or(date))
            .unwrap_or_default();

            let label = if date.is_empty() {
                deep_field(row, &["newstype"])
                    .and_then(text)
                    .unwrap_or_else(|| "-".to_owned())
            } else {
                date.clone()
            };

            Some((date, label, headline))
        })
        .collect();

    if items.is_empty() {
        return Ok(Page {
            title: format!("No news for {symbol}"),
            subtitle: "NEPSE has nothing listed for this company".to_owned(),
            lines: Vec::new(),
        });
    }

    let latest = items
        .iter()
        .map(|(date, _, _)| date.as_str())
        .max()
        .unwrap_or_default()
        .to_owned();

    // Headlines must not contain the `|` that separates label and value.
    let lines = items
        .iter()
        .take(40)
        .map(|(_, label, headline)| format!("{label}|{}", headline.replace('|', "/")))
        .collect();

    Ok(Page {
        title: format!("{symbol} news"),
        subtitle: if latest.is_empty() {
            format!("{} items", items.len())
        } else {
            format!("{} items \u{b7} latest {latest}", items.len())
        },
        lines,
    })
}

// ---- Alerts ----------------------------------------------------------------------------------

fn alert_notice(id: &str, title: String, subtitle: String) -> SearchResult {
    SearchResult::new(format!("notice:{id}"), title, ResultKind::Notice, 1850)
        .with_subtitle(subtitle)
}

/// What setting an alert would do, as a result that Enter confirms.
fn alert_result(quote: &Quote, above: Option<bool>, target: f64) -> SearchResult {
    let above = above.unwrap_or(target > quote.price);
    let alert = alerts::Alert {
        symbol: quote.symbol.clone(),
        above,
        price: target,
    };

    if alert.reached(quote.price) {
        return alert_notice(
            &format!("nepse-alert-{}", quote.symbol),
            format!(
                "{} is already {} {}",
                quote.symbol,
                alert.word(),
                rs(target)
            ),
            format!("Now {}. Pick a price on the other side", rs(quote.price)),
        );
    }

    let distance = (target - quote.price) / quote.price * 100.0;

    SearchResult::new(
        format!("nepse:alert:{}", alert.id_part()),
        format!("Alert: {} {} {}", quote.symbol, alert.word(), rs(target)),
        ResultKind::Stock,
        1850,
    )
    .with_subtitle(format!("Now {} \u{b7} Enter sets it", rs(quote.price)))
    .with_detail(
        [
            format!("Now|{}", rs(quote.price)),
            format!("Target|{}", rs(target)),
            format!("Distance|{distance:+.2}%"),
            "Checked|every minute while the market is open".to_owned(),
            "Notified|once, with a desktop notification".to_owned(),
        ]
        .join("\n"),
    )
}

fn alerts_results(prices: Option<&[Quote]>) -> Vec<SearchResult> {
    let saved = alerts::load();

    if saved.is_empty() {
        return vec![alert_notice(
            "nepse-no-alerts",
            "No price alerts".to_owned(),
            format!("Set one: {ALERT_EXAMPLE}"),
        )];
    }

    saved
        .iter()
        .enumerate()
        .map(|(index, alert)| {
            let now = prices
                .and_then(|quotes| quotes.iter().find(|quote| quote.symbol == alert.symbol))
                .map(|quote| format!("Now {}", rs(quote.price)));

            SearchResult::new(
                format!("nepse:unalert:{}", alert.id_part()),
                format!("{} {} {}", alert.symbol, alert.word(), rs(alert.price)),
                ResultKind::Stock,
                1850 - index as u32,
            )
            .with_subtitle(format!(
                "{}Enter removes this alert",
                now.map(|now| format!("{now} \u{b7} ")).unwrap_or_default()
            ))
        })
        .collect()
}

fn notify_spec(title: &str, body: &str) -> LaunchSpec {
    LaunchSpec {
        title: title.to_owned(),
        program: "notify-send".to_owned(),
        args: vec![
            "--app-name=Aujar".to_owned(),
            "--expire-time=8000".to_owned(),
            "--".to_owned(),
            title.to_owned(),
            body.to_owned(),
        ],
    }
}

/// Enter on an alert result: save or remove it, and confirm with a notification.
fn resolve_alert(set: bool, text: &str) -> Result<LaunchSpec, String> {
    let alert = alerts::Alert::from_id_part(text).ok_or_else(|| "not a NEPSE alert".to_owned())?;

    if set {
        let replaced = alerts::add(alert.clone())?;

        Ok(notify_spec(
            if replaced {
                "Price alert updated"
            } else {
                "Price alert set"
            },
            &format!(
                "{} {} {}. You will be notified once.",
                alert.symbol,
                alert.word(),
                rs(alert.price)
            ),
        ))
    } else {
        alerts::remove(&alert)?;

        Ok(notify_spec(
            "Price alert removed",
            &format!("{} {} {}", alert.symbol, alert.word(), rs(alert.price)),
        ))
    }
}

// ---- Company details -----------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Fmt {
    Text,
    Plain,
    Count,
    Money,
}

/// What to show and the field names to look for. The details endpoints do not document their
/// answers, so each line appears only if one of its names is found anywhere in the answer.
const DETAIL_FIELDS: &[(&str, &[&str], Fmt)] = &[
    // From `/company/details/` (`securityDailyTradeDto`, `security`).
    (
        "Sector",
        &["sectordescription", "sectorname", "sector"],
        Fmt::Text,
    ),
    ("Last traded on", &["businessdate"], Fmt::Text),
    ("Open", &["openprice"], Fmt::Plain),
    ("Day high", &["highprice", "dayhigh"], Fmt::Plain),
    ("Day low", &["lowprice", "daylow"], Fmt::Plain),
    ("Trades", &["totaltrades"], Fmt::Count),
    (
        "Market cap",
        &["marketcapitalization", "marketcap"],
        Fmt::Money,
    ),
    (
        "Listed shares",
        &[
            "stocklistedshares",
            "listedshares",
            "totallistedshares",
            "shareslisted",
        ],
        Fmt::Count,
    ),
    ("Face value", &["facevalue"], Fmt::Plain),
    ("52-week high", &["fiftytwoweekhigh"], Fmt::Plain),
    ("52-week low", &["fiftytwoweeklow"], Fmt::Plain),
    // From `/company/financial-details/` (`fiscalReport` of the latest report).
    (
        "EPS",
        &["epsvalue", "eps", "earningpershare", "earningspershare"],
        Fmt::Plain,
    ),
    (
        "P/E ratio",
        &[
            "pevalue",
            "peratio",
            "pe",
            "priceearningratio",
            "priceearningsratio",
        ],
        Fmt::Plain,
    ),
    (
        "Book value",
        &["networthpershare", "bookvalue", "bookvaluepershare"],
        Fmt::Plain,
    ),
    ("Net profit", &["profitamount", "netprofit"], Fmt::Money),
    ("Paid-up capital", &["paidupcapital"], Fmt::Money),
    ("Latest report", &["quartername"], Fmt::Text),
    ("Fiscal year", &["fyname"], Fmt::Text),
];

/// Every scalar in the answer by normalised name, however deeply nested (first one wins).
fn flatten<'a>(value: &'a Value, out: &mut Vec<(String, &'a Value)>, depth: usize) {
    if depth > 4 {
        return;
    }

    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                match inner {
                    Value::Object(_) | Value::Array(_) => flatten(inner, out, depth + 1),
                    Value::Null => {}
                    _ => out.push((norm(key), inner)),
                }
            }
        }
        Value::Array(items) => {
            if let Some(first) = items.first() {
                flatten(first, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// Like [`field`], but also looks inside nested objects (`{"companyNews": {"newsHeadline": ..}}`).
/// Fields of the row itself win over nested ones.
fn deep_field<'a>(row: &'a Map<String, Value>, aliases: &[&str]) -> Option<&'a Value> {
    let mut flat = Vec::new();

    for (key, inner) in row {
        match inner {
            Value::Object(_) | Value::Array(_) => {}
            Value::Null => {}
            _ => flat.push((norm(key), inner)),
        }
    }

    for inner in row.values() {
        if matches!(inner, Value::Object(_) | Value::Array(_)) {
            flatten(inner, &mut flat, 1);
        }
    }

    aliases.iter().find_map(|alias| {
        flat.iter()
            .find(|(key, _)| key == alias)
            .map(|(_, value)| *value)
    })
}

/// `label|value` lines for whichever known details `value` contains.
fn detail_lines(value: &Value) -> Vec<String> {
    let mut flat = Vec::new();

    flatten(value, &mut flat, 0);

    DETAIL_FIELDS
        .iter()
        .filter_map(|(label, aliases, format)| {
            let found = aliases.iter().find_map(|alias| {
                flat.iter()
                    .find(|(key, _)| key == alias)
                    .map(|(_, value)| *value)
            })?;

            let shown = match format {
                Fmt::Text => text(found)?,
                Fmt::Plain => price_text(num(found)?),
                Fmt::Count => compact(num(found)?),
                Fmt::Money => format!("Rs {}", compact(num(found)?)),
            };

            Some(format!("{label}|{shown}"))
        })
        .collect()
}

fn fetch_details(symbol: &str) -> Result<Vec<String>, String> {
    let symbol = web::url_encode(symbol);
    let mut lines: Vec<String> = Vec::new();
    let mut reached = false;
    let mut last_error = String::new();

    for path in ["/company/details/", "/company/financial-details/"] {
        match fetch_json(&format!("{path}?symbol={symbol}")) {
            Ok(value) => {
                reached = true;

                for line in detail_lines(&value) {
                    let label = line.split('|').next().unwrap_or_default().to_owned();

                    if !lines
                        .iter()
                        .any(|have| have.starts_with(&format!("{label}|")))
                    {
                        lines.push(line);
                    }
                }
            }
            Err(error) => last_error = error,
        }
    }

    if reached { Ok(lines) } else { Err(last_error) }
}

// ---- Fetching ------------------------------------------------------------------------------

fn fetch_json(path: &str) -> Result<Value, String> {
    let key = api_key().ok_or_else(|| "no NEPSE API key is set".to_owned())?;
    let url = format!("{API}{path}");

    // The key looks like a `prefix.secret` API key, which such services usually expect as
    // `Authorization: Api-Key <key>`. Try that, then the bare key; a key written with its own
    // scheme (it contains a space) is used exactly as given.
    let forms = if key.contains(' ') {
        vec![key]
    } else {
        vec![format!("Api-Key {key}"), key]
    };

    for form in &forms {
        let response = web::get_with_header(&url, Some(&format!("Authorization: {form}")))?;

        match response.status {
            200 => {
                return serde_json::from_str(&response.body)
                    .map_err(|_| "the NEPSE service sent unreadable data".to_owned());
            }
            401 | 403 => continue,
            429 => {
                return Err(
                    "the NEPSE service is rate limiting requests; try again in a minute".to_owned(),
                );
            }
            500..=599 => return Err("the NEPSE service is having problems".to_owned()),
            status => return Err(format!("the NEPSE service answered HTTP {status}")),
        }
    }

    Err("the NEPSE service refused the API key (HTTP 401/403)".to_owned())
}

/// Whether the market is open now; `None` when that cannot be told.
fn market_is_open() -> Option<bool> {
    let page = parse_open(&fetch_json("/is-open/").ok()?).ok()?;

    Some(page.title.ends_with("open"))
}

/// Every scrip's latest price: the price/volume table, or the live market feed when that one
/// has no readable rows.
fn fetch_table() -> Result<Vec<Quote>, String> {
    for path in ["/price-volume/", "/live-market/"] {
        let quotes: Vec<Quote> = match fetch_json(path) {
            Ok(value) => find_rows(&value)
                .into_iter()
                .filter_map(quote_from)
                .collect(),
            Err(error) if path == "/live-market/" => return Err(error),
            Err(_) => Vec::new(),
        };

        if !quotes.is_empty() {
            return Ok(quotes);
        }
    }

    Err("the NEPSE service sent no readable price rows".to_owned())
}

pub struct NepseProvider {
    table: WebCache<Vec<Quote>>,
    pages: WebCache<Page>,
    details: WebCache<Vec<String>>,
}

impl NepseProvider {
    pub fn new() -> Self {
        Self {
            table: WebCache::new(),
            pages: WebCache::new(),
            details: WebCache::new(),
        }
    }
}

impl Default for NepseProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for NepseProvider {
    fn id(&self) -> &'static str {
        "nepse"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let Some(intent) = parse_query(query) else {
            return Vec::new();
        };

        if api_key().is_none() {
            return vec![no_key_result()];
        }

        match intent {
            Intent::Find(text) => {
                let what = format!("{text} on NEPSE");

                match self.table.get("price-volume", TTL, fetch_table) {
                    Lookup::Loading => vec![web::loading_result(&text, &what)],
                    Lookup::Failed(reason) => vec![web::failure_result(&text, &what, &reason)],
                    Lookup::Ready(quotes) => {
                        let hits = find_matches(&quotes, &text);

                        if hits.is_empty() {
                            return vec![
                                SearchResult::new(
                                    format!("notice:nepse-{}", text.replace(' ', "-")),
                                    format!("No NEPSE company matches \u{201c}{text}\u{201d}"),
                                    ResultKind::Notice,
                                    1850,
                                )
                                .with_subtitle("Try the symbol or part of the company name"),
                            ];
                        }

                        let mut results = Vec::new();
                        let mut waiting_for = None;

                        for (index, (quote, score)) in hits.iter().enumerate() {
                            // Details only for an exact symbol, and only for the top hit.
                            let extra = if index == 0 && *score == 1850 {
                                let symbol = quote.symbol.clone();
                                let fetch_symbol = symbol.clone();

                                match self
                                    .details
                                    .get(&symbol, DETAILS_TTL, move || fetch_details(&fetch_symbol))
                                {
                                    Lookup::Ready(lines) => lines,
                                    Lookup::Loading => {
                                        waiting_for = Some(symbol);
                                        Vec::new()
                                    }
                                    // Details are a bonus: the price is shown either way.
                                    Lookup::Failed(_) => Vec::new(),
                                }
                            } else {
                                Vec::new()
                            };

                            results.push(quote_result(quote, *score, &extra));
                        }

                        // While the details load, a "loading" row makes the launcher ask again.
                        if let Some(symbol) = waiting_for {
                            let mut loading = web::loading_result(
                                &format!("nepse-{symbol}"),
                                &format!("{symbol} details"),
                            );

                            loading.score = 1000;
                            results.push(loading);
                        }

                        results
                    }
                }
            }
            Intent::Page(kind) => {
                let what = match kind {
                    PageKind::Index => "the NEPSE index",
                    PageKind::Open => "the NEPSE market status",
                    PageKind::Gainers => "NEPSE top gainers",
                    PageKind::Losers => "NEPSE top losers",
                    PageKind::Sectors => "the NEPSE sector indices",
                    PageKind::Holidays => "NEPSE holidays",
                };

                let lookup = self.pages.get(kind.key(), TTL, move || {
                    parse_page(kind, &fetch_json(kind.path())?, &today_npt())
                });

                match lookup {
                    Lookup::Loading => vec![web::loading_result(kind.key(), what)],
                    Lookup::Failed(reason) => vec![web::failure_result(kind.key(), what, &reason)],
                    Lookup::Ready(page) => {
                        vec![page_result(format!("nepse:{}", kind.key()), &page)]
                    }
                }
            }
            Intent::Extra(kind, symbol) => {
                let key = format!("{}:{symbol}", kind.word());
                let what = format!(
                    "{} for {symbol}",
                    if kind == ExtraKind::Depth {
                        "market depth"
                    } else {
                        "news"
                    }
                );
                let path = format!("{}?symbol={}", kind.path(), web::url_encode(&symbol));
                let fetch_symbol = symbol.clone();

                let lookup = self.pages.get(&key, TTL, move || {
                    let value = fetch_json(&path)?;

                    match kind {
                        ExtraKind::Depth => parse_depth(&value, &fetch_symbol),
                        ExtraKind::News => parse_news(&value, &fetch_symbol),
                    }
                });

                match lookup {
                    Lookup::Loading => vec![web::loading_result(&key, &what)],
                    Lookup::Failed(reason) => vec![web::failure_result(&key, &what, &reason)],
                    Lookup::Ready(page) => vec![page_result(format!("nepse:{key}"), &page)],
                }
            }
            Intent::Alert {
                symbol,
                above,
                price,
            } => {
                let what = format!("{symbol} price alert");

                match self.table.get("price-volume", TTL, fetch_table) {
                    Lookup::Loading => vec![web::loading_result(&symbol, &what)],
                    Lookup::Failed(reason) => vec![web::failure_result(&symbol, &what, &reason)],
                    Lookup::Ready(quotes) => {
                        match quotes.iter().find(|quote| quote.symbol == symbol) {
                            Some(quote) => vec![alert_result(quote, above, price)],
                            None => vec![alert_notice(
                                &format!("nepse-{symbol}"),
                                format!("No NEPSE scrip called {symbol}"),
                                "Use the trading symbol, e.g. nepse alert nabil above 600"
                                    .to_owned(),
                            )],
                        }
                    }
                }
            }
            Intent::Alerts => {
                if alerts::load().is_empty() {
                    return alerts_results(None);
                }

                // Current prices are a bonus: the saved alerts show even when they fail.
                match self.table.get("price-volume", TTL, fetch_table) {
                    Lookup::Loading => vec![web::loading_result("alerts", "current prices")],
                    Lookup::Failed(_) => alerts_results(None),
                    Lookup::Ready(quotes) => alerts_results(Some(&quotes)),
                }
            }
            Intent::Usage(example) => vec![
                SearchResult::new(
                    "notice:nepse-usage",
                    "Add a company symbol",
                    ResultKind::Notice,
                    1850,
                )
                .with_subtitle(format!("For example: {example}")),
            ],
        }
    }

    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let target = id.strip_prefix("nepse:")?;

        if let Some(text) = target.strip_prefix("alert:") {
            return Some(resolve_alert(true, text));
        }

        if let Some(text) = target.strip_prefix("unalert:") {
            return Some(resolve_alert(false, text));
        }

        let (symbol, security_id) = match target.split_once(':') {
            Some((symbol, security_id)) => (symbol, Some(security_id)),
            None => (target, None),
        };

        // Always the official NEPSE site: the company's own page when its id is known, else
        // the home page (index, market status, movers and unknown ids).
        // `nepse:depth:NABIL` and `nepse:news:NABIL` have no page of their own: the home page.
        if ExtraKind::from_word(symbol).is_some() {
            return Some(match security_id {
                Some(symbol) if valid_symbol(symbol) => Ok(LaunchSpec {
                    title: symbol.to_owned(),
                    program: "xdg-open".to_owned(),
                    args: vec![format!("{SITE}/")],
                }),
                _ => Err("not a NEPSE symbol".to_owned()),
            });
        }

        let url = match security_id {
            Some(security_id) if valid_security_id(security_id) && valid_symbol(symbol) => {
                format!("{SITE}/company/detail/{security_id}")
            }
            Some(_) => return Some(Err("not a NEPSE company".to_owned())),
            None if ["index", "open", "gainers", "losers", "sectors", "holidays"]
                .contains(&symbol)
                || valid_symbol(symbol) =>
            {
                format!("{SITE}/")
            }
            None => return Some(Err("not a NEPSE symbol".to_owned())),
        };

        Some(Ok(LaunchSpec {
            title: symbol.to_owned(),
            program: "xdg-open".to_owned(),
            args: vec![url],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Sample shapes only: the API documents its paths but not its answers, so these mirror the
    // field names NEPSE's own endpoints use. Adjust if the real answer differs.
    const TABLE: &str = r#"{"data":[
        {"symbol":"NABIL","securityName":"Nabil Bank Limited","lastTradedPrice":512.5,
         "previousClose":505.0,"openPrice":506,"highPrice":515,"lowPrice":503,
         "totalTradeQuantity":123456,"totalTradeValue":63000000},
        {"symbol":"NICA","securityName":"NIC Asia Bank","lastTradedPrice":"401.00","percentageChange":-2.5}
    ]}"#;

    #[test]
    fn recognises_trigger_forms() {
        assert_eq!(
            parse_query("nepse nabil"),
            Some(Intent::Find("nabil".into()))
        );
        assert_eq!(
            parse_query("nabil nepse"),
            Some(Intent::Find("nabil".into()))
        );
        assert_eq!(
            parse_query("NEPSE  Nic   Asia"),
            Some(Intent::Find("nic asia".into()))
        );
        assert_eq!(
            parse_query("hydro power nepse"),
            Some(Intent::Find("hydro power".into()))
        );
        assert_eq!(parse_query("nepse"), Some(Intent::Page(PageKind::Index)));
        assert_eq!(
            parse_query("nepse gainers"),
            Some(Intent::Page(PageKind::Gainers))
        );
        assert_eq!(
            parse_query("nepse losers"),
            Some(Intent::Page(PageKind::Losers))
        );
        assert_eq!(
            parse_query("nepse open"),
            Some(Intent::Page(PageKind::Open))
        );
        assert_eq!(parse_query("nabil"), None);
        assert_eq!(parse_query("nepse a"), None);
        assert_eq!(parse_query("nepse ../x"), None);
        assert_eq!(parse_query("nepse 123"), None);
        assert_eq!(parse_query("nepse \"quoted\""), None);
    }

    fn sample_quotes() -> Vec<Quote> {
        let value: Value = serde_json::from_str(
            r#"{"data":[
                {"symbol":"NIC","securityName":"NIC Asia Limited","lastTradedPrice":1},
                {"symbol":"NICA","securityName":"NIC Asia Bank Limited","lastTradedPrice":2},
                {"symbol":"NABIL","securityName":"Nabil Bank Limited","lastTradedPrice":3},
                {"symbol":"HDHPC","securityName":"Himal Dolakha Hydropower Company Limited","lastTradedPrice":4},
                {"symbol":"CHCL","securityName":"Chilime Hydropower Company Limited","lastTradedPrice":5}]}"#,
        )
        .unwrap();

        find_rows(&value)
            .into_iter()
            .filter_map(quote_from)
            .collect()
    }

    #[test]
    fn finds_companies_by_part_of_symbol_or_name() {
        let quotes = sample_quotes();
        let symbols = |text: &str| -> Vec<String> {
            find_matches(&quotes, text)
                .iter()
                .map(|(quote, _)| quote.symbol.clone())
                .collect()
        };

        // Exact symbol first, then other symbols that start with it.
        assert_eq!(symbols("nic"), ["NIC", "NICA"]);
        assert_eq!(find_matches(&quotes, "nic")[0].1, 1850);
        assert_eq!(symbols("nabil"), ["NABIL"]);
        // Words of the name, in any order.
        assert_eq!(symbols("nic asia"), ["NIC", "NICA"]);
        assert_eq!(symbols("hydropower company"), ["CHCL", "HDHPC"]);
        assert_eq!(symbols("hydro"), ["CHCL", "HDHPC"]);
        assert_eq!(symbols("bank nic"), ["NICA"]);
        assert!(symbols("zzz").is_empty());
    }

    #[test]
    fn matches_are_capped() {
        let rows: Vec<String> = (0..30)
            .map(|index| format!(r#"{{"symbol":"BK{index:02}","securityName":"Some Bank {index}","lastTradedPrice":1}}"#))
            .collect();
        let value: Value =
            serde_json::from_str(&format!(r#"{{"data":[{}]}}"#, rows.join(","))).unwrap();
        let quotes: Vec<Quote> = find_rows(&value)
            .into_iter()
            .filter_map(quote_from)
            .collect();

        assert_eq!(find_matches(&quotes, "bank").len(), MAX_MATCHES);
    }

    // The details endpoints do not document their answers: these are sample shapes (nested and
    // flat) with the field names NEPSE's own endpoints use.
    #[test]
    fn reads_company_details_wherever_they_are() {
        let value: Value = serde_json::from_str(
            r#"{"status":"success","data":{"security":{"companyName":"Nabil","stockListedShares":270000000,
                "paidUpCapital":27000000000,"faceValue":100,"sectorName":"Commercial Banks"},
                "securityDailyTradeDto":{"fiftyTwoWeekHigh":650.5,"fiftyTwoWeekLow":480}},
                "financial":[{"eps":"18.25","peRatio":28.1,"bookValue":210.4}]}"#,
        )
        .unwrap();
        let lines = detail_lines(&value);

        assert!(
            lines.contains(&"Sector|Commercial Banks".to_owned()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"Listed shares|270.00M".to_owned()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"Paid-up capital|Rs 27.00B".to_owned()),
            "{lines:?}"
        );
        assert!(lines.contains(&"EPS|18.25".to_owned()), "{lines:?}");
        assert!(lines.contains(&"P/E ratio|28.10".to_owned()), "{lines:?}");
        assert!(
            lines.contains(&"52-week high|650.50".to_owned()),
            "{lines:?}"
        );
        assert!(detail_lines(&serde_json::json!({"unrelated": 1})).is_empty());
    }

    #[test]
    fn details_appear_in_the_quote_before_the_exchange_line() {
        let quote = &sample_quotes()[2];
        let extra = vec!["Sector|Commercial Banks".to_owned(), "EPS|18.25".to_owned()];
        let detail = quote_result(quote, 1850, &extra).detail.unwrap();
        let lines: Vec<&str> = detail.lines().collect();

        assert_eq!(lines[lines.len() - 3], "Sector|Commercial Banks");
        assert_eq!(lines[lines.len() - 1], "Exchange|NEPSE");
    }

    #[test]
    fn reads_rows_wherever_they_are() {
        let wrapped: Value = serde_json::from_str(TABLE).unwrap();
        let bare: Value = serde_json::from_str(r#"[{"symbol":"A1","ltp":1}]"#).unwrap();

        assert_eq!(find_rows(&wrapped).len(), 2);
        assert_eq!(find_rows(&bare).len(), 1);
    }

    #[test]
    fn builds_a_quote_with_a_coloured_arrow() {
        let value: Value = serde_json::from_str(TABLE).unwrap();
        let quotes: Vec<Quote> = find_rows(&value)
            .into_iter()
            .filter_map(quote_from)
            .collect();

        assert_eq!(quotes.len(), 2);

        let result = quote_result(&quotes[0], 1850, &[]);

        assert_eq!(result.title, "NABIL  Rs 512.50");
        assert!(
            result
                .subtitle
                .as_deref()
                .unwrap()
                .starts_with("\u{25b2} 7.50 (1.49%)")
        );
        assert!(
            result
                .subtitle
                .as_deref()
                .unwrap()
                .ends_with("Nabil Bank Limited")
        );

        let detail = result.detail.unwrap();

        assert!(detail.contains("Volume|123.5K"));
        assert!(detail.contains("Day range|503.00 \u{2013} 515.00"));
    }

    #[test]
    fn falls_back_to_the_percentage_when_there_is_no_previous_close() {
        let value: Value = serde_json::from_str(TABLE).unwrap();
        let quotes: Vec<Quote> = find_rows(&value)
            .into_iter()
            .filter_map(quote_from)
            .collect();
        let (delta, percent) = change_of(&quotes[1]).unwrap();

        assert!(delta < 0.0);
        assert!((percent + 2.5).abs() < 1e-9);
        assert!(
            quote_result(&quotes[1], 1850, &[])
                .subtitle
                .unwrap()
                .starts_with('\u{25bc}')
        );
    }

    #[test]
    fn rows_without_a_price_are_skipped() {
        let value: Value =
            serde_json::from_str(r#"[{"symbol":"X1"},{"symbol":"Y2","ltp":5}]"#).unwrap();

        assert_eq!(
            find_rows(&value).into_iter().filter_map(quote_from).count(),
            1
        );
    }

    #[test]
    fn reads_the_market_status_in_several_shapes() {
        for (json, open) in [
            ("true", true),
            ("false", false),
            (r#"{"isOpen":false}"#, false),
            (r#"{"data":{"is_open":true}}"#, true),
            (r#"{"status":"CLOSE"}"#, false),
            (r#"{"marketStatus":"Open"}"#, true),
        ] {
            let value: Value = serde_json::from_str(json).unwrap();
            let page = parse_open(&value).unwrap();

            assert_eq!(page.title.ends_with("open"), open, "{json}");
        }

        assert!(parse_open(&serde_json::json!({"x": 1})).is_err());
    }

    #[test]
    fn reads_the_index() {
        let value: Value = serde_json::from_str(
            r#"[{"index":"Banking SubIndex","currentValue":1500.0,"change":3.0},
                {"index":"NEPSE Index","currentValue":2015.32,"change":-12.5,"perChange":-0.62}]"#,
        )
        .unwrap();
        let page = parse_index(&value).unwrap();

        assert_eq!(page.title, "NEPSE Index  2,015.32");
        assert!(page.subtitle.starts_with("\u{25bc} 12.50 (0.62%)"));
    }

    // Shapes copied from the live API (trimmed).
    #[test]
    fn reads_the_real_index_answer() {
        let value: Value = serde_json::from_str(
            r#"{"status":"success","message":"Operation successful","data":{
                "Sensitive Index":{"id":57,"index":"Sensitive Index","close":470.17,"high":475.0075,
                  "low":468.7695,"previousClose":474.9679,"change":4.79,"perChange":1.01,"currentValue":474.96},
                "NEPSE Index":{"id":58,"generatedTime":"2026-10-14T15:12:09.553","index":"NEPSE Index",
                  "close":2599.93,"high":2621.2604,"low":2592.1504,"previousClose":2621.2604,
                  "change":21.32,"perChange":0.82,"fiftyTwoWeekHigh":2960.4,"fiftyTwoWeekLow":2400.0,
                  "currentValue":2621.26}}}"#,
        )
        .unwrap();
        let page = parse_index(&value).unwrap();

        assert_eq!(page.title, "NEPSE Index  2,599.93");
        // The API's `change` is positive although the index fell: the sign comes from the close.
        assert!(
            page.subtitle.starts_with("\u{25bc} 21.33 (0.81%)"),
            "{}",
            page.subtitle
        );
        assert!(
            page.lines
                .iter()
                .any(|line| line.starts_with("52-week range|2,400.00"))
        );
    }

    #[test]
    fn reads_the_real_market_status_answer() {
        let value: Value = serde_json::from_str(
            r#"{"status":"success","message":"Operation successful","data":{"market_open":
                {"isOpen":"CLOSE","asOf":"2026-10-14T15:00:00","id":80}}}"#,
        )
        .unwrap();
        let page = parse_open(&value).unwrap();

        assert_eq!(page.title, "NEPSE market is closed");
        assert_eq!(page.subtitle, "As of 2026-10-14 15:00");
    }

    #[test]
    fn reads_the_real_price_and_mover_answers() {
        let prices: Value = serde_json::from_str(
            r#"{"status":"success","message":"Operation successful","data":[
                {"securityId":"397","securityName":"Agricultural Development Bank Limited","symbol":"ADBL",
                 "indexId":58,"totalTradeQuantity":139162,"lastTradedPrice":328.0,
                 "percentageChange":2.467978757,"previousClose":320.1,"closePrice":328.0}]}"#,
        )
        .unwrap();
        let quotes: Vec<Quote> = find_rows(&prices)
            .into_iter()
            .filter_map(quote_from)
            .collect();
        let result = quote_result(&quotes[0], 1850, &[]);

        assert_eq!(result.title, "ADBL  Rs 328.00");
        assert!(
            result
                .subtitle
                .as_deref()
                .unwrap()
                .starts_with("\u{25b2} 7.90 (2.47%)")
        );

        let movers: Value = serde_json::from_str(
            r#"{"status":"success","data":[{"symbol":"ULBSL","ltp":2626.8,"cp":2626.8,
                "pointChange":123.8,"percentageChange":4.95,"securityName":"Upakar"}]}"#,
        )
        .unwrap();

        assert!(
            parse_movers(&movers, true).unwrap().lines[0]
                .starts_with("ULBSL|Rs 2,626.80  \u{25b2}")
        );
        // Whatever sign the API uses, the losers list points down.
        assert!(parse_movers(&movers, false).unwrap().lines[0].contains('\u{25bc}'));
    }

    #[test]
    fn lists_movers() {
        let value: Value = serde_json::from_str(
            r#"[{"symbol":"AAA","ltp":100,"percentageChange":10},{"symbol":"BBB","ltp":50,"percentageChange":9.5}]"#,
        )
        .unwrap();
        let page = parse_movers(&value, true).unwrap();

        assert_eq!(page.title, "NEPSE top gainers");
        assert_eq!(page.lines.len(), 2);
        assert!(page.lines[0].starts_with("AAA|Rs 100.00"));
        assert!(parse_movers(&serde_json::json!([]), true).is_err());
    }

    #[test]
    fn keys_with_quotes_or_line_breaks_are_rejected() {
        assert!(valid_key("5aYZuryN.hXlFFefyZytA9JEjRhg1MgjGrUg5uShx"));
        assert!(valid_key("Api-Key abcdefgh.ijklmnop"));
        assert!(!valid_key("abc\"def12345"));
        assert!(!valid_key("abcdefgh\nX-Evil: 1"));
        assert!(!valid_key("short"));
    }

    #[test]
    fn opens_the_official_company_page() {
        let provider = NepseProvider::new();
        let prices: Value = serde_json::from_str(
            r#"{"data":[{"securityId":"397","symbol":"ADBL","lastTradedPrice":328.0},
                        {"securityId":9150,"symbol":"AHL","lastTradedPrice":10.0},
                        {"securityId":"bad id","symbol":"ZZZ","lastTradedPrice":1.0}]}"#,
        )
        .unwrap();
        let quotes: Vec<Quote> = find_rows(&prices)
            .into_iter()
            .filter_map(quote_from)
            .collect();

        assert_eq!(quote_result(&quotes[0], 1850, &[]).id, "nepse:ADBL:397");
        assert_eq!(quote_result(&quotes[1], 1850, &[]).id, "nepse:AHL:9150");
        // An unusable id is dropped rather than put in a URL.
        assert_eq!(quote_result(&quotes[2], 1850, &[]).id, "nepse:ZZZ");

        let spec = provider.resolve("nepse:ADBL:397").unwrap().unwrap();

        assert_eq!(spec.program, "xdg-open");
        assert_eq!(
            spec.args,
            vec!["https://nepalstock.com.np/company/detail/397"]
        );
    }

    #[test]
    fn resolves_safely_and_never_to_other_sites() {
        let provider = NepseProvider::new();

        for id in ["nepse:NABIL", "nepse:index", "nepse:gainers", "nepse:open"] {
            let spec = provider.resolve(id).unwrap().unwrap();

            assert_eq!(spec.args, vec!["https://nepalstock.com.np/"], "{id}");
        }

        assert!(provider.resolve("nepse:NABIL:../../evil").unwrap().is_err());
        assert!(provider.resolve("nepse:NABIL:12a").unwrap().is_err());
        assert!(provider.resolve("nepse:../x").unwrap().is_err());
        assert!(provider.resolve("stock:AAPL").is_none());
    }

    // The four commands below read endpoints whose answers are not documented: these are sample
    // shapes using the field names NEPSE's own endpoints use.
    #[test]
    fn recognises_the_new_commands() {
        assert_eq!(
            parse_query("nepse sectors"),
            Some(Intent::Page(PageKind::Sectors))
        );
        assert_eq!(
            parse_query("nepse holidays"),
            Some(Intent::Page(PageKind::Holidays))
        );
        assert_eq!(
            parse_query("nepse depth nabil"),
            Some(Intent::Extra(ExtraKind::Depth, "NABIL".into()))
        );
        assert_eq!(
            parse_query("nepse news adbl"),
            Some(Intent::Extra(ExtraKind::News, "ADBL".into()))
        );
        assert_eq!(
            parse_query("nepse depth"),
            Some(Intent::Usage("nepse depth nabil"))
        );
        assert_eq!(
            parse_query("nepse news"),
            Some(Intent::Usage("nepse news nabil"))
        );
        assert_eq!(parse_query("nepse depth ../x"), None);
    }

    #[test]
    fn converts_days_to_dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(19_723), "2024-01-01");
        assert_eq!(civil_date(20_000), "2024-10-04");
        assert_eq!(civil_date(19_782), "2024-02-29");
        assert_eq!(
            date_part("2026-10-14T15:00:00").as_deref(),
            Some("2026-10-14")
        );
        assert_eq!(date_part("14 Oct 2026"), None);
    }

    #[test]
    fn lists_sectors_best_first() {
        let value: Value = serde_json::from_str(
            r#"{"status":"success","data":{
                "Banking SubIndex":{"index":"Banking SubIndex","close":1200.0,"previousClose":1190.0},
                "Hydropower Index":{"index":"Hydropower Index","close":2000.0,"previousClose":2020.0},
                "Hotels And Tourism":{"index":"Hotels And Tourism","close":5000.0,"perChange":3.5}}}"#,
        )
        .unwrap();
        let page = parse_sectors(&value).unwrap();

        assert_eq!(page.subtitle, "3 sub-indices, best first");
        assert!(
            page.lines[0].starts_with("Hotels And Tourism|5,000.00"),
            "{:?}",
            page.lines
        );
        assert!(
            page.lines[1].starts_with("Banking SubIndex|1,200.00  \u{25b2}"),
            "{:?}",
            page.lines
        );
        assert!(page.lines[2].contains('\u{25bc}'), "{:?}", page.lines);
    }

    #[test]
    fn lists_only_upcoming_holidays() {
        let value: Value = serde_json::from_str(
            r#"{"data":[
                {"holidayDate":"2026-10-20T00:00:00","description":"Dashain"},
                {"holidayDate":"2026-01-01","description":"New Year"},
                {"holidayDate":"2026-10-14","description":"Today holiday"},
                {"holidayDate":"2026-11-10","description":"Tihar"}]}"#,
        )
        .unwrap();
        let page = parse_holidays(&value, "2026-10-14").unwrap();

        assert_eq!(page.subtitle, "Next: 2026-10-14 \u{b7} Today holiday");
        assert_eq!(
            page.lines,
            [
                "2026-10-14|Today holiday",
                "2026-10-20|Dashain",
                "2026-11-10|Tihar"
            ]
        );

        let past = parse_holidays(&value, "2027-01-01").unwrap();

        assert_eq!(past.title, "No more NEPSE holidays listed");
    }

    #[test]
    fn reads_market_depth() {
        let value: Value = serde_json::from_str(
            r#"{"data":{"totalBuyQuantity":5000,"totalSellQuantity":4200,
                "marketDepth":{
                  "buyMarketDepthList":[{"orderBookOrderPrice":510,"quantity":100,"orderCount":2},
                                        {"orderBookOrderPrice":512,"quantity":300,"orderCount":5}],
                  "sellMarketDepthList":[{"orderBookOrderPrice":515,"quantity":50,"orderCount":1},
                                         {"orderBookOrderPrice":514,"quantity":1200,"orderCount":4}]}}}"#,
        )
        .unwrap();
        let page = parse_depth(&value, "NABIL").unwrap();

        assert_eq!(page.title, "NABIL market depth");
        assert_eq!(
            page.subtitle,
            "Best bid Rs 512.00 \u{b7} best ask Rs 514.00"
        );
        assert_eq!(page.lines[0], "Buy 1|Rs 512.00 \u{d7} 300 (5 orders)");
        assert_eq!(page.lines[2], "Sell 1|Rs 514.00 \u{d7} 1.2K (4 orders)");
        assert!(
            page.lines.contains(&"Total buy qty|5.0K".to_owned()),
            "{:?}",
            page.lines
        );
    }

    #[test]
    fn an_empty_order_book_is_not_an_error() {
        let page = parse_depth(
            &serde_json::json!({"data": {"buyMarketDepthList": [], "sellMarketDepthList": []}}),
            "NABIL",
        )
        .unwrap();

        assert_eq!(page.title, "No market depth for NABIL");
    }

    #[test]
    fn reads_company_news() {
        let value: Value = serde_json::from_str(
            r#"{"data":[
                {"newsHeadline":"Dividend | bonus announced","addedDate":"2026-10-12T10:00:00"},
                {"newsHeadline":"AGM notice","addedDate":"2026-09-30T09:00:00"}]}"#,
        )
        .unwrap();
        let page = parse_news(&value, "NABIL").unwrap();

        assert_eq!(page.title, "NABIL news");
        assert_eq!(page.subtitle, "2 items \u{b7} latest 2026-10-12");
        // The separator inside a headline must not break the label|value line.
        assert_eq!(page.lines[0], "2026-10-12|Dividend / bonus announced");

        assert_eq!(
            parse_news(&serde_json::json!({"data": []}), "NABIL")
                .unwrap()
                .title,
            "No news for NABIL"
        );
    }

    #[test]
    fn new_results_open_the_home_page_only() {
        let provider = NepseProvider::new();

        for id in [
            "nepse:sectors",
            "nepse:holidays",
            "nepse:depth:NABIL",
            "nepse:news:ADBL",
        ] {
            let spec = provider.resolve(id).unwrap().unwrap();

            assert_eq!(spec.args, vec!["https://nepalstock.com.np/"], "{id}");
        }

        assert!(provider.resolve("nepse:depth:../x").unwrap().is_err());
        assert!(provider.resolve("nepse:news").unwrap().is_err());
    }

    #[test]
    fn recognises_alert_commands() {
        let alert = |symbol: &str, above: Option<bool>, price: f64| Intent::Alert {
            symbol: symbol.into(),
            above,
            price,
        };

        assert_eq!(
            parse_query("nepse alert nabil above 600"),
            Some(alert("NABIL", Some(true), 600.0))
        );
        assert_eq!(
            parse_query("nepse alert nabil below 1,250.5"),
            Some(alert("NABIL", Some(false), 1250.5))
        );
        assert_eq!(
            parse_query("nepse alert nabil > 600"),
            Some(alert("NABIL", Some(true), 600.0))
        );
        assert_eq!(
            parse_query("nepse alert nabil rs600"),
            Some(alert("NABIL", None, 600.0))
        );
        assert_eq!(
            parse_query("nepse alert nabil 600"),
            Some(alert("NABIL", None, 600.0))
        );
        assert_eq!(parse_query("nepse alerts"), Some(Intent::Alerts));

        for bad in [
            "nepse alert",
            "nepse alert nabil",
            "nepse alert nabil above",
            "nepse alert nabil sideways 5",
            "nepse alert nabil above -5",
            "nepse alert nabil above 0",
            "nepse alert nabil above nan",
            "nepse alert nabil above 99999999",
            "nepse alert ../x above 5",
        ] {
            assert_eq!(
                parse_query(bad),
                Some(Intent::Usage(ALERT_EXAMPLE)),
                "{bad}"
            );
        }
    }

    #[test]
    fn offers_an_alert_or_explains_why_not() {
        let quote = &sample_quotes()[2];

        // NABIL is 3.0 in the sample table.
        let set = alert_result(quote, Some(true), 5.0);

        assert_eq!(set.id, "nepse:alert:NABIL:above:5.00");
        assert_eq!(set.title, "Alert: NABIL above Rs 5.00");
        assert!(set.detail.unwrap().contains("Distance|+66.67%"));

        // Direction picked from the price when not given.
        assert_eq!(
            alert_result(quote, None, 2.0).id,
            "nepse:alert:NABIL:below:2.00"
        );

        // Already past the target: nothing to set.
        let done = alert_result(quote, Some(true), 2.0);

        assert_eq!(done.kind, ResultKind::Notice);
        assert_eq!(done.title, "NABIL is already above Rs 2.00");
    }

    #[test]
    fn alert_ids_resolve_to_a_notification_and_nothing_else() {
        let provider = NepseProvider::new();

        // Malformed ids never touch the alerts file.
        for id in [
            "nepse:alert:NABIL:above:-3",
            "nepse:alert:../x:above:5",
            "nepse:unalert:junk",
        ] {
            assert!(provider.resolve(id).unwrap().is_err(), "{id}");
        }

        let spec = notify_spec("A title", "-n a body");

        assert_eq!(spec.program, "notify-send");
        // The text sits after `--`, so it cannot be read as an option.
        assert_eq!(spec.args[2], "--");
        assert_eq!(&spec.args[3..], ["A title", "-n a body"]);
    }

    // Shapes copied from the live API (trimmed), checked against the parsers.
    #[test]
    fn reads_the_real_sector_answer() {
        let value: Value = serde_json::from_str(
            r#"{"status":"success","message":"Operation successful","data":{
                "Microfinance Index":{"id":64,"index":"Microfinance Index","change":23.97,"perChange":0.53,"currentValue":4507.17},
                "Mutual Fund":{"id":66,"index":"Mutual Fund","change":0.14,"perChange":0.73,"currentValue":19.75},
                "Investment Index":{"id":67,"index":"Investment Index","change":-0.07,"perChange":-0.08,"currentValue":94.82},
                "Banking SubIndex":{"id":51,"index":"Banking SubIndex","change":11.4,"perChange":0.72,"currentValue":1574.47}}}"#,
        )
        .unwrap();
        let page = parse_sectors(&value).unwrap();

        assert_eq!(page.subtitle, "4 sub-indices, best first");
        // Best first: Mutual Fund +0.73, Banking +0.72, Microfinance +0.53, Investment -0.08.
        assert!(
            page.lines[0].starts_with("Mutual Fund|19.75  \u{25b2}"),
            "{:?}",
            page.lines
        );
        assert!(
            page.lines[1].starts_with("Banking SubIndex|1,574.47"),
            "{:?}",
            page.lines
        );
        assert!(
            page.lines[3].starts_with("Investment Index|94.82  \u{25bc}"),
            "{:?}",
            page.lines
        );
    }

    #[test]
    fn reads_the_real_holiday_answer() {
        let value: Value = serde_json::from_str(
            r#"{"status":"success","message":"Operation successful","data":[
                {"id":null,"instrumentTypeId":null,"holidayDate":"2026-12-30","holidayDescription":"Tamu Lhosar","activeStatus":null},
                {"id":null,"instrumentTypeId":null,"holidayDate":"2026-11-15","holidayDescription":"Chhath Parva","activeStatus":null},
                {"id":null,"instrumentTypeId":null,"holidayDate":"2026-10-01","holidayDescription":"Already past","activeStatus":null}]}"#,
        )
        .unwrap();
        let page = parse_holidays(&value, "2026-10-23").unwrap();

        // The API lists them newest first: they come out soonest first, past ones dropped.
        assert_eq!(page.subtitle, "Next: 2026-11-15 \u{b7} Chhath Parva");
        assert_eq!(
            page.lines,
            ["2026-11-15|Chhath Parva", "2026-12-30|Tamu Lhosar"]
        );
    }

    #[test]
    fn the_real_empty_market_depth_answer_is_not_an_error() {
        // Outside trading hours `data` is an empty string.
        let value: Value = serde_json::from_str(
            r#"{"status":"success","message":"Operation successful","data":""}"#,
        )
        .unwrap();
        let page = parse_depth(&value, "NABIL").unwrap();

        assert_eq!(page.title, "No market depth for NABIL");
    }

    #[test]
    fn reads_the_real_nested_news_answer() {
        let value: Value = serde_json::from_str(
            r#"{"status":"success","message":"Operation successful","data":[
                {"id":47714,"activeStatus":"A","applicationType":39,"applicationStatus":3,
                 "companyNews":{"id":70829,"activeStatus":"A","newsSource":"Letter",
                   "newsHeadline":"Regarding the AGM, Appointment of Directors and Statutory Auditor [NABIL] ",
                   "newsBody":"","newsType":"Others",
                   "security":{"id":131,"symbol":"NABIL","listingDate":"1985-11-24","faceValue":100.0}}}]}"#,
        )
        .unwrap();
        let page = parse_news(&value, "NABIL").unwrap();

        assert_eq!(page.title, "NABIL news");
        // No date field in this sample (the security's listing date must not be used):
        // the kind of news labels the line instead.
        assert_eq!(
            page.lines,
            ["Others|Regarding the AGM, Appointment of Directors and Statutory Auditor [NABIL]"]
        );
        assert_eq!(page.subtitle, "1 items");

        let dated: Value = serde_json::from_str(
            r#"{"data":[{"companyNews":{"newsHeadline":"A","addedDate":"2026-10-12T10:00:00"}},
                        {"companyNews":{"newsHeadline":"B","addedDate":"2026-10-20T10:00:00"}}]}"#,
        )
        .unwrap();
        let page = parse_news(&dated, "X1").unwrap();

        assert_eq!(page.subtitle, "2 items \u{b7} latest 2026-10-20");
        assert_eq!(page.lines[0], "2026-10-12|A");
    }

    // Shapes copied from the live API (trimmed).
    #[test]
    fn reads_the_real_company_detail_answers() {
        let details: Value = serde_json::from_str(
            r#"{"status":"success","message":"Operation successful","data":{
                "securityDailyTradeDto":{"securityId":"131","openPrice":548.0,"highPrice":549.2,"lowPrice":544.0,
                  "totalTradeQuantity":30945,"totalTrades":280,"lastTradedPrice":547.0,"previousClose":546.0,
                  "businessDate":"2026-10-16","closePrice":547.0,"fiftyTwoWeekHigh":581.0,"fiftyTwoWeekLow":485.0},
                "security":{"id":131,"symbol":"NABIL","isin":"NPE025A00004","listingDate":"1985-11-24",
                  "creditRating":null,"tickSize":0.1,"instrumentType":{"id":1,"code":"EQ"},"faceValue":100.0}}}"#,
        )
        .unwrap();
        let lines = detail_lines(&details);

        for expected in [
            "Last traded on|2026-10-16",
            "Open|548.00",
            "Day high|549.20",
            "Day low|544.00",
            "Trades|280",
            "Face value|100.00",
            "52-week high|581.00",
            "52-week low|485.00",
        ] {
            assert!(
                lines.contains(&expected.to_owned()),
                "{expected}: {lines:?}"
            );
        }

        let financial: Value = serde_json::from_str(
            r#"{"status":"success","message":"Operation successful","data":[
                {"id":46727,"activeStatus":"A","applicationType":111,"companyNews":null,
                 "fiscalReport":{"id":6835,"quarterMaster":{"id":4,"quarterName":"Fourth Quarter"},
                   "reportTypeMaster":{"id":2,"reportName":"Quarterly Report"},
                   "financialYear":{"id":41,"fyName":"2025-2026","fromYear":"2025-07-17"},
                   "peValue":18.86,"epsValue":28.36,"paidUpCapital":27056996729.0,
                   "profitAmount":7905796000.0,"netWorthPerShare":247.28,"remarks":null}}]}"#,
        )
        .unwrap();
        let lines = detail_lines(&financial);

        for expected in [
            "EPS|28.36",
            "P/E ratio|18.86",
            "Book value|247.28",
            "Net profit|Rs 7.91B",
            "Paid-up capital|Rs 27.06B",
            "Latest report|Fourth Quarter",
            "Fiscal year|2025-2026",
        ] {
            assert!(
                lines.contains(&expected.to_owned()),
                "{expected}: {lines:?}"
            );
        }
    }

    #[test]
    fn details_never_repeat_a_line_the_price_list_gave() {
        let value: Value = serde_json::from_str(
            r#"[{"symbol":"NABIL","lastTradedPrice":547,"previousClose":546,"openPrice":540}]"#,
        )
        .unwrap();
        let quote = find_rows(&value)
            .into_iter()
            .filter_map(quote_from)
            .next()
            .unwrap();
        let extra = vec!["Open|548.00".to_owned(), "Trades|280".to_owned()];
        let detail = quote_result(&quote, 1850, &extra).detail.unwrap();

        // The price list already gave the open price, so the details one is skipped.
        assert_eq!(
            detail
                .lines()
                .filter(|line| line.starts_with("Open|"))
                .count(),
            1
        );
        assert!(detail.contains("Trades|280"));
    }
}
