use anyhow::Result;
use aujar_core::{Health, Session};
use aujar_ipc::{
    Event, ExecuteRequest, LauncherCommand, RenameRequest, Request, Response, SearchRequest, send,
    subscribe,
};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use std::path::PathBuf;

mod gnome_extension;
mod mouse_borders;
mod nexdesk;
mod shortcut;

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
    /// Pick a color from the screen with the desktop's color picker.
    Pick,

    /// Show a hex color in every supported format.
    Convert {
        /// A hex color such as `#FF8800` or `f80`.
        value: String,
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
    Doctor,

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

        Command::Doctor => doctor().await?,

        Command::GnomeExtension { action } => match action {
            ExtensionAction::Install => gnome_extension::install()?,
            ExtensionAction::Remove => gnome_extension::remove()?,
            ExtensionAction::Status => gnome_extension::status()?,
        },
        Command::Shortcut { action } => match action {
            ShortcutAction::Install { keys } => shortcut::install(&keys)?,
            ShortcutAction::Remove => shortcut::remove()?,
        },

        Command::Color { action } => match action {
            ColorAction::Pick => match send(&resolve_socket(), Request::PickColor).await? {
                Response::Color(color) => println!("{}\n{}\n{}", color.hex, color.rgb, color.hsl),
                Response::Error(message) => anyhow::bail!(message),
                other => println!("{other:?}"),
            },
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

async fn doctor() -> anyhow::Result<()> {
    let socket = resolve_socket();

    println!("socket:     {}", socket.display());

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
