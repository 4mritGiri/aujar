//! Running Tesseract.
//!
//! Tesseract is an external program (`tesseract`, packaged by every distribution), like `curl`
//! for the online lookups. The picture goes in on standard input and the text comes back on
//! standard output, so nothing extra is written to disk. `AUJAR_TESSERACT_BIN` points at another
//! executable (tests, or a build outside `PATH`).

use anyhow::{Context, Result, bail};
use image::{DynamicImage, GenericImageView, imageops::FilterType};
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// A stuck Tesseract must not hold the request forever.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Upscaled pictures stay below this many pixels (Tesseract's time grows with the area).
const MAX_PIXELS: u64 = 24_000_000;

/// Language used when none is configured, if installed.
pub const DEFAULT_LANGUAGE: &str = "eng";

/// The Tesseract executable to run.
pub fn program() -> PathBuf {
    std::env::var_os("AUJAR_TESSERACT_BIN")
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from("tesseract"), PathBuf::from)
}

const INSTALL_HINT: &str = "install Tesseract (Debian/Ubuntu: `sudo apt install tesseract-ocr`, \
     Fedora: `sudo dnf install tesseract`, Arch: `sudo pacman -S tesseract tesseract-data-eng`)";

/// What `aujar doctor` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// `tesseract 5.3.4`.
    pub version: String,
    pub languages: Vec<String>,
}

fn run_simple(args: &[&str]) -> Result<String> {
    let output = Command::new(program())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => {
                anyhow::anyhow!("Tesseract is not installed: {INSTALL_HINT}")
            }
            _ => anyhow::anyhow!("could not run Tesseract: {error}"),
        })?;

    if !output.status.success() {
        bail!(
            "`tesseract {}` failed: {}",
            args.join(" "),
            last_line(&output.stderr)
        );
    }

    // Older releases print `--version` and `--list-langs` on standard error.
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&output.stderr));

    Ok(text)
}

/// Whether Tesseract runs, its version and its installed languages.
pub fn status() -> Result<Status> {
    let version = run_simple(&["--version"])?
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("tesseract"))
        .unwrap_or("tesseract")
        .to_owned();

    Ok(Status {
        version,
        languages: languages()?,
    })
}

/// The installed recognition languages (`eng`, `nep`, ...), without the `osd` helper data.
pub fn languages() -> Result<Vec<String>> {
    Ok(parse_languages(&run_simple(&["--list-langs"])?))
}

/// Parse `tesseract --list-langs`: a header line, then one code per line.
pub fn parse_languages(output: &str) -> Vec<String> {
    let mut languages: Vec<String> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.contains(' ') && !line.ends_with(':'))
        .filter(|line| valid_code(line) && *line != "osd" && *line != "equ")
        .map(str::to_owned)
        .collect();

    languages.sort();
    languages.dedup();

    languages
}

/// A Tesseract language code: `eng`, `chi_sim`, `script/Devanagari`.
fn valid_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 40
        && !code.starts_with('/')
        && !code.contains("..")
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '/'))
}

/// The languages to recognise: the requested ones (`eng+nep` or separate entries), each of which
/// must be installed. Nothing requested means English, or the only installed language.
pub fn resolve_languages(requested: &[String], installed: &[String]) -> Result<Vec<String>> {
    let mut wanted: Vec<String> = requested
        .iter()
        .flat_map(|entry| entry.split(['+', ',', ' ']))
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .map(str::to_owned)
        .collect();

    wanted.dedup();

    if installed.is_empty() {
        bail!(
            "Tesseract has no languages installed: install one (Debian/Ubuntu: \
             `sudo apt install tesseract-ocr-eng`)"
        );
    }

    if wanted.is_empty() {
        let fallback = if installed.iter().any(|code| code == DEFAULT_LANGUAGE) {
            DEFAULT_LANGUAGE.to_owned()
        } else {
            installed[0].clone()
        };

        return Ok(vec![fallback]);
    }

    for code in &wanted {
        if !valid_code(code) {
            bail!("`{code}` is not a Tesseract language code (try `eng`, `nep` or `deu`)");
        }

        if !installed.contains(code) {
            bail!(
                "the Tesseract language `{code}` is not installed (installed: {}). Debian/Ubuntu: \
                 `sudo apt install tesseract-ocr-{}`",
                installed.join(", "),
                code.replace('_', "-").to_lowercase()
            );
        }
    }

    Ok(wanted)
}

