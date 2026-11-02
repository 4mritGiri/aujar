//! Preview pictures for files that are not pictures themselves: the first page of a PDF, a frame
//! of a video, the first slide of a presentation.
//!
//! A picture is made once, into Aujar's cache (`<cache>/aujar/thumbs/<hash>.png`, see
//! `crate::thumb_prefix`), by whatever the computer already has:
//!
//! * PDFs: poppler's `pdftoppm`;
//! * videos: `ffmpegthumbnailer`, or else `ffmpeg`;
//! * everything else the desktop can thumbnail (documents, books, ...): the programs named in the
//!   standard `*.thumbnailer` files (`/usr/share/thumbnailers`, `~/.local/share/thumbnailers`),
//!   the same ones the file manager uses.
//!
//! Nothing here runs unless a preview or a grid tile asks for it, and the pictures are made one
//! at a time so a screenful of videos does not start a screenful of ffmpegs.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

/// Longest side of the pictures made here.
const SIZE: u32 = 560;

/// How long one program may take before it is stopped.
const TIMEOUT_SECS: u64 = 20;

/// Extensions of videos.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "mov", "avi", "mpg", "mpeg", "wmv", "flv", "3gp", "ogv", "mts",
    "m2ts",
];

/// Files that may get a picture, with their MIME type (what the `*.thumbnailer` files list).
const MIME_TYPES: &[(&str, &str)] = &[
    ("pdf", "application/pdf"),
    ("mp4", "video/mp4"),
    ("m4v", "video/mp4"),
    ("mkv", "video/x-matroska"),
    ("webm", "video/webm"),
    ("mov", "video/quicktime"),
    ("avi", "video/x-msvideo"),
    ("mpg", "video/mpeg"),
    ("mpeg", "video/mpeg"),
    ("wmv", "video/x-ms-wmv"),
    ("flv", "video/x-flv"),
    ("3gp", "video/3gpp"),
    ("ogv", "video/ogg"),
    ("mts", "video/mp2t"),
    ("m2ts", "video/mp2t"),
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    (
        "pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    ("odt", "application/vnd.oasis.opendocument.text"),
    ("ods", "application/vnd.oasis.opendocument.spreadsheet"),
    ("odp", "application/vnd.oasis.opendocument.presentation"),
    ("doc", "application/msword"),
    ("xls", "application/vnd.ms-excel"),
    ("ppt", "application/vnd.ms-powerpoint"),
    ("rtf", "application/rtf"),
    ("epub", "application/epub+zip"),
];

/// The MIME type of a file extension (lower case, no dot), if it may get a picture.
pub fn mime_for(extension: &str) -> Option<&'static str> {
    MIME_TYPES
        .iter()
        .find(|(ext, _)| *ext == extension)
        .map(|(_, mime)| *mime)
}

pub fn is_video(extension: &str) -> bool {
    VIDEO_EXTENSIONS.contains(&extension)
}

/// Whether files with this extension may get a picture at all (not whether this computer can
/// make it: see [`available`]).
pub fn wanted(extension: &str) -> bool {
    mime_for(extension).is_some()
}

/// The lower-case extension of a path.
pub fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

// ---- The programs that make pictures ----------------------------------------------------------

/// A `*.thumbnailer` file: a command for some MIME types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnailer {
    pub exec: String,
    pub try_exec: Option<String>,
    pub mimes: Vec<String>,
}

/// Read one `*.thumbnailer` file (`[Thumbnailer Entry]` with `Exec=` and `MimeType=`).
pub fn parse_thumbnailer(contents: &str) -> Option<Thumbnailer> {
    let mut in_entry = false;
    let mut exec = None;
    let mut try_exec = None;
    let mut mimes = Vec::new();

    for line in contents.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Thumbnailer Entry]";
            continue;
        }

        if !in_entry || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        match key.trim() {
            "Exec" => exec = Some(value.trim().to_owned()),
            "TryExec" => try_exec = Some(value.trim().to_owned()),
            "MimeType" => {
                mimes = value
                    .split(';')
                    .map(str::trim)
                    .filter(|mime| !mime.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            _ => {}
        }
    }

    let exec = exec.filter(|exec| !exec.is_empty())?;

    (!mimes.is_empty()).then_some(Thumbnailer {
        exec,
        try_exec,
        mimes,
    })
}

