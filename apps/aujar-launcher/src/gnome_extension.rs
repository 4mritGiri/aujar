//! Install the Aujar Windows GNOME Shell extension (see `packaging/gnome-extension`).
//!
//! The files are embedded in the binary, so this works from a packaged install. GNOME on
//! Wayland only loads newly installed extensions at login, which the output says.

use anyhow::{Context, Result, bail};
use std::{path::PathBuf, process::Command};

const UUID: &str = "aujar-windows@aujar.dev";
const METADATA: &str = include_str!("../../../packaging/gnome-extension/aujar-windows@aujar.dev/metadata.json");
const EXTENSION_JS: &str = include_str!("../../../packaging/gnome-extension/aujar-windows@aujar.dev/extension.js");

fn target_dir() -> Result<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .context("neither XDG_DATA_HOME nor HOME is set")?;

    Ok(data.join("gnome-shell/extensions").join(UUID))
}

/// Run `gnome-extensions <args>`; `None` when the tool is missing.
fn gnome_extensions(args: &[&str]) -> Option<std::process::Output> {
    Command::new("gnome-extensions").args(args).output().ok()
}

pub fn install() -> Result<()> {
    let dir = target_dir()?;

    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    std::fs::write(dir.join("metadata.json"), METADATA)?;
    std::fs::write(dir.join("extension.js"), EXTENSION_JS)?;

    println!("Installed the extension to {}", dir.display());

    match gnome_extensions(&["enable", UUID]) {
        Some(output) if output.status.success() => {
            println!("Enabled {UUID}.");
        }
        _ => println!("Could not enable it yet (GNOME has not seen it); this is normal before the next login."),
    }

    println!(
        "Log out and back in, then run `aujar gnome-extension status`.\n\
         (X11 sessions can instead press Alt+F2, type `r`, and press Enter.)"
    );

    Ok(())
}

pub fn remove() -> Result<()> {
    let _ = gnome_extensions(&["disable", UUID]);

    let dir = target_dir()?;

    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("could not remove {}", dir.display()))?;
        println!("Removed {}", dir.display());
    } else {
        println!("The extension is not installed.");
    }

    Ok(())
}

pub fn status() -> Result<()> {
    let dir = target_dir()?;

    println!("installed: {}", if dir.join("extension.js").exists() { "yes" } else { "no" });

    match gnome_extensions(&["info", UUID]) {
        Some(output) if output.status.success() => {
            let info = String::from_utf8_lossy(&output.stdout);
            let state = info
                .lines()
                .find_map(|line| line.trim().strip_prefix("State:"))
                .map_or("unknown", str::trim);

            println!("shell state: {state}");
        }
        Some(_) => println!("shell state: not loaded (log out and back in after installing)"),
        None => println!("shell state: `gnome-extensions` is not installed"),
    }

    let reply = Command::new("busctl")
        .args([
            "--user",
            "--timeout=2",
            "call",
            aujar_launcher::GNOME_BUS_NAME,
            aujar_launcher::GNOME_OBJECT_PATH,
            aujar_launcher::GNOME_BUS_NAME,
            "List",
        ])
        .output();

    match reply {
        Ok(output) if output.status.success() => println!("window service: answering"),
        Ok(_) => bail!("window service: not answering (is the extension enabled and the session restarted?)"),
        Err(error) => bail!("window service: could not run busctl: {error}"),
    }

    Ok(())
}