/// How much to enlarge a picture before recognition. Screen text is small (about 96 dpi) and
/// Tesseract reads it much better at two or three times the size.
pub fn scale_factor(width: u32, height: u32) -> u32 {
    let fits = |factor: u64| u64::from(width) * u64::from(height) * factor * factor <= MAX_PIXELS;

    // A single line of text: there is room to go bigger.
    if height <= 80 && fits(3) {
        3
    } else if fits(2) {
        2
    } else {
        1
    }
}

/// Grayscale, enlarged PNG bytes ready for Tesseract.
pub fn prepare(image: &DynamicImage) -> Result<Vec<u8>> {
    prepare_scaled(image).map(|(png, _)| png)
}

/// Like [`prepare`], with the factor the picture was enlarged by (word boxes come back in the
/// enlarged picture's pixels).
pub fn prepare_scaled(image: &DynamicImage) -> Result<(Vec<u8>, u32)> {
    let (width, height) = image.dimensions();

    if width == 0 || height == 0 {
        bail!("the selected area is empty");
    }

    let factor = scale_factor(width, height);
    let gray = image.grayscale();
    let prepared = if factor > 1 {
        gray.resize_exact(width * factor, height * factor, FilterType::CatmullRom)
    } else {
        gray
    };

    let mut png = Vec::new();
    prepared
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .context("could not encode the picture for Tesseract")?;

    Ok((png, factor))
}

fn last_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("no details")
        .to_owned()
}

/// Recognise the text in `png` (Tesseract page segmentation mode `psm`).
pub fn recognize(png: &[u8], languages: &[String], psm: u8) -> Result<String> {
    run_tesseract(png, languages, psm, None)
}

/// One recognised word and where it is, in the recognised picture's pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
    /// Block, paragraph and line number: words with the same `line` are on one line.
    pub line: (u32, u32, u32),
    pub text: String,
}

/// Recognise the words in `png`, with their boxes (Tesseract's TSV output).
pub fn recognize_words(png: &[u8], languages: &[String], psm: u8) -> Result<Vec<Word>> {
    run_tesseract(png, languages, psm, Some("tsv")).map(|tsv| parse_tsv(&tsv))
}

/// Parse Tesseract's TSV: `level page block par line word left top width height conf text`,
/// keeping the words (level 5) that have text.
pub fn parse_tsv(tsv: &str) -> Vec<Word> {
    tsv.lines()
        .skip(1)
        .filter_map(|row| {
            let columns: Vec<&str> = row.splitn(12, '\t').collect();

            if columns.len() < 12 || columns[0] != "5" {
                return None;
            }

            let n = |i: usize| columns[i].trim().parse::<u32>().ok();
            let text = columns[11].trim();

            (!text.is_empty()).then(|| Word {
                left: n(6).unwrap_or(0),
                top: n(7).unwrap_or(0),
                width: n(8).unwrap_or(0),
                height: n(9).unwrap_or(0),
                line: (n(2).unwrap_or(0), n(3).unwrap_or(0), n(4).unwrap_or(0)),
                text: text.to_owned(),
            })
        })
        .collect()
}

