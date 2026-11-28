//! `CaptureScreen`, shared by the Unix and Windows daemons: a picture of every monitor, where the
//! monitors are in it, and the visible windows at that moment.

use aujar_ipc::{Response, ScreenMonitor, ScreenWindow};

pub(crate) async fn capture() -> Response {
    let backend = aujar_config::Settings::load_or_default()
        .text_extractor
        .backend;

    let backend = match backend.parse::<aujar_screen::Backend>() {
        Ok(backend) => backend,
        Err(error) => return Response::Error(format!("text_extractor.backend: {error}")),
    };

    // The windows are listed right after the capture, so their places match the picture.
    let captured = tokio::task::spawn_blocking(move || {
        aujar_screen::capture::capture_screen_with_monitors(backend)
            .map(|(path, monitors)| (path, monitors, aujar_screen::windows::visible_windows()))
    })
    .await;

    match captured {
        Ok(Ok((path, monitors, windows))) => Response::Screen {
            path: path.to_string_lossy().into_owned(),
            windows: windows
                .into_iter()
                .map(|window| ScreenWindow {
                    title: window.title,
                    app: window.app,
                    x: window.x,
                    y: window.y,
                    width: window.width,
                    height: window.height,
                })
                .collect(),
            monitors: monitors
                .into_iter()
                .map(|m| ScreenMonitor {
                    x: m.x,
                    y: m.y,
                    width: m.width,
                    height: m.height,
                    picture_x: m.picture_x,
                    picture_y: m.picture_y,
                    picture_width: m.picture_width,
                    picture_height: m.picture_height,
                })
                .collect(),
        },
        Ok(Err(error)) => Response::Error(format!("{error:#}")),
        Err(error) => Response::Error(format!("the screen capture task failed: {error}")),
    }
}
