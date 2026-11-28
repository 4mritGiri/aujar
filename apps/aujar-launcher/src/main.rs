use anyhow::Result;
use aujar_core::{Health, Session};
use aujar_ipc::{
    Event, ExecuteRequest, LauncherCommand, RenameRequest, Request, Response, SearchRequest, send,
    subscribe,
};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use std::path::PathBuf;

mod awake;
mod backup;
mod gnome_extension;
mod mouse_borders;
mod nexdesk;
mod shortcut;
mod text;

#[cfg(not(windows))]
const DEFAULT_SOCKET_NAME: &str = "aujar.sock";
#[cfg(not(windows))]
const FALLBACK_SOCKET: &str = "/tmp/aujar.sock";
#[cfg(windows)]
const WINDOWS_PIPE_NAME: &str = r"\\.\pipe\aujar";

#[derive(Debug, Parser)]
#[command(
    name = "aujar",
    version,
    about = "Aujar — Cross-platform productivity toolkit"
)]
struct Cli {
    /// Log more to stderr: -v shows Aujar's debug messages, -vv its trace messages.
    /// `RUST_LOG` overrides this; see docs/DEVELOPMENT.md ("Logs and diagnostics").
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum LauncherAction {
    Show,
    Hide,
    Toggle,
    /// Open the settings window.
    Settings,
}

impl From<LauncherAction> for LauncherCommand {
    fn from(action: LauncherAction) -> Self {
        match action {
            LauncherAction::Show => Self::Show,
            LauncherAction::Hide => Self::Hide,
            LauncherAction::Toggle => Self::Toggle,
            LauncherAction::Settings => Self::Settings,
        }
    }
}

#[derive(Debug, Subcommand)]
enum SnipAction {
    /// Let Aujar take screenshots without asking (GNOME): what "Allow" in GNOME's dialog
    /// stores, which cannot open for the background daemon. Needed for the snip overlay.
    Allow {
        /// Remove the permission again.
        #[arg(long)]
        revoke: bool,
    },

