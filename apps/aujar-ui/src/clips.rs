//! Clipboard entries that are more than text: a copied picture, or files copied in the file
//! manager.
//!
//! * A picture is saved into Aujar's cache (`<cache>/aujar/clips/<hash>.<ext>`) so the history can
//!   show it as a thumbnail; the clipboard item itself stays in memory to copy it back. The saved
//!   files belong to this session only (the history is never written to disk): they are deleted
//!   when the UI starts, when an entry goes, and on "Clear history".
//! * Files copied in the file manager also put their paths on the clipboard as text. When every
//!   line of a text names something that exists, the entry is shown as those files.

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

/// The largest picture kept in the history.
pub const MAX_IMAGE_BYTES: usize = 12 * 1024 * 1024;

/// How many pictures the history keeps; older ones are forgotten first.
pub const MAX_IMAGES: usize = 8;

/// A copy of more files than this is shown as plain text.
const MAX_FILES: usize = 50;

/// Where the pictures are saved.
pub fn dir() -> PathBuf {
    crate::cache_dir().join("clips")
}

/// Delete every saved picture.
pub fn clear_saved() {
    let _ = fs::remove_dir_all(dir());
}

/// The file a picture is saved as: named after its content, so the same picture copied twice is
/// one file.
pub fn image_path(bytes: &[u8], extension: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();

    bytes.hash(&mut hasher);

    dir().join(format!("{:016x}.{extension}", hasher.finish()))
}

/// Save a picture at `path` (made by [`image_path`]). `true` when the file exists afterwards.
pub fn save_image(path: &Path, bytes: &[u8]) -> bool {
    if path.is_file() {
        return true;
    }

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // Written under another name and renamed, so a half-written picture is never shown.
    let part = path.with_extension("part");

    if fs::write(&part, bytes).is_ok() && fs::rename(&part, path).is_ok() {
        return true;
    }

    let _ = fs::remove_file(&part);

    false
}

/// `PNG` for `png`, and so on.
pub fn image_label(extension: &str) -> &'static str {
    match extension {
        "png" => "PNG",
        "jpg" | "jpeg" => "JPEG",
        "webp" => "WebP",
        "gif" => "GIF",
        "bmp" => "BMP",
        _ => "Image",
    }
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// `a b/c` for `a%20b%2Fc`. A `%` that is not followed by two hex digits is kept as it is.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        let pair = if bytes[index] == b'%' && index + 2 < bytes.len() {
            hex(bytes[index + 1]).zip(hex(bytes[index + 2]))
        } else {
            None
        };

        if let Some((high, low)) = pair {
            out.push(high * 16 + low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// The files a clipboard text names: every line is an absolute path or a `file://` address of
/// something that exists. Empty when the text is anything else.
pub fn clip_files(text: &str) -> Vec<PathBuf> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    if lines.is_empty() || lines.len() > MAX_FILES {
        return Vec::new();
    }

    let mut paths = Vec::with_capacity(lines.len());

    for line in lines {
        let path = match line.strip_prefix("file://") {
            Some(rest) => PathBuf::from(percent_decode(rest)),
            None if line.starts_with('/') => PathBuf::from(line),
            None => return Vec::new(),
        };

        if !path.is_absolute() || !path.exists() {
            return Vec::new();
        }

        paths.push(path);
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh temporary directory.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aujar-clips-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        fs::create_dir_all(&dir).unwrap();

        dir
    }

    #[test]
    fn percent_signs_are_decoded_where_they_make_sense() {
        assert_eq!(percent_decode("a%20b%2Fc"), "a b/c");
        assert_eq!(percent_decode("%C3%A9"), "\u{e9}");
        assert_eq!(percent_decode("plain"), "plain");
        // Not followed by two hex digits: kept.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%zzb"), "a%zzb");
        assert_eq!(percent_decode("%4"), "%4");
        assert_eq!(percent_decode(""), "");
    }

    #[test]
    fn copied_files_are_recognised_from_their_paths() {
        let dir = temp_dir("files");
        let plain = dir.join("report.pdf");
        let spaced = dir.join("My Book.epub");

        fs::write(&plain, b"x").unwrap();
        fs::write(&spaced, b"y").unwrap();

        // One path.
        assert_eq!(clip_files(&plain.to_string_lossy()), [plain.clone()]);

        // Several, one per line, as paths and as addresses (spaces encoded).
        let text = format!(
            "{}\nfile://{}\n",
            plain.display(),
            spaced.to_string_lossy().replace(' ', "%20")
        );

        assert_eq!(clip_files(&text), [plain.clone(), spaced.clone()]);

        // Anything that is not wholly existing files is plain text.
        assert!(clip_files("hello world").is_empty());
        assert!(clip_files(&format!("{}\nhello", plain.display())).is_empty());
        assert!(clip_files(&format!("{}\n/no/such/file", plain.display())).is_empty());
        assert!(clip_files("relative/path.txt").is_empty());
        assert!(clip_files("").is_empty());
        assert!(clip_files("   \n  ").is_empty());
        assert!(clip_files("file://remotehost/some/file").is_empty());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_very_long_list_is_not_treated_as_files() {
        let dir = temp_dir("many");
        let file = dir.join("a.txt");

        fs::write(&file, b"x").unwrap();

        let line = file.to_string_lossy().into_owned();
        let many = vec![line; MAX_FILES + 1].join("\n");

        assert!(clip_files(&many).is_empty());
        assert_eq!(
            clip_files(&vec![file.to_string_lossy().into_owned(); 3].join("\n")).len(),
            3
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn pictures_are_saved_once_under_a_name_from_their_content() {
        let first = image_path(b"picture one", "png");
        let same = image_path(b"picture one", "png");
        let other = image_path(b"picture two", "png");

        assert_eq!(first, same);
        assert_ne!(first, other);
        assert_eq!(first.extension().and_then(|e| e.to_str()), Some("png"));
        assert!(first.starts_with(dir()));

        // Saving goes to a path we choose: use a temporary one so no real cache is touched.
        let tmp = temp_dir("save");
        let path = tmp.join("a.png");

        assert!(save_image(&path, b"abc"));
        assert_eq!(fs::read(&path).unwrap(), b"abc");

        // Already there: kept, not rewritten.
        assert!(save_image(&path, b"different"));
        assert_eq!(fs::read(&path).unwrap(), b"abc");

        // No half-written file is left behind.
        assert!(!tmp.join("a.part").exists());

        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn picture_formats_have_labels() {
        assert_eq!(image_label("png"), "PNG");
        assert_eq!(image_label("jpg"), "JPEG");
        assert_eq!(image_label("webp"), "WebP");
        assert_eq!(image_label("gif"), "GIF");
        assert_eq!(image_label("bmp"), "BMP");
        assert_eq!(image_label("tiff"), "Image");
    }
}
