//! Awake: keep the computer from sleeping, until you say otherwise or for a set time.
//!
//! The mode is remembered across restarts (see `aujar_config::State`), a timed mode ends by
//! itself, and the "keep the screen on too" setting is picked up without a restart.

pub mod api;
mod core;
#[cfg(target_os = "linux")]
mod portal;

use std::{path::PathBuf, time::SystemTime};

#[cfg(not(target_os = "linux"))]
use async_trait::async_trait;
use aujar_config::{Settings, State};
use tokio::sync::Mutex;

pub use self::core::{Awake, AwakeStatus, Inhibitor};
pub use aujar_config::AwakeMode;

/// Used where nothing can keep the computer awake (other platforms, for now).
#[cfg(not(target_os = "linux"))]
struct Unsupported;

#[cfg(not(target_os = "linux"))]
#[async_trait]
impl Inhibitor for Unsupported {
    async fn hold(&mut self, _screen: bool) -> Result<(), String> {
        Err("keeping the computer awake is not supported on this platform yet".into())
    }

    async fn release(&mut self) {}
}

#[cfg(target_os = "linux")]
fn system_inhibitor() -> Box<dyn Inhibitor> {
    Box::new(portal::PortalInhibitor::default())
}

#[cfg(not(target_os = "linux"))]
fn system_inhibitor() -> Box<dyn Inhibitor> {
    Box::new(Unsupported)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Where the settings come from, and how to tell they changed (one `stat` per tick).
struct SettingsWatch {
    path: PathBuf,
    stamp: Option<SystemTime>,
    keep_screen_on: bool,
}

impl SettingsWatch {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            stamp: None,
            keep_screen_on: false,
        }
    }

    /// Whether the screen is kept on too, re-read only when the file changed.
    fn keep_screen_on(&mut self) -> bool {
        let stamp = std::fs::metadata(&self.path)
            .and_then(|meta| meta.modified())
            .ok();

        if stamp != self.stamp {
            self.stamp = stamp;
            self.keep_screen_on = Settings::load_from(&self.path).map_or_else(
                |_| Settings::default().awake.keep_screen_on,
                |settings| settings.awake.keep_screen_on,
            );
        }

        self.keep_screen_on
    }
}

pub struct AwakeModule {
    awake: Mutex<Awake>,
    settings: std::sync::Mutex<SettingsWatch>,
}

impl Default for AwakeModule {
    fn default() -> Self {
        Self::new()
    }
}

impl AwakeModule {
    /// Keeps the real state file and the real settings, and uses the system's inhibitor.
    pub fn new() -> Self {
        Self::with(system_inhibitor(), Some(State::path()), Settings::path())
    }

    pub fn with(
        inhibitor: Box<dyn Inhibitor>,
        state_path: Option<PathBuf>,
        settings_path: PathBuf,
    ) -> Self {
        Self {
            awake: Mutex::new(Awake::new(inhibitor, state_path)),
            settings: std::sync::Mutex::new(SettingsWatch::new(settings_path)),
        }
    }

    fn screen_on(&self) -> bool {
        self.settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keep_screen_on()
    }

    /// Pick up where the last run stopped.
    pub async fn start(&self) {
        let screen_on = self.screen_on();

        self.awake.lock().await.restore(screen_on, now()).await;
    }

    /// Call about once a second.
    pub async fn tick(&self) {
        let screen_on = self.screen_on();

        self.awake.lock().await.tick(screen_on, now()).await;
    }

    pub async fn status(&self) -> AwakeStatus {
        self.awake.lock().await.status(now())
    }

    pub async fn set_off(&self) {
        self.awake.lock().await.set_off(now()).await;
    }

    pub async fn set_indefinite(&self) {
        self.awake.lock().await.set_indefinite(now()).await;
    }

    pub async fn set_for(&self, seconds: u64) {
        self.awake.lock().await.set_for(seconds, now()).await;
    }

