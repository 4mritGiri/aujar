use anyhow::Result;
use clap::{Parser, Subcommand};
use powertoys_core::{Request, Response};
use powertoys_ipc::send;
use std::path::PathBuf;
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}
#[derive(Subcommand)]
enum Cmd {
    Status,
    Color {
        value: String,
    },
    Rename {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        files: Vec<String>,
    },
}
fn socket() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("aujar.sock")
}
#[tokio::main]
async fn main() -> Result<()> {
    let c = Cli::parse();
    let q = match c.command {
        Cmd::Status => Request::Status,
        Cmd::Color { value } => Request::Color { value },
        Cmd::Rename { from, to, files } => Request::Rename { from, to, files },
    };
    match send(&socket(), &q).await? {
        Response::Ok { message } | Response::Error { message } => println!("{message}"),
    }
    Ok(())
}
