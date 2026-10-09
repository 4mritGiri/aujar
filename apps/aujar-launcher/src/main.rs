use anyhow::Result;
use aujar_core::{Health, Session};
use aujar_ipc::{
    Event, ExecuteRequest, LauncherCommand, RenameRequest, Request, Response, SearchRequest, send,
    subscribe,
};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

const DEFAULT_SOCKET_NAME: &str = "aujar.sock";
const FALLBACK_SOCKET: &str = "/tmp/aujar.sock";

#[derive(Debug, Parser)]
#[command(
    name = "aujar",
    version,
    about = "Aujar — Native Linux Productivity Platform"
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
}

impl From<LauncherAction> for LauncherCommand {
    fn from(action: LauncherAction) -> Self {
        match action {
            LauncherAction::Show => Self::Show,
            LauncherAction::Hide => Self::Hide,
            LauncherAction::Toggle => Self::Toggle,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
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

    /// Print daemon events as they happen (debugging and scripting).
    Events,

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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
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
                    eprintln!("No launcher UI is subscribed yet (0 subscribers).");
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

        Command::Daemon { socket } => {
            let socket_path = socket.unwrap_or_else(resolve_socket);

            aujar_daemon::run(&socket_path).await?;
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

fn resolve_socket() -> PathBuf {
    if let Some(socket) = std::env::var_os("AUJAR_SOCKET") {
        return PathBuf::from(socket);
    }

    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime_dir).join(DEFAULT_SOCKET_NAME);
    }

    PathBuf::from(FALLBACK_SOCKET)
}

fn print_health(health: &Health) {
    let session = match health.session {
        Session::X11 => "X11",
        Session::Wayland => "Wayland",
        Session::Unknown => "Unknown",
    };

    println!(
        "Health(name={}, version={}, session={})",
        health.name, health.version, session
    );
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