/// Every installed thumbnailer, read once.
fn thumbnailers() -> &'static [Thumbnailer] {
    static ALL: OnceLock<Vec<Thumbnailer>> = OnceLock::new();

    ALL.get_or_init(|| {
        let mut dirs: Vec<PathBuf> = Vec::new();

        match std::env::var_os("XDG_DATA_HOME").filter(|dir| !dir.is_empty()) {
            Some(home) => dirs.push(PathBuf::from(home).join("thumbnailers")),
            None => {
                if let Some(home) = std::env::var_os("HOME") {
                    dirs.push(PathBuf::from(home).join(".local/share/thumbnailers"));
                }
            }
        }

        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|dirs| !dirs.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());

        dirs.extend(
            data_dirs
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(|dir| PathBuf::from(dir).join("thumbnailers")),
        );

        let mut found = Vec::new();

        for dir in dirs {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };

            let mut files: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "thumbnailer"))
                .collect();

            files.sort();

            for file in files {
                if let Some(thumbnailer) = fs::read_to_string(&file)
                    .ok()
                    .and_then(|text| parse_thumbnailer(&text))
                {
                    found.push(thumbnailer);
                }
            }
        }

        found
    })
}

/// The first installed thumbnailer for a MIME type whose program is really there.
fn thumbnailer_for(mime: &str) -> Option<&'static Thumbnailer> {
    thumbnailers().iter().find(|thumbnailer| {
        thumbnailer.mimes.iter().any(|candidate| candidate == mime)
            && thumbnailer.try_exec.as_deref().is_none_or(program_exists)
            && split_args(&thumbnailer.exec)
                .first()
                .is_some_and(|program| program_exists(program))
    })
}

fn program_exists(program: &str) -> bool {
    if program.contains('/') {
        Path::new(program).is_file()
    } else {
        crate::on_path(program)
    }
}

/// Whether this computer can make a picture for files with this extension.
pub fn available(extension: &str) -> bool {
    let Some(mime) = mime_for(extension) else {
        return false;
    };

    if extension == "pdf" && crate::on_path("pdftoppm") {
        return true;
    }

    if is_video(extension) && (crate::on_path("ffmpegthumbnailer") || crate::on_path("ffmpeg")) {
        return true;
    }

    thumbnailer_for(mime).is_some()
}

// ---- Running them ------------------------------------------------------------------------------

/// Split a command line into words. Double quotes group words (with `\"` and `\\` inside), as
/// in the desktop entry `Exec=` format.
pub fn split_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quoted = false;
    let mut chars = line.chars();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }

    if started {
        args.push(current);
    }

    args
}

/// `file:///home/me/My%20Video.mp4` for a path.
pub fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");

    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                uri.push(byte as char);
            }
            other => uri.push_str(&format!("%{other:02X}")),
        }
    }

    uri
}

/// The words of a thumbnailer's `Exec=` for one file: `%i` is the file, `%u` its URI, `%o` the
/// picture to write, `%s` the size and `%%` a percent sign.
pub fn expand_exec(exec: &str, input: &Path, output: &Path, size: u32) -> Vec<String> {
    split_args(exec)
        .into_iter()
        .map(|word| {
            let mut out = String::new();
            let mut chars = word.chars();

            while let Some(c) = chars.next() {
                if c != '%' {
                    out.push(c);
                    continue;
                }

                match chars.next() {
                    Some('i') => out.push_str(&input.to_string_lossy()),
                    Some('u') => out.push_str(&file_uri(input)),
                    Some('o') => out.push_str(&output.to_string_lossy()),
                    Some('s') => out.push_str(&size.to_string()),
                    Some('%') => out.push('%'),
                    Some(other) => {
                        out.push('%');
                        out.push(other);
                    }
                    None => out.push('%'),
                }
            }

            out
        })
        .collect()
}

/// Run a command to its end, or stop it after `secs`. `true` when it exited successfully.
pub fn run_with_timeout(mut command: Command, secs: u64) -> bool {
    let Ok(mut child) = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };

    let deadline = Instant::now() + Duration::from_secs(secs);

    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(40)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();

                return false;
            }
        }
    }
}