    /// Register the Aujar Snip shortcut in GNOME Keyboard Shortcuts.
    ShortcutInstall {
        /// Modifier names followed by one key (default: the settings, SUPER+SHIFT+S).
        #[arg(long)]
        keys: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum ExtensionAction {
    /// Copy the extension into your GNOME Shell extensions and enable it.
    Install,

    /// Disable and remove the extension.
    Remove,

    /// Say whether the extension is installed, enabled and answering.
    Status,
}

#[derive(Debug, Subcommand)]
enum ShortcutAction {
    /// Add a GNOME custom shortcut that runs `aujar launcher toggle`.
    Install {
        /// Key combination in GNOME accelerator syntax.
        #[arg(long, default_value = shortcut::DEFAULT_KEYS)]
        keys: String,
    },

    /// Remove the Aujar shortcut again.
    Remove,
}

#[derive(Debug, Subcommand)]
enum ColorAction {
    /// Start the color picker: the same as the color picker shortcut. Without the Aujar UI
    /// running it falls back to the desktop's own picker and prints the color.
    Pick,

    /// Open the color editor with the color history.
    Editor,

    /// Show a hex color in every supported format.
    Convert {
        /// A hex color such as `#FF8800` or `f80`.
        value: String,
    },

    /// Register the screen picker shortcut in GNOME Keyboard Shortcuts.
    ShortcutInstall {
        /// Modifier names followed by one key, e.g. SUPER+SHIFT+C.
        #[arg(long, default_value = "SUPER+SHIFT+C")]
        keys: String,
    },
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Report the features supported by this operating system and desktop backend.
    Platform,

    /// Check whether the Aujar daemon is reachable.
    Ping,

    /// Display Aujar runtime health information.
    Health,

    /// List available windows.
    Windows,

    /// List the modules registered in the daemon.
    Modules,

    /// Search applications and calculations (what the launcher UI will show).
    Search {
        /// Search text, e.g. `fire` or `2*(3+4)`.
        query: String,

        /// Maximum number of results.
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },

    /// Launch a search result by id (see `aujar search`).
    Run {
        /// Result id, e.g. `apps:firefox.desktop`.
        id: String,
    },

    /// Show, hide or toggle the launcher UI. Bind this to a keyboard shortcut.
    Launcher {
        #[arg(value_enum)]
        action: LauncherAction,
    },

    /// Diagnose the daemon, UI connection and global shortcut.
    Doctor {
        /// Also show developer details: which socket this program uses and why, the
        /// environment variables that change Aujar's behaviour, log and settings locations,
        /// and the state of an installed daemon.
        #[arg(long)]
        dev: bool,
    },

    /// Manage the keyboard shortcut that shows the launcher (GNOME).
    Shortcut {
        #[command(subcommand)]
        action: ShortcutAction,
    },

    /// Install the GNOME Shell extension that lets the launcher switch windows.
    GnomeExtension {
        #[command(subcommand)]
        action: ExtensionAction,
    },

    /// Aujar Snip: freeze the screen, select an area, then copy it, read its text or save it.
    /// Needs the Aujar UI running. Also `snip shortcut-install`.
    Snip {
        #[command(subcommand)]
        action: Option<SnipAction>,
    },

    /// Text Extractor: select part of the screen and copy the text in it (OCR). Also
    /// `--file PICTURE`, `text languages` and `text shortcut-install`.
    Text(text::TextArgs),

    /// Color tools.
    Color {
        #[command(subcommand)]
        action: ColorAction,
    },

    /// Resize an image without requiring a running daemon or desktop integration.
    Image {
        #[command(subcommand)]
        action: ImageAction,
    },

    /// Share one keyboard, mouse, clipboard and files between your machines.
    #[command(alias = "mwb")]
    MouseBorders {
        #[command(subcommand)]
        action: mouse_borders::MwbAction,
    },

    /// Back up databases and folders (local or any server over SSH) to this computer, with
    /// optional encryption, retention and cron scheduling.
    Backup {
        #[command(subcommand)]
        action: backup::BackupAction,
    },

    /// Keep the computer from sleeping: on, off, or for a set time.
    #[command(name = "awake")]
    Awake {
        #[command(subcommand)]
        action: awake::AwakeAction,
    },

    /// Open the NexDesk remote-access manager (RDP, remote control), or check it is installed.
    #[command(name = "nexdesk")]
    NexDesk {
        #[command(subcommand)]
        action: nexdesk::NexDeskAction,
    },

    /// Print daemon events as they happen (debugging and scripting).
    Events,

    /// Print a shell completion script (bash, zsh, fish, elvish, powershell).
    Completions {
        /// Shell to generate completions for.
        shell: Shell,
    },

    /// Run the Aujar daemon.
    Daemon {
        /// Unix socket path used by the daemon.
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Preview or execute a batch rename operation.
    Rename {
        /// Search pattern.
        #[arg(long)]
        pattern: String,

        /// Replacement text.
        #[arg(long)]
        replacement: String,

        /// Interpret the pattern as a regular expression.
        #[arg(long)]
        regex: bool,

        /// Actually execute the rename.
        #[arg(long)]
        apply: bool,

        /// Files or directories to rename.
        #[arg(required = true)]
        paths: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum ImageAction {
    /// Resize one image while preserving its aspect ratio.
    Resize {
        /// Source image path.
        input: PathBuf,

        /// Destination image path; its extension selects PNG, JPEG, WebP, etc.
        output: PathBuf,

        /// Maximum output width in pixels.
        #[arg(long)]
        width: Option<u32>,

        /// Maximum output height in pixels.
        #[arg(long)]
        height: Option<u32>,

        /// Never enlarge an image that already fits the requested bounds.
        #[arg(long)]
        shrink_only: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Silent unless asked for with -v, RUST_LOG, AUJAR_DEBUG, AUJAR_IPC_TRACE or AUJAR_LOG_FILE,
    // so ordinary command output stays clean.
    aujar_log::init_cli("aujar", cli.verbose);

    match cli.command {
        Command::Platform => print_platform_report(),

        Command::Ping => {
            let response = send(&resolve_socket(), Request::Ping).await?;
            println!("{response:?}");
        }

        Command::Health => {
            let response = send(&resolve_socket(), Request::Health).await?;

            match response {
                Response::Health(health) => print_health(&health),
                other => println!("{other:?}"),
            }
        }

        Command::Windows => {
            let response = send(&resolve_socket(), Request::Windows).await?;
            println!("{response:?}");
        }

        Command::Modules => {
            let response = send(&resolve_socket(), Request::Modules).await?;

            match response {
                Response::Modules(modules) => {
                    for module in modules.modules {
                        println!(
                            "{} {} (api v{}) - {}",
                            module.id, module.version, module.api_version, module.description
                        );
                    }
                }
                Response::Error(message) => anyhow::bail!(message),
                other => println!("{other:?}"),
            }
        }

        Command::Search { query, limit } => {
            let request = Request::Search(SearchRequest {
                query,
                limit: Some(limit),
            });

            match send(&resolve_socket(), request).await? {
                Response::Search(search) => {
                    for hit in search.results {
                        match hit.subtitle {
                            Some(subtitle) => println!(
                                "{:>5}  [{}] {} - {}  ({})",
                                hit.score, hit.kind, hit.title, subtitle, hit.id
                            ),
                            None => println!(
                                "{:>5}  [{}] {}  ({})",
                                hit.score, hit.kind, hit.title, hit.id
                            ),
                        }
                    }
                }
                Response::Error(message) => anyhow::bail!(message),
                other => println!("{other:?}"),
            }
        }

        Command::Run { id } => {
            let request = Request::Execute(ExecuteRequest { id });

            match send(&resolve_socket(), request).await? {
                Response::Executed(done) => match done.pid {
                    Some(pid) => println!("Launched {} (pid {pid})", done.title),
                    None => println!("Launched {}", done.title),
                },
                Response::Error(message) => anyhow::bail!(message),
                other => println!("{other:?}"),
            }
        }

        Command::Launcher { action } => {
            let request = Request::Launcher(action.into());

            match send(&resolve_socket(), request).await? {
                Response::Delivered { subscribers: 0 } => {
                    eprintln!("No launcher UI is running.");
                    eprintln!("Start it with: systemctl --user start aujar-ui.service");
                    eprintln!("Enable for login: systemctl --user enable aujar-ui.service");
                }
                Response::Delivered { subscribers } => {
                    println!("Delivered to {subscribers} subscriber(s).");
                }
                Response::Error(message) => anyhow::bail!(message),
                other => println!("{other:?}"),
            }
        }

        Command::Events => {
            let mut stream = subscribe(&resolve_socket()).await?;

            eprintln!("Subscribed. Waiting for events (Ctrl-C to stop).");

            while let Some(event) = stream.recv().await? {
                println!("{event:?}");

                if event == Event::Shutdown {
                    break;
                }
            }
        }

        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "aujar", &mut std::io::stdout());
        }

        Command::Doctor { dev } => doctor(dev).await?,

        Command::GnomeExtension { action } => match action {
            ExtensionAction::Install => gnome_extension::install()?,
            ExtensionAction::Remove => gnome_extension::remove()?,
            ExtensionAction::Status => gnome_extension::status()?,
        },
        Command::Shortcut { action } => match action {
            ShortcutAction::Install { keys } => shortcut::install(&keys)?,
            ShortcutAction::Remove => shortcut::remove()?,
        },

        Command::Text(args) => text::run(args, resolve_socket()).await?,

        Command::Snip { action: None } => {
            match send(&resolve_socket(), Request::Launcher(LauncherCommand::Snip)).await? {
                Response::Delivered { subscribers } if subscribers > 0 => {}
                Response::Delivered { .. } => anyhow::bail!(
                    "Aujar Snip draws its overlay in the Aujar UI, which is not running. Start it \
                     with `systemctl --user start aujar-ui` (or run `aujar-ui`)."
                ),
                Response::Error(message) => anyhow::bail!(message),
                other => println!("{other:?}"),
            }
        }

        Command::Snip {
            action: Some(SnipAction::Allow { revoke }),
        } => {
            use aujar_text_extractor::permission;

            permission::set(!revoke)?;

            if revoke {
                println!(
                    "Removed: {} must be allowed again before Aujar Snip can freeze the screen.",
                    permission::PORTAL_APP_ID
                );
            } else {
                println!(
                    "Allowed: {} may take screenshots without asking (portal permission store, \
                     table `screenshot`). Undo with `aujar snip allow --revoke`.",
                    permission::PORTAL_APP_ID
                );
            }
        }

        Command::Snip {
            action: Some(SnipAction::ShortcutInstall { keys }),
        } => {
            let keys =
                keys.unwrap_or_else(|| aujar_config::Settings::load_or_default().snip.shortcut);

            shortcut::install_snip(&keys)?;
        }

        Command::Color { action } => match action {
            ColorAction::Editor => {
                match send(
                    &resolve_socket(),
                    Request::Launcher(LauncherCommand::ColorEditor),
                )
                .await?
                {
                    Response::Delivered { subscribers: 0 } => anyhow::bail!(
                        "the Aujar UI is not running (try `systemctl --user start aujar-ui.service`)"
                    ),
                    Response::Error(message) => anyhow::bail!(message),
                    _ => {}
                }
            }
            ColorAction::Pick => {
                // The UI shows the picker. Only when nothing is listening, use the desktop's own.
                let delivered = send(
                    &resolve_socket(),
                    Request::Launcher(LauncherCommand::ColorPicker),
                )
                .await;

                if matches!(delivered, Ok(Response::Delivered { subscribers }) if subscribers > 0) {
                    return Ok(());
                }

                match send(&resolve_socket(), Request::PickColor).await? {
                    Response::Color(color) => {
                        let settings = aujar_config::Settings::load_or_default();
                        let picked = aujar_color::Rgb::parse_hex(&color.hex)
                            .unwrap_or(aujar_color::Rgb::new(0, 0, 0));
                        let format = settings.color_picker.formats.iter().find(|format| {
                            format.enabled
                                && format
                                    .name
                                    .eq_ignore_ascii_case(&settings.color_picker.default_format)
                        });
                        let copied = format.map_or_else(
                            || picked.to_hex(),
                            |format| picked.format(&format.pattern),
                        );
                        if let Err(error) = copy_to_clipboard(&copied) {
                            eprintln!("Could not copy color to clipboard: {error}");
                        }
                        println!("{}\n{}\n{}", color.hex, color.rgb, color.hsl);
                    }
                    Response::Error(message) => anyhow::bail!(message),
                    other => println!("{other:?}"),
                }
            }
            ColorAction::Convert { value } => {
                let color = aujar_color::Rgb::parse_hex(&value).ok_or_else(|| {
                    anyhow::anyhow!("`{value}` is not a hex color (try #FF8800 or f80)")
                })?;

                println!(
                    "{}\n{}\n{}",
                    color.to_hex(),
                    color.to_rgb_string(),
                    color.to_hsl_string()
                );
            }
            ColorAction::ShortcutInstall { keys } => {
                shortcut::install_color_picker(&keys)?;
            }
        },

        Command::Image { action } => match action {
            ImageAction::Resize {
                input,
                output,
                width,
                height,
                shrink_only,
            } => {
                let result = aujar_image_resizer::resize(
                    &input,
                    &output,
                    aujar_image_resizer::ResizeOptions {
                        width,
                        height,
                        shrink_only,
                    },
                )?;
                println!(
                    "Resized {} ({}×{}) -> {} ({}×{})",
                    result.input.display(),
                    result.source_width,
                    result.source_height,
                    result.output.display(),
                    result.width,
                    result.height,
                );
            }
        },

        Command::MouseBorders { action } => mouse_borders::run(action, resolve_socket()).await?,

        Command::NexDesk { action } => nexdesk::run(action, resolve_socket()).await?,

        Command::Backup { action } => backup::run(action)?,

        Command::Awake { action } => awake::run(action, resolve_socket()).await?,

        Command::Daemon { socket } => {
            let socket_path = socket.unwrap_or_else(resolve_socket);
            run_daemon(socket_path).await?;
        }

        Command::Rename {
            pattern,
            replacement,
            regex,
            apply,
            paths,
        } => {
            let request = Request::Rename(RenameRequest {
                paths,
                pattern,
                replacement,
                regex,
                apply,
            });

            let response = send(&resolve_socket(), request).await?;

            match response {
                Response::Rename(rename_response) => {
                    print_rename_response(&rename_response.plan, rename_response.applied);
                }

                Response::Error(message) => {
                    anyhow::bail!(message);
                }

                other => {
                    println!("{other:?}");
                }
            }
        }
    }

    Ok(())
}

fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[cfg(target_os = "linux")]
    let candidates: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    #[cfg(not(target_os = "linux"))]
    let candidates: &[(&str, &[&str])] = &[];

    for (program, args) in candidates {
        let mut child = match Command::new(program)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|error| error.to_string())?;
        }
        return child
            .wait()
            .map_err(|error| error.to_string())?
            .success()
            .then_some(())
            .ok_or_else(|| format!("{program} exited unsuccessfully"));
    }
    Err("install wl-clipboard, xclip or xsel".to_owned())
}

async fn run_daemon(socket: PathBuf) -> Result<()> {
    if let Err(error) = aujar_daemon::run(&socket).await {
        if error
            .downcast_ref::<aujar_daemon::AlreadyRunning>()
            .is_some()
        {
            eprintln!("aujar: {error}");
            std::process::exit(aujar_daemon::EXIT_ALREADY_RUNNING);
        }

        return Err(error);
    }

    Ok(())
}

async fn doctor(dev: bool) -> anyhow::Result<()> {
    let result = doctor_report().await;

    if dev {
        print_dev_info();
    }

    result
}

/// The extra section of `aujar doctor --dev`: where this program looks for things and which
/// settings change its behaviour. Everything here is local; nothing is sent anywhere.
fn print_dev_info() {
    println!();
    println!("--- development ---");

    if let Ok(exe) = std::env::current_exe() {
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };

        println!("binary:     {} ({profile} build)", exe.display());
    }

