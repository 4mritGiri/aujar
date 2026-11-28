//! Cleaning up what Tesseract prints.

/// Tidy raw OCR output: Unix line ends, no form feed (Tesseract ends each page with one), no
/// trailing spaces, at most one blank line between paragraphs, nothing blank at either end.
///
/// With `single_line` the text becomes one line: words split by a hyphen at a line end are
/// joined (`exam-` / `ple` is `example`) and every other break becomes a space. Handy for
/// pasting a heading or a sentence that wrapped.
pub fn tidy(raw: &str, single_line: bool) -> String {
    let normalized = raw.replace("\r\n", "\n").replace(['\r', '\u{c}'], "\n");
    let lines: Vec<&str> = normalized.lines().map(str::trim_end).collect();

    if single_line {
        return join_lines(&lines);
    }

    let mut out = String::new();
    let mut blank_run = 0;

    for line in lines {
        if line.trim().is_empty() {
            blank_run += 1;
            continue;
        }

        if !out.is_empty() {
            out.push_str(if blank_run > 0 { "\n\n" } else { "\n" });
        }

        out.push_str(line);
        blank_run = 0;
    }

    out
}

fn join_lines(lines: &[&str]) -> String {
    let mut out = String::new();

    for line in lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
    {
        let hyphenated = out.ends_with('-')
            && out.chars().rev().nth(1).is_some_and(char::is_alphabetic)
            && line.chars().next().is_some_and(char::is_lowercase);

        if hyphenated {
            out.pop();
        } else if !out.is_empty() {
            out.push(' ');
        }

        out.push_str(line);
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_form_feeds_trailing_space_and_extra_blank_lines() {
        let raw = "  Title  \n\n\n\nFirst line   \nsecond line\r\n\n\u{c}";

        assert_eq!(tidy(raw, false), "  Title\n\nFirst line\nsecond line");
    }

    #[test]
    fn nothing_recognised_is_empty() {
        assert_eq!(tidy("\n \u{c}", false), "");
        assert_eq!(tidy("\n \u{c}", true), "");
    }

    #[test]
    fn single_line_joins_and_repairs_hyphenation() {
        let raw = "The exam-\nple wraps\n\nacross   lines.\nWell-\nKnown stays.";

        assert_eq!(
            tidy(raw, true),
            "The example wraps across lines. Well- Known stays."
        );
    }

    #[test]
    fn keeps_non_latin_text() {
        assert_eq!(tidy("नमस्ते  \nसंसार", true), "नमस्ते संसार");
    }
}
