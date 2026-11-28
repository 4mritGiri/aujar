//! Text Extractor through the daemon: a real socket and request, with stand-ins for the
//! desktop's area selection (`slurp` and `grim`, as on Sway) and for Tesseract. Nothing touches
//! the real screen. One test per binary: it changes the process environment.
#![cfg(unix)]

use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};

use aujar_ipc::{ExtractTextRequest, Request, Response, send};

fn script(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn extracts_text_and_reports_a_cancelled_selection() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let fixture = dir.path().join("area.png");
    let cancel = dir.path().join("cancel");
    std::fs::create_dir(&bin).unwrap();
    image::DynamicImage::new_rgb8(40, 12)
        .save(&fixture)
        .unwrap();

    // slurp: an area, or Esc (exit 1) when the `cancel` file exists.
    script(
        &bin,
        "slurp",
        &format!("[ -e '{}' ] && exit 1\necho '0,0 40x12'", cancel.display()),
    );
    // grim -g <geometry> <file>: "captures" the fixture.
    script(
        &bin,
        "grim",
        &format!(
            "[ \"$1\" = -g ] && [ \"$2\" = '0,0 40x12' ] || exit 9\ncp '{}' \"$3\"",
            fixture.display()
        ),
    );
    script(
        &bin,
        "tesseract",
        "if [ \"$1\" = --list-langs ]; then printf 'List (2):\\neng\\nnep\\n'; exit 0; fi\n\
         cat >/dev/null\nprintf 'Invoice  42\\n\\n\\nTotal: 1,200\\n\\f'",
    );

    let runtime = dir.path().join("run");
    std::fs::create_dir(&runtime).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // SAFETY: the only test in this binary, set before the daemon starts.
    unsafe {
        std::env::set_var("PATH", path);
        std::env::set_var("AUJAR_TESSERACT_BIN", bin.join("tesseract"));
        std::env::set_var("WAYLAND_DISPLAY", "wayland-test");
        std::env::set_var("SWAYSOCK", dir.path().join("sway.sock"));
        std::env::set_var("XDG_RUNTIME_DIR", &runtime);
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    }

    let socket = dir.path().join("aujar.sock");
    let server = tokio::spawn({
        let socket = socket.clone();

        async move { aujar_daemon::run_with(socket, aujar_daemon::Options { hotkeys: false }).await }
    });

    for _ in 0..100 {
        if socket.exists() {
            break;
        }

        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let Response::Modules(modules) = send(&socket, Request::Modules).await.unwrap() else {
        panic!("expected modules");
    };
    assert!(modules.modules.iter().any(|m| m.id == "text_extractor"));

    let request = |single_line| {
        Request::ExtractText(ExtractTextRequest {
            languages: Vec::new(),
            single_line,
        })
    };

    match send(&socket, request(false)).await.unwrap() {
        Response::Text(text) => {
            assert!(!text.cancelled);
            assert_eq!(text.languages, ["eng"]);
            assert_eq!(text.text, "Invoice  42\n\nTotal: 1,200");
        }
        other => panic!("unexpected {other:?}"),
    }

    match send(&socket, request(true)).await.unwrap() {
        Response::Text(text) => assert_eq!(text.text, "Invoice 42 Total: 1,200"),
        other => panic!("unexpected {other:?}"),
    }

    // The capture is deleted once read.
    let leftovers: Vec<_> = std::fs::read_dir(&runtime).unwrap().flatten().collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // Esc in the selection: a cancel, not an error.
    std::fs::write(&cancel, b"").unwrap();
    match send(&socket, request(false)).await.unwrap() {
        Response::Text(text) => {
            assert!(text.cancelled);
            assert!(text.text.is_empty());
        }
        other => panic!("unexpected {other:?}"),
    }

    // A language that is not installed is refused before anything is captured.
    let german = Request::ExtractText(ExtractTextRequest {
        languages: vec!["deu".into()],
        single_line: false,
    });
    match send(&socket, german).await.unwrap() {
        Response::Error(message) => assert!(message.contains("tesseract-ocr-deu"), "{message}"),
        other => panic!("unexpected {other:?}"),
    }

    server.abort();
}