    println!("version:    {}", env!("CARGO_PKG_VERSION"));

    let socket = resolve_socket();
    let source = if std::env::var_os("AUJAR_SOCKET").is_some() {
        "AUJAR_SOCKET"
    } else if std::env::var_os("XDG_RUNTIME_DIR").is_some() {
        "XDG_RUNTIME_DIR"
    } else {
        "the built-in fallback"
    };

    println!(
        "socket:     {} (from {source}; exists: {})",
        socket.display(),
        socket.exists()
    );

    println!(
        "settings:   {}",
        aujar_config::platform::config_dir().display()
    );
    println!(
        "state:      {}",
        aujar_config::platform::state_dir().display()
    );

    match aujar_log::log_file_path("aujar-daemon") {
        Some(path) => println!(
            "log files:  {} (AUJAR_LOG_FILE is set; the UI writes aujar-ui.log next to it)",
            path.display()
        ),
        None => println!("log files:  none (set AUJAR_LOG_FILE=1 to also log to a file)"),
    }

    for name in [
        "AUJAR_SOCKET",
        "AUJAR_NO_HOTKEYS",
        "AUJAR_DEBUG",
        "AUJAR_IPC_TRACE",
        "AUJAR_LOG_FILE",
        "RUST_LOG",
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
    ] {
        if let Some(value) = std::env::var_os(name) {
            println!("env:        {name}={}", value.to_string_lossy());
        }
    }

