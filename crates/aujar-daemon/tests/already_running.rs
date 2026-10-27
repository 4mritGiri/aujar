//! A second daemon must not take the socket of a running one.

use std::time::Duration;

use aujar_daemon::{AlreadyRunning, Options};
use aujar_ipc::{Request, Response, send};

#[tokio::test]
async fn a_second_daemon_refuses_to_replace_a_live_one() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("aujar.sock");

    let first = tokio::spawn({
        let socket = socket.clone();

        async move { aujar_daemon::run_with(socket, Options { hotkeys: false }).await }
    });

    for _ in 0..100 {
        if socket.exists() {
            break;
        }

        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let error = aujar_daemon::run_with(&socket, Options { hotkeys: false })
        .await
        .expect_err("the second daemon must not start");

    assert!(error.downcast_ref::<AlreadyRunning>().is_some(), "{error:#}");

    // The first daemon still owns the socket and still answers.
    assert!(matches!(
        send(&socket, Request::Ping).await.unwrap(),
        Response::Pong
    ));

    first.abort();
}

#[tokio::test]
async fn a_stale_socket_file_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("aujar.sock");

    // A leftover from a crash: the path exists but nobody listens.
    std::fs::write(&socket, b"").unwrap();

    let server = tokio::spawn({
        let socket = socket.clone();

        async move { aujar_daemon::run_with(socket, Options { hotkeys: false }).await }
    });

    let mut answered = false;

    for _ in 0..100 {
        if let Ok(Response::Pong) = send(&socket, Request::Ping).await {
            answered = true;
            break;
        }

        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert!(answered, "the daemon never replaced the stale socket");

    server.abort();
}
