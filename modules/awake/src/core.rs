//! The Awake state machine: what is being kept awake, until when, and whether the lock that
//! does it is actually held. Everything here takes the time as a parameter (Unix seconds) and
//! talks to the system only through [`Inhibitor`], so it is tested without a clock or a desktop.

use std::path::PathBuf;

use async_trait::async_trait;
use aujar_config::{AwakeMode, AwakeState, State};

/// How long to wait before trying again after the system refused to keep the computer awake.
const RETRY_SECONDS: u64 = 30;

/// Holds the "do not sleep" lock.
#[async_trait]
pub trait Inhibitor: Send {
    /// Keep the computer awake, and the screen on too when `screen` is set. Calling it again
    /// with the other `screen` value changes what is held.
    async fn hold(&mut self, screen: bool) -> Result<(), String>;

    /// Let the computer sleep again.
    async fn release(&mut self);
}

/// What a client is told about Awake.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AwakeStatus {
    pub mode: AwakeMode,
    /// The computer is being kept awake right now.
    pub active: bool,
    /// Seconds left in a timed mode.
    pub remaining_seconds: Option<u64>,
    pub keep_screen_on: bool,
    /// Why keeping the computer awake does not work, when it does not.
    pub error: Option<String>,
}

pub struct Awake {
    inhibitor: Box<dyn Inhibitor>,
    state: AwakeState,
    /// Whether the screen is kept on too (a setting).
    screen_on: bool,
    /// What the inhibitor currently holds: `None` for nothing, else whether it includes the screen.
    held: Option<bool>,
    error: Option<String>,
    /// Do not try again before this time after a failure.
    retry_at: u64,
    /// Where the state is kept across restarts (none in tests that do not want it).
    store: Option<PathBuf>,
}

impl Awake {
    pub fn new(inhibitor: Box<dyn Inhibitor>, store: Option<PathBuf>) -> Self {
        Self {
            inhibitor,
            state: AwakeState::default(),
            screen_on: false,
            held: None,
            error: None,
            retry_at: 0,
            store,
        }
    }

    fn is_active(&self, now: u64) -> bool {
        match self.state.mode {
            AwakeMode::Off => false,
            AwakeMode::Indefinite => true,
            AwakeMode::Timed => now < self.state.until,
        }
    }

    pub fn status(&self, now: u64) -> AwakeStatus {
        AwakeStatus {
            mode: self.state.mode,
            active: self.held.is_some(),
            remaining_seconds: (self.state.mode == AwakeMode::Timed)
                .then(|| self.state.until.saturating_sub(now)),
            keep_screen_on: self.screen_on,
            error: self.error.clone(),
        }
    }

    fn save(&self) {
        if let Some(path) = &self.store
            && let Err(error) = (State { awake: self.state }).save_to(path)
        {
            tracing::warn!("cannot save the Awake state: {error}");
        }
    }

    /// Make the lock match what is wanted.
    async fn apply(&mut self, now: u64) {
        if !self.is_active(now) {
            if self.held.take().is_some() {
                self.inhibitor.release().await;
            }

            self.error = None;

            return;
        }

        if self.held == Some(self.screen_on) {
            return;
        }

        if now < self.retry_at {
            return;
        }

        match self.inhibitor.hold(self.screen_on).await {
            Ok(()) => {
                self.held = Some(self.screen_on);
                self.error = None;
            }
            Err(error) => {
                self.held = None;
                self.error = Some(error);
                self.retry_at = now + RETRY_SECONDS;
            }
        }
    }

    /// Pick up where the last run stopped (a timed mode that ran out meanwhile is just off).
    pub async fn restore(&mut self, screen_on: bool, now: u64) {
        let saved = self
            .store
            .as_deref()
            .map_or_else(State::default, State::load_from)
            .awake;

        self.screen_on = screen_on;
        self.state = saved;

        if self.state.mode == AwakeMode::Timed && now >= self.state.until {
            self.state = AwakeState::default();
            self.save();
        }

        self.apply(now).await;
    }

    pub async fn set_off(&mut self, now: u64) {
        self.state = AwakeState::default();
        self.save();
        self.apply(now).await;
    }

    pub async fn set_indefinite(&mut self, now: u64) {
        self.state = AwakeState {
            mode: AwakeMode::Indefinite,
            until: 0,
        };
        self.retry_at = 0;
        self.save();
        self.apply(now).await;
    }

