//! End-to-end tests: real engines talking over loopback TCP with the real encrypted
//! protocol, driven through in-memory input, clipboard and screen backends.

use std::{collections::BTreeMap, time::Duration};

use async_trait::async_trait;
use aujar_config::{Machine, MouseBorders};
use aujar_core::{Position, Size};
use aujar_mouse_borders::{
    backend::{
        Backends, CapturedInput, ClipboardContent, InputSource,
        memory::{
            MemoryClipboard, MemoryDesktop, MemoryLocker, MemorySink, MemorySource,
            MemorySourceHandle, SinkEvent, SinkLog,
        },
    },
    config::EngineConfig,
    engine::{Command, Engine, EngineHandle},
    error::{Error, Result as MbResult},
    input::Button,
    keys::{KEY_LEFTALT, KEY_LEFTCTRL, KEY_LEFTSHIFT, char_code},
    layout::MachineName,
    secure::SecurityKey,
};

const SCREEN: Size = Size {
    width: 1920,
    height: 1080,
};

struct Node {
    handle: EngineHandle,
    source: MemorySourceHandle,
    sink: SinkLog,
    desktop: MemoryDesktop,
    clipboard: MemoryClipboard,
    locker: MemoryLocker,
    audit: std::path::PathBuf,
    dir: tempfile::TempDir,
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn name(value: &str) -> MachineName {
    MachineName::new(value).unwrap()
}

fn ports(names: &[&str]) -> BTreeMap<String, u16> {
    names
        .iter()
        .map(|n| ((*n).to_owned(), free_port()))
        .collect()
}

/// A row of machines in the given order, `local` being one of them.
async fn start(
    local: &str,
    row: &[&str],
    ports: &BTreeMap<String, u16>,
    key: &SecurityKey,
    tracks_pointer: bool,
    tweak: impl FnOnce(&mut MouseBorders),
) -> Node {
    start_with_source(local, row, ports, key, tracks_pointer, tweak, None).await
}

async fn start_with_source(
    local: &str,
    row: &[&str],
    ports: &BTreeMap<String, u16>,
    key: &SecurityKey,
    tracks_pointer: bool,
    tweak: impl FnOnce(&mut MouseBorders),
    custom_source: Option<Box<dyn InputSource>>,
) -> Node {
    let dir = tempfile::tempdir().unwrap();

    let mut settings = MouseBorders {
        enabled: true,
        machine_name: local.to_owned(),
        port: ports[local],
        machines: row
            .iter()
            .enumerate()
            .map(|(slot, machine)| Machine {
                name: (*machine).to_owned(),
                slot: slot as u8,
                address: format!("127.0.0.1:{}", ports[*machine]),
            })
            .collect(),
        receive_dir: dir.path().join("received").display().to_string(),
        ..MouseBorders::default()
    };
    tweak(&mut settings);

    let mut config = EngineConfig::from_settings(&settings).unwrap();
    let audit = dir.path().join("audit.log");
    config.audit_log = Some(audit.clone());
    config.listen = format!("127.0.0.1:{}", ports[local]).parse().unwrap();
    config.timing.reconnect_min = Duration::from_millis(50);
    config.timing.reconnect_max = Duration::from_millis(200);
    config.timing.clipboard_poll = Duration::from_millis(30);
    config.timing.keep_awake = Duration::from_millis(100);
    config.timing.dial_grace = Duration::from_millis(400);

    let (source, source_handle) = MemorySource::new();
    let source: Box<dyn InputSource> = custom_source.unwrap_or_else(|| Box::new(source));
    let (sink, sink_log) = MemorySink::new();
    let desktop = MemoryDesktop::new(SCREEN, tracks_pointer);
    let clipboard = MemoryClipboard::default();
    let locker = MemoryLocker::default();

    let handle = Engine::start(
        config,
        key.clone(),
        Backends {
            source,
            sink: Box::new(sink),
            desktop: Box::new(desktop.clone()),
            clipboard: Some(Box::new(clipboard.clone())),
            locker: Box::new(locker.clone()),
            notes: Vec::new(),
        },
    )
    .await
    .unwrap();

    Node {
        handle,
        source: source_handle,
        sink: sink_log,
        desktop,
        clipboard,
        locker,
        audit,
        dir,
    }
}

async fn eventually(what: &str, check: impl Fn() -> bool) {
    for _ in 0..250 {
        if check() {
            return;
        }

        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    panic!("timed out waiting for: {what}");
}

async fn connected(nodes: &[&Node], count: usize) {
    eventually("peers to connect", || {
        nodes
            .iter()
            .all(|node| node.handle.status().connected_peers() == count)
    })
    .await;
}

/// Press `Ctrl+Alt+<letter or digit>` on a node's keyboard.
fn press_shortcut(node: &Node, character: char) {
    let code = char_code(character).unwrap();

    for (code, pressed) in [
        (KEY_LEFTCTRL, true),
        (KEY_LEFTALT, true),
        (code, true),
        (code, false),
        (KEY_LEFTALT, false),
        (KEY_LEFTCTRL, false),
    ] {
        node.source.send(CapturedInput::Key { key: code, pressed });
    }
}

fn push_right(node: &Node, y: i32) {
    node.desktop.set_pointer(Position { x: 1919, y });
    node.source
        .send(CapturedInput::PointerMotion { dx: 5, dy: 0 });
}

fn pair_status(node: &Node) -> String {
    format!("{:?}", node.handle.status())
}

#[tokio::test]
async fn machines_find_each_other_and_report_status() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;

    connected(&[&alpha, &beta], 1).await;

    let status = alpha.handle.status();
    assert!(status.running);
    assert_eq!(status.local, "alpha");
    assert_eq!(status.focus, "alpha");
    assert_eq!(status.peers[0].name, "beta");
    assert_eq!(status.peers[0].screen, Some(SCREEN));

    alpha.handle.stop().await;
    beta.handle.stop().await;
}

#[tokio::test]
async fn the_wrong_security_key_never_connects() {
    let ports = ports(&["alpha", "beta"]);
    let alpha = start(
        "alpha",
        &["alpha", "beta"],
        &ports,
        &SecurityKey::generate().unwrap(),
        true,
        |_| {},
    )
    .await;
    let beta = start(
        "beta",
        &["alpha", "beta"],
        &ports,
        &SecurityKey::generate().unwrap(),
        true,
        |_| {},
    )
    .await;

    // Wait for the refusal itself rather than for a fixed time.
    eventually("the key mismatch to be reported", || {
        alpha.handle.status().peers[0]
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("security key"))
    })
    .await;

