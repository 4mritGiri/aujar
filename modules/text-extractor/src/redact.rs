//! Auto-Redact: find the words in a picture that look like secrets or personal data, so they
//! can be covered before the picture is shared.
//!
//! Works on the words Tesseract found ([`crate::ocr::recognize_words`]). A word is covered when
//! it is an email address, a known kind of key or token (AWS, Stripe, GitHub, Slack, GitLab,
//! Google, JWT), a long random-looking token, or a card number; and every word after a label
//! such as `PASSWORD:` / `SECRET=` / `token:` on the same line. Recognition is not perfect, so
//! the result must be checked before sharing; the overlay shows it.

use crate::ocr::Word;
use regex::Regex;
use std::sync::OnceLock;

/// A box to cover, in the same pixels as the words' boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cover {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

fn patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();

    PATTERNS.get_or_init(|| {
        [
            // Email address.
            r"^[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}$",
            // AWS access key id.
            r"^(AKIA|ASIA|AGPA|AIDA|AROA)[A-Z0-9]{12,}$",
            // Stripe keys.
            r"^(sk|pk|rk)_(live|test)_[A-Za-z0-9]{6,}$",
            // GitHub tokens.
            r"^(gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})$",
            // Slack tokens.
            r"^xox[abprs]-[A-Za-z0-9-]{10,}$",
            // GitLab personal access token.
            r"^glpat-[A-Za-z0-9_-]{16,}$",
            // Google API key.
            r"^AIza[0-9A-Za-z_-]{30,}$",
            // JSON Web Token.
            r"^eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}$",
            // Private key block marker.
            r"^-----BEGIN$",
        ]
        .iter()
        .map(|pattern| Regex::new(pattern).expect("valid redaction pattern"))
        .collect()
    })
}

/// Labels whose value follows them on the same line.
const LABELS: [&str; 10] = [
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "apikey",
    "api_key",
    "credential",
    "private_key",
    "access_key",
];

/// The word without punctuation around it (`"key",` is `key`).
fn bare(word: &str) -> &str {
    word.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '`'
        )
    })
}

/// A card number: 13 to 19 digits (spaces and dashes allowed) that pass the Luhn check.
fn card_number(word: &str) -> bool {
    let digits: Vec<u32> = word
        .chars()
        .filter(|c| !matches!(c, ' ' | '-'))
        .map(|c| c.to_digit(10))
        .collect::<Option<_>>()
        .unwrap_or_default();

    if !(13..=19).contains(&digits.len()) {
        return false;
    }

    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                if d * 2 > 9 { d * 2 - 9 } else { d * 2 }
            } else {
                d
            }
        })
        .sum();

    sum % 10 == 0
}

/// Long, mixed letters and digits, no spaces: an API key, a hash, a session id.
fn random_token(word: &str) -> bool {
    word.len() >= 24
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '/' | '='))
        && word.chars().any(|c| c.is_ascii_digit())
        && word.chars().any(|c| c.is_ascii_alphabetic())
}

/// Whether this single word looks sensitive on its own.
pub fn sensitive_word(word: &str) -> bool {
    let word = bare(word);

    // `KEY=value` in one word: judge the value.
    let value = word.split_once('=').map_or(word, |(_, value)| value);

    patterns()
        .iter()
        .any(|pattern| pattern.is_match(word) || pattern.is_match(value))
        || card_number(word)
        || random_token(value)
}

/// Whether this word is a label whose value follows (`PASSWORD:`, `api_key=`, `Token:`).
fn label(word: &str) -> bool {
    let word = bare(word);
    let Some(name) = word.strip_suffix(':').or_else(|| word.strip_suffix('=')) else {
        return false;
    };
    let name = name.to_lowercase();

    LABELS.iter().any(|label| name.contains(label))
}

