//! Shared plumbing for the lookup providers (dictionary, weather, stocks, sports).
//!
//! `Provider::search` runs on every keystroke and must never wait for the network. So a
//! lookup goes through [`WebCache::get`]: it answers at once with what it has
//! ([`Lookup::Ready`], [`Lookup::Failed`]) or with [`Lookup::Loading`] while a worker thread
//! fetches. The UI asks again shortly and then finds the answer in the cache.
//!
//! Requests use the system `curl` (as the currency provider does): no TLS stack in the
//! daemon, no shell, HTTPS only.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// How long a failed lookup is remembered before it may be retried.
const FAILURE_TTL: Duration = Duration::from_secs(20);
/// A fetch that has not finished after this long is reported as failed.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
/// Distinct lookups kept per provider.
const MAX_ENTRIES: usize = 64;
/// Simultaneous fetch threads per provider.
const MAX_IN_FLIGHT: usize = 3;

/// Answer of [`WebCache::get`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup<T> {
    Ready(T),
    Loading,
    Failed(String),
}

enum Slot<T> {
    Pending(Instant),
    Ready(T, Instant),
    Failed(String, Instant),
}

struct Inner<T> {
    slots: HashMap<String, Slot<T>>,
    in_flight: usize,
}

/// A small keyed cache whose misses are filled by background threads.
pub struct WebCache<T> {
    inner: Arc<Mutex<Inner<T>>>,
}

impl<T> Clone for WebCache<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Default for WebCache<T> {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                slots: HashMap::new(),
                in_flight: 0,
            })),
        }
    }
}

fn lock<T>(inner: &Mutex<Inner<T>>) -> std::sync::MutexGuard<'_, Inner<T>> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl<T: Clone + Send + 'static> WebCache<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look `key` up. On a miss (or an entry older than `ttl`) `fetch` runs on a worker thread
    /// and the caller gets [`Lookup::Loading`] until it finishes.
    pub fn get(
        &self,
        key: &str,
        ttl: Duration,
        fetch: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Lookup<T> {
        let mut inner = lock(&self.inner);
        let now = Instant::now();

        match inner.slots.get(key) {
            Some(Slot::Ready(value, at)) if now.duration_since(*at) < ttl => {
                return Lookup::Ready(value.clone());
            }
            Some(Slot::Failed(message, at)) if now.duration_since(*at) < FAILURE_TTL => {
                return Lookup::Failed(message.clone());
            }
            Some(Slot::Pending(started)) => {
                if now.duration_since(*started) < FETCH_TIMEOUT {
                    return Lookup::Loading;
                }

                // The worker is stuck (or very slow): give up on it. When it finally
                // finishes it will see this slot is no longer pending and drop its answer.
                let message = "the lookup timed out".to_owned();
                inner
                    .slots
                    .insert(key.to_owned(), Slot::Failed(message.clone(), now));

                return Lookup::Failed(message);
            }
            _ => {}
        }

        if inner.in_flight >= MAX_IN_FLIGHT {
            return Lookup::Loading;
        }

        if inner.slots.len() >= MAX_ENTRIES {
            evict_oldest(&mut inner.slots);
        }

        inner.slots.insert(key.to_owned(), Slot::Pending(now));
        inner.in_flight += 1;
        drop(inner);

        let shared = Arc::clone(&self.inner);
        let slot_key = key.to_owned();

        let spawned = std::thread::Builder::new()
            .name("aujar-web".into())
            .spawn(move || {
                let outcome = fetch();
                let mut inner = lock(&shared);

                inner.in_flight = inner.in_flight.saturating_sub(1);

                // Only store the answer if this lookup still owns the slot.
                if matches!(inner.slots.get(&slot_key), Some(Slot::Pending(_))) {
                    let slot = match outcome {
                        Ok(value) => Slot::Ready(value, Instant::now()),
                        Err(message) => Slot::Failed(message, Instant::now()),
                    };

                    inner.slots.insert(slot_key, slot);
                }
            });

        if spawned.is_err() {
            let mut inner = lock(&self.inner);

            inner.in_flight = inner.in_flight.saturating_sub(1);
            inner.slots.remove(key);

            return Lookup::Failed("could not start a network thread".to_owned());
        }

        Lookup::Loading
    }

    /// Put a ready value in the cache (tests).
    #[cfg(test)]
    pub fn insert(&self, key: &str, value: T) {
        lock(&self.inner)
            .slots
            .insert(key.to_owned(), Slot::Ready(value, Instant::now()));
    }
}