fn run_tesseract(
    png: &[u8],
    languages: &[String],
    psm: u8,
    config: Option<&str>,
) -> Result<String> {
    let languages = languages.join("+");
    let psm = psm.to_string();
    let mut args = vec!["stdin", "stdout", "-l", &languages, "--psm", &psm];

    args.extend(config);

    let mut child = Command::new(program())
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => {
                anyhow::anyhow!("Tesseract is not installed: {INSTALL_HINT}")
            }
            _ => anyhow::anyhow!("could not run Tesseract: {error}"),
        })?;

    // Feed and drain on their own threads: a full pipe must not deadlock either side.
    let mut stdin = child.stdin.take().context("Tesseract has no input pipe")?;
    let input = png.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));

    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();

            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }

            bytes
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().context("could not wait for Tesseract")? {
            break status;
        }

        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "Tesseract took longer than {} seconds and was stopped",
                TIMEOUT.as_secs()
            );
        }

        std::thread::sleep(Duration::from_millis(20));
    };

    // Tesseract may exit before reading everything (bad input); that shows up in its status.
    let _ = writer.join();
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();

    if !status.success() {
        bail!(
            "Tesseract could not read the picture: {}",
            last_line(&stderr)
        );
    }

    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_word_boxes_from_tsv() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   4\t1\t1\t1\t1\t0\t10\t20\t300\t30\t-1\t\n\
                   5\t1\t1\t1\t1\t1\t10\t20\t90\t30\t96.1\tSECRET:\n\
                   5\t1\t1\t1\t1\t2\t110\t20\t200\t30\t91.0\tsk_live_abc\n\
                   5\t1\t1\t1\t2\t1\t10\t60\t50\t30\t95.0\t \n";

        let words = parse_tsv(tsv);

        assert_eq!(words.len(), 2);
        assert_eq!(words[1].text, "sk_live_abc");
        assert_eq!(
            (words[1].left, words[1].top, words[1].width),
            (110, 20, 200)
        );
        assert_eq!(words[0].line, words[1].line);
    }

    #[test]
    fn parses_list_langs_output() {
        let output = "List of available languages in \"/usr/share/tesseract-ocr/5/tessdata/\" (4):\n\
                      eng\nnep\nosd\nchi_sim\n";

        assert_eq!(parse_languages(output), ["chi_sim", "eng", "nep"]);
    }

    #[test]
    fn default_language_is_english_or_the_only_one() {
        let installed = vec!["eng".to_owned(), "nep".to_owned()];

        assert_eq!(resolve_languages(&[], &installed).unwrap(), ["eng"]);
        assert_eq!(
            resolve_languages(&[], &["deu".to_owned()]).unwrap(),
            ["deu"]
        );
        assert!(resolve_languages(&[], &[]).is_err());
    }

    #[test]
    fn requested_languages_must_be_installed_and_well_formed() {
        let installed = vec!["eng".to_owned(), "nep".to_owned(), "chi_sim".to_owned()];

        assert_eq!(
            resolve_languages(&["eng+nep".to_owned()], &installed).unwrap(),
            ["eng", "nep"]
        );
        assert_eq!(
            resolve_languages(&["nep".to_owned(), "eng".to_owned()], &installed).unwrap(),
            ["nep", "eng"]
        );

        let missing = resolve_languages(&["deu".to_owned()], &installed)
            .unwrap_err()
            .to_string();
        assert!(missing.contains("tesseract-ocr-deu"), "{missing}");

        let hint = resolve_languages(&["chi_tra".to_owned()], &installed)
            .unwrap_err()
            .to_string();
        assert!(hint.contains("tesseract-ocr-chi-tra"), "{hint}");

        for bad in ["../etc", "/abs", "eng;rm", "-l"] {
            assert!(
                resolve_languages(&[bad.to_owned()], &installed).is_err(),
                "{bad} accepted"
            );
        }
    }

    #[test]
    fn small_pictures_are_enlarged_big_ones_are_not() {
        assert_eq!(scale_factor(400, 40), 3);
        assert_eq!(scale_factor(800, 600), 2);
        assert_eq!(scale_factor(3840, 2160), 1);
    }

    #[test]
    fn prepare_makes_a_bigger_gray_png() {
        let picture = DynamicImage::new_rgb8(10, 20);
        let png = prepare(&picture).unwrap();
        let decoded = image::load_from_memory(&png).unwrap();

        assert_eq!(decoded.dimensions(), (30, 60));
        assert!(matches!(decoded.color(), image::ColorType::L8));
        assert!(prepare(&DynamicImage::new_rgb8(0, 5)).is_err());
    }
}