    assert_eq!(alpha.handle.status().connected_peers(), 0);
    assert_eq!(beta.handle.status().connected_peers(), 0);

    alpha.handle.stop().await;
    beta.handle.stop().await;
}

#[tokio::test]
async fn pushing_past_the_edge_hands_over_input_and_back() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta], 1).await;

    push_right(&alpha, 540);

    eventually("beta to be entered", || {
        beta.sink
            .snapshot()
            .contains(&SinkEvent::PointerAbs(Position { x: 2, y: 540 }))
    })
    .await;
    assert!(alpha.source.is_grabbed(), "alpha must capture its input");
    assert_eq!(alpha.handle.status().focus, "beta");
    eventually("beta to know its controller", || {
        beta.handle.status().controlled_by.as_deref() == Some("alpha")
    })
    .await;

    // Keys, buttons, wheel and motion now go to beta only.
    alpha.sink.clear();
    alpha.source.send(CapturedInput::Key {
        key: 0x04,
        pressed: true,
    });
    alpha.source.send(CapturedInput::Button {
        button: Button::Left,
        pressed: true,
    });
    alpha.source.send(CapturedInput::Wheel {
        horizontal: 0,
        vertical: -120,
    });
    alpha
        .source
        .send(CapturedInput::PointerMotion { dx: 10, dy: 5 });

    eventually("input to arrive on beta", || {
        let events = beta.sink.snapshot();

        events.contains(&SinkEvent::Key(0x04, true))
            && events.contains(&SinkEvent::Button(Button::Left, true))
            && events.contains(&SinkEvent::Wheel(0, -120))
            && events.contains(&SinkEvent::PointerAbs(Position { x: 12, y: 545 }))
    })
    .await;
    // (A keep-awake nudge is how a grabbed machine avoids idling out; it is not input.)
    assert!(
        alpha
            .sink
            .snapshot()
            .iter()
            .all(|event| *event == SinkEvent::Nudge),
        "alpha must not act on grabbed input"
    );

    // Move back past beta's left edge.
    tokio::time::sleep(Duration::from_millis(250)).await;
    alpha
        .source
        .send(CapturedInput::PointerMotion { dx: -500, dy: 0 });

    eventually("control to return", || !alpha.source.is_grabbed()).await;
    assert_eq!(alpha.handle.status().focus, "alpha");

    // Beta released everything it was holding, and alpha's pointer was put back.
    eventually("beta to release held input", || {
        let events = beta.sink.snapshot();

        events.contains(&SinkEvent::Key(0x04, false))
            && events.contains(&SinkEvent::Button(Button::Left, false))
    })
    .await;
    eventually("alpha's pointer to reappear", || {
        alpha
            .sink
            .snapshot()
            .iter()
            .any(|event| matches!(event, SinkEvent::PointerAbs(p) if p.x >= 1900))
    })
    .await;
    eventually("beta to be free", || {
        beta.handle.status().controlled_by.is_none()
    })
    .await;

    alpha.handle.stop().await;
    beta.handle.stop().await;
}

