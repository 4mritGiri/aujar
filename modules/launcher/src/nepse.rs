//! NEPSE (Nepal Stock Exchange) lookups: `nepse nabil`, `nabil nepse`, `nepse` (the index),
//! `nepse gainers`, `nepse losers`, `nepse open`.
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

use crate::{
    model::{LaunchSpec, Provider, ResultKind, SearchResult},
    stocks::{compact, price_text},
    web::{self, Lookup, WebCache},
};
use serde_json::{Map, Value};
use std::{fs, path::PathBuf, time::Duration};

const API: &str = "https://warehouse.fintechnepal.com.np/api/nepse";
const TTL: Duration = Duration::from_secs(60);
const KEY_ENV: &str = "AUJAR_NEPSE_KEY";
/// The official NEPSE site. A company page is `{SITE}/company/detail/<securityId>`.
const SITE: &str = "https://nepalstock.com.np";

// ---- API key -------------------------------------------------------------------------------

fn key_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;

    Some(base.join("aujar").join("nepse.key"))
}

/// Keys are letters, digits and a few separators, with at most one space (a scheme prefix such
/// as `Api-Key`). Anything else (quotes, line breaks) is rejected before it reaches curl.
fn valid_key(key: &str) -> bool {
    (8..=200).contains(&key.len())
        && key.matches(' ').count() <= 1
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '~' | '+' | '/' | '=' | ' ' | ':'))
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
}