fn evict_oldest<T>(slots: &mut HashMap<String, Slot<T>>) {
    let oldest = slots
        .iter()
        .filter_map(|(key, slot)| match slot {
            Slot::Ready(_, at) | Slot::Failed(_, at) => Some((key.clone(), *at)),
            Slot::Pending(_) => None,
        })
        .min_by_key(|(_, at)| *at)
        .map(|(key, _)| key);

    if let Some(key) = oldest {
        slots.remove(&key);
    }
}

/// Percent-encode for a URL query value or path segment (RFC 3986 unreserved characters
/// stay; everything else, including every byte of a multi-byte character, is escaped).
pub fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }

    out
}

/// A completed HTTP exchange.
pub struct Response {
    pub status: u16,
    pub body: String,
}

// ---- offline fixtures (development and tests) -----------------------------------------------
//
// `AUJAR_FIXTURES=<dir>` makes every lookup read its answer from a file in <dir> instead of
// asking the network: fast, deterministic, and it works on a plane. `AUJAR_FIXTURES_RECORD=1`
// (together with `AUJAR_FIXTURES`) goes the other way: lookups ask the network as usual and
// each answer is saved in <dir>, so a set of fixtures is recorded once and replayed after.
//
// Only the status and the body are kept, never request headers (the NEPSE API key travels in
// one). The mode is for development: nothing else in Aujar reads these variables.

/// The environment variable that turns the offline mode on (a directory).
pub const FIXTURES_ENV: &str = "AUJAR_FIXTURES";
const FIXTURES_RECORD_ENV: &str = "AUJAR_FIXTURES_RECORD";
/// File names longer than this get a hash instead of the rest of the URL.
const MAX_FIXTURE_NAME: usize = 120;

struct Fixtures {
    dir: PathBuf,
    record: bool,
}

fn fixtures_from_env() -> Option<Fixtures> {
    let dir = std::env::var_os(FIXTURES_ENV).filter(|value| !value.is_empty())?;
    let record = std::env::var_os(FIXTURES_RECORD_ENV)
        .is_some_and(|value| !value.is_empty() && value != "0");

    Some(Fixtures {
        dir: PathBuf::from(dir),
        record,
    })
}

/// FNV-1a: a hash that never changes between Rust releases (fixture names must not).
fn fnv1a(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;

    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }

    hash
}

/// The file a URL's answer is kept in: the URL without `https://`, with everything except
/// letters, digits, `.` and `-` turned into `_` (so a name can never leave the directory),
/// and `.http` appended. Very long URLs are cut and given a hash to stay distinct.
pub fn fixture_name(url: &str) -> String {
    let url = url.strip_prefix("https://").unwrap_or(url);

    let mut name: String = url
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();

    if name.len() > MAX_FIXTURE_NAME {
        // The name is ASCII by now, so cutting at a byte count is safe.
        name.truncate(MAX_FIXTURE_NAME);
        name.push_str(&format!("-{:016x}", fnv1a(url)));
    }

    name.push_str(".http");
    name
}

/// A fixture is `HTTP <status>` on the first line and the body after it.
fn parse_fixture(text: &str) -> Option<Response> {
    let (first, body) = text.split_once('\n').unwrap_or((text, ""));
    let status = first.trim().strip_prefix("HTTP ")?.trim().parse().ok()?;

    Some(Response {
        status,
        body: body.to_owned(),
    })
}

fn replay_from(dir: &Path, url: &str) -> Result<Response, String> {
    let name = fixture_name(url);

    let text = std::fs::read_to_string(dir.join(&name)).map_err(|_| {
        format!("offline mode: no recorded answer {name} (record it with AUJAR_FIXTURES_RECORD=1)")
    })?;

    parse_fixture(&text)
        .ok_or_else(|| format!("offline mode: {name} is not a fixture (first line: HTTP <status>)"))
}

fn record_to(dir: &Path, url: &str, response: &Response) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        dir.join(fixture_name(url)),
        format!("HTTP {}\n{}", response.status, response.body),
    )
}

/// GET `url` (HTTPS only) with `curl`. Network and tool problems become readable errors;
/// HTTP error statuses are returned for the caller to interpret (a 404 can mean "no such
/// word").
pub fn get(url: &str) -> Result<Response, String> {
    get_with_header(url, None)
}