#[tokio::test]
async fn keys_held_while_crossing_are_released_locally() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta], 1).await;

    // Shift is down when the pointer crosses.
    alpha.source.send(CapturedInput::Key {
        key: KEY_LEFTSHIFT,
        pressed: true,
    });
    push_right(&alpha, 100);

    eventually("grab", || alpha.source.is_grabbed()).await;
    eventually("alpha's desktop to see shift released", || {
        alpha
            .sink
            .snapshot()
            .contains(&SinkEvent::Key(KEY_LEFTSHIFT, false))
    })
    .await;

    alpha.handle.stop().await;
    beta.handle.stop().await;
}

#[tokio::test]
async fn shortcuts_jump_between_machines() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta", "gamma"]);
    let row = ["alpha", "beta", "gamma"];
    let alpha = start("alpha", &row, &ports, &key, true, |_| {}).await;
    let beta = start("beta", &row, &ports, &key, true, |_| {}).await;
    let gamma = start("gamma", &row, &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta, &gamma], 2).await;

    // Ctrl+Alt+3 jumps to the third machine, skipping the middle one.
    press_shortcut(&alpha, '3');
    eventually("gamma to be controlled", || {
        gamma.handle.status().controlled_by.as_deref() == Some("alpha")
    })
    .await;
    assert!(alpha.source.is_grabbed());
    assert_eq!(beta.handle.status().controlled_by, None);

    // Ctrl+Alt+1 brings it home.
    press_shortcut(&alpha, '1');
    eventually("home again", || !alpha.source.is_grabbed()).await;
    eventually("gamma released", || {
        gamma.handle.status().controlled_by.is_none()
    })
    .await;

    for node in [alpha, beta, gamma] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn a_controlled_machine_can_take_over_with_its_own_shortcut() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta", "gamma"]);
    let row = ["alpha", "beta", "gamma"];
    let alpha = start("alpha", &row, &ports, &key, true, |_| {}).await;
    let beta = start("beta", &row, &ports, &key, true, |_| {}).await;
    let gamma = start("gamma", &row, &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta, &gamma], 2).await;

    alpha
        .handle
        .request(Command::SwitchTo(Some(name("beta"))))
        .await
        .unwrap();
    eventually("beta controlled", || {
        beta.handle.status().controlled_by.as_deref() == Some("alpha")
    })
    .await;

    // At beta's keyboard, Ctrl+Alt+3: "I want to drive gamma".
    press_shortcut(&beta, '3');

    eventually("gamma driven by beta", || {
        gamma.handle.status().controlled_by.as_deref() == Some("beta")
    })
    .await;
    assert!(beta.source.is_grabbed());
    assert!(!alpha.source.is_grabbed(), "alpha must have let go");
    assert_eq!(alpha.handle.status().focus, "alpha");

    for node in [alpha, beta, gamma] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn control_passes_through_a_middle_machine_to_the_far_one() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta", "gamma"]);
    let row = ["alpha", "beta", "gamma"];
    let alpha = start("alpha", &row, &ports, &key, true, |_| {}).await;
    let beta = start("beta", &row, &ports, &key, true, |_| {}).await;
    let gamma = start("gamma", &row, &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta, &gamma], 2).await;

    push_right(&alpha, 300);
    eventually("beta entered", || {
        beta.handle.status().controlled_by.as_deref() == Some("alpha")
    })
    .await;

    tokio::time::sleep(Duration::from_millis(250)).await;
    // Sweep across beta (1920 wide) and push on through its right edge.
    alpha
        .source
        .send(CapturedInput::PointerMotion { dx: 1930, dy: 0 });
    tokio::time::sleep(Duration::from_millis(250)).await;
    alpha
        .source
        .send(CapturedInput::PointerMotion { dx: 20, dy: 0 });

    eventually("gamma entered", || {
        gamma.handle.status().controlled_by.as_deref() == Some("alpha")
    })
    .await;
    eventually("beta released", || {
        beta.handle.status().controlled_by.is_none()
    })
    .await;
    assert_eq!(alpha.handle.status().focus, "gamma");

    for node in [alpha, beta, gamma] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn losing_the_machine_you_control_returns_your_input() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta], 1).await;

    push_right(&alpha, 500);
    eventually("grab", || alpha.source.is_grabbed()).await;

    beta.handle.stop().await;

    eventually("alpha to get its input back", || !alpha.source.is_grabbed()).await;
    assert_eq!(alpha.handle.status().focus, "alpha");
    assert_eq!(alpha.handle.status().connected_peers(), 0);

    alpha.handle.stop().await;
}

