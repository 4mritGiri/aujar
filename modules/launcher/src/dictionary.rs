//! Dictionary lookup (Spotlight's "Look Up"): `define serendipity`, `meaning of ubiquitous`,
//! `what does ephemeral mean`.
//!
//! Definitions come from the free dictionaryapi.dev service (English). Only the single word
//! you asked about is sent, and only after you type one of the trigger phrases.

use crate::{
    model::{LaunchSpec, Provider, ResultKind, SearchResult},
    web::{self, Lookup, WebCache},
};
use std::time::Duration;

const API: &str = "https://api.dictionaryapi.dev/v2/entries/en/";
const TTL: Duration = Duration::from_secs(24 * 3600);
/// Parts of speech shown (a word like "set" has a dozen).
const MAX_MEANINGS: usize = 3;
/// Definitions listed per part of speech.
const MAX_DEFINITIONS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Meaning {
    part_of_speech: String,
    definitions: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    word: String,
    phonetic: Option<String>,
    meanings: Vec<Meaning>,
}

/// Outcome of a lookup; a word the service does not know is an answer, not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Found {
    Entry(Entry),
    Unknown,
}

const PREFIXES: [&str; 9] = [
    "define ",
    "definition of ",
    "definition ",
    "def ",
    "dict ",
    "lookup ",
    "look up ",
    "meaning of ",
    "meaning ",
];

/// A plausible English word: letters, with inner hyphens or apostrophes.
fn valid_word(word: &str) -> bool {
    let count = word.chars().count();

    (2..=40).contains(&count)
        && word
            .chars()
            .all(|c| c.is_alphabetic() || c == '-' || c == '\'')
        && word.chars().next().is_some_and(char::is_alphabetic)
        && word.chars().last().is_some_and(char::is_alphabetic)
}

/// `define cat` / `what does cat mean` -> `cat`.
fn parse_query(query: &str) -> Option<String> {
    let lower = query.trim().to_lowercase();

    let word = PREFIXES
        .iter()
        .find_map(|prefix| lower.strip_prefix(prefix))
        .or_else(|| {
            lower
                .strip_prefix("what does ")
                .and_then(|rest| rest.strip_suffix(" mean"))
        })
        .or_else(|| lower.strip_prefix("what is the meaning of "))?
        .trim()
        .trim_matches(|c| matches!(c, '"' | '\'' | '\u{201c}' | '\u{201d}' | '?'));

    valid_word(word).then(|| word.to_owned())
}

fn parse_entry(value: &serde_json::Value) -> Option<Found> {
    // An unknown word is `{"title": "No Definitions Found", ...}`, not an array.
    let first = value.as_array()?.first()?;

    let word = first.get("word")?.as_str()?.to_owned();

    let phonetic = first
        .get("phonetic")
        .and_then(|text| text.as_str())
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            first
                .get("phonetics")?
                .as_array()?
                .iter()
                .find_map(|item| item.get("text")?.as_str().map(str::to_owned))
        });

    let meanings: Vec<Meaning> = first
        .get("meanings")?
        .as_array()?
        .iter()
        .filter_map(|meaning| {
            let definitions: Vec<(String, Option<String>)> = meaning
                .get("definitions")?
                .as_array()?
                .iter()
                .filter_map(|item| {
                    let text = item.get("definition")?.as_str()?.trim().to_owned();
                    let example = item
                        .get("example")
                        .and_then(|example| example.as_str())
                        .map(|example| example.trim().to_owned())
                        .filter(|example| !example.is_empty());

                    (!text.is_empty()).then_some((text, example))
                })
                .take(MAX_DEFINITIONS)
                .collect();

            (!definitions.is_empty()).then(|| Meaning {
                part_of_speech: meaning
                    .get("partOfSpeech")
                    .and_then(|part| part.as_str())
                    .unwrap_or("")
                    .to_owned(),
                definitions,
            })
        })
        .collect();

    (!meanings.is_empty()).then_some(Found::Entry(Entry {
        word,
        phonetic,
        meanings,
    }))
}

fn fetch(word: &str) -> Result<Found, String> {
    let response = web::get(&format!("{API}{}", web::url_encode(word)))?;

    match response.status {
        404 => Ok(Found::Unknown),
        200 => {
            let value: serde_json::Value = serde_json::from_str(&response.body)
                .map_err(|_| "the dictionary sent unreadable data".to_owned())?;

            parse_entry(&value).ok_or_else(|| "the dictionary sent an unexpected answer".to_owned())
        }
        429 => Err("the dictionary is rate limiting requests; try again in a minute".to_owned()),
        status => Err(format!("the dictionary answered HTTP {status}")),
    }
}

fn detail_text(meaning: &Meaning) -> String {
    let mut lines = Vec::new();

    for (index, (definition, example)) in meaning.definitions.iter().enumerate() {
        lines.push(format!("{}. {definition}", index + 1));

        if let Some(example) = example {
            lines.push(format!("   \u{201c}{example}\u{201d}"));
        }
    }

    lines.join("\n")
}

fn results_for(entry: &Entry) -> Vec<SearchResult> {
    entry
        .meanings
        .iter()
        .take(MAX_MEANINGS)
        .enumerate()
        .map(|(index, meaning)| {
            let mut subtitle = meaning.part_of_speech.clone();

            if let Some(phonetic) = entry.phonetic.as_ref().filter(|_| index == 0) {
                if !subtitle.is_empty() {
                    subtitle.push_str(" \u{b7} ");
                }

                subtitle.push_str(phonetic);
            }

            let first = meaning
                .definitions
                .first()
                .map_or("", |(text, _)| text.as_str());

            SearchResult::new(
                format!("dict:{}:{index}", entry.word),
                &entry.word,
                ResultKind::Definition,
                1850 - index as u32,
            )
            .with_subtitle(if subtitle.is_empty() {
                first.to_owned()
            } else {
                subtitle
            })
            .with_detail(format!("{first}\n{}", detail_text(meaning)))
        })
        .collect()
}