    #[cfg(target_os = "linux")]
    {
        for unit in ["aujar.service", "aujar-ui.service"] {
            let state = std::process::Command::new("systemctl")
                .args(["--user", "is-active", unit])
                .output()
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
                .unwrap_or_else(|_| "unknown (systemctl not found)".to_owned());

            println!("service:    {unit}: {state}");
        }

        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
            let installed = PathBuf::from(runtime).join(DEFAULT_SOCKET_NAME);

            if installed != socket && installed.exists() {
                println!(
                    "note:       a daemon also listens on {}; this command talks to the socket above",
                    installed.display()
                );
            }
        }
    }
}

async fn doctor_report() -> anyhow::Result<()> {
    let socket = resolve_socket();

    println!("socket:     {}", socket.display());
    // Local: Text Extractor's Tesseract does not depend on the daemon running.
    println!("ocr:        {}", ocr_report().await);
    println!("snip:       {}", snip_report());

    let health = match send(&socket, Request::Health).await {
        Ok(Response::Health(health)) => health,
        Ok(other) => anyhow::bail!("unexpected response: {other:?}"),
        Err(error) => {
            println!("daemon:     NOT REACHABLE ({error:#})");
            println!("\nStart it with `systemctl --user start aujar` (or run `aujar daemon`).");

            return Ok(());
        }
    };

    println!("daemon:     running, v{}", health.version);
    println!("session:    {:?}", health.session);
    println!("protocol:   {}", protocol_report(&socket).await);

    let Ok(Response::Status(status)) = send(&socket, Request::Status).await else {
        println!("status:     the daemon is older than this client; restart it");

        return Ok(());
    };

    println!("UI clients: {}", status.subscribers);
    println!("hotkey:     {}", status.hotkeys);
    println!();

    if status.subscribers == 0 {
        println!(
            "! No UI is subscribed. A shortcut can only show a UI that is running: start `aujar-ui`."
        );
    }

    if status.hotkeys.contains("app id") {
        println!(
            "! The desktop could not identify Aujar (\"app id required\"). See docs/TROUBLESHOOTING.md."
        );
    }

    if !status.hotkeys.starts_with("bound") {
        let exe = std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "aujar".to_owned());

        println!("! The native (portal) shortcut is not active. On GNOME, run:");
        println!("    aujar shortcut install");
        println!("  On other desktops, bind this command in the keyboard settings:");
        println!("    {exe} launcher toggle");
    }

    Ok(())
}

