//! The desktop's "recently used files" list (`~/.local/share/recently-used.xbel`, written by
//! GNOME, GTK and many other apps). This is the file history shown in the empty launcher
//! and used to rank file results.

use std::path::PathBuf;

fn xbel_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
        })?;

    Some(base.join("recently-used.xbel"))
}

/// `YYYY-MM-DDTHH:MM:SS...` to Unix seconds (UTC); `None` for anything else.
pub fn iso_to_unix(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();

    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }

    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();

    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // Days since 1970-01-01 (Howard Hinnant's civil-from-days, inverted).
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;

    u64::try_from(days * 86_400 + hour * 3600 + minute * 60 + second).ok()
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')?;

    Some(&tag[start..start + end])
}

fn decode_entities(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        let escaped = if bytes[index] == b'%' && index + 2 < bytes.len() {
            text.get(index + 1..index + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        } else {
            None
        };

        if let Some(value) = escaped {
            out.push(value);
            index += 3;
            continue;
        }

        out.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// Parse the XBEL text into `(path, last used as Unix seconds)`, newest first.
pub fn parse_xbel(content: &str) -> Vec<(PathBuf, u64)> {
    let mut entries = Vec::new();

    for chunk in content.split("<bookmark ").skip(1) {
        let Some(end) = chunk.find('>') else {
            continue;
        };

        // Re-add a leading space so attribute() can match the first attribute too.
        let tag = format!(" {}", &chunk[..end]);

        let Some(href) = attribute(&tag, "href") else {
            continue;
        };

        let Some(encoded) = href.strip_prefix("file://") else {
            continue;
        };

        let path = PathBuf::from(percent_decode(&decode_entities(encoded)));

        let stamp = ["modified", "visited", "added"]
            .iter()
            .filter_map(|name| attribute(&tag, name).and_then(iso_to_unix))
            .max()
            .unwrap_or(0);

        entries.push((path, stamp));
    }

    entries.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    entries.truncate(500);

    entries
}

/// The user's recent files that still exist, newest first.
pub fn read_recent_files() -> Vec<(PathBuf, u64)> {
    let Some(path) = xbel_path() else {
        return Vec::new();
    };

    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };

    parse_xbel(&content)
        .into_iter()
        .filter(|(path, _)| path.is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_timestamps() {
        assert_eq!(iso_to_unix("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_to_unix("2000-03-01T00:00:00.5Z"), Some(951_868_800));
        assert_eq!(iso_to_unix("nonsense"), None);
    }

    #[test]
    fn parses_bookmarks_newest_first() {
        let xml = r#"<?xml version="1.0"?>
<xbel version="1.0">
  <bookmark href="file:///home/u/Documents/My%20Report.pdf" added="2026-01-01T00:00:00Z" modified="2026-02-01T00:00:00Z" visited="2026-01-01T00:00:00Z">
    <info/>
  </bookmark>
  <bookmark href="file:///home/u/a&amp;b.txt" added="2026-03-01T00:00:00Z" modified="2026-03-01T00:00:00Z" visited="2026-03-01T00:00:00Z"/>
  <bookmark href="https://example.org/" modified="2026-04-01T00:00:00Z"/>
</xbel>"#;

        let entries = parse_xbel(xml);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, PathBuf::from("/home/u/a&b.txt"));
        assert_eq!(
            entries[1].0,
            PathBuf::from("/home/u/Documents/My Report.pdf")
        );
    }
}