/// A frame of a video, from `ffmpegthumbnailer` or else `ffmpeg`.
fn video_frame(path: &Path, out: &Path) -> bool {
    if crate::on_path("ffmpegthumbnailer") {
        let mut command = Command::new("ffmpegthumbnailer");

        command.arg("-i").arg(path).arg("-o").arg(out).args([
            "-s",
            &SIZE.to_string(),
            "-c",
            "png",
            "-t",
            "10%",
        ]);

        if run_with_timeout(command, TIMEOUT_SECS) && out.is_file() {
            return true;
        }
    }

    if crate::on_path("ffmpeg") {
        // Three seconds in, to skip a black intro; the very first frame for a short clip.
        for seek in ["3", "0"] {
            let mut command = Command::new("ffmpeg");

            command
                .args(["-v", "error", "-y", "-ss", seek, "-i"])
                .arg(path)
                .args(["-frames:v", "1", "-vf", &format!("scale={SIZE}:-2")])
                .arg(out);

            if run_with_timeout(command, TIMEOUT_SECS) && out.is_file() {
                return true;
            }
        }
    }

    false
}

/// A picture from an installed `*.thumbnailer` for this MIME type.
fn from_thumbnailer(path: &Path, mime: &str, out: &Path) -> bool {
    let Some(thumbnailer) = thumbnailer_for(mime) else {
        return false;
    };

    let mut words = expand_exec(&thumbnailer.exec, path, out, SIZE).into_iter();

    let Some(program) = words.next() else {
        return false;
    };

    let mut command = Command::new(program);

    command.args(words);

    run_with_timeout(command, TIMEOUT_SECS) && out.is_file()
}

/// Make the picture for `path` at `final_png` (a `thumb_prefix` path with `.png`). `true` when
/// it exists afterwards.
fn generate(path: &Path, final_png: &Path) -> bool {
    if final_png.is_file() {
        return true;
    }

    let extension = extension_of(path);

    let Some(mime) = mime_for(&extension) else {
        return false;
    };

    if extension == "pdf" && crate::on_path("pdftoppm") {
        return crate::generate_pdf_thumb(path);
    }

    if let Some(parent) = final_png.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // Written under another name and renamed, so a half-made picture is never shown.
    let part = final_png.with_extension("part.png");

    let _ = fs::remove_file(&part);

    let made =
        (is_video(&extension) && video_frame(path, &part)) || from_thumbnailer(path, mime, &part);

    if made && fs::rename(&part, final_png).is_ok() {
        return true;
    }

    let _ = fs::remove_file(&part);

    false
}

// ---- The queue -------------------------------------------------------------------------------

/// Pictures being made or already tried and failed this session, so a file is not tried again
/// every time its preview is drawn.
fn state() -> &'static Mutex<(HashSet<PathBuf>, HashSet<PathBuf>)> {
    static STATE: OnceLock<Mutex<(HashSet<PathBuf>, HashSet<PathBuf>)>> = OnceLock::new();

    STATE.get_or_init(|| Mutex::new((HashSet::new(), HashSet::new())))
}

/// Claim a file for making its picture. `false` when that is already under way or failed before.
pub fn begin(path: &Path) -> bool {
    let mut state = state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if state.1.contains(path) || state.0.contains(path) {
        return false;
    }

    state.0.insert(path.to_path_buf());

    true
}

/// The attempt is over; a failure is remembered for the rest of the session.
pub fn finish(path: &Path, made: bool) {
    let mut state = state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    state.0.remove(path);

    if !made {
        state.1.insert(path.to_path_buf());
    }
}

