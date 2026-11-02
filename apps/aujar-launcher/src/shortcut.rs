//! Install a GNOME custom keyboard shortcut that runs `aujar launcher toggle`.
//!
//! This uses GNOME's own settings (`gsettings`), so it needs no portal and no app id,
//! unlike the native `GlobalShortcuts` portal route, which GNOME only offers to
//! applications it can identify.

use anyhow::{Context, Result, bail};
use std::process::Command;

const SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
const LIST_KEY: &str = "custom-keybindings";
const ENTRY_PATH: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/aujar/";
const COLOR_ENTRY_PATH: &str =
    "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/aujar-color/";

/// Default key combination in GNOME's accelerator syntax.
pub const DEFAULT_KEYS: &str = "<Control><Alt>space";

fn gsettings(args: &[&str]) -> Result<String> {
    let output = Command::new("gsettings")
        .args(args)
        .output()
        .context("could not run `gsettings` (is this a GNOME session?)")?;

    if !output.status.success() {
        bail!(
            "`gsettings {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Parse a GVariant string array as printed by `gsettings get`:
/// `['a', 'b']` or `@as []`.
pub fn parse_list(raw: &str) -> Vec<String> {
    let raw = raw.trim().trim_start_matches("@as").trim();
    let inner = raw.trim_start_matches('[').trim_end_matches(']');

    inner
        .split(',')
        .map(|item| item.trim().trim_matches('\'').trim_matches('"'))
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Format a string array in the syntax `gsettings set` expects.
pub fn format_list(items: &[String]) -> String {
    if items.is_empty() {
        return "@as []".to_owned();
    }

    let quoted: Vec<String> = items.iter().map(|item| format!("'{item}'")).collect();

    format!("[{}]", quoted.join(", "))
}

fn toggle_command() -> Result<String> {
    let exe = std::env::current_exe().context("could not determine the path of `aujar`")?;
    let exe = exe.display().to_string();

    // GNOME splits the command like a shell; quote paths containing spaces.
    let exe = if exe.contains(char::is_whitespace) {
        format!("'{exe}'")
    } else {
        exe
    };

    Ok(format!("{exe} launcher toggle"))
}

/// Encode a string as a GVariant string literal for `gsettings set`.
/// `gsettings` parses its final argument as a variant, so a command containing
/// spaces must be quoted as a value even though it is passed as one argv item.
fn variant_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn install(keys: &str) -> Result<()> {
    let command = toggle_command()?;

    let mut items = parse_list(&gsettings(&["get", SCHEMA, LIST_KEY])?);

    if !items.iter().any(|item| item == ENTRY_PATH) {
        items.push(ENTRY_PATH.to_owned());
    }

    gsettings(&["set", SCHEMA, LIST_KEY, &format_list(&items)])?;

    let entry = format!("{SCHEMA}.custom-keybinding:{ENTRY_PATH}");

    gsettings(&["set", &entry, "name", "Aujar launcher"])?;
    let command_value = variant_string(&command);
    gsettings(&["set", &entry, "command", &command_value])?;
    gsettings(&["set", &entry, "binding", keys])?;

    println!("Shortcut installed: {keys} runs `{command}`.");
    println!("The launcher UI must be running for it to show anything (see `aujar doctor`).");

    Ok(())
}

/// Install a second GNOME custom shortcut for the screen color picker.
pub fn install_color_picker(keys: &str) -> Result<()> {
    let binding = gnome_binding(keys)?;
    let executable = std::env::current_exe().context("could not determine the path of `aujar`")?;
    let executable = executable.display().to_string();
    let command = format!("'{}' color pick", executable.replace('\'', "'\\''"));
    let mut items = parse_list(&gsettings(&["get", SCHEMA, LIST_KEY])?);
    if !items.iter().any(|item| item == COLOR_ENTRY_PATH) {
        items.push(COLOR_ENTRY_PATH.to_owned());
    }
    gsettings(&["set", SCHEMA, LIST_KEY, &format_list(&items)])?;
    let entry = format!("{SCHEMA}.custom-keybinding:{COLOR_ENTRY_PATH}");
    gsettings(&["set", &entry, "name", "Aujar Color Picker"])?;
    let command_value = variant_string(&command);
    gsettings(&["set", &entry, "command", &command_value])?;
    gsettings(&["set", &entry, "binding", &binding])?;

    let saved_binding = gsettings(&["get", &entry, "binding"])?;
    if !saved_binding.contains(&binding) {
        bail!("GNOME did not save the color picker shortcut: {saved_binding}");
    }

    println!("Color picker shortcut installed: {keys} runs `{command}`.");
    Ok(())
}

fn gnome_binding(shortcut: &str) -> Result<String> {
    let mut binding = String::new();
    let mut parts = shortcut.split('+').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            if part.chars().count() != 1 || !part.chars().all(|c| c.is_ascii_alphanumeric()) {
                bail!("shortcut must end with one letter or number");
            }
            binding.push(part.to_ascii_lowercase().chars().next().unwrap_or('c'));
            continue;
        }
        binding.push_str(match part.to_ascii_uppercase().as_str() {
            "SUPER" | "WIN" => "<Super>",
            "CTRL" | "CONTROL" => "<Control>",
            "ALT" => "<Alt>",
            "SHIFT" => "<Shift>",
            _ => bail!("unsupported shortcut modifier `{part}`"),
        });
    }
    Ok(binding)
}

pub fn remove() -> Result<()> {
    let mut items = parse_list(&gsettings(&["get", SCHEMA, LIST_KEY])?);

    items.retain(|item| item != ENTRY_PATH);

    gsettings(&["set", SCHEMA, LIST_KEY, &format_list(&items)])?;

    let entry = format!("{SCHEMA}.custom-keybinding:{ENTRY_PATH}");
    gsettings(&["reset-recursively", &entry])?;

    println!("Aujar shortcut removed.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gsettings_lists() {
        assert!(parse_list("@as []").is_empty());
        assert!(parse_list("[]").is_empty());
        assert_eq!(parse_list("['/a/']"), ["/a/"]);
        assert_eq!(parse_list("['/a/', '/b/']"), ["/a/", "/b/"]);
    }

    #[test]
    fn formats_gsettings_lists() {
        assert_eq!(format_list(&[]), "@as []");
        assert_eq!(format_list(&["/a/".into(), "/b/".into()]), "['/a/', '/b/']");
    }

    #[test]
    fn round_trips() {
        let items = vec!["/x/".to_owned(), ENTRY_PATH.to_owned()];

        assert_eq!(parse_list(&format_list(&items)), items);
    }
}
