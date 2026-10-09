use aujar_ipc::{Event, LauncherCommand, Request, Response, send, subscribe};
use std::{path::PathBuf, time::Duration};

fn temp_socket() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let dir = std::env::temp_dir().join(format!("aujar-events-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    dir.join("aujar.sock")
}

#[tokio::test]
async fn launcher_commands_reach_subscribers() {
    let socket = temp_socket();

    let server = tokio::spawn({
        let socket = socket.clone();
        async move { aujar_daemon::run(socket).await }
    });

    for _ in 0..100 {
        if socket.exists() {
            break;
        }

        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert!(socket.exists(), "daemon did not create its socket");

    let mut stream = subscribe(&socket).await.unwrap();

    let response = send(&socket, Request::Launcher(LauncherCommand::Toggle))
        .await
        .unwrap();

    assert!(matches!(response, Response::Delivered { subscribers: 1 }));

    let event = tokio::time::timeout(Duration::from_secs(2), stream.recv())
        .await
        .expect("timed out waiting for event")
        .unwrap();

    assert_eq!(event, Some(Event::Launcher(LauncherCommand::Toggle)));

    server.abort();
    let _ = std::fs::remove_dir_all(socket.parent().unwrap());
}

#[tokio::test]
async fn launcher_command_without_subscribers_reports_zero() {
    let socket = temp_socket();

    let server = tokio::spawn({
        let socket = socket.clone();
        async move { aujar_daemon::run(socket).await }
    });

    for _ in 0..100 {
        if socket.exists() {
            break;
        }

        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let response = send(&socket, Request::Launcher(LauncherCommand::Show))
        .await
        .unwrap();

    assert!(matches!(response, Response::Delivered { subscribers: 0 }));

    server.abort();
    let _ = std::fs::remove_dir_all(socket.parent().unwrap());
}
