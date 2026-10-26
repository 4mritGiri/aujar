//! NEPSE price alerts: `nepse alert nabil above 600` saves an alert; the daemon checks prices
//! about once a minute while the market is open (every ten minutes when it is closed) and shows
//! a desktop notification, once, when a price crosses its target. Alerts live in
//! `~/.config/aujar/nepse-alerts.json`.
//!
//! Notifications use `notify-send` (package `libnotify-bin` / `libnotify`). If it is missing the
//! alert is kept and tried again next time instead of being lost.

use super::{Quote, arrow_text, change_of, fetch_table, market_is_open, rs, valid_symbol};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, Once},
    time::Duration,
};

pub const MAX_ALERTS: usize = 50;

const OPEN_INTERVAL: Duration = Duration::from_secs(60);
/// While the market is closed prices do not move: look every this many intervals.
const CLOSED_EVERY: u32 = 10;

/// Serialises file access between the watcher thread and Enter on a result.
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    pub symbol: String,
    /// Fire when the price rises to `price` (true) or falls to it (false).
    pub above: bool,
    pub price: f64,
}

impl Alert {
    pub fn word(&self) -> &'static str {
        if self.above { "above" } else { "below" }
    }

    /// One alert per symbol and direction: setting another replaces it.
    fn same_slot(&self, other: &Alert) -> bool {
        self.symbol == other.symbol && self.above == other.above
    }

    pub fn reached(&self, price: f64) -> bool {
        if self.above {
            price >= self.price
        } else {
            price <= self.price
        }
    }

    /// `NABIL:above:600.00`, the tail of a result id.
    pub fn id_part(&self) -> String {
        format!("{}:{}:{:.2}", self.symbol, self.word(), self.price)
    }

    pub fn from_id_part(text: &str) -> Option<Alert> {
        let mut parts = text.split(':');
        let symbol = parts.next()?;
        let direction = parts.next()?;
        let price = parts.next()?;

        if parts.next().is_some() || !valid_symbol(symbol) {
            return None;
        }

        let above = match direction {
            "above" => true,
            "below" => false,
            _ => return None,
        };
        let price: f64 = price
            .parse()
            .ok()
            .filter(|price: &f64| price.is_finite() && *price > 0.0)?;

        Some(Alert {
            symbol: symbol.to_owned(),
            above,
            price,
        })
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    alerts: Vec<Alert>,
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn default_path() -> Option<PathBuf> {
    super::config_dir().map(|dir| dir.join("nepse-alerts.json"))
}

fn sane(alert: &Alert) -> bool {
    valid_symbol(&alert.symbol) && alert.price.is_finite() && alert.price > 0.0
}

fn load_from(path: &Path) -> Vec<Alert> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Stored>(&text).ok())
        .map(|stored| {
            stored
                .alerts
                .into_iter()
                .filter(sane)
                .take(MAX_ALERTS)
                .collect()
        })
        .unwrap_or_default()
}

fn save_to(path: &Path, alerts: &[Alert]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|error| format!("could not save alerts: {error}"))?;
    }

    let text = serde_json::to_string_pretty(&Stored {
        alerts: alerts.to_vec(),
    })
    .map_err(|error| format!("could not save alerts: {error}"))?;
    let temp = path.with_extension("json.tmp");

    fs::write(&temp, text)
        .and_then(|()| fs::rename(&temp, path))
        .map_err(|error| {
            let _ = fs::remove_file(&temp);

            format!("could not save alerts: {error}")
        })
}

/// Add an alert; `Ok(true)` when it replaced one for the same symbol and direction.
fn add_to(path: &Path, alert: Alert) -> Result<bool, String> {
    let mut alerts = load_from(path);
    let before = alerts.len();

    alerts.retain(|existing| !existing.same_slot(&alert));

    let replaced = alerts.len() < before;

    if !replaced && alerts.len() >= MAX_ALERTS {
        return Err(format!(
            "you already have {MAX_ALERTS} alerts: remove one first (nepse alerts)"
        ));
    }

    alerts.push(alert);
    save_to(path, &alerts)?;

    Ok(replaced)
}

fn remove_from(path: &Path, alert: &Alert) -> Result<bool, String> {
    let mut alerts = load_from(path);
    let before = alerts.len();

    alerts.retain(|existing| {
        !(existing.same_slot(alert) && (existing.price - alert.price).abs() < 0.005)
    });

    if alerts.len() == before {
        return Ok(false);
    }

    save_to(path, &alerts)?;

    Ok(true)
}