#[tokio::test]
async fn easy_mouse_can_be_turned_off_and_modifiers_required() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start(
        "alpha",
        &["alpha", "beta"],
        &ports,
        &key,
        true,
        |settings| {
            settings.easy_mouse = aujar_config::EasyMouse::RequireCtrl;
        },
    )
    .await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta], 1).await;

    // Without Ctrl nothing happens.
    push_right(&alpha, 500);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!alpha.source.is_grabbed());

    // With Ctrl held it does.
    alpha.source.send(CapturedInput::Key {
        key: KEY_LEFTCTRL,
        pressed: true,
    });
    push_right(&alpha, 500);
    eventually("switch with ctrl", || alpha.source.is_grabbed()).await;

    for node in [alpha, beta] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn without_pointer_tracking_edges_are_disabled_but_shortcuts_work() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, false, |_| {}).await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta], 1).await;

    assert!(
        alpha.handle.status().edge_switching_unavailable.is_some(),
        "{}",
        pair_status(&alpha)
    );

    alpha
        .source
        .send(CapturedInput::PointerMotion { dx: 5, dy: 0 });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!alpha.source.is_grabbed());

    press_shortcut(&alpha, '2'); // Ctrl+Alt+2
    eventually("shortcut switch", || alpha.source.is_grabbed()).await;

    for node in [alpha, beta] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn the_clipboard_follows_you_to_the_machine_you_enter() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta], 1).await;

    alpha
        .clipboard
        .set(ClipboardContent::Text("from alpha".into()));
    // Let the watcher notice it before crossing.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        beta.clipboard.get(),
        None,
        "nothing is shared before you cross"
    );

    push_right(&alpha, 500);
    eventually("clipboard on beta", || {
        beta.clipboard.get() == Some(ClipboardContent::Text("from alpha".into()))
    })
    .await;

    // Copying on beta while it is controlled flows back to alpha right away.
    beta.clipboard
        .set(ClipboardContent::Text("from beta".into()));
    eventually("clipboard back on alpha", || {
        alpha.clipboard.get() == Some(ClipboardContent::Text("from beta".into()))
    })
    .await;

    for node in [alpha, beta] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn clipboard_sharing_can_be_disabled() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |s| {
        s.share_clipboard = false;
    })
    .await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta], 1).await;

    alpha
        .clipboard
        .set(ClipboardContent::Text("private".into()));
    tokio::time::sleep(Duration::from_millis(150)).await;
    push_right(&alpha, 500);
    eventually("grab", || alpha.source.is_grabbed()).await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    assert_eq!(beta.clipboard.get(), None);

    for node in [alpha, beta] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn files_are_sent_verified_and_never_overwrite() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta], 1).await;

    let file = alpha.dir.path().join("report.bin");
    let data: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 256) as u8).collect();
    std::fs::write(&file, &data).unwrap();

    for _ in 0..2 {
        alpha
            .handle
            .request(Command::SendFiles {
                machine: name("beta"),
                paths: vec![file.clone()],
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let received = beta.dir.path().join("received");

    eventually("both copies to arrive", || {
        std::fs::read_dir(&received)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| !e.file_name().to_string_lossy().ends_with(".part"))
                    .count()
                    == 2
            })
            .unwrap_or(false)
    })
    .await;

    assert_eq!(std::fs::read(received.join("report.bin")).unwrap(), data);
    assert_eq!(
        std::fs::read(received.join("report (1).bin")).unwrap(),
        data
    );

    for node in [alpha, beta] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn file_transfer_can_be_refused_by_the_receiver() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |s| {
        s.transfer_files = false;
    })
    .await;
    connected(&[&alpha, &beta], 1).await;

    let file = alpha.dir.path().join("x.txt");
    std::fs::write(&file, "hi").unwrap();

    let error = alpha
        .handle
        .request(Command::SendFiles {
            machine: name("beta"),
            paths: vec![file],
        })
        .await
        .unwrap_err();

    assert!(error.to_string().contains("disabled"), "{error}");
    assert!(!beta.dir.path().join("received").exists());

    for node in [alpha, beta] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn multi_machine_input_reaches_every_machine() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta", "gamma"]);
    let row = ["alpha", "beta", "gamma"];
    let alpha = start("alpha", &row, &ports, &key, true, |_| {}).await;
    let beta = start("beta", &row, &ports, &key, true, |_| {}).await;
    let gamma = start("gamma", &row, &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta, &gamma], 2).await;

    // Ctrl+Alt+M.
    press_shortcut(&alpha, 'm');
    eventually("broadcast on", || alpha.handle.status().multi_machine_input).await;
    eventually("peers listening", || {
        beta.handle
            .status()
            .log
            .iter()
            .any(|l| l.contains("every machine"))
            && gamma
                .handle
                .status()
                .log
                .iter()
                .any(|l| l.contains("every machine"))
    })
    .await;

    alpha.source.send(CapturedInput::Key {
        key: 0x04,
        pressed: true,
    });
    alpha
        .source
        .send(CapturedInput::PointerMotion { dx: 3, dy: 4 });

    for node in [&beta, &gamma] {
        eventually("broadcast input", || {
            let events = node.sink.snapshot();

            events.contains(&SinkEvent::Key(0x04, true))
                && events.contains(&SinkEvent::PointerRel(3, 4))
        })
        .await;
    }
    assert!(
        !alpha.source.is_grabbed(),
        "the sender keeps using its own desktop"
    );

    // Off again: nothing more arrives, and held keys are released.
    press_shortcut(&alpha, 'm');
    eventually("broadcast off", || {
        !alpha.handle.status().multi_machine_input
    })
    .await;
    eventually("keys released", || {
        beta.sink.snapshot().contains(&SinkEvent::Key(0x04, false))
    })
    .await;
    // (Keep-awake nudges are not input.)
    let input_events = |node: &Node| {
        node.sink
            .snapshot()
            .into_iter()
            .filter(|event| *event != SinkEvent::Nudge)
            .count()
    };
    let seen = input_events(&beta);
    alpha
        .source
        .send(CapturedInput::PointerMotion { dx: 9, dy: 9 });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(input_events(&beta), seen);

    for node in [alpha, beta, gamma] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn lock_all_locks_everyone_unless_a_machine_refuses() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta", "gamma"]);
    let row = ["alpha", "beta", "gamma"];
    let alpha = start("alpha", &row, &ports, &key, true, |_| {}).await;
    let beta = start("beta", &row, &ports, &key, true, |_| {}).await;
    let gamma = start("gamma", &row, &ports, &key, true, |s| {
        s.allow_remote_lock = false;
    })
    .await;
    connected(&[&alpha, &beta, &gamma], 2).await;

    // Ctrl+Alt+L.
    press_shortcut(&alpha, 'l');

    eventually("locks", || {
        alpha.locker.count() == 1 && beta.locker.count() == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(gamma.locker.count(), 0);

    for node in [alpha, beta, gamma] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn working_keeps_the_other_machines_awake() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta], 1).await;

    // Any local input marks alpha as active; beta is nudged so its screen saver stays off.
    alpha.source.send(CapturedInput::Key {
        key: 0x04,
        pressed: true,
    });
    alpha.source.send(CapturedInput::Key {
        key: 0x04,
        pressed: false,
    });

    eventually("beta to be nudged", || {
        beta.sink.snapshot().contains(&SinkEvent::Nudge)
    })
    .await;

    for node in [alpha, beta] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn reconnect_re_establishes_every_link() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta], 1).await;

    alpha.handle.request(Command::Reconnect).await.unwrap();

    // Down, then up again.
    eventually("links restored", || {
        alpha.handle.status().connected_peers() == 1
            && beta.handle.status().connected_peers() == 1
            && alpha
                .handle
                .status()
                .log
                .iter()
                .filter(|l| l.contains("connected to beta"))
                .count()
                >= 2
    })
    .await;

    for node in [alpha, beta] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn a_stranger_not_in_the_layout_is_turned_away() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta", "mallory"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    // Mallory knows the key but is not in alpha's layout.
    let mallory = start("mallory", &["mallory", "alpha"], &ports, &key, true, |_| {}).await;

    // Mallory may glimpse alpha's Hello before being refused; wait until it has been dropped.
    eventually("mallory to be turned away", || {
        mallory.handle.status().peers[0].last_error.is_some()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(alpha.handle.status().connected_peers(), 0);
    assert_eq!(mallory.handle.status().connected_peers(), 0);

    alpha.handle.stop().await;
    mallory.handle.stop().await;
}

#[tokio::test]
async fn commands_can_take_control_of_a_remote_machine() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta], 1).await;

    // Beta drives alpha through the command interface, as the CLI would.
    beta.handle
        .request(Command::SwitchTo(Some(name("alpha"))))
        .await
        .unwrap();

    eventually("alpha controlled", || {
        alpha.handle.status().controlled_by.as_deref() == Some("beta")
    })
    .await;
    assert!(beta.source.is_grabbed());

    for node in [alpha, beta] {
        node.handle.stop().await;
    }
}