/// Whether the desktop lets Aujar take the snip overlay's screenshot without asking.
fn snip_report() -> String {
    match aujar_text_extractor::permission::status() {
        Ok(Some(true)) => "screenshots allowed without asking".to_owned(),
        Ok(Some(false)) => "screenshots refused; run `aujar snip allow` to allow them".to_owned(),
        Ok(None) => "screenshots not allowed yet (GNOME); run `aujar snip allow` once".to_owned(),
        Err(error) => format!("could not read the permission ({error:#}); fine outside GNOME"),
    }
}

/// One line on Text Extractor's Tesseract: version and languages, or how to install it.
async fn ocr_report() -> String {
    match tokio::task::spawn_blocking(aujar_text_extractor::ocr::status).await {
        Ok(Ok(status)) => format!("{} ({})", status.version, status.languages.join(", ")),
        Ok(Err(error)) => format!("{error:#}"),
        Err(error) => format!("could not check Tesseract: {error}"),
    }
}

/// One line on IPC protocol compatibility between this program and the daemon.
async fn protocol_report(socket: &std::path::Path) -> String {
    use aujar_ipc::{PROTOCOL_VERSION, VersionResponse};

    match send(socket, Request::Version).await {
        Ok(Response::Version(daemon)) => describe_protocol(&daemon, PROTOCOL_VERSION),
        // Older daemons do not know `Version` and only speak v1.
        Ok(Response::Error(_)) => describe_protocol(
            &VersionResponse {
                protocol: 1,
                min_protocol: 1,
                daemon: "older release".to_owned(),
            },
            PROTOCOL_VERSION,
        ),
        Ok(other) => format!("unexpected answer {other:?}"),
        Err(error) => format!("{error:#}"),
    }
}

