use anyhow::Result;
use clap::Parser;
use aujar_daemon::Daemon;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;
#[derive(Parser)] struct Args { #[arg(long)] socket: Option<PathBuf> }
#[tokio::main] async fn main()->Result<()>{tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env().add_directive("aujar_daemon=info".parse()?)).init(); let args=Args::parse(); let socket=args.socket.unwrap_or_else(default_socket); Daemon::new(socket).run().await}
fn default_socket()->PathBuf{std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(||PathBuf::from("/tmp")).join("aujar.sock")}