/// Like [`get`], with one extra request header (e.g. `Authorization: ...`). The header goes to
/// curl through its config on stdin, never the command line, so it does not show up in `ps`.
/// Header text with a quote or line break is refused.
pub fn get_with_header(url: &str, header: Option<&str>) -> Result<Response, String> {
    if !url.starts_with("https://") {
        return Err("refusing a non-HTTPS request".to_owned());
    }

    if header.is_some_and(|h| h.chars().any(|c| matches!(c, '"' | '\\' | '\n' | '\r'))) {
        return Err("refusing a malformed request header".to_owned());
    }

    let fixtures = fixtures_from_env();

    // Offline mode: answer from the recorded file, never from the network.
    if let Some(fixtures) = fixtures.as_ref().filter(|fixtures| !fixtures.record) {
        return replay_from(&fixtures.dir, url);
    }

    let response = curl_get(url, header)?;

    // Recording: keep the answer for later replays (status and body only).
    if let Some(fixtures) = &fixtures {
        let _ = record_to(&fixtures.dir, url, &response);
    }

    Ok(response)
}

/// The real request, through `curl`.
fn curl_get(url: &str, header: Option<&str>) -> Result<Response, String> {
    use std::io::Write;
    use std::process::Stdio;

    let mut command = Command::new("curl");

    if header.is_some() {
        command.args(["--config", "-"]).stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }

    let child = command
        .args([
            "-sS",
            "--location",
            "--max-time",
            "8",
            "--max-filesize",
            "2000000",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--user-agent",
            "Mozilla/5.0 (X11; Linux x86_64) aujar/0.1",
            "--header",
            "Accept: application/json",
            "--write-out",
            "\n%{http_code}",
            "--",
            url,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "`curl` is not installed".to_owned()
            } else {
                format!("could not run curl: {error}")
            }
        })?;

    let mut child = child;

    if let (Some(header), Some(mut stdin)) = (header, child.stdin.take()) {
        let _ = writeln!(stdin, "header = \"{header}\"");
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("could not run curl: {error}"))?;

    if !output.status.success() {
        return Err(match output.status.code() {
            Some(6 | 7) => "no internet connection".to_owned(),
            Some(28) => "the server did not answer in time".to_owned(),
            Some(35 | 51 | 58 | 60) => "a secure connection could not be made".to_owned(),
            _ => "the request failed".to_owned(),
        });
    }

    parse_curl_output(&String::from_utf8_lossy(&output.stdout))
}

/// Split `<body>\n<status>` as produced by `--write-out "\n%{http_code}"`.
fn parse_curl_output(text: &str) -> Result<Response, String> {
    let (body, status) = text
        .rsplit_once('\n')
        .ok_or_else(|| "an empty answer from the server".to_owned())?;

    let status: u16 = status
        .trim()
        .parse()
        .map_err(|_| "an unreadable answer from the server".to_owned())?;

    Ok(Response {
        status,
        body: body.to_owned(),
    })
}

/// GET and parse JSON; any status other than 200 is an error naming the status.
pub fn get_json(url: &str) -> Result<serde_json::Value, String> {
    let response = get(url)?;

    if response.status != 200 {
        return Err(match response.status {
            429 => "the service is rate limiting requests; try again in a minute".to_owned(),
            500..=599 => "the service is having problems".to_owned(),
            status => format!("the service answered HTTP {status}"),
        });
    }

    serde_json::from_str(&response.body).map_err(|_| "the service sent unreadable data".to_owned())
}

/// The three-state placeholder every lookup provider shows while it works.
pub fn loading_result(id: &str, what: &str) -> crate::model::SearchResult {
    crate::model::SearchResult::new(
        format!("loading:{id}"),
        format!("Looking up {what}\u{2026}"),
        crate::model::ResultKind::Loading,
        1850,
    )
}