    pub async fn set_for(&mut self, seconds: u64, now: u64) {
        self.state = AwakeState {
            mode: AwakeMode::Timed,
            until: now.saturating_add(seconds),
        };
        self.retry_at = 0;
        self.save();
        self.apply(now).await;
    }

    /// The daemon is stopping: let go of the lock, but keep the saved mode, so the next start
    /// carries on with it.
    pub async fn release_for_shutdown(&mut self) {
        if self.held.take().is_some() {
            self.inhibitor.release().await;
        }
    }

    /// Called every second or so: ends a timed mode that ran out, follows the screen setting,
    /// and tries again after a failure.
    pub async fn tick(&mut self, screen_on: bool, now: u64) {
        self.screen_on = screen_on;

        if self.state.mode == AwakeMode::Timed && now >= self.state.until {
            self.state = AwakeState::default();
            self.save();
        }

        self.apply(now).await;
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Call {
        Hold { screen: bool },
        Release,
    }

    /// An inhibitor that only writes down what it was asked.
    #[derive(Clone, Default)]
    pub struct FakeInhibitor {
        pub calls: Arc<Mutex<Vec<Call>>>,
        pub refuse: Arc<AtomicBool>,
    }

    impl FakeInhibitor {
        pub fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Inhibitor for FakeInhibitor {
        async fn hold(&mut self, screen: bool) -> Result<(), String> {
            if self.refuse.load(Ordering::SeqCst) {
                return Err("the desktop said no".into());
            }

            self.calls.lock().unwrap().push(Call::Hold { screen });

            Ok(())
        }

        async fn release(&mut self) {
            self.calls.lock().unwrap().push(Call::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fake::*, *};
    use std::sync::atomic::Ordering;

    const NOW: u64 = 1_800_000_000;

    fn awake() -> (Awake, FakeInhibitor) {
        let fake = FakeInhibitor::default();

        (Awake::new(Box::new(fake.clone()), None), fake)
    }

    fn scratch_store() -> PathBuf {
        std::env::temp_dir().join(format!(
            "aujar-awake-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ))
    }

    #[tokio::test]
    async fn nothing_is_held_until_asked() {
        let (mut awake, fake) = awake();

        awake.tick(false, NOW).await;
        awake.tick(false, NOW + 100).await;

        assert!(fake.calls().is_empty());
        assert_eq!(awake.status(NOW).mode, AwakeMode::Off);
        assert!(!awake.status(NOW).active);
    }

    #[tokio::test]
    async fn keeping_awake_indefinitely_holds_once_and_until_told_otherwise() {
        let (mut awake, fake) = awake();

        awake.set_indefinite(NOW).await;

        for second in 1..=5 {
            awake.tick(false, NOW + second * 3600).await;
        }

        // Held once, and still held days later.
        assert_eq!(fake.calls(), vec![Call::Hold { screen: false }]);
        assert!(awake.status(NOW).active);
        assert_eq!(awake.status(NOW).remaining_seconds, None);

        awake.set_off(NOW + 10).await;

        assert_eq!(
            fake.calls(),
            vec![Call::Hold { screen: false }, Call::Release]
        );
        assert!(!awake.status(NOW + 10).active);
    }

    #[tokio::test]
    async fn a_timed_mode_counts_down_and_ends_by_itself() {
        let (mut awake, fake) = awake();

        awake.set_for(600, NOW).await;

        assert_eq!(awake.status(NOW).remaining_seconds, Some(600));
        assert_eq!(awake.status(NOW + 100).remaining_seconds, Some(500));

        awake.tick(false, NOW + 599).await;
        assert!(awake.status(NOW + 599).active, "one second left");

        awake.tick(false, NOW + 600).await;

        let status = awake.status(NOW + 600);

        assert!(!status.active);
        assert_eq!(status.mode, AwakeMode::Off);
        assert_eq!(status.remaining_seconds, None);
        assert_eq!(
            fake.calls(),
            vec![Call::Hold { screen: false }, Call::Release]
        );
    }

    #[tokio::test]
    async fn a_new_timer_replaces_the_old_one_without_letting_go() {
        let (mut awake, fake) = awake();

        awake.set_for(600, NOW).await;
        awake.set_for(7200, NOW + 60).await;

        assert_eq!(awake.status(NOW + 60).remaining_seconds, Some(7200));
        assert_eq!(fake.calls(), vec![Call::Hold { screen: false }]);
    }

    #[tokio::test]
    async fn the_screen_setting_changes_what_is_held() {
        let (mut awake, fake) = awake();

        awake.set_indefinite(NOW).await;
        awake.tick(true, NOW + 1).await;
        awake.tick(true, NOW + 2).await;
        awake.tick(false, NOW + 3).await;

        assert_eq!(
            fake.calls(),
            vec![
                Call::Hold { screen: false },
                Call::Hold { screen: true },
                Call::Hold { screen: false },
            ]
        );
    }

    #[tokio::test]
    async fn a_refusal_is_reported_and_retried_later_not_in_a_loop() {
        let (mut awake, fake) = awake();

        fake.refuse.store(true, Ordering::SeqCst);
        awake.set_indefinite(NOW).await;

        let status = awake.status(NOW);

        assert!(!status.active);
        assert_eq!(status.error.as_deref(), Some("the desktop said no"));

        // Even when the desktop would say yes now, nothing is tried before the pause is over.
        fake.refuse.store(false, Ordering::SeqCst);
        awake.tick(false, NOW + 10).await;
        assert!(!awake.status(NOW + 10).active);

        awake.tick(false, NOW + 31).await;

        let status = awake.status(NOW + 31);

        assert!(status.active);
        assert_eq!(status.error, None);
        assert_eq!(fake.calls(), vec![Call::Hold { screen: false }]);
    }

    #[tokio::test]
    async fn asking_again_tries_at_once_even_during_the_pause() {
        let (mut awake, fake) = awake();

        fake.refuse.store(true, Ordering::SeqCst);
        awake.set_indefinite(NOW).await;
        fake.refuse.store(false, Ordering::SeqCst);
        awake.set_indefinite(NOW + 1).await;

        assert!(awake.status(NOW + 1).active);
    }

    #[tokio::test]
    async fn the_mode_survives_a_restart() {
        let store = scratch_store();
        let fake = FakeInhibitor::default();

        // First run: keep awake for an hour.
        let mut first = Awake::new(Box::new(fake.clone()), Some(store.clone()));

        first.set_for(3600, NOW).await;

        // The daemon restarts twenty minutes later: still awake, with the time left.
        let second_fake = FakeInhibitor::default();
        let mut second = Awake::new(Box::new(second_fake.clone()), Some(store.clone()));

        second.restore(false, NOW + 1200).await;

        let status = second.status(NOW + 1200);

        assert!(status.active);
        assert_eq!(status.mode, AwakeMode::Timed);
        assert_eq!(status.remaining_seconds, Some(2400));
        assert_eq!(second_fake.calls(), vec![Call::Hold { screen: false }]);

        let _ = std::fs::remove_file(store);
    }

    #[tokio::test]
    async fn a_timer_that_ran_out_while_stopped_is_off_after_a_restart() {
        let store = scratch_store();
        let mut first = Awake::new(Box::new(FakeInhibitor::default()), Some(store.clone()));

        first.set_for(600, NOW).await;

        let fake = FakeInhibitor::default();
        let mut second = Awake::new(Box::new(fake.clone()), Some(store.clone()));

        second.restore(false, NOW + 7200).await;

        assert!(!second.status(NOW + 7200).active);
        assert!(fake.calls().is_empty());

        // And the file says so, so a third start does not resurrect it.
        assert_eq!(State::load_from(&store).awake.mode, AwakeMode::Off);
        let _ = std::fs::remove_file(store);
    }

    #[tokio::test]
    async fn indefinite_stays_on_across_restarts_and_off_stays_off() {
        let store = scratch_store();
        let mut first = Awake::new(Box::new(FakeInhibitor::default()), Some(store.clone()));

        first.set_indefinite(NOW).await;

        let fake = FakeInhibitor::default();
        let mut second = Awake::new(Box::new(fake.clone()), Some(store.clone()));

        second.restore(true, NOW + 999_999).await;
        assert_eq!(fake.calls(), vec![Call::Hold { screen: true }]);

        second.set_off(NOW + 1_000_000).await;

        let mut third = Awake::new(Box::new(FakeInhibitor::default()), Some(store.clone()));

        third.restore(false, NOW + 1_000_001).await;
        assert!(!third.status(NOW + 1_000_001).active);
        let _ = std::fs::remove_file(store);
    }

    #[tokio::test]
    async fn the_status_reports_the_screen_setting() {
        let (mut awake, _fake) = awake();

        awake.tick(true, NOW).await;

        assert!(awake.status(NOW).keep_screen_on);
    }
}