/// Make the picture for `path`, waiting for any other picture being made. Run on a blocking
/// thread. `true` when the picture exists afterwards.
pub fn make(path: &Path) -> bool {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    let final_png = crate::thumb_prefix(path).with_extension("png");

    if final_png.is_file() {
        return true;
    }

    let _turn = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    generate(path, &final_png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_to_mime_types() {
        assert_eq!(mime_for("pdf"), Some("application/pdf"));
        assert_eq!(mime_for("mp4"), Some("video/mp4"));
        assert_eq!(mime_for("mkv"), Some("video/x-matroska"));
        assert!(mime_for("docx").unwrap().contains("wordprocessingml"));
        assert_eq!(mime_for("rs"), None);
        // Pictures and text have their own previews.
        assert_eq!(mime_for("png"), None);
        assert_eq!(mime_for("txt"), None);

        // Every video extension has a MIME type, and nothing else is called a video.
        for extension in VIDEO_EXTENSIONS {
            assert!(is_video(extension));
            assert!(
                mime_for(extension).unwrap().starts_with("video/"),
                "{extension}"
            );
        }

        assert!(!is_video("pdf") && !is_video("docx"));
        assert!(wanted("mov") && !wanted("png"));
    }

    #[test]
    fn extensions_are_lower_cased() {
        assert_eq!(extension_of(Path::new("/a/Movie.MKV")), "mkv");
        assert_eq!(extension_of(Path::new("/a/noext")), "");
    }

    #[test]
    fn command_lines_split_like_desktop_entries() {
        assert_eq!(split_args("a b  c"), ["a", "b", "c"]);
        assert_eq!(
            split_args(r#"tool "two words" x"#),
            ["tool", "two words", "x"]
        );
        assert_eq!(
            split_args(r#"t "a \"quoted\" b""#),
            ["t", r#"a "quoted" b"#]
        );
        assert_eq!(split_args(r#"t "" x"#), ["t", "", "x"]);
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn uris_are_percent_encoded() {
        assert_eq!(
            file_uri(Path::new("/home/u/My Video #1.mp4")),
            "file:///home/u/My%20Video%20%231.mp4"
        );
        assert_eq!(file_uri(Path::new("/a/b-c_d.e~f")), "file:///a/b-c_d.e~f");
        assert_eq!(file_uri(Path::new("/é")), "file:///%C3%A9");
    }

    #[test]
    fn exec_lines_get_their_placeholders() {
        let input = Path::new("/v/a b.mp4");
        let output = Path::new("/c/out.png");

        assert_eq!(
            expand_exec(
                "/usr/bin/ffmpegthumbnailer -i %i -o %o -s %s -f",
                input,
                output,
                560
            ),
            [
                "/usr/bin/ffmpegthumbnailer",
                "-i",
                "/v/a b.mp4",
                "-o",
                "/c/out.png",
                "-s",
                "560",
                "-f"
            ]
        );
        assert_eq!(
            expand_exec("tool %u 100%% %x", input, output, 128),
            ["tool", "file:///v/a%20b.mp4", "100%", "%x"]
        );
        assert_eq!(
            expand_exec("tool --size=%s", input, output, 7),
            ["tool", "--size=7"]
        );
    }

    #[test]
    fn thumbnailer_files_are_read() {
        let text = "\
# a comment
[Other]
Exec=wrong
[Thumbnailer Entry]
TryExec=ffmpegthumbnailer
Exec=ffmpegthumbnailer -i %i -o %o -s %s -f
MimeType=video/mp4;video/webm;;
";

        assert_eq!(
            parse_thumbnailer(text),
            Some(Thumbnailer {
                exec: "ffmpegthumbnailer -i %i -o %o -s %s -f".into(),
                try_exec: Some("ffmpegthumbnailer".into()),
                mimes: vec!["video/mp4".into(), "video/webm".into()],
            })
        );

        // No command, or no MIME types: not a usable entry.
        assert_eq!(
            parse_thumbnailer("[Thumbnailer Entry]\nMimeType=a/b;"),
            None
        );
        assert_eq!(parse_thumbnailer("[Thumbnailer Entry]\nExec=x"), None);
        assert_eq!(parse_thumbnailer(""), None);
    }

    #[cfg(unix)]
    #[test]
    fn programs_are_run_and_stopped() {
        let mut ok = Command::new("sh");
        ok.args(["-c", "exit 0"]);
        assert!(run_with_timeout(ok, 5));

        let mut failing = Command::new("sh");
        failing.args(["-c", "exit 3"]);
        assert!(!run_with_timeout(failing, 5));

        assert!(!run_with_timeout(Command::new("/definitely/not/here"), 5));

        // A program that hangs is stopped at the deadline.
        let started = Instant::now();
        let mut slow = Command::new("sh");
        slow.args(["-c", "sleep 30"]);

        assert!(!run_with_timeout(slow, 1));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_file_is_claimed_once_and_failures_are_remembered() {
        let fresh = Path::new("/aujar-test/thumbs/fresh.mp4");
        let broken = Path::new("/aujar-test/thumbs/broken.mp4");

        assert!(begin(fresh));
        assert!(!begin(fresh), "already under way");

        finish(fresh, true);
        assert!(
            begin(fresh),
            "a made picture may be asked for again (it is cached)"
        );
        finish(fresh, true);

        assert!(begin(broken));
        finish(broken, false);
        assert!(!begin(broken), "a failure is not retried this session");
    }

    #[test]
    fn unknown_files_make_no_picture() {
        let dir = std::env::temp_dir().join(format!("aujar-thumbs-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("notes.rs");
        let _ = fs::write(&file, "fn main() {}");

        assert!(!available("rs"));
        assert!(!generate(&file, &dir.join("x.png")));

        let _ = fs::remove_dir_all(dir);
    }
}