/// An input source that cannot read any device, like a user outside the `input` group.
struct BrokenSource;

#[async_trait]
impl InputSource for BrokenSource {
    async fn next(&mut self) -> MbResult<Option<CapturedInput>> {
        Err(Error::unsupported(
            "input capture",
            "no readable input devices",
        ))
    }

    fn set_grab(&mut self, _grabbed: bool) -> MbResult<()> {
        Err(Error::unsupported(
            "input capture",
            "no readable input devices",
        ))
    }
}

#[tokio::test]
async fn a_machine_that_cannot_capture_input_says_so_and_can_still_be_controlled() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    let beta = start_with_source(
        "beta",
        &["alpha", "beta"],
        &ports,
        &key,
        true,
        |_| {},
        Some(Box::new(BrokenSource)),
    )
    .await;
    connected(&[&alpha, &beta], 1).await;

    eventually("capture failure to be reported", || {
        beta.handle
            .status()
            .capture_unavailable
            .is_some_and(|reason| reason.contains("no readable input devices"))
    })
    .await;

    // Beta cannot drive others...
    let error = beta
        .handle
        .request(Command::SwitchTo(Some(name("alpha"))))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot capture input: no readable input devices"),
        "the user must be told the real cause: {error}"
    );
    assert_eq!(beta.handle.status().focus, "beta", "focus must revert");
    assert_eq!(alpha.handle.status().controlled_by, None);

    // ...but alpha can drive beta.
    push_right(&alpha, 400);
    eventually("beta to be controlled", || {
        beta.handle.status().controlled_by.as_deref() == Some("alpha")
    })
    .await;
    eventually("input to arrive", || {
        beta.sink
            .snapshot()
            .iter()
            .any(|event| matches!(event, SinkEvent::PointerAbs(_)))
    })
    .await;

    for node in [alpha, beta] {
        node.handle.stop().await;
    }
}