/// The boxes to cover, padded a little so no edge of a glyph shows.
pub fn covers(words: &[Word]) -> Vec<Cover> {
    let mut out: Vec<Cover> = Vec::new();
    let mut after_label_on: Option<(u32, u32, u32)> = None;
    // The line and right edge of the last cover, to join neighbours: OCR sometimes splits one
    // token in two, and the gap between two boxes would show a sliver of it.
    let mut last: Option<((u32, u32, u32), u32)> = None;

    for word in words {
        if after_label_on.is_some_and(|line| line != word.line) {
            after_label_on = None;
        }

        let inline_secret = word
            .text
            .split_once('=')
            .is_some_and(|(name, value)| !value.is_empty() && label(&format!("{name}=")));

        if after_label_on.is_some() || sensitive_word(&word.text) || inline_secret {
            let pad = (word.height / 6).max(2);
            let cover = Cover {
                left: word.left.saturating_sub(pad),
                top: word.top.saturating_sub(pad),
                width: word.width + 2 * pad,
                height: word.height + 2 * pad,
            };
            let right = cover.left + cover.width;

            match (last, out.last_mut()) {
                (Some((line, edge)), Some(previous))
                    if line == word.line && cover.left <= edge + word.height =>
                {
                    let top = previous.top.min(cover.top);
                    let bottom = (previous.top + previous.height).max(cover.top + cover.height);

                    previous.width = right.max(edge) - previous.left;
                    previous.top = top;
                    previous.height = bottom - top;
                }
                _ => out.push(cover),
            }

            last = Some((word.line, right));
        } else {
            last = None;
        }

        if label(&word.text) {
            after_label_on = Some(word.line);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, line: u32, left: u32) -> Word {
        Word {
            left,
            top: 10,
            width: 50,
            height: 12,
            line: (1, 1, line),
            text: text.to_owned(),
        }
    }

    #[test]
    fn known_secret_shapes() {
        for secret in [
            "secops-lead@enterprise.io",
            "AKIAIOSFODNN7EXAMPLE",
            "sk_live_51N9x82K9LmPqR7vT",
            "ghp_abcdefghijklmnopqrstuvwxyz0123",
            "xoxb-1234567890-abcdef",
            "glpat-abcdefghijklmnop1234",
            "AIzaSyA1234567890abcdefghijklmnopqrstu",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
            "4111111111111111",
            "\"AKIAIOSFODNN7EXAMPLE\",",
            "a1b2c3d4e5f6a7b8c9d0e1f2a3b4",
        ] {
            assert!(sensitive_word(secret), "{secret} not caught");
        }
    }

    #[test]
    fn ordinary_words_stay() {
        for plain in [
            "kubectl",
            "prod-api-credentials",
            "AWS_ACCESS_KEY_ID:",
            "https://example.com",
            "4111111111111112",
            "2026-11-28",
            "Hello,",
            "internationalization-tests",
        ] {
            assert!(!sensitive_word(plain), "{plain} wrongly caught");
        }
    }

    #[test]
    fn values_after_a_label_are_covered_until_the_line_ends() {
        let words = [
            word("PASSWORD:", 1, 0),
            word("hunter2", 1, 60),
            word("again", 1, 120),
            word("next", 2, 0),
            word("line", 2, 60),
        ];

        let covered = covers(&words);

        // `hunter2` and `again` as one box (left 60 to 170, padded by 2); nothing on line 2.
        assert_eq!(covered.len(), 1);
        assert_eq!(
            (covered[0].left, covered[0].left + covered[0].width),
            (58, 172)
        );
    }

    #[test]
    fn a_token_split_in_two_becomes_one_cover() {
        // `sk_live_51N9x82K9LmPqR7vT` read as `sk_` and `live_51N9x82K9LmPqR7vT` after a label.
        let words = [
            word("SECRET:", 1, 0),
            word("sk_", 1, 60),
            word("live_51N9x82K9LmPqR7vT", 1, 115),
        ];
        let covers = covers(&words);

        assert_eq!(covers.len(), 1);
        assert_eq!((covers[0].left, covers[0].width), (58, 115 + 50 + 2 - 58));
    }

    #[test]
    fn inline_assignments_are_covered() {
        let words = [
            word("DB_PASSWORD=hunter2", 1, 0),
            word("PATH=/usr/bin", 1, 200),
        ];

        assert_eq!(covers(&words).len(), 1);
    }

    #[test]
    fn covers_are_padded_but_never_negative() {
        let cover = covers(&[word("AKIAIOSFODNN7EXAMPLE", 1, 1)])[0];

        assert_eq!(cover.left, 0);
        assert_eq!(cover.width, 54);
        assert_eq!(cover.top, 8);
    }
}
