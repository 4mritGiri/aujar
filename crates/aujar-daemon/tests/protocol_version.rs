//! Protocol versioning over the real socket: the `Version` request, and clients that speak a
//! version this daemon does not.
#![cfg(unix)]

use std::time::Duration;

use aujar_ipc::{MIN_PROTOCOL_VERSION, PROTOCOL_VERSION, Request, Response, VersionResponse, send};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

/// Send one raw line and return the raw reply line.
async fn raw(socket: &std::path::Path, line: &str) -> String {
    let mut stream = UnixStream::connect(socket).await.unwrap();
    stream.write_all(line.as_bytes()).await.unwrap();

    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).await.unwrap();

    reply
}

#[tokio::test]
async fn version_request_and_mismatched_clients() {
    let dir = tempfile::tempdir().unwrap();
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

    match send(&socket, Request::Version).await.unwrap() {
        Response::Version(version) => {
            assert_eq!(version, VersionResponse::current());
            assert_eq!(version.protocol, PROTOCOL_VERSION);
            assert_eq!(version.min_protocol, MIN_PROTOCOL_VERSION);
        }
        other => panic!("unexpected {other:?}"),
    }

    // A client from the future gets a readable refusal, in its own version.
    let reply = raw(&socket, "{\"version\":42,\"payload\":\"Ping\"}\n").await;
    assert!(
        reply.starts_with("{\"version\":42,\"payload\":{\"Error\":"),
        "{reply}"
    );
    assert!(reply.contains("client uses v42"), "{reply}");

    // A known version with a request this daemon does not know.
    let reply = raw(
        &socket,
        &format!("{{\"version\":{PROTOCOL_VERSION},\"payload\":\"Teleport\"}}\n"),
    )
    .await;
    assert!(reply.contains("unrecognised request"), "{reply}");

    // Subscribing still works through the version-aware path.
    let _events = aujar_ipc::subscribe(&socket).await.unwrap();

    server.abort();
}
