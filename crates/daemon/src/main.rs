use anyhow::Result;
use powertoys_config::load;
use powertoys_core::{Request, Response, app_info};
use powertoys_platform::session;
use std::{fs, path::PathBuf};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("nexora.sock")
}
async fn handle(s: UnixStream) -> Result<()> {
    let (r, mut w) = s.into_split();
    let mut lines = BufReader::new(r).lines();
    while let Some(line) = lines.next_line().await? {
        let req: Request = serde_json::from_str(&line)?;
        let res = match req {
            Request::Status => Response::Ok {
                message: format!(
                    "{} {} | session={:?}",
                    app_info().name,
                    app_info().version,
                    session()
                ),
            },
            Request::Color { value } => Response::Ok {
                message: color(&value),
            },
            Request::Rename { from, to, files } => {
                let mut out = Vec::new();
                for f in files {
                    out.push(f.replace(&from, &to));
                }
                Response::Ok {
                    message: out.join("\n"),
                }
            }
        };
        w.write_all(serde_json::to_string(&res)?.as_bytes()).await?;
        w.write_all(b"\n").await?;
    }
    Ok(())
}
fn color(v: &str) -> String {
    let x = v.trim().trim_start_matches('#');
    if x.len() != 6 {
        return "Invalid color: expected #RRGGBB".into();
    }
    match u32::from_str_radix(x, 16) {
        Ok(n) => format!(
            "RGB({}, {}, {}) | #{x}",
            (n >> 16) & 255,
            (n >> 8) & 255,
            n & 255
        ),
        Err(_) => "Invalid hexadecimal color".into(),
    }
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    let _ = load()?;
    let p = socket_path();
    let _ = fs::remove_file(&p);
    let l = UnixListener::bind(&p)?;
    println!("Linux PowerToys daemon listening on {}", p.display());
    loop {
        let (s, _) = l.accept().await?;
        tokio::spawn(async move {
            if let Err(e) = handle(s).await {
                eprintln!("client error: {e}");
            }
        });
    }
}