/// Every audit record of a node, as JSON.
fn audit_records(node: &Node) -> Vec<serde_json::Value> {
    std::fs::read_to_string(&node.audit)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("each audit line is JSON"))
        .collect()
}

fn audited(node: &Node, event: &str, check: impl Fn(&serde_json::Value) -> bool) -> bool {
    audit_records(node)
        .iter()
        .any(|record| record["event"] == event && check(record))
}

#[tokio::test]
async fn the_audit_log_records_connections_control_files_and_locks() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta], 1).await;

    // Control moves to beta.
    push_right(&alpha, 500);
    eventually("beta controlled", || {
        beta.handle.status().controlled_by.as_deref() == Some("alpha")
    })
    .await;

    // A file goes across, and alpha locks everything.
    let file = alpha.dir.path().join("plan.txt");
    std::fs::write(&file, "twelve bytes").unwrap();
    alpha
        .handle
        .request(Command::SendFiles {
            machine: name("beta"),
            paths: vec![file],
        })
        .await
        .unwrap();
    alpha.handle.request(Command::LockAll).await.unwrap();

    eventually("the file and the lock to be audited on beta", || {
        audited(&beta, "file_received", |r| {
            r["name"] == "plan.txt" && r["bytes"] == 12
        }) && audited(&beta, "lock_requested", |r| {
            r["peer"] == "alpha" && r["allowed"] == true
        })
    })
    .await;

    // What alpha recorded.
    assert!(audited(&alpha, "service_started", |_| true));
    assert!(audited(&alpha, "peer_connected", |r| r["peer"] == "beta"));
    assert!(audited(&alpha, "control_moved", |r| r["from"] == "alpha"
        && r["to"] == "beta"));
    assert!(audited(&alpha, "file_offered", |r| r["name"] == "plan.txt"
        && r["peer"] == "beta"));
    assert!(audited(&alpha, "lock_requested", |r| r["peer"].is_null()
        && r["allowed"] == true));

    // What beta recorded.
    assert!(audited(&beta, "peer_connected", |r| r["peer"] == "alpha"));
    assert!(audited(&beta, "controlled_by", |r| r["peer"] == "alpha"));

    // Every record says who and when.
    for record in audit_records(&alpha) {
        assert_eq!(record["machine"], "alpha");
        assert!(record["time"].as_str().unwrap().ends_with('Z'), "{record}");
    }

    let alpha_log = alpha.audit.clone();

    alpha.handle.stop().await;
    assert!(
        std::fs::read_to_string(alpha_log)
            .unwrap()
            .contains("\"event\":\"service_stopped\"")
    );
    beta.handle.stop().await;
}