impl PageKind {
    fn key(self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::Open => "open",
            Self::Gainers => "gainers",
            Self::Losers => "losers",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Index => "/index/",
            Self::Open => "/is-open/",
            Self::Gainers => "/top-gainers/",
            Self::Losers => "/top-losers/",
        }
    }

    fn from_word(word: &str) -> Option<Self> {
        match word {
            "index" | "nepse" => Some(Self::Index),
            "open" | "status" | "market" => Some(Self::Open),
            "gainers" | "gainer" | "gain" => Some(Self::Gainers),
            "losers" | "loser" | "loss" => Some(Self::Losers),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Intent {
    Symbol(String),
    Page(PageKind),
}

fn valid_security_id(id: &str) -> bool {
    (1..=10).contains(&id.len()) && id.chars().all(|c| c.is_ascii_digit())
}

fn valid_symbol(symbol: &str) -> bool {
    (2..=12).contains(&symbol.len())
        && symbol.chars().all(|c| c.is_ascii_alphanumeric())
        && symbol.chars().any(|c| c.is_ascii_alphabetic())
}

fn parse_query(query: &str) -> Option<Intent> {
    let lower = query.trim().to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    match words.as_slice() {
        ["nepse"] => Some(Intent::Page(PageKind::Index)),
        ["nepse", word] => match PageKind::from_word(word) {
            Some(page) => Some(Intent::Page(page)),
            None => {
                let symbol = word.to_uppercase();

                valid_symbol(&symbol).then_some(Intent::Symbol(symbol))
            }
        },
        [word, "nepse"] => {
            let symbol = word.to_uppercase();

            valid_symbol(&symbol).then_some(Intent::Symbol(symbol))
        }
        _ => None,
    }
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

/// A single record: the object itself when it carries numbers, else the first nested one.
fn find_record(value: &Value) -> Option<&Map<String, Value>> {
    match value {
        Value::Array(items) => items.iter().find_map(find_record),
        Value::Object(map) => {
            if map.values().any(|inner| num(inner).is_some()) {
                Some(map)
            } else {
                map.values().find_map(find_record)
            }
        }
        _ => None,
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
    let symbol = field(row, &["symbol", "stocksymbol", "securitysymbol", "scripsymbol", "ticker"])
        .and_then(text)?
        .to_uppercase();

    let price = number_of(
        row,
        &["lasttradedprice", "ltp", "closeprice", "lastprice", "price", "close", "closingprice"],
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
        name: field(row, &["securityname", "companyname", "name", "fullname", "companyfullname"])
            .and_then(text),
        price,
        previous_close: number_of(
            row,
            &["previousclose", "previousdayclose", "previousdaycloseprice", "prevclose", "previousclosingprice"],
        ),
        change_percent: number_of(
            row,
            &["percentagechange", "percentchange", "perchange", "changepercent", "pricechangepercent", "changepct"],
        ),
        point_change: number_of(row, &["pointchange", "pricechange", "change", "diff", "difference"]),
        open: number_of(row, &["openprice", "open"]),
        high: number_of(row, &["highprice", "high", "dayhigh", "maxprice"]),
        low: number_of(row, &["lowprice", "low", "daylow", "minprice"]),
        volume: number_of(
            row,
            &["totaltradequantity", "totaltradedquantity", "totaltradedvolume", "volume", "sharestraded", "totalquantity", "lasttradedvolume"],
        ),
        turnover: number_of(
            row,
            &["totaltradevalue", "totaltradedvalue", "turnover", "totalturnover", "amount"],
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
        let percent = if previous != 0.0 { delta / previous * 100.0 } else { 0.0 };

        (delta, percent)
    })
}

fn arrow_text(delta: f64, percent: f64) -> String {
    let arrow = if delta >= 0.0 { '\u{25b2}' } else { '\u{25bc}' };

    format!("{arrow} {} ({:.2}%)", price_text(delta.abs()), percent.abs())
}

fn rs(value: f64) -> String {
    format!("Rs {}", price_text(value))
}

fn quote_result(quote: &Quote) -> SearchResult {
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
    row("Day range", quote.low.zip(quote.high).map(|(low, high)| {
        format!("{} \u{2013} {}", price_text(low), price_text(high))
    }));
    row("Volume", quote.volume.map(compact));
    row("Turnover", quote.turnover.map(|value| format!("Rs {}", compact(value))));
    row("Exchange", Some("NEPSE".to_owned()));

    // `nepse:NABIL:397` when the company id is known, so Enter can open its official page.
    let id = match &quote.security_id {
        Some(security_id) => format!("nepse:{}:{security_id}", quote.symbol),
        None => format!("nepse:{}", quote.symbol),
    };

    SearchResult::new(
        id,
        format!("{}  {}", quote.symbol, rs(quote.price)),
        ResultKind::Stock,
        1850,
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
        &["close", "closeprice", "currentvalue", "indexvalue", "currentindex", "index", "value", "lastvalue"],
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
            number_of(record, &["percentagechange", "perchange", "percentchange", "changepercent"]),
        ) {
            (Some(delta), Some(percent)) => Some((delta, percent)),
            (Some(delta), None) => {
                let base = current - delta;

                Some((delta, if base != 0.0 { delta / base * 100.0 } else { 0.0 }))
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
    row("Day range", number_of(record, &["low", "lowvalue", "daylow"])
        .zip(number_of(record, &["high", "highvalue", "dayhigh"]))
        .map(|(low, high)| format!("{} \u{2013} {}", price_text(low), price_text(high))));
    row("52-week range", number_of(record, &["fiftytwoweeklow"])
        .zip(number_of(record, &["fiftytwoweekhigh"]))
        .map(|(low, high)| format!("{} \u{2013} {}", price_text(low), price_text(high))));
    row("Turnover", number_of(record, &["turnover", "totalturnover"]).map(|v| format!("Rs {}", compact(v))));
    row("Volume", number_of(record, &["volume", "totalvolume"]).map(compact));

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
            Value::Object(map) => ["isopen", "open", "marketopen", "isnepseopen", "status", "marketstatus"]
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

    let open = open_in(value).ok_or_else(|| "the NEPSE service sent an unexpected answer".to_owned())?;

    let subtitle = match as_of(value) {
        Some(time) => {
            let time = time.replace('T', " ");

            format!("As of {}", time.get(..16).unwrap_or(&time))
        }
        None => "Nepal Stock Exchange".to_owned(),
    };

    Ok(Page {
        title: if open { "NEPSE market is open" } else { "NEPSE market is closed" }.to_owned(),
        subtitle,
        lines: Vec::new(),
    })
}

fn parse_movers(value: &Value, rising: bool) -> Result<Page, String> {
    let quotes: Vec<Quote> = find_rows(value).into_iter().filter_map(quote_from).take(10).collect();

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
                    let (delta, percent) = if rising { (delta, percent) } else { (-delta, -percent) };

                    format!("  {}", arrow_text(delta, percent))
                })
                .unwrap_or_default();

            format!("{}|{}{change}", quote.symbol, rs(quote.price))
        })
        .collect::<Vec<_>>();

    let (arrow, word) = if rising { ('\u{25b2}', "gainers") } else { ('\u{25bc}', "losers") };

    Ok(Page {
        title: format!("NEPSE top {word}"),
        subtitle: format!("{arrow} Top {} by price {}", quotes.len(), if rising { "gain" } else { "loss" }),
        lines,
    })
}

fn parse_page(kind: PageKind, value: &Value) -> Result<Page, String> {
    match kind {
        PageKind::Index => parse_index(value),
        PageKind::Open => parse_open(value),
        PageKind::Gainers => parse_movers(value, true),
        PageKind::Losers => parse_movers(value, false),
    }
}

fn page_result(kind: PageKind, page: &Page) -> SearchResult {
    let mut result = SearchResult::new(
        format!("nepse:{}", kind.key()),
        page.title.clone(),
        ResultKind::Stock,
        1850,
    )
    .with_subtitle(page.subtitle.clone());

    if !page.lines.is_empty() {
        result = result.with_detail(page.lines.join("\n"));
    }

    result
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
            429 => return Err("the NEPSE service is rate limiting requests; try again in a minute".to_owned()),
            500..=599 => return Err("the NEPSE service is having problems".to_owned()),
            status => return Err(format!("the NEPSE service answered HTTP {status}")),
        }
    }

    Err("the NEPSE service refused the API key (HTTP 401/403)".to_owned())
}

