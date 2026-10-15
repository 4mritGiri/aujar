//! `aujar mwb ...`: control sharing of keyboard, mouse, clipboard and files between machines.

use std::{collections::HashSet, fmt::Write as _, path::PathBuf};

use anyhow::{Context, Result, bail};
use aujar_config::Settings;
use aujar_core::Capability;
use aujar_ipc::{
    MouseBordersRequest, MouseBordersResponse, MouseBordersStatus, Request, Response, send,
};
use aujar_mouse_borders::{
    keystore::KeyStore, layout::MachineName, net, pairing::PairingCode, settings_edit,
};
use clap::{Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Toggle {
    On,
    Off,
}

impl Toggle {
    fn enabled(self) -> bool {
        matches!(self, Self::On)
    }
}

#[derive(Debug, Subcommand)]
pub enum KeyAction {
    /// Create the security key that pairs your machines.
    Generate {
        /// Replace an existing key (disconnects machines paired with the old one).
        #[arg(long)]
        force: bool,
    },

    /// Print the security key to copy to your other machines.
    Show,

    /// Store the key created on another machine.
    Set {
        /// The key, e.g. `K7QD-2XJP-...` (all 32 characters).
        key: String,

        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum MwbAction {
    /// Show whether sharing works, which machines are connected and what happened recently.
    Status,

    /// Print a pairing code: the security key and the layout in one line to paste on your
    /// other machines. Creates the key and adds this machine to the layout if needed.
    Pair,

    /// Join the machines of a pairing code made with `pair` on another machine.
    Join {
        /// The code, starting with `aujar-pair1`.
        code: String,

        /// Replace this machine's security key if it has a different one.
        #[arg(long)]
        force: bool,
    },

    /// Manage the security key.
    Key {
        #[command(subcommand)]
        action: KeyAction,
    },

    /// Give the keyboard and mouse to a machine by name, or `local` for this one.
    Switch { machine: String },

    /// Switch machines by moving the pointer across screen edges.
    EasyMouse {
        #[arg(value_enum)]
        state: Toggle,
    },

    /// Send your keyboard and mouse input to every connected machine at once.
    MultiInput {
        #[arg(value_enum)]
        state: Toggle,
    },

    /// Lock every connected machine (and this one).
    LockAll,

    /// Drop all connections and connect again.
    Reconnect,

    /// Re-read settings and the key now.
    Reload,

    /// Send files (up to 100 MB each) to a connected machine.
    Send {
        machine: String,

        #[arg(required = true)]
        files: Vec<PathBuf>,
    },

    /// Run as a system service (for the login screen and elevated windows). See
    /// docs/MOUSE_BORDERS.md.
    Service,
}

pub async fn run(action: MwbAction, socket: PathBuf) -> Result<()> {
    match action {
        MwbAction::Service => service().await,

        MwbAction::Pair => pair(),

        MwbAction::Join { code, force } => join(&code, force),

        MwbAction::Status => match ask(&socket, MouseBordersRequest::Status).await? {
            MouseBordersResponse::Status(status) => {
                print!("{}", format_status(&status));

                Ok(())
            }
            other => bail!("unexpected answer: {other:?}"),
        },

        MwbAction::Key { action } => key(&socket, action).await,

        MwbAction::Switch { machine } => {
            let machine = (machine != "local").then_some(machine);

            done(&socket, MouseBordersRequest::Switch { machine }).await
        }

        MwbAction::EasyMouse { state } => {
            done(
                &socket,
                MouseBordersRequest::SetEasyMouse {
                    enabled: state.enabled(),
                },
            )
            .await
        }

        MwbAction::MultiInput { state } => {
            done(
                &socket,
                MouseBordersRequest::MultiMachineInput {
                    enabled: state.enabled(),
                },
            )
            .await
        }

        MwbAction::LockAll => done(&socket, MouseBordersRequest::LockAll).await,
        MwbAction::Reconnect => done(&socket, MouseBordersRequest::Reconnect).await,
        MwbAction::Reload => done(&socket, MouseBordersRequest::Reload).await,

        MwbAction::Send { machine, files } => {
            // The daemon has its own working directory: send absolute paths.
            let paths = files
                .iter()
                .map(|file| {
                    std::fs::canonicalize(file)
                        .with_context(|| format!("cannot find {}", file.display()))
                        .map(|path| path.display().to_string())
                })
                .collect::<Result<Vec<_>>>()?;

            done(&socket, MouseBordersRequest::SendFiles { machine, paths }).await
        }
    }
}

async fn ask(
    socket: &std::path::Path,
    request: MouseBordersRequest,
) -> Result<MouseBordersResponse> {
    match send(socket, Request::MouseBorders(request)).await? {
        Response::MouseBorders(response) => Ok(response),
        Response::Error(message) => bail!(message),
        other => bail!("unexpected answer: {other:?}"),
    }
}

async fn done(socket: &std::path::Path, request: MouseBordersRequest) -> Result<()> {
    match ask(socket, request).await? {
        MouseBordersResponse::Done(message) => {
            println!("{message}");

            Ok(())
        }
        other => bail!("unexpected answer: {other:?}"),
    }
}

async fn key(socket: &std::path::Path, action: KeyAction) -> Result<()> {
    match action {
        KeyAction::Generate { force } => {
            let MouseBordersResponse::Key(key) =
                ask(socket, MouseBordersRequest::GenerateKey { force }).await?
            else {
                bail!("the daemon did not return a key");
            };

            println!("Security key created:\n\n    {key}\n");
            println!("On every other machine run:\n\n    aujar mwb key set {key}\n");
            println!("Keep it private: anyone with the key can connect to your machines.");

            Ok(())
        }

        KeyAction::Show => match ask(socket, MouseBordersRequest::ShowKey).await? {
            MouseBordersResponse::Key(key) => {
                println!("{key}");

                Ok(())
            }
            MouseBordersResponse::NoKey => {
                bail!("there is no security key yet; run `aujar mwb key generate`")
            }
            other => bail!("unexpected answer: {other:?}"),
        },

        KeyAction::Set { key, force } => {
            done(socket, MouseBordersRequest::SetKey { key, force }).await
        }
    }
}

/// Settings and key changes that make this machine ready to hand out a pairing code.
/// Returns the code. Nothing is written by this function; the caller saves the settings.
fn make_pairing_code(
    settings: &mut Settings,
    store: &KeyStore,
    host: Option<std::net::IpAddr>,
) -> Result<String> {
    let mb = &mut settings.mouse_borders;
    let local = settings_edit::local_name(mb);

    settings_edit::add_machine(mb, &local, None)?;
    mb.enabled = true;

    let key = match store.load()? {
        Some(key) => key,
        None => store.generate(false)?,
    };

    Ok(PairingCode::from_settings(mb, key, &local, host)?.encode())
}

/// Adopt a pairing code into `settings` and store its key. A different existing key is only
/// replaced with `force`. Returns a message for the user.
fn join_with_code(
    settings: &mut Settings,
    store: &KeyStore,
    text: &str,
    force: bool,
) -> Result<String> {
    let code = PairingCode::parse(text)?;

    if let Some(existing) = store.load()?
        && existing != code.key
        && !force
    {
        bail!(
            "this machine already has a different security key; machines still using it \
             would disconnect. Run again with --force to replace it"
        );
    }

    let local: MachineName = settings_edit::local_name(&settings.mouse_borders);
    let joined = code.apply(&mut settings.mouse_borders, &local)?;

    store.save(&code.key, true)?;

    Ok(format!(
        "Joined {} machines as `{local}`{}.",
        joined.machines,
        if joined.added_local {
            " (this machine was added to the layout)"
        } else {
            ""
        }
    ))
}

fn pair() -> Result<()> {
    let mut settings = Settings::load_or_default();
    let code = make_pairing_code(
        &mut settings,
        &KeyStore::user_default(),
        net::pick_host(&net::local_networks()),
    )?;

    settings.save().context("cannot save settings")?;

    println!("Pairing code:\n\n    {code}\n");
    println!("On your other machine run:\n\n    aujar mwb join '{code}'\n");
    println!("Or click \"Join from clipboard\" in Settings \u{2192} Mouse Borders.");
    println!("The code contains your security key: do not share it.");

    Ok(())
}

fn join(code: &str, force: bool) -> Result<()> {
    let mut settings = Settings::load_or_default();
    let message = join_with_code(&mut settings, &KeyStore::user_default(), code, force)?;

    settings.save().context("cannot save settings")?;
    println!("{message}");
    println!("Mouse Borders is on. Check with `aujar mwb status`.");

    Ok(())
}

/// Human-readable status.
pub fn format_status(status: &MouseBordersStatus) -> String {
    let mut out = String::new();

    match status.state.as_str() {
        "running" => {
            let _ = writeln!(out, "Mouse Borders: running as `{}`", status.local);

            if let Some(address) = &status.listening {
                let _ = writeln!(out, "Listening on:  {address}");
            }

            let holder = if status.focus == status.local {
                "this machine".to_owned()
            } else {
                status.focus.clone()
            };
            let _ = writeln!(out, "Keyboard and mouse are on: {holder}");

            if let Some(controller) = &status.controlled_by {
                let _ = writeln!(out, "Controlled by: {controller}");
            }

            let _ = writeln!(out, "Switching with the mouse: {}", status.easy_mouse);

            if status.multi_machine_input {
                let _ = writeln!(out, "Multi-machine input: ON (input goes to every machine)");
            }
        }
        state => {
            let label = match state {
                "disabled" => "off",
                "waiting_for_key" => "waiting for a security key",
                "blocked" => "blocked by policy",
                _ => "not working",
            };
            let _ = writeln!(out, "Mouse Borders: {label}");

            if !status.detail.is_empty() {
                let _ = writeln!(out, "  {}", status.detail);
            }
        }
    }

    if !status.peers.is_empty() {
        let _ = writeln!(out, "\nOther machines:");

        let width = status
            .peers
            .iter()
            .map(|peer| peer.name.len())
            .max()
            .unwrap_or(0);

        for peer in &status.peers {
            let slot = peer
                .slot
                .map_or_else(String::new, |slot| format!("#{}", slot + 1));
            let state = if peer.connected {
                let screen = peer
                    .screen
                    .map_or_else(String::new, |(w, h)| format!("  {w}x{h}"));
                let address = peer.address.as_deref().unwrap_or("");

                format!("connected  {address}{screen}")
            } else {
                match &peer.last_error {
                    Some(error) => format!("not connected ({error})"),
                    None => "not connected".to_owned(),
                }
            };

            let _ = writeln!(out, "  {slot:<3} {:<width$}  {state}", peer.name);
        }
    }

    for (label, problem) in [
        (
            "Switching at screen edges",
            &status.edge_switching_unavailable,
        ),
        (
            "Capturing this machine's input",
            &status.capture_unavailable,
        ),
    ] {
        if let Some(problem) = problem {
            let _ = writeln!(out, "\n{label} is unavailable: {problem}");
        }
    }

    if !status.log.is_empty() {
        let _ = writeln!(out, "\nRecent:");

        for line in status.log.iter().rev().take(8).rev() {
            let _ = writeln!(out, "  {line}");
        }
    }

    out
}

/// Run the system service: the same supervisor as the daemon, configured from `/etc/aujar`,
/// without a clipboard (there is no desktop session to share one with).
async fn service() -> Result<()> {
    use aujar_mouse_borders::service::{Profile, ServiceState, Supervisor};

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .try_init();

    let denied: HashSet<Capability> = aujar_config::Policy::load_optional("/etc/aujar/policy.toml")
        .context("cannot load /etc/aujar/policy.toml")?
        .denied_capabilities
        .iter()
        .filter_map(|name| Capability::parse(name))
        .collect();

    let supervisor = Supervisor::spawn(Profile::system(denied));
    let mut report = supervisor.handle().subscribe();
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut last = None;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = sigterm.recv() => break,
            changed = report.changed() => {
                if changed.is_err() {
                    break;
                }

                let state = report.borrow().state.clone();

                if last.as_ref() != Some(&state) {
                    match &state {
                        ServiceState::Running => tracing::info!("Mouse Borders service running"),
                        other => tracing::warn!(?other, "Mouse Borders service is not running"),
                    }

                    last = Some(state);
                }
            }
        }
    }

    supervisor.shutdown().await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aujar_ipc::MouseBordersPeer;

    fn running() -> MouseBordersStatus {
        MouseBordersStatus {
            state: "running".into(),
            detail: String::new(),
            local: "desk".into(),
            focus: "desk".into(),
            controlled_by: None,
            multi_machine_input: false,
            easy_mouse: "Enabled".into(),
            listening: Some("0.0.0.0:15101".into()),
            edge_switching_unavailable: None,
            capture_unavailable: None,
            peers: vec![
                MouseBordersPeer {
                    name: "laptop".into(),
                    connected: true,
                    address: Some("192.168.1.20:15101".into()),
                    screen: Some((1920, 1080)),
                    slot: Some(1),
                    last_error: None,
                },
                MouseBordersPeer {
                    name: "tv".into(),
                    connected: false,
                    address: None,
                    screen: None,
                    slot: Some(2),
                    last_error: Some("connection refused".into()),
                },
            ],
            log: vec!["connected to laptop".into()],
        }
    }

    #[test]
    fn running_status_lists_machines_and_what_happened() {
        let text = format_status(&running());

        assert!(text.contains("running as `desk`"), "{text}");
        assert!(text.contains("are on: this machine"));
        assert!(text.contains("#2  laptop  connected  192.168.1.20:15101  1920x1080"));
        assert!(text.contains("#3  tv"));
        assert!(text.contains("not connected (connection refused)"));
        assert!(text.contains("Recent:\n  connected to laptop"));
    }

    #[test]
    fn status_says_who_has_the_keyboard_and_who_controls_this_machine() {
        let mut status = running();
        status.focus = "laptop".into();
        status.controlled_by = Some("laptop".into());
        status.multi_machine_input = true;

        let text = format_status(&status);

        assert!(text.contains("are on: laptop"));
        assert!(text.contains("Controlled by: laptop"));
        assert!(text.contains("Multi-machine input: ON"));
    }

    #[test]
    fn problems_are_explained_not_hidden() {
        let mut status = running();
        status.edge_switching_unavailable = Some("Wayland does not allow it".into());
        status.capture_unavailable = Some("no permission".into());

        let text = format_status(&status);

        assert!(text.contains("Switching at screen edges is unavailable: Wayland"));
        assert!(text.contains("Capturing this machine's input is unavailable: no permission"));
    }

    #[test]
    fn stopped_states_give_the_next_step() {
        let mut status = running();
        status.state = "waiting_for_key".into();
        status.detail = "run `aujar mwb key generate`".into();
        status.peers.clear();

        let text = format_status(&status);

        assert!(text.starts_with("Mouse Borders: waiting for a security key"));
        assert!(text.contains("aujar mwb key generate"));
        assert!(!text.contains("Other machines"));
    }

    #[test]
    fn only_the_latest_log_lines_are_shown() {
        let mut status = running();
        status.log = (0..20).map(|i| format!("event {i}")).collect();

        let text = format_status(&status);

        assert!(text.contains("event 19") && text.contains("event 12"));
        assert!(!text.contains("event 11"));
    }

    fn temp_store() -> (tempfile::TempDir, KeyStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::new(dir.path().join("mouse-borders.key"));

        (dir, store)
    }

    fn named(name: &str) -> Settings {
        let mut settings = Settings::default();
        settings.mouse_borders.machine_name = name.into();
        settings.mouse_borders.receive_dir = "/tmp/aujar-cli-test".into();

        settings
    }

    #[test]
    fn pairing_two_machines_end_to_end() {
        let (_a_dir, a_store) = temp_store();
        let (_b_dir, b_store) = temp_store();
        let mut desk = named("desk");
        let mut laptop = named("laptop");

        // First machine: makes the key, adds itself, turns the feature on.
        let code =
            make_pairing_code(&mut desk, &a_store, Some("192.168.1.10".parse().unwrap())).unwrap();
        assert!(code.starts_with("aujar-pair1;row;"), "{code}");
        assert!(desk.mouse_borders.enabled);
        assert_eq!(desk.mouse_borders.machines.len(), 1);

        // Second machine joins: same key, layout adopted, itself appended.
        let message = join_with_code(&mut laptop, &b_store, &code, false).unwrap();
        assert!(message.contains("added to the layout"), "{message}");
        assert_eq!(a_store.load().unwrap(), b_store.load().unwrap());
        assert_eq!(laptop.mouse_borders.machines.len(), 2);

        // Second machine hands its code back: the first one now knows both.
        let reply = make_pairing_code(&mut laptop, &b_store, None).unwrap();
        join_with_code(&mut desk, &a_store, &reply, false).unwrap();

        assert_eq!(desk.mouse_borders.machines, laptop.mouse_borders.machines);
        assert!(
            aujar_mouse_borders::config::EngineConfig::from_settings(&desk.mouse_borders).is_ok()
        );
    }

    #[test]
    fn joining_never_silently_replaces_a_different_key() {
        let (_a_dir, a_store) = temp_store();
        let (_b_dir, b_store) = temp_store();
        let code = make_pairing_code(&mut named("desk"), &a_store, None).unwrap();

        b_store.generate(false).unwrap();
        let before = b_store.load().unwrap();
        let mut laptop = named("laptop");

        let error = join_with_code(&mut laptop, &b_store, &code, false)
            .unwrap_err()
            .to_string();

        assert!(error.contains("--force"), "{error}");
        assert_eq!(b_store.load().unwrap(), before, "the key must be untouched");
        assert!(
            laptop.mouse_borders.machines.is_empty(),
            "so must the layout"
        );

        join_with_code(&mut laptop, &b_store, &code, true).unwrap();
        assert_eq!(b_store.load().unwrap(), a_store.load().unwrap());
    }

    #[test]
    fn a_bad_code_changes_nothing() {
        let (_dir, store) = temp_store();
        let mut settings = named("laptop");

        assert!(join_with_code(&mut settings, &store, "not a code", false).is_err());
        assert!(store.load().unwrap().is_none());
        assert!(!settings.mouse_borders.enabled);
    }

    #[test]
    fn pair_and_join_parse_as_commands() {
        use clap::Parser;

        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            action: MwbAction,
        }

        assert!(matches!(
            Cli::try_parse_from(["x", "pair"]).unwrap().action,
            MwbAction::Pair
        ));
        assert!(matches!(
            Cli::try_parse_from(["x", "join", "aujar-pair1;row;k", "--force"])
                .unwrap()
                .action,
            MwbAction::Join { force: true, .. }
        ));
        assert!(Cli::try_parse_from(["x", "join"]).is_err());
    }
}