pub fn load() -> Vec<Alert> {
    let _guard = lock();

    default_path()
        .map(|path| load_from(&path))
        .unwrap_or_default()
}

pub fn add(alert: Alert) -> Result<bool, String> {
    let _guard = lock();
    let path = default_path().ok_or_else(|| "no configuration directory".to_owned())?;

    add_to(&path, alert)
}

pub fn remove(alert: &Alert) -> Result<bool, String> {
    let _guard = lock();
    let path = default_path().ok_or_else(|| "no configuration directory".to_owned())?;

    remove_from(&path, alert)
}

/// Alerts whose price has been reached, with the quote that reached it.
fn triggered(alerts: &[Alert], quotes: &[Quote]) -> Vec<(Alert, Quote)> {
    alerts
        .iter()
        .filter_map(|alert| {
            let quote = quotes.iter().find(|quote| quote.symbol == alert.symbol)?;

            alert
                .reached(quote.price)
                .then(|| (alert.clone(), quote.clone()))
        })
        .collect()
}

/// `(title, body)` of the notification for a reached alert.
fn notification(alert: &Alert, quote: &Quote) -> (String, String) {
    let verb = if alert.above {
        "rose above"
    } else {
        "fell below"
    };
    let change = change_of(quote)
        .map(|(delta, percent)| format!(" \u{b7} {}", arrow_text(delta, percent)))
        .unwrap_or_default();

    (
        format!("{} {verb} {}", alert.symbol, rs(alert.price)),
        format!("{} is now {}{change}", alert.symbol, rs(quote.price)),
    )
}