    /// Let go of the lock (the daemon is stopping).
    pub async fn stop(&self) {
        self.awake.lock().await.release_for_shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::{api::*, core::fake::*, *};
    use std::time::Duration;

    fn scratch_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aujar-awake-module-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));

        std::fs::create_dir_all(&dir).unwrap();

        dir
    }

    /// Write the settings and make sure their time stamp is different from the last write.
    fn write_settings(path: &std::path::Path, text: &str, seconds_from_now: u64) {
        std::fs::write(path, text).unwrap();

        let file = std::fs::File::options().write(true).open(path).unwrap();

        file.set_modified(SystemTime::now() + Duration::from_secs(seconds_from_now))
            .unwrap();
    }

    fn module(dir: &std::path::Path) -> (AwakeModule, FakeInhibitor) {
        let fake = FakeInhibitor::default();
        let module = AwakeModule::with(
            Box::new(fake.clone()),
            Some(dir.join("state.toml")),
            dir.join("settings.toml"),
        );

        (module, fake)
    }

    #[tokio::test]
    async fn the_screen_setting_is_picked_up_without_a_restart() {
        let dir = scratch_dir();
        let settings = dir.join("settings.toml");
        let (module, fake) = module(&dir);

        write_settings(&settings, "[awake]\nkeep_screen_on = false\n", 0);
        module.start().await;
        module.set_indefinite().await;
        module.tick().await;

        // The person turns the setting on while Awake runs.
        write_settings(&settings, "[awake]\nkeep_screen_on = true\n", 5);
        module.tick().await;
        module.tick().await;

        assert_eq!(
            fake.calls(),
            vec![Call::Hold { screen: false }, Call::Hold { screen: true }]
        );
        assert!(module.status().await.keep_screen_on);

        // And off again.
        write_settings(&settings, "[awake]\nkeep_screen_on = false\n", 10);
        module.tick().await;
        assert!(!module.status().await.keep_screen_on);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_missing_or_broken_settings_file_means_the_defaults() {
        let dir = scratch_dir();
        let settings = dir.join("settings.toml");
        let (module, _fake) = module(&dir);

        module.start().await;
        assert!(!module.status().await.keep_screen_on, "no file");

        write_settings(&settings, "this is [not toml", 5);
        module.tick().await;
        assert!(!module.status().await.keep_screen_on, "broken file");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn requests_change_the_mode_and_answer_with_the_status() {
        let dir = scratch_dir();
        let (module, fake) = module(&dir);

        let AwakeResponse::Status(status) = handle(AwakeRequest::Status, &module).await.unwrap();

        assert_eq!(status.mode, aujar_config::AwakeMode::Off);

        let AwakeResponse::Status(status) = handle(AwakeRequest::For { seconds: 1800 }, &module)
            .await
            .unwrap();

        assert_eq!(status.mode, aujar_config::AwakeMode::Timed);
        assert!(status.active);
        assert!(
            status
                .remaining_seconds
                .is_some_and(|left| left > 1790 && left <= 1800)
        );

        let AwakeResponse::Status(status) =
            handle(AwakeRequest::Indefinite, &module).await.unwrap();

        assert_eq!(status.mode, aujar_config::AwakeMode::Indefinite);
        assert_eq!(status.remaining_seconds, None);

        let AwakeResponse::Status(status) = handle(AwakeRequest::Off, &module).await.unwrap();

        assert!(!status.active);
        assert_eq!(fake.calls().last(), Some(&Call::Release));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_time_out_of_range_is_refused_with_the_limits() {
        let dir = scratch_dir();
        let (module, fake) = module(&dir);

        for seconds in [0, 59, MAX_SECONDS + 1] {
            let error = handle(AwakeRequest::For { seconds }, &module)
                .await
                .unwrap_err();

            assert!(error.contains("1m") && error.contains("7d"), "{error}");
        }

        // Nothing was started by the refused requests.
        assert!(fake.calls().is_empty());
        assert!(!module.status().await.active);

        // The limits themselves are fine.
        assert!(
            handle(
                AwakeRequest::For {
                    seconds: MIN_SECONDS
                },
                &module
            )
            .await
            .is_ok()
        );
        assert!(
            handle(
                AwakeRequest::For {
                    seconds: MAX_SECONDS
                },
                &module
            )
            .await
            .is_ok()
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn stopping_lets_go_but_the_next_start_carries_on() {
        let dir = scratch_dir();
        let (first, first_fake) = module(&dir);

        first.set_indefinite().await;
        first.stop().await;

        assert_eq!(
            first_fake.calls(),
            vec![Call::Hold { screen: false }, Call::Release]
        );

        // A new daemon process: the mode was kept, the lock is taken again.
        let (second, second_fake) = module(&dir);

        second.start().await;

        assert!(second.status().await.active);
        assert_eq!(second_fake.calls(), vec![Call::Hold { screen: false }]);

        let _ = std::fs::remove_dir_all(dir);
    }
}