pub struct DictionaryProvider {
    cache: WebCache<Found>,
}

impl DictionaryProvider {
    pub fn new() -> Self {
        Self {
            cache: WebCache::new(),
        }
    }
}

impl Default for DictionaryProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for DictionaryProvider {
    fn id(&self) -> &'static str {
        "dict"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let Some(word) = parse_query(query) else {
            return Vec::new();
        };

        let what = format!("\u{201c}{word}\u{201d}");
        let owned = word.clone();

        match self.cache.get(&word, TTL, move || fetch(&owned)) {
            Lookup::Loading => vec![web::loading_result(&word, &what)],
            Lookup::Failed(reason) => vec![web::failure_result(&word, &what, &reason)],
            Lookup::Ready(Found::Unknown) => vec![
                SearchResult::new(
                    format!("notice:{word}"),
                    format!("No definition for {what}"),
                    ResultKind::Notice,
                    1850,
                )
                .with_subtitle("Check the spelling"),
            ],
            Lookup::Ready(Found::Entry(entry)) => results_for(&entry),
        }
    }

    /// Enter opens the entry on Wiktionary, which has far more than three definitions.
    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let rest = id.strip_prefix("dict:")?;
        let word = rest.rsplit_once(':').map_or(rest, |(word, _)| word);

        if !valid_word(word) {
            return Some(Err("not a dictionary word".to_owned()));
        }

        Some(Ok(LaunchSpec {
            title: word.to_owned(),
            program: "xdg-open".to_owned(),
            args: vec![format!(
                "https://en.wiktionary.org/wiki/{}",
                web::url_encode(word)
            )],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[{
        "word": "serendipity",
        "phonetic": "/ˌsɛɹənˈdɪpɪti/",
        "phonetics": [{"text": "/ˌsɛɹənˈdɪpɪti/", "audio": ""}],
        "meanings": [
            {"partOfSpeech": "noun", "definitions": [
                {"definition": "The faculty of making fortunate discoveries by accident.", "example": "A happy serendipity."},
                {"definition": "An unplanned fortunate discovery."}
            ]},
            {"partOfSpeech": "adjective", "definitions": [{"definition": "Fortunate."}]}
        ]
    }]"#;

    #[test]
    fn recognises_trigger_phrases() {
        assert_eq!(parse_query("define cat").as_deref(), Some("cat"));
        assert_eq!(
            parse_query("Meaning of Ubiquitous").as_deref(),
            Some("ubiquitous")
        );
        assert_eq!(
            parse_query("what does ephemeral mean").as_deref(),
            Some("ephemeral")
        );
        assert_eq!(parse_query("define \"cat\"").as_deref(), Some("cat"));
        assert_eq!(
            parse_query("define well-known").as_deref(),
            Some("well-known")
        );
    }

    #[test]
    fn ignores_everything_else() {
        assert_eq!(parse_query("cat"), None);
        assert_eq!(parse_query("define"), None);
        assert_eq!(parse_query("define a"), None);
        assert_eq!(parse_query("define two words"), None);
        assert_eq!(parse_query("define ../etc"), None);
        assert_eq!(parse_query("define 123"), None);
        assert_eq!(parse_query("firefox"), None);
    }

    #[test]
    fn parses_the_service_answer() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let Some(Found::Entry(entry)) = parse_entry(&value) else {
            panic!("expected an entry");
        };

        assert_eq!(entry.word, "serendipity");
        assert_eq!(entry.meanings.len(), 2);
        assert_eq!(entry.meanings[0].definitions.len(), 2);
        assert_eq!(
            entry.meanings[0].definitions[0].1.as_deref(),
            Some("A happy serendipity.")
        );
    }

    #[test]
    fn the_not_found_object_is_not_an_entry() {
        let value: serde_json::Value =
            serde_json::from_str(r#"{"title":"No Definitions Found"}"#).unwrap();

        assert_eq!(parse_entry(&value), None);
    }

    #[test]
    fn one_result_per_part_of_speech() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let Some(Found::Entry(entry)) = parse_entry(&value) else {
            panic!("expected an entry");
        };

        let results = results_for(&entry);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].kind, ResultKind::Definition);
        assert!(results[0].subtitle.as_deref().unwrap().starts_with("noun"));
        assert!(
            results[0]
                .detail
                .as_deref()
                .unwrap()
                .contains("1. The faculty")
        );
        assert!(results[0].score > results[1].score);
    }

    #[test]
    fn serves_cached_entries_without_the_network() {
        let provider = DictionaryProvider::new();
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();

        provider
            .cache
            .insert("serendipity", parse_entry(&value).unwrap());

        assert_eq!(provider.search("define serendipity").len(), 2);
        assert!(provider.search("serendipity").is_empty());
    }

    #[test]
    fn enter_opens_wiktionary_and_rejects_bad_words() {
        let provider = DictionaryProvider::new();
        let spec = provider.resolve("dict:cat:0").unwrap().unwrap();

        assert_eq!(spec.program, "xdg-open");
        assert_eq!(spec.args, ["https://en.wiktionary.org/wiki/cat"]);
        assert!(provider.resolve("dict:../x:0").unwrap().is_err());
        assert!(provider.resolve("apps:x").is_none());
    }
}