/// The result shown when a lookup failed.
pub fn failure_result(id: &str, what: &str, reason: &str) -> crate::model::SearchResult {
    let mut reason = reason.to_owned();

    if let Some(first) = reason.get(..1) {
        reason.replace_range(..1, &first.to_uppercase());
    }

    crate::model::SearchResult::new(
        format!("notice:{id}"),
        format!("Couldn\u{2019}t look up {what}"),
        crate::model::ResultKind::Notice,
        1850,
    )
    .with_subtitle(reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn wait_ready(cache: &WebCache<u32>, key: &str, ttl: Duration) -> Lookup<u32> {
        for _ in 0..200 {
            match cache.get(key, ttl, || Ok(0)) {
                Lookup::Loading => std::thread::sleep(Duration::from_millis(5)),
                other => return other,
            }
        }

        Lookup::Loading
    }

    #[test]
    fn fixture_names_are_readable_and_cannot_leave_the_directory() {
        assert_eq!(
            fixture_name("https://api.dictionaryapi.dev/v2/entries/en/fire"),
            "api.dictionaryapi.dev_v2_entries_en_fire.http"
        );

        let nasty = fixture_name("https://example.com/a/../../etc/passwd?x=1&y=\u{e4}\\z");

        assert!(!nasty.contains('/') && !nasty.contains('\\'));
        assert!(nasty.ends_with(".http"));

        assert_ne!(
            fixture_name("https://example.com/a?x=1"),
            fixture_name("https://example.com/a?x=2")
        );
    }

    #[test]
    fn long_urls_get_short_names_that_stay_distinct() {
        let long = format!("https://example.com/{}", "a".repeat(400));
        let other = format!("{long}b");

        assert!(fixture_name(&long).len() < 200);
        assert_ne!(fixture_name(&long), fixture_name(&other));
        // And the same URL always gets the same name.
        assert_eq!(fixture_name(&long), fixture_name(&long));
    }

    #[test]
    fn a_recorded_answer_replays_exactly() {
        let dir = std::env::temp_dir().join(format!(
            "aujar-fixtures-roundtrip-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);

        let url = "https://example.com/v1/thing?q=1";
        let response = Response {
            status: 404,
            body: "{\"title\":\"No\"}\nsecond line".to_owned(),
        };

        record_to(&dir, url, &response).unwrap();

        let again = replay_from(&dir, url).unwrap();

        assert_eq!(again.status, 404);
        assert_eq!(again.body, response.body);

        // An answer that was never recorded is an explained error, not a network call.
        let missing = replay_from(&dir, "https://example.com/other").err().unwrap();

        assert!(missing.contains("no recorded answer"), "{missing}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_fixture_is_an_error_not_a_guess() {
        let dir = std::env::temp_dir().join(format!(
            "aujar-fixtures-damaged-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let url = "https://example.com/broken";

        std::fs::write(dir.join(fixture_name(url)), "this is not a fixture").unwrap();

        let error = replay_from(&dir, url).err().unwrap();

        assert!(error.contains("not a fixture"), "{error}");
        assert!(parse_fixture("HTTP 200\n{}").is_some());
        assert!(parse_fixture("HTTP abc\n{}").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn miss_loads_then_hits_without_refetching() {
        let cache = WebCache::<u32>::new();
        let fetches = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&fetches);

        let first = cache.get("k", Duration::from_secs(60), move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(7)
        });

        assert_eq!(first, Lookup::Loading);
        assert_eq!(
            wait_ready(&cache, "k", Duration::from_secs(60)),
            Lookup::Ready(7)
        );

        // A second ask is served from the cache.
        let counter = Arc::clone(&fetches);
        let again = cache.get("k", Duration::from_secs(60), move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(8)
        });

        assert_eq!(again, Lookup::Ready(7));
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failures_are_remembered_briefly() {
        let cache = WebCache::<u32>::new();

        cache.get("k", Duration::from_secs(60), || Err("offline".to_owned()));

        assert_eq!(
            wait_ready(&cache, "k", Duration::from_secs(60)),
            Lookup::Failed("offline".to_owned())
        );
    }

    #[test]
    fn pending_lookups_are_not_started_twice() {
        let cache = WebCache::<u32>::new();
        let starts = Arc::new(AtomicUsize::new(0));

        for _ in 0..3 {
            let starts = Arc::clone(&starts);

            cache.get("k", Duration::from_secs(60), move || {
                starts.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(50));
                Ok(1)
            });
        }

        std::thread::sleep(Duration::from_millis(120));

        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn encodes_every_byte_of_unicode() {
        assert_eq!(url_encode("São Paulo"), "S%C3%A3o%20Paulo");
        assert_eq!(url_encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(url_encode("plain-text_1.0~"), "plain-text_1.0~");
    }

    #[test]
    fn splits_the_status_off_curl_output() {
        let response = parse_curl_output("{\"a\":1}\n200").unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.body, "{\"a\":1}");
        assert!(parse_curl_output("no newline").is_err());
        assert!(parse_curl_output("body\nabc").is_err());
    }

    #[test]
    fn refuses_plain_http() {
        assert!(get("http://example.org").is_err());
    }

    #[test]
    fn failure_text_starts_with_a_capital() {
        let result = failure_result("x", "\u{201c}x\u{201d}", "no internet connection");

        assert_eq!(result.subtitle.as_deref(), Some("No internet connection"));
    }
}
