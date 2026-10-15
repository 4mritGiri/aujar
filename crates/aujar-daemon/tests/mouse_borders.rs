//! Mouse Borders through the daemon: IPC socket, supervisor and engine together. Runs with
//! the real Linux backends, which degrade gracefully where devices are not accessible.

use std::{path::PathBuf, time::Duration};

use aujar_ipc::{
    MouseBordersRequest, MouseBordersResponse, MouseBordersStatus, Request, Response, send,
};

async fn ask(socket: &PathBuf, request: MouseBordersRequest) -> Response {
    send(socket, Request::MouseBorders(request)).await.unwrap()
}

async fn status(socket: &PathBuf) -> MouseBordersStatus {
    match ask(socket, MouseBordersRequest::Status).await {
        Response::MouseBorders(MouseBordersResponse::Status(status)) => *status,
        other => panic!("unexpected {other:?}"),
    }
}

async fn wait_for_state(socket: &PathBuf, wanted: &str) -> MouseBordersStatus {
    for _ in 0..200 {
        let current = status(socket).await;

        if current.state == wanted {
            return current;
        }

        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    panic!(
        "never reached `{wanted}`; now `{}`",
        status(socket).await.state
    );
}

#[tokio::test]
async fn the_service_is_controlled_over_ipc() {
    let dir = tempfile::tempdir().unwrap();
    let config_home = dir.path().join("config");
    std::fs::create_dir_all(config_home.join("aujar")).unwrap();

    // This test binary has a single test, so changing the environment cannot race.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &config_home) };

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

    // Off by default, and it says how to turn it on.
    let off = status(&socket).await;
    assert_eq!(off.state, "disabled");
    assert!(off.detail.contains("enabled = true"));
    assert!(matches!(
        ask(&socket, MouseBordersRequest::ShowKey).await,
        Response::MouseBorders(MouseBordersResponse::NoKey)
    ));

    // Commands that need a running service explain themselves.
    let Response::Error(message) = ask(&socket, MouseBordersRequest::LockAll).await else {
        panic!("expected an error");
    };
    assert!(message.contains("not running"), "{message}");

    // Enable it with a two-machine layout.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    std::fs::write(
        config_home.join("aujar/settings.toml"),
        format!(
            r#"
            [mouse_borders]
            enabled = true
            machine_name = "desk"
            port = {port}
            receive_dir = "{}"

            [[mouse_borders.machines]]
            name = "desk"
            slot = 0

            [[mouse_borders.machines]]
            name = "laptop"
            slot = 1
            "#,
            dir.path().join("received").display()
        ),
    )
    .unwrap();

    let waiting = {
        // Settings are picked up by polling or by an explicit reload.
        ask(&socket, MouseBordersRequest::Reload).await;

        wait_for_state(&socket, "waiting_for_key").await
    };
    assert!(waiting.detail.contains("aujar mwb key generate"));

    // Generating the key starts the engine.
    let Response::MouseBorders(MouseBordersResponse::Key(key)) =
        ask(&socket, MouseBordersRequest::GenerateKey { force: false }).await
    else {
        panic!("expected a key");
    };
    assert_eq!(key.len(), 39, "{key}");

    let running = wait_for_state(&socket, "running").await;
    assert_eq!(running.local, "desk");
    assert_eq!(running.peers.len(), 1);
    assert!(!running.peers[0].connected);

    // The key shows back, and cannot be replaced by accident.
    assert!(matches!(
        ask(&socket, MouseBordersRequest::ShowKey).await,
        Response::MouseBorders(MouseBordersResponse::Key(shown)) if shown == key
    ));
    let Response::Error(message) =
        ask(&socket, MouseBordersRequest::GenerateKey { force: false }).await
    else {
        panic!("expected a refusal");
    };
    assert!(message.contains("--force"), "{message}");

    // Switching to a machine that is not connected is an error, not a hang.
    let Response::Error(message) = ask(
        &socket,
        MouseBordersRequest::Switch {
            machine: Some("laptop".into()),
        },
    )
    .await
    else {
        panic!("expected an error");
    };
    assert!(message.contains("laptop"), "{message}");

    // Bad input is rejected before it reaches the engine.
    assert!(matches!(
        ask(
            &socket,
            MouseBordersRequest::SendFiles {
                machine: "laptop".into(),
                paths: vec!["relative/path".into()],
            }
        )
        .await,
        Response::Error(message) if message.contains("absolute")
    ));
    assert!(matches!(
        ask(
            &socket,
            MouseBordersRequest::SetKey {
                key: "nope".into(),
                force: true
            }
        )
        .await,
        Response::Error(_)
    ));

    // The module is registered with the runtime.
    let Response::Modules(modules) = send(&socket, Request::Modules).await.unwrap() else {
        panic!("expected modules");
    };
    assert!(modules.modules.iter().any(|m| m.id == "mouse-borders"));

    server.abort();
}
