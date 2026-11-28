//! The operating system's own text recognition: Windows.Media.Ocr on Windows 10/11, Apple's
//! Vision framework on macOS. Both are on every machine, fast, and keep everything local.
//! Tesseract remains the engine on Linux and the fallback for languages the native engine does
//! not read (Nepali, for one).

use crate::ocr::Word;
use anyhow::Result;

#[cfg(target_os = "macos")]
mod vision;
#[cfg(windows)]
mod windows_ocr;

/// Which engine reads the text (`[text_extractor] engine`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Engine {
    /// The native engine when it reads the language, else Tesseract.
    #[default]
    Auto,
    /// Only the native engine (an error where there is none, or for an unsupported language).
    Native,
    /// Only Tesseract.
    Tesseract,
}

impl Engine {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Native, Self::Tesseract];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Native => "native",
            Self::Tesseract => "tesseract",
        }
    }
}

impl std::str::FromStr for Engine {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|engine| engine.as_str().eq_ignore_ascii_case(value.trim()))
            .ok_or_else(|| format!("unknown OCR engine `{value}` (auto, native or tesseract)"))
    }
}

/// The name of this platform's native engine, if it has one.
pub const fn native_name() -> Option<&'static str> {
    if cfg!(windows) {
        Some("Windows OCR")
    } else if cfg!(target_os = "macos") {
        Some("Apple Vision")
    } else {
        None
    }
}

/// A Tesseract language code as the BCP-47 tag the native engines use (`eng` is `en`).
pub fn native_language(code: &str) -> Option<&'static str> {
    Some(match code {
        "eng" => "en",
        "deu" => "de",
        "fra" => "fr",
        "spa" => "es",
        "ita" => "it",
        "por" => "pt",
        "nld" => "nl",
        "pol" => "pl",
        "rus" => "ru",
        "ukr" => "uk",
        "tur" => "tr",
        "swe" => "sv",
        "dan" => "da",
        "nor" => "nb",
        "fin" => "fi",
        "ces" => "cs",
        "ell" => "el",
        "hun" => "hu",
        "ron" => "ro",
        "ara" => "ar",
        "hin" => "hi",
        "jpn" => "ja",
        "kor" => "ko",
        "chi_sim" => "zh-Hans",
        "chi_tra" => "zh-Hant",
        "vie" => "vi",
        "tha" => "th",
        "ind" => "id",
        _ => return None,
    })
}

/// The tags to try, in order: the requested languages the native engines know (English when
/// none were requested). Empty means only Tesseract can read what was asked for.
pub fn native_tags(languages: &[String]) -> Vec<&'static str> {
    let codes: Vec<&str> = languages
        .iter()
        .flat_map(|entry| entry.split(['+', ',', ' ']))
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .collect();

    if codes.is_empty() {
        return vec!["en"];
    }

    // All or nothing: half the requested languages would quietly misread the rest.
    let tags: Option<Vec<&'static str>> = codes.iter().map(|code| native_language(code)).collect();

    tags.unwrap_or_default()
}

/// Recognise the words in `picture` with the native engine. `Ok(None)` when this platform has
/// none or it cannot read the requested languages. The words' boxes are in `picture` pixels;
/// the second value names the language tags used.
#[allow(unused_variables)]
pub fn recognize(
    picture: &image::RgbaImage,
    languages: &[String],
) -> Result<Option<(Vec<Word>, Vec<String>)>> {
    let tags = native_tags(languages);

    if tags.is_empty() {
        return Ok(None);
    }

    #[cfg(windows)]
    return windows_ocr::recognize(picture, &tags);

    #[cfg(target_os = "macos")]
    return vision::recognize(picture, &tags);

    #[cfg(not(any(windows, target_os = "macos")))]
    Ok(None)
}

/// Words back into text: one line per line, a blank line between blocks.
pub fn words_to_text(words: &[Word]) -> String {
    let mut text = String::new();
    let mut previous: Option<(u32, u32, u32)> = None;

    for word in words {
        match previous {
            None => {}
            Some(line) if line == word.line => text.push(' '),
            Some(line) if line.0 != word.line.0 => text.push_str("\n\n"),
            Some(_) => text.push('\n'),
        }

        text.push_str(&word.text);
        previous = Some(word.line);
    }

    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, block: u32, line: u32) -> Word {
        Word {
            left: 0,
            top: 0,
            width: 1,
            height: 1,
            line: (block, 0, line),
            text: text.into(),
        }
    }

    #[test]
    fn engines_parse_from_settings() {
        assert_eq!("Native".parse::<Engine>().unwrap(), Engine::Native);
        assert_eq!(" auto ".parse::<Engine>().unwrap(), Engine::Auto);
        assert!("paddle".parse::<Engine>().is_err());
    }

    #[test]
    fn languages_map_all_or_nothing() {
        assert_eq!(native_tags(&[]), ["en"]);
        assert_eq!(native_tags(&["eng+deu".into()]), ["en", "de"]);
        assert_eq!(native_tags(&["chi_sim".into()]), ["zh-Hans"]);
        // Nepali is Tesseract's: the whole request goes there.
        assert!(native_tags(&["eng+nep".into()]).is_empty());
    }

    #[test]
    fn words_become_lines_and_paragraphs() {
        let words = [
            word("Hello", 0, 0),
            word("world", 0, 0),
            word("second", 0, 1),
            word("Next", 1, 2),
            word("block", 1, 2),
        ];

        assert_eq!(words_to_text(&words), "Hello world\nsecond\n\nNext block");
        assert_eq!(words_to_text(&[]), "");
    }
}
