//! Awake end to end: a real daemon on a real socket, a real client, and a lock that only writes
//! down what it is asked (so nothing touches the desktop, and the saved mode lives in a scratch
//! directory, not in your home).

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use aujar_awake::{AwakeMode, AwakeModule, Inhibitor};
use aujar_daemon::Options;
use aujar_ipc::{AwakeRequest, AwakeResponse, Request, Response, send};

#[derive(Clone, Default)]
struct Lock {
    held: Arc<Mutex<Vec<String>>>,
}

impl Lock {
    fn log(&self) -> Vec<String> {
        self.held.lock().unwrap().clone()
    }
}

#[async_trait]
impl Inhibitor for Lock {
    async fn hold(&mut self, screen: bool) -> Result<(), String> {
        self.held
            .lock()
            .unwrap()
            .push(format!("hold screen={screen}"));

        Ok(())
    }

    async fn release(&mut self) {
        self.held.lock().unwrap().push("release".into());
    }
}

struct Daemon {
    socket: PathBuf,
    lock: Lock,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

/// Start a daemon whose Awake keeps its state in `dir` (so a second daemon on the same `dir`
/// is the same installation after a restart).
async fn start(dir: &Path, name: &str) -> Daemon {
    let socket = dir.join(format!("{name}.sock"));
    let lock = Lock::default();
    let awake = Arc::new(AwakeModule::with(
        Box::new(lock.clone()),
        Some(dir.join("state.toml")),
        dir.join("settings.toml"),
    ));
    let task = tokio::spawn({
        let socket = socket.clone();

        async move { aujar_daemon::run_with_awake(socket, Options { hotkeys: false }, awake).await }
    });

    for _ in 0..200 {
        if let Ok(Response::Pong) = send(&socket, Request::Ping).await {
            break;
        }

        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    Daemon { socket, lock, task }
}

async fn awake(daemon: &Daemon, request: AwakeRequest) -> Result<aujar_ipc::AwakeStatus, String> {
    match send(&daemon.socket, Request::Awake(request)).await.unwrap() {
        Response::Awake(AwakeResponse::Status(status)) => Ok(status),
        Response::Error(message) => Err(message),
        other => panic!("unexpected answer: {other:?}"),
    }
}

#[tokio::test]
async fn awake_is_off_until_asked_and_the_answers_follow_the_requests() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = start(dir.path(), "a").await;

    let status = awake(&daemon, AwakeRequest::Status).await.unwrap();

    assert_eq!(status.mode, AwakeMode::Off);
    assert!(!status.active);
    assert!(daemon.lock.log().is_empty());

    let status = awake(&daemon, AwakeRequest::For { seconds: 600 })
        .await
        .unwrap();

    assert_eq!(status.mode, AwakeMode::Timed);
    assert!(status.active);
    assert!(
        status
            .remaining_seconds
            .is_some_and(|left| (590..=600).contains(&left))
    );

    let status = awake(&daemon, AwakeRequest::Indefinite).await.unwrap();

    assert_eq!(status.mode, AwakeMode::Indefinite);
    assert_eq!(status.remaining_seconds, None);

    let status = awake(&daemon, AwakeRequest::Off).await.unwrap();

    assert!(!status.active);

    // One lock taken for the timer (replaced by the open-ended one without letting go), and
    // released at the end.
    assert_eq!(daemon.lock.log(), vec!["hold screen=false", "release"]);

    daemon.task.abort();
}

#[tokio::test]
async fn a_time_out_of_range_is_an_error_with_the_limits_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = start(dir.path(), "a").await;

    for seconds in [0, 30, 8 * 24 * 3600] {
        let error = awake(&daemon, AwakeRequest::For { seconds })
            .await
            .unwrap_err();

        assert!(error.contains("1m") && error.contains("7d"), "{error}");
    }

    assert!(!awake(&daemon, AwakeRequest::Status).await.unwrap().active);
    assert!(daemon.lock.log().is_empty());

    daemon.task.abort();
}

#[tokio::test]
async fn the_mode_is_kept_when_the_daemon_restarts() {
    let dir = tempfile::tempdir().unwrap();

    // The first daemon is asked to keep the computer awake for an hour, then dies hard.
    let first = start(dir.path(), "first").await;

    awake(&first, AwakeRequest::For { seconds: 3600 })
        .await
        .unwrap();
    first.task.abort();
    let _ = first.task.await;

    // A new daemon on the same installation carries on, with the time that is left.
    let second = start(dir.path(), "second").await;
    let status = awake(&second, AwakeRequest::Status).await.unwrap();

    assert_eq!(status.mode, AwakeMode::Timed);
    assert!(status.active, "the lock must be taken again at start");
    assert!(
        status
            .remaining_seconds
            .is_some_and(|left| left > 3500 && left <= 3600)
    );
    assert_eq!(second.lock.log(), vec!["hold screen=false"]);

    // And turning it off sticks across the next restart too.
    awake(&second, AwakeRequest::Off).await.unwrap();
    second.task.abort();
    let _ = second.task.await;

    let third = start(dir.path(), "third").await;

    assert!(!awake(&third, AwakeRequest::Status).await.unwrap().active);
    assert!(third.lock.log().is_empty());

    third.task.abort();
}

#[tokio::test]
async fn many_clients_at_once_get_consistent_answers() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = Arc::new(start(dir.path(), "a").await);

    let mut clients = Vec::new();

    for index in 0..12u64 {
        let daemon = Arc::clone(&daemon);

        clients.push(tokio::spawn(async move {
            // Some ask for a timer, some only look.
            if index % 2 == 0 {
                awake(
                    &daemon,
                    AwakeRequest::For {
                        seconds: 600 + index,
                    },
                )
                .await
                .unwrap()
            } else {
                awake(&daemon, AwakeRequest::Status).await.unwrap()
            }
        }));
    }

    for client in clients {
        let status = client.await.unwrap();

        // Whatever the order, an answer never contradicts itself.
        assert_eq!(status.active, status.mode != AwakeMode::Off);
    }

    let status = awake(&daemon, AwakeRequest::Status).await.unwrap();

    assert_eq!(status.mode, AwakeMode::Timed);

    // The lock was taken once and kept, however many timers replaced each other.
    assert_eq!(daemon.lock.log(), vec!["hold screen=false"]);

    daemon.task.abort();
}

#[tokio::test]
async fn the_screen_setting_is_followed_while_the_daemon_runs() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = start(dir.path(), "a").await;

    awake(&daemon, AwakeRequest::Indefinite).await.unwrap();

    // The person turns "keep the screen on" on in the settings, without restarting anything.
    let settings = dir.path().join("settings.toml");

    std::fs::write(&settings, "[awake]\nkeep_screen_on = true\n").unwrap();

    let mut followed = false;

    for _ in 0..80 {
        if daemon.lock.log().contains(&"hold screen=true".to_owned()) {
            followed = true;
            break;
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(followed, "{:?}", daemon.lock.log());
    assert!(
        awake(&daemon, AwakeRequest::Status)
            .await
            .unwrap()
            .keep_screen_on
    );

    daemon.task.abort();
}