fn describe_protocol(daemon: &aujar_ipc::VersionResponse, ours: u16) -> String {
    let range = if daemon.min_protocol == daemon.protocol {
        format!("v{}", daemon.protocol)
    } else {
        format!("v{} to v{}", daemon.min_protocol, daemon.protocol)
    };

    if daemon.accepts(ours) {
        format!("v{ours} (daemon speaks {range}), compatible")
    } else {
        format!(
            "MISMATCH: this program speaks v{ours}, the daemon {range}. Install the same Aujar \
             release for both and run `systemctl --user restart aujar`"
        )
    }
}

fn resolve_socket() -> PathBuf {
    if let Some(socket) = std::env::var_os("AUJAR_SOCKET") {
        return PathBuf::from(socket);
    }

    #[cfg(windows)]
    {
        return PathBuf::from(WINDOWS_PIPE_NAME);
    }

    #[cfg(not(windows))]
    {
        if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            return PathBuf::from(runtime_dir).join(DEFAULT_SOCKET_NAME);
        }

        PathBuf::from(FALLBACK_SOCKET)
    }
}

fn print_health(health: &Health) {
    let session = match health.session {
        Session::X11 => "X11",
        Session::Wayland => "Wayland",
        Session::MacOs => "macOS",
        Session::Windows => "Windows",
        Session::Unknown => "Unknown",
    };

    println!(
        "Health(name={}, version={}, platform={}, session={})",
        health.name,
        health.version,
        health.platform.as_str(),
        session
    );
}

fn print_platform_report() {
    let platform = aujar_platform::current_platform();
    let session = aujar_platform::detect_session();

    println!("Platform: {} ({session:?})", platform.as_str());
    println!("Feature                         Support");

    for support in aujar_platform::feature_support(platform) {
        println!(
            "{:<31} {}",
            support.feature.as_str(),
            support.level.as_str()
        );
    }
}

fn print_rename_response(plan: &aujar_rename::RenamePlan, applied: bool) {
    if applied {
        println!("Rename completed.");
    } else {
        println!("Rename preview:");
    }

    println!();

    for item in &plan.items {
        println!(
            "{:?}: {} -> {}",
            item.status,
            item.source.display(),
            item.target.display()
        );
    }

    println!();
    println!("Items: {}", plan.items.len());

    if plan.has_errors() {
        println!("Result: blocked by validation errors.");
    } else if applied {
        println!("Result: applied.");
    } else {
        println!("Result: ready to apply.");
        println!("Use --apply to execute the rename.");
    }
}