/// Every scrip's latest price: the price/volume table, or the live market feed when that one
/// has no readable rows.
fn fetch_table() -> Result<Vec<Quote>, String> {
    for path in ["/price-volume/", "/live-market/"] {
        let quotes: Vec<Quote> = match fetch_json(path) {
            Ok(value) => find_rows(&value).into_iter().filter_map(quote_from).collect(),
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
}

impl NepseProvider {
    pub fn new() -> Self {
        Self {
            table: WebCache::new(),
            pages: WebCache::new(),
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
            Intent::Symbol(symbol) => {
                let what = format!("{symbol} on NEPSE");

                match self.table.get("price-volume", TTL, fetch_table) {
                    Lookup::Loading => vec![web::loading_result(&symbol, &what)],
                    Lookup::Failed(reason) => vec![web::failure_result(&symbol, &what, &reason)],
                    Lookup::Ready(quotes) => match quotes.iter().find(|quote| quote.symbol == symbol) {
                        Some(quote) => vec![quote_result(quote)],
                        None => vec![
                            SearchResult::new(
                                format!("notice:nepse-{symbol}"),
                                format!("No NEPSE scrip called {symbol}"),
                                ResultKind::Notice,
                                1850,
                            )
                            .with_subtitle("Use the trading symbol, e.g. nepse nabil"),
                        ],
                    },
                }
            }
            Intent::Page(kind) => {
                let what = match kind {
                    PageKind::Index => "the NEPSE index",
                    PageKind::Open => "the NEPSE market status",
                    PageKind::Gainers => "NEPSE top gainers",
                    PageKind::Losers => "NEPSE top losers",
                };

                let lookup = self.pages.get(kind.key(), TTL, move || {
                    parse_page(kind, &fetch_json(kind.path())?)
                });

                match lookup {
                    Lookup::Loading => vec![web::loading_result(kind.key(), what)],
                    Lookup::Failed(reason) => vec![web::failure_result(kind.key(), what, &reason)],
                    Lookup::Ready(page) => vec![page_result(kind, &page)],
                }
            }
        }
    }

    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let target = id.strip_prefix("nepse:")?;
        let (symbol, security_id) = match target.split_once(':') {
            Some((symbol, security_id)) => (symbol, Some(security_id)),
            None => (target, None),
        };

        // Always the official NEPSE site: the company's own page when its id is known, else
        // the home page (index, market status, movers and unknown ids).
        let url = match security_id {
            Some(security_id) if valid_security_id(security_id) && valid_symbol(symbol) => {
                format!("{SITE}/company/detail/{security_id}")
            }
            Some(_) => return Some(Err("not a NEPSE company".to_owned())),
            None if ["index", "open", "gainers", "losers"].contains(&symbol) || valid_symbol(symbol) => {
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
        assert_eq!(parse_query("nepse nabil"), Some(Intent::Symbol("NABIL".into())));
        assert_eq!(parse_query("nabil nepse"), Some(Intent::Symbol("NABIL".into())));
        assert_eq!(parse_query("nepse"), Some(Intent::Page(PageKind::Index)));
        assert_eq!(parse_query("nepse gainers"), Some(Intent::Page(PageKind::Gainers)));
        assert_eq!(parse_query("nepse losers"), Some(Intent::Page(PageKind::Losers)));
        assert_eq!(parse_query("nepse open"), Some(Intent::Page(PageKind::Open)));
        assert_eq!(parse_query("nabil"), None);
        assert_eq!(parse_query("nepse a b"), None);
        assert_eq!(parse_query("nepse ../x"), None);
        assert_eq!(parse_query("nepse 123"), None);
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
        let quotes: Vec<Quote> = find_rows(&value).into_iter().filter_map(quote_from).collect();

        assert_eq!(quotes.len(), 2);

        let result = quote_result(&quotes[0]);

        assert_eq!(result.title, "NABIL  Rs 512.50");
        assert!(result.subtitle.as_deref().unwrap().starts_with("\u{25b2} 7.50 (1.49%)"));
        assert!(result.subtitle.as_deref().unwrap().ends_with("Nabil Bank Limited"));

        let detail = result.detail.unwrap();

        assert!(detail.contains("Volume|123.5K"));
        assert!(detail.contains("Day range|503.00 \u{2013} 515.00"));
    }

    #[test]
    fn falls_back_to_the_percentage_when_there_is_no_previous_close() {
        let value: Value = serde_json::from_str(TABLE).unwrap();
        let quotes: Vec<Quote> = find_rows(&value).into_iter().filter_map(quote_from).collect();
        let (delta, percent) = change_of(&quotes[1]).unwrap();

        assert!(delta < 0.0);
        assert!((percent + 2.5).abs() < 1e-9);
        assert!(quote_result(&quotes[1]).subtitle.unwrap().starts_with('\u{25bc}'));
    }

    #[test]
    fn rows_without_a_price_are_skipped() {
        let value: Value = serde_json::from_str(r#"[{"symbol":"X1"},{"symbol":"Y2","ltp":5}]"#).unwrap();

        assert_eq!(find_rows(&value).into_iter().filter_map(quote_from).count(), 1);
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
        assert!(page.subtitle.starts_with("\u{25bc} 21.33 (0.81%)"), "{}", page.subtitle);
        assert!(page.lines.iter().any(|line| line.starts_with("52-week range|2,400.00")));
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
        let quotes: Vec<Quote> = find_rows(&prices).into_iter().filter_map(quote_from).collect();
        let result = quote_result(&quotes[0]);

        assert_eq!(result.title, "ADBL  Rs 328.00");
        assert!(result.subtitle.as_deref().unwrap().starts_with("\u{25b2} 7.90 (2.47%)"));

        let movers: Value = serde_json::from_str(
            r#"{"status":"success","data":[{"symbol":"ULBSL","ltp":2626.8,"cp":2626.8,
                "pointChange":123.8,"percentageChange":4.95,"securityName":"Upakar"}]}"#,
        )
        .unwrap();

        assert!(parse_movers(&movers, true).unwrap().lines[0].starts_with("ULBSL|Rs 2,626.80  \u{25b2}"));
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
        let quotes: Vec<Quote> = find_rows(&prices).into_iter().filter_map(quote_from).collect();

        assert_eq!(quote_result(&quotes[0]).id, "nepse:ADBL:397");
        assert_eq!(quote_result(&quotes[1]).id, "nepse:AHL:9150");
        // An unusable id is dropped rather than put in a URL.
        assert_eq!(quote_result(&quotes[2]).id, "nepse:ZZZ");

        let spec = provider.resolve("nepse:ADBL:397").unwrap().unwrap();

        assert_eq!(spec.program, "xdg-open");
        assert_eq!(spec.args, vec!["https://nepalstock.com.np/company/detail/397"]);
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
}
