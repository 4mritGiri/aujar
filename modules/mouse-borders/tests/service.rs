//! The supervisor: settings and key file in, engine lifecycle and status out.

use std::{collections::HashSet, sync::Arc, time::Duration};

use aujar_config::{Machine, MouseBorders, Settings};
use aujar_core::{Capability, Size};
use aujar_mouse_borders::{
    backend::{
        Backends,
        memory::{MemoryClipboard, MemoryDesktop, MemoryLocker, MemorySink, MemorySource},
    },
    keystore::KeyStore,
    service::{MouseBordersModule, Profile, ServiceHandle, ServiceState, Supervisor},
};
use aujar_runtime::{Module, ModuleContext};

struct Fixture {
    dir: tempfile::TempDir,
    profile: Profile,
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn fixture(denied: &[Capability]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let profile = Profile {
        settings_path: dir.path().join("settings.toml"),
        key_store: KeyStore::new(dir.path().join("mouse-borders.key")),
        audit_log: dir.path().join("audit.log"),
        denied: denied.iter().copied().collect::<HashSet<_>>(),
        clipboard: true,
        backends: Arc::new(|_config| {
            let (source, _) = MemorySource::new();
            let (sink, _) = MemorySink::new();

            Backends {
                source: Box::new(source),
                sink: Box::new(sink),
                desktop: Box::new(MemoryDesktop::new(
                    Size {
                        width: 1920,
                        height: 1080,
                    },
                    true,
                )),
                clipboard: Some(Box::new(MemoryClipboard::default())),
                locker: Box::new(MemoryLocker::default()),
                notes: Vec::new(),
            }
        }),
    };

    Fixture { dir, profile }
}

fn machine(name: &str, slot: u8) -> Machine {
    Machine {
        name: name.into(),
        slot,
        address: String::new(),
    }
}

fn write_settings(fixture: &Fixture, edit: impl FnOnce(&mut MouseBorders)) {
    let mut settings = Settings {
        mouse_borders: MouseBorders {
            enabled: true,
            machine_name: "desk".into(),
            port: free_port(),
            machines: vec![machine("desk", 0), machine("laptop", 1)],
            receive_dir: fixture.dir.path().join("in").display().to_string(),
            ..MouseBorders::default()
        },
        ..Settings::default()
    };
    edit(&mut settings.mouse_borders);

    std::fs::write(
        &fixture.profile.settings_path,
        toml::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
}

async fn until(handle: &ServiceHandle, what: &str, check: impl Fn(&ServiceState) -> bool) {
    for _ in 0..300 {
        if check(&handle.report().state) {
            return;
        }

        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    panic!(
        "timed out waiting for {what}; state is {:?}",
        handle.report().state
    );
}

#[tokio::test]
async fn it_stays_off_until_enabled() {
    let fixture = fixture(&[]);
    let supervisor = Supervisor::spawn(fixture.profile.clone());
    let handle = supervisor.handle();

    until(&handle, "disabled", |s| *s == ServiceState::Disabled).await;

    let error = handle
        .request(aujar_mouse_borders::engine::Command::LockAll)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not running"));

    supervisor.shutdown().await;
}

#[tokio::test]
async fn enabled_without_a_key_waits_then_runs_once_a_key_exists() {
    let fixture = fixture(&[]);
    write_settings(&fixture, |_| {});

    let supervisor = Supervisor::spawn(fixture.profile.clone());
    let handle = supervisor.handle();

    until(&handle, "waiting for a key", |s| {
        *s == ServiceState::WaitingForKey
    })
    .await;

    let key = handle.generate_key(false).await.unwrap();
    until(&handle, "running", |s| *s == ServiceState::Running).await;

    let report = handle.report();
    let engine = report.engine.expect("engine status while running");
    assert!(engine.running);
    assert_eq!(engine.local, "desk");

    // The key is the one on disk.
    assert_eq!(handle.show_key().unwrap().unwrap(), key);

    // Replacing it needs --force, and restarts the engine.
    assert!(handle.generate_key(false).await.is_err());
    assert!(handle.generate_key(true).await.is_ok());

    supervisor.shutdown().await;
}

#[tokio::test]
async fn policy_can_forbid_it() {
    let fixture = fixture(&[Capability::Network]);
    write_settings(&fixture, |_| {});
    fixture.profile.key_store.generate(false).unwrap();

    let supervisor = Supervisor::spawn(fixture.profile.clone());
    let handle = supervisor.handle();

    until(
        &handle,
        "blocked",
        |s| matches!(s, ServiceState::BlockedByPolicy(reason) if reason.contains("network")),
    )
    .await;

    supervisor.shutdown().await;
}

#[tokio::test]
async fn configuration_problems_are_reported_not_fatal() {
    let fixture = fixture(&[]);
    write_settings(&fixture, |settings| {
        settings.machine_name = "stranger".into()
    });
    fixture.profile.key_store.generate(false).unwrap();

    let supervisor = Supervisor::spawn(fixture.profile.clone());
    let handle = supervisor.handle();

    until(&handle, "a layout error", |s| {
        matches!(s, ServiceState::Error(message) if message.contains("not in `mouse_borders.machines`"))
    })
    .await;

    // Fixing the file fixes the service, without restarting anything by hand.
    write_settings(&fixture, |_| {});
    handle.reload().await;
    until(&handle, "running after the fix", |s| {
        *s == ServiceState::Running
    })
    .await;

    supervisor.shutdown().await;
}

#[tokio::test]
async fn a_broken_settings_file_is_an_error_state() {
    let fixture = fixture(&[]);
    std::fs::write(&fixture.profile.settings_path, "this is = not [valid").unwrap();

    let supervisor = Supervisor::spawn(fixture.profile.clone());
    let handle = supervisor.handle();

    until(
        &handle,
        "a settings error",
        |s| matches!(s, ServiceState::Error(message) if message.contains("settings.toml")),
    )
    .await;

    supervisor.shutdown().await;
}

#[tokio::test]
async fn disabling_in_settings_stops_the_engine() {
    let fixture = fixture(&[]);
    write_settings(&fixture, |_| {});
    fixture.profile.key_store.generate(false).unwrap();

    let supervisor = Supervisor::spawn(fixture.profile.clone());
    let handle = supervisor.handle();
    until(&handle, "running", |s| *s == ServiceState::Running).await;

    write_settings(&fixture, |settings| settings.enabled = false);
    handle.reload().await;

    until(&handle, "disabled", |s| *s == ServiceState::Disabled).await;
    assert!(handle.report().engine.is_none());

    supervisor.shutdown().await;
}

#[tokio::test]
async fn a_port_that_is_taken_is_reported() {
    let fixture = fixture(&[]);
    let blocker = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = blocker.local_addr().unwrap().port();

    write_settings(&fixture, |settings| settings.port = port);
    fixture.profile.key_store.generate(false).unwrap();

    let supervisor = Supervisor::spawn(fixture.profile.clone());
    let handle = supervisor.handle();

    until(
        &handle,
        "a port error",
        |s| matches!(s, ServiceState::Error(message) if message.contains("cannot listen")),
    )
    .await;

    drop(blocker);

    supervisor.shutdown().await;
}

#[tokio::test]
async fn pasting_a_key_from_another_machine_works_and_bad_keys_are_refused() {
    let fixture = fixture(&[]);
    let supervisor = Supervisor::spawn(fixture.profile.clone());
    let handle = supervisor.handle();

    assert!(handle.set_key("too short", false).await.is_err());

    let other = aujar_mouse_borders::secure::SecurityKey::generate().unwrap();
    handle.set_key(&other.to_display(), false).await.unwrap();

    assert_eq!(handle.show_key().unwrap().unwrap(), other);

    supervisor.shutdown().await;
}

#[tokio::test]
async fn the_module_never_fails_to_start_because_of_configuration() {
    let fixture = fixture(&[]);
    std::fs::write(&fixture.profile.settings_path, "garbage = [").unwrap();

    let module = MouseBordersModule::new(fixture.profile.clone());

    assert!(module.handle().is_none());
    module.start(ModuleContext::new("test")).await.unwrap();
    assert!(module.handle().is_some());
    assert_eq!(module.metadata().id, "mouse-borders");
    assert!(
        module
            .metadata()
            .capabilities
            .contains(&Capability::ControlInput)
    );

    // Starting twice is harmless.
    module.start(ModuleContext::new("test")).await.unwrap();
    module.stop().await.unwrap();
}

#[tokio::test]
async fn key_changes_are_written_to_the_audit_log_without_the_key() {
    let fixture = fixture(&[]);
    write_settings(&fixture, |_| {});

    let supervisor = Supervisor::spawn(fixture.profile.clone());
    let handle = supervisor.handle();

    let first = handle.generate_key(false).await.unwrap();
    let second = handle.generate_key(true).await.unwrap();
    let pasted = aujar_mouse_borders::secure::SecurityKey::generate().unwrap();
    handle.set_key(&pasted.to_display(), true).await.unwrap();

    let log = std::fs::read_to_string(&fixture.profile.audit_log).unwrap();
    let hows: Vec<String> = log
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|record| record["event"] == "key_changed")
        .map(|record| record["how"].as_str().unwrap().to_owned())
        .collect();

    assert_eq!(hows, ["generated", "replaced", "set from another machine"]);
    for key in [first, second, pasted] {
        assert!(
            !log.contains(&key.to_display()),
            "a key must never be logged"
        );
    }

    supervisor.shutdown().await;
}

#[tokio::test]
async fn auditing_off_in_settings_writes_nothing() {
    let fixture = fixture(&[]);
    write_settings(&fixture, |settings| settings.audit_log = false);

    let supervisor = Supervisor::spawn(fixture.profile.clone());
    let handle = supervisor.handle();

    handle.generate_key(false).await.unwrap();

    assert!(!fixture.profile.audit_log.exists());

    supervisor.shutdown().await;
}
