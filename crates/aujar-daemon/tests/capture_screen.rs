//! `CaptureScreen` (the frozen picture behind Aujar Snip) through the daemon, with a stand-in
//! `grim` as on Sway. One test per binary: it changes the process environment.
#![cfg(target_os = "linux")]

use std::{os::unix::fs::PermissionsExt, time::Duration};

use aujar_ipc::{Request, Response, send};

#[test]
fn the_screenshot_permission_is_for_the_app_id_the_daemon_registers() {
    assert_eq!(
        aujar_text_extractor::permission::PORTAL_APP_ID,
        aujar_hotkeys::APP_ID
    );
}

#[tokio::test]
async fn captures_the_screen_into_a_private_file() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let runtime = dir.path().join("run");
    let fixture = dir.path().join("screen.png");
    std::fs::create_dir(&bin).unwrap();
    std::fs::create_dir(&runtime).unwrap();
    image::DynamicImage::new_rgb8(64, 32)
        .save(&fixture)
        .unwrap();

    // `grim <file>` (no -g: the whole screen) "captures" the fixture.
    let grim = bin.join("grim");
    std::fs::write(
        &grim,
        format!(
            "#!/bin/sh\n[ \"$#\" = 1 ] || exit 9\ncp '{}' \"$1\"\n",
            fixture.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&grim, std::fs::Permissions::from_mode(0o755)).unwrap();

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // SAFETY: the only test in this binary, set before the daemon starts.
    unsafe {
        std::env::set_var("PATH", path);
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

    let Response::Screen {
        path,
        windows,
        monitors,
    } = send(&socket, Request::CaptureScreen).await.unwrap()
    else {
        panic!("expected a screen picture");
    };
    let path = std::path::PathBuf::from(path);

    // A Sway session without `swaymsg`: no windows, not an error.
    assert!(windows.is_empty());
    // Linux tools do not say where the monitors are; the UI matches its own displays.
    assert!(monitors.is_empty());

    assert!(path.starts_with(&runtime), "{}", path.display());
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(image::open(&path).unwrap().width(), 64);

    // The client owns the file and deletes it.
    std::fs::remove_file(&path).unwrap();

    server.abort();
}
