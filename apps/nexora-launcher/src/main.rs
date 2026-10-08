use anyhow::Result;
use clap::{Parser, Subcommand};
use nexora_core::{Health, Session};
use nexora_ipc::{RenameRequest, Request, Response, send};
use std::path::PathBuf;

const DEFAULT_SOCKET_NAME: &str = "nexora.sock";
const FALLBACK_SOCKET: &str = "/tmp/nexora.sock";

#[derive(Debug, Parser)]
#[command(
    name = "nexora",
    version,
    about = "Nexora — Native Linux Productivity Platform"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check whether the Nexora daemon is reachable.
    Ping,

    /// Display Nexora runtime health information.
    Health,

    /// List available windows.
    Windows,

    /// Run the Nexora daemon.
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

        Command::Daemon { socket } => {
            let socket_path = socket.unwrap_or_else(resolve_socket);

            nexora_daemon::run(&socket_path).await?;
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
    if let Some(socket) = std::env::var_os("NEXORA_SOCKET") {
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

fn print_rename_response(plan: &nexora_rename::RenamePlan, applied: bool) {
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