/// Show a desktop notification. Arguments go to `notify-send` as separate words (never a shell),
/// after `--` so text can never be read as an option.
pub fn notify(title: &str, body: &str) -> bool {
    Command::new("notify-send")
        .args(["--app-name=Aujar", "--expire-time=20000", "--"])
        .arg(title)
        .arg(body)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Fire every reached alert once: it is removed only after its notification was shown.
fn run_check(path: &Path, quotes: &[Quote], notify: &dyn Fn(&str, &str) -> bool) -> usize {
    let _guard = lock();
    let alerts = load_from(path);
    let mut fired = 0;

    for (alert, quote) in triggered(&alerts, quotes) {
        let (title, body) = notification(&alert, &quote);

        if notify(&title, &body) {
            let _ = remove_from(path, &alert);

            fired += 1;
        } else {
            eprintln!(
                "aujar: could not show the {} alert (is notify-send installed?)",
                alert.symbol
            );
        }
    }

    fired
}

fn watch(enabled: fn() -> bool) {
    let mut closed_ticks: u32 = 0;

    loop {
        std::thread::sleep(OPEN_INTERVAL);

        if !enabled() || load().is_empty() {
            continue;
        }

        if market_is_open() == Some(false) {
            closed_ticks += 1;

            // Look on the first closed minute, then only now and then.
            if closed_ticks % CLOSED_EVERY != 1 {
                continue;
            }
        } else {
            closed_ticks = 0;
        }

        let Ok(quotes) = fetch_table() else {
            continue;
        };

        if let Some(path) = default_path() {
            run_check(&path, &quotes, &notify);
        }
    }
}

/// Start the background checker (once per process). `enabled` is asked each time, so turning
/// the NEPSE plugin off pauses alerts without a restart.
pub fn spawn_watcher(enabled: fn() -> bool) {
    static STARTED: Once = Once::new();

    STARTED.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("nepse-alerts".to_owned())
            .spawn(move || watch(enabled));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aujar-alerts-{}-{name}", std::process::id()));

        let _ = fs::remove_dir_all(&dir);

        dir.join("nepse-alerts.json")
    }

    fn alert(symbol: &str, above: bool, price: f64) -> Alert {
        Alert {
            symbol: symbol.to_owned(),
            above,
            price,
        }
    }

    fn quote(symbol: &str, price: f64, previous: f64) -> Quote {
        serde_json::from_value::<serde_json::Value>(serde_json::json!({
            "symbol": symbol, "lastTradedPrice": price, "previousClose": previous
        }))
        .ok()
        .and_then(|value| super::super::quote_from(value.as_object()?))
        .unwrap()
    }

    #[test]
    fn reaching_a_price_depends_on_the_direction() {
        assert!(alert("A1", true, 600.0).reached(600.0));
        assert!(alert("A1", true, 600.0).reached(612.0));
        assert!(!alert("A1", true, 600.0).reached(599.99));
        assert!(alert("A1", false, 500.0).reached(500.0));
        assert!(!alert("A1", false, 500.0).reached(500.01));
    }

    #[test]
    fn ids_round_trip_and_reject_junk() {
        let original = alert("NABIL", true, 612.5);

        assert_eq!(original.id_part(), "NABIL:above:612.50");
        assert_eq!(Alert::from_id_part(&original.id_part()), Some(original));
        assert_eq!(
            Alert::from_id_part("NABIL:below:500.00"),
            Some(alert("NABIL", false, 500.0))
        );

        for bad in [
            "",
            "NABIL",
            "NABIL:sideways:5",
            "NABIL:above:-1",
            "NABIL:above:nan",
            "NABIL:above:5:6",
            "../x:above:5",
            "N:above:5",
        ] {
            assert_eq!(Alert::from_id_part(bad), None, "{bad}");
        }
    }

    #[test]
    fn stores_replaces_and_removes_alerts() {
        let path = temp_file("store");

        assert!(load_from(&path).is_empty());
        assert_eq!(add_to(&path, alert("NABIL", true, 600.0)), Ok(false));
        assert_eq!(add_to(&path, alert("NABIL", false, 500.0)), Ok(false));
        // The same symbol and direction is replaced, not duplicated.
        assert_eq!(add_to(&path, alert("NABIL", true, 650.0)), Ok(true));

        let alerts = load_from(&path);

        assert_eq!(alerts.len(), 2);
        assert!(alerts.contains(&alert("NABIL", true, 650.0)));

        // Removing needs the matching price.
        assert_eq!(remove_from(&path, &alert("NABIL", true, 600.0)), Ok(false));
        assert_eq!(remove_from(&path, &alert("NABIL", true, 650.0)), Ok(true));
        assert_eq!(load_from(&path), vec![alert("NABIL", false, 500.0)]);

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_number_of_alerts_is_capped() {
        let path = temp_file("cap");

        for index in 0..MAX_ALERTS {
            add_to(&path, alert(&format!("SYM{index}"), true, 10.0)).unwrap();
        }

        assert!(add_to(&path, alert("EXTRA", true, 10.0)).is_err());
        // Replacing an existing one still works when full.
        assert_eq!(add_to(&path, alert("SYM0", true, 11.0)), Ok(true));

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_damaged_file_is_ignored_and_bad_entries_dropped() {
        let path = temp_file("bad");

        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "not json").unwrap();
        assert!(load_from(&path).is_empty());

        fs::write(
            &path,
            r#"{"alerts":[{"symbol":"../x","above":true,"price":5},{"symbol":"OK1","above":true,"price":-1},
                          {"symbol":"GOOD","above":false,"price":5}]}"#,
        )
        .unwrap();
        assert_eq!(load_from(&path), vec![alert("GOOD", false, 5.0)]);

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn fires_once_and_only_after_the_notification_was_shown() {
        let path = temp_file("fire");

        add_to(&path, alert("NABIL", true, 600.0)).unwrap();
        add_to(&path, alert("NICA", false, 400.0)).unwrap();
        add_to(&path, alert("ADBL", true, 999.0)).unwrap();

        let quotes = [
            quote("NABIL", 603.2, 595.0),
            quote("NICA", 410.0, 412.0),
            quote("ADBL", 330.0, 320.0),
        ];
        let shown: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());

        // A failing notifier keeps the alert for next time.
        assert_eq!(run_check(&path, &quotes, &|_, _| false), 0);
        assert_eq!(load_from(&path).len(), 3);

        let fired = run_check(&path, &quotes, &|title, body| {
            shown.borrow_mut().push((title.to_owned(), body.to_owned()));
            true
        });

        assert_eq!(fired, 1);
        assert_eq!(shown.borrow()[0].0, "NABIL rose above Rs 600.00");
        assert!(
            shown.borrow()[0]
                .1
                .starts_with("NABIL is now Rs 603.20 \u{b7} \u{25b2} 8.20"),
            "{:?}",
            shown.borrow()
        );

        // Fired alerts are gone; the others stay.
        assert_eq!(load_from(&path).len(), 2);
        assert_eq!(run_check(&path, &quotes, &|_, _| true), 0);

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_falling_price_is_described_as_such() {
        let (title, body) =
            notification(&alert("NICA", false, 400.0), &quote("NICA", 398.5, 410.0));

        assert_eq!(title, "NICA fell below Rs 400.00");
        assert!(body.contains('\u{25bc}'), "{body}");
    }
}
