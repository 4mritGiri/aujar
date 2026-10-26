//! NexDesk through the daemon: IPC socket, runtime registration and policy-free status.
//! The real NexDesk is never started: the executable override points nowhere.

use std::time::Duration;

use aujar_ipc::{NexDeskRequest, NexDeskResponse, Request, Response, send};

#[tokio::test]
async fn nexdesk_is_registered_and_reports_a_missing_install() {
    let dir = tempfile::tempdir().unwrap();

    // This test binary has a single test, so changing the environment cannot race.
    unsafe { std::env::set_var("AUJAR_NEXDESK_BIN", dir.path().join("no-such-nexdesk")) };

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
    assert!(modules.modules.iter().any(|m| m.id == "nexdesk"));

    match send(&socket, Request::NexDesk(NexDeskRequest::Status))
        .await
        .unwrap()
    {
        Response::NexDesk(NexDeskResponse::Status(status)) => {
            assert!(!status.installed);
            assert_eq!(status.running_pid, None);
        }
        other => panic!("unexpected {other:?}"),
    }

    match send(&socket, Request::NexDesk(NexDeskRequest::Open))
        .await
        .unwrap()
    {
        Response::Error(message) => assert!(message.contains("not installed"), "{message}"),
        other => panic!("unexpected {other:?}"),
    }

    server.abort();
}