#[tokio::test]
async fn refused_connections_are_audited_with_a_reason() {
    let ports = ports(&["alpha", "beta", "mallory"]);
    let key = SecurityKey::generate().unwrap();
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    // Wrong key: the handshake fails on alpha's side.
    let intruder = start(
        "beta",
        &["alpha", "beta"],
        &ports,
        &SecurityKey::generate().unwrap(),
        true,
        |_| {},
    )
    .await;

    eventually("the wrong key to be audited", || {
        audited(&alpha, "peer_refused", |r| {
            r["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("security key"))
        })
    })
    .await;

    // Right key, but not in alpha's layout.
    let stranger = start("mallory", &["mallory", "alpha"], &ports, &key, true, |_| {}).await;

    eventually("the stranger to be audited", || {
        audited(&alpha, "peer_refused", |r| {
            r["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("mallory"))
        })
    })
    .await;

    for node in [alpha, intruder, stranger] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn the_audit_log_never_contains_clipboard_contents() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    connected(&[&alpha, &beta], 1).await;

    alpha
        .clipboard
        .set(ClipboardContent::Text("TOP-SECRET-PASSWORD".into()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    push_right(&alpha, 500);

    eventually("the clipboard to arrive", || {
        beta.clipboard.get() == Some(ClipboardContent::Text("TOP-SECRET-PASSWORD".into()))
    })
    .await;
    eventually("the transfer to be audited", || {
        audited(&alpha, "clipboard_sent", |r| {
            r["kind"] == "text" && r["bytes"] == 19
        }) && audited(&beta, "clipboard_received", |r| {
            r["kind"] == "text" && r["bytes"] == 19
        })
    })
    .await;

    for node in [&alpha, &beta] {
        let log = std::fs::read_to_string(&node.audit).unwrap();

        assert!(
            !log.contains("SECRET"),
            "clipboard content leaked into the audit log:\n{log}"
        );
        assert!(
            !log.contains(&key.to_display()),
            "the key leaked into the audit log"
        );
    }

    for node in [alpha, beta] {
        node.handle.stop().await;
    }
}

#[tokio::test]
async fn auditing_can_be_switched_off() {
    let key = SecurityKey::generate().unwrap();
    let ports = ports(&["alpha", "beta"]);
    let alpha = start("alpha", &["alpha", "beta"], &ports, &key, true, |_| {}).await;
    let beta = start("beta", &["alpha", "beta"], &ports, &key, true, |_| {}).await;

    // The helper always sets a path; switching it off is a config decision.
    let mut settings = MouseBorders {
        audit_log: false,
        machine_name: "alpha".into(),
        machines: vec![
            Machine {
                name: "alpha".into(),
                slot: 0,
                address: String::new(),
            },
            Machine {
                name: "beta".into(),
                slot: 1,
                address: String::new(),
            },
        ],
        receive_dir: "/tmp/aujar-audit-off".into(),
        ..MouseBorders::default()
    };
    assert!(
        EngineConfig::from_settings(&settings)
            .unwrap()
            .audit_log
            .is_none()
    );

    settings.audit_log = true;
    assert!(
        EngineConfig::from_settings(&settings)
            .unwrap()
            .audit_log
            .is_some()
    );

    for node in [alpha, beta] {
        node.handle.stop().await;
    }
}
