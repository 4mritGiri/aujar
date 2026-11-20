//! A launcher lookup answered entirely from a recorded fixture: no network, deterministic.
//!
//! The daemon runs for real and is asked `define fire` over its socket; the dictionary provider
//! reads `dev/fixtures/api.dictionaryapi.dev_v2_entries_en_fire.http` instead of calling
//! dictionaryapi.dev (see `AUJAR_FIXTURES` in modules/launcher/src/web.rs).
//!
//! This file holds a single test on purpose: it sets process-wide environment variables, which
//! is only sound while nothing else in the process is reading them.

use aujar_ipc::{Request, Response, SearchRequest, send};
use std::{path::PathBuf, time::Duration};

#[test]
fn a_definition_is_answered_from_the_fixtures_without_the_network() {
    let root = std::env::temp_dir().join(format!("aujar-offline-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dev/fixtures");

    assert!(
        fixtures.is_dir(),
        "the fixtures directory is missing: {}",
        fixtures.display()
    );

    // SAFETY: this is the only test in this binary and no other thread exists yet: the async
    // runtime is built after these calls.
    unsafe {
        std::env::set_var("AUJAR_FIXTURES", &fixtures);
        std::env::remove_var("AUJAR_FIXTURES_RECORD");

        // Keep the daemon away from the developer's real settings, files and applications.
        std::env::set_var("HOME", &root);
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        std::env::set_var("XDG_STATE_HOME", root.join("state"));
        std::env::set_var("XDG_DATA_HOME", root.join("data"));
        std::env::set_var("XDG_CACHE_HOME", root.join("cache"));
        std::env::set_var("XDG_DATA_DIRS", root.join("no-such-dir"));
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        let socket = root.join("aujar.sock");

        let server = tokio::spawn({
            let socket = socket.clone();
            async move {
                // Never trigger the desktop's shortcut dialog from tests.
                aujar_daemon::run_with(socket, aujar_daemon::Options { hotkeys: false }).await
            }
        });

        for _ in 0..100 {
            if socket.exists() {
                break;
            }

            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert!(socket.exists(), "daemon did not create its socket");

        // The first answer is the "looking up" placeholder: the lookup finishes on a worker
        // thread, and asking again finds the answer in the cache.
        let mut found = None;

        for _ in 0..100 {
            let response = send(
                &socket,
                Request::Search(SearchRequest {
                    query: "define fire".into(),
                    limit: Some(10),
                }),
            )
            .await
            .unwrap();

            let Response::Search(search) = response else {
                panic!("expected a search answer");
            };

            if let Some(hit) = search
                .results
                .iter()
                .find(|hit| hit.id.starts_with("dict:fire:"))
            {
                found = Some(hit.clone());
                break;
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let hit = found.expect(
            "no definition arrived: is dev/fixtures/api.dictionaryapi.dev_v2_entries_en_fire.http \
             present, and is the dictionary plugin on?",
        );

        assert_eq!(hit.kind, "definition");
        assert_eq!(hit.title, "fire");
        assert!(
            hit.detail
                .as_deref()
                .unwrap_or_default()
                .contains("Combustion that gives off heat"),
            "unexpected detail: {:?}",
            hit.detail
        );

        server.abort();
    });

    let _ = std::fs::remove_dir_all(&root);
}
