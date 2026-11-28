//! The whole file path (picture → Tesseract → tidy) against a stand-in `tesseract` script.
//! One test per binary: it sets `AUJAR_TESSERACT_BIN` for the process.
#![cfg(unix)]

use aujar_text_extractor::{Options, extract_from_file};
use std::os::unix::fs::PermissionsExt;

/// Answers `--list-langs`, checks it is called as `stdin stdout -l <langs> --psm <n>` with a PNG
/// on standard input, and prints text. Page segmentation 3 "finds nothing", so the retry with 6
/// is what produces the text.
const FAKE: &str = r#"#!/bin/sh
if [ "$1" = "--list-langs" ]; then
  printf 'List of available languages in "/fake" (3):\neng\nnep\nosd\n'
  exit 0
fi
[ "$1" = stdin ] && [ "$2" = stdout ] && [ "$3" = -l ] && [ "$5" = --psm ] || { echo "bad args: $*" >&2; exit 2; }
magic=$(head -c 4 | od -An -c | tr -d ' ')
[ "$magic" = "211PNG" ] || { echo "not a png: $magic" >&2; exit 3; }
cat >/dev/null
if [ "$6" = 3 ]; then printf '\n\f'; exit 0; fi
printf 'Hello exam-\nple   \n\n\n%s\n\f' "$4"
"#;

#[test]
fn extracts_text_from_a_picture_file() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("tesseract");
    std::fs::write(&fake, FAKE).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    // SAFETY: the only test in this binary; nothing else reads the environment concurrently.
    unsafe { std::env::set_var("AUJAR_TESSERACT_BIN", &fake) };

    let picture = dir.path().join("shot.png");
    image::DynamicImage::new_rgb8(40, 12)
        .save(&picture)
        .unwrap();

    let multi = extract_from_file(
        &picture,
        &Options {
            languages: vec!["eng+nep".into()],
            ..Options::default()
        },
    )
    .unwrap();

    assert_eq!(multi.languages, ["eng", "nep"]);
    assert_eq!(multi.text, "Hello exam-\nple\n\neng+nep");

    let joined = extract_from_file(
        &picture,
        &Options {
            single_line: true,
            ..Options::default()
        },
    )
    .unwrap();

    assert_eq!(joined.languages, ["eng"]);
    assert_eq!(joined.text, "Hello example eng");

    let missing = extract_from_file(
        &picture,
        &Options {
            languages: vec!["deu".into()],
            ..Options::default()
        },
    )
    .unwrap_err()
    .to_string();

    assert!(missing.contains("tesseract-ocr-deu"), "{missing}");
    assert!(extract_from_file(&dir.path().join("none.png"), &Options::default()).is_err());
}
