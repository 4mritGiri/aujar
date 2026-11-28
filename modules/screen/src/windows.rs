//! Where the visible windows are, for Aujar Snip's window mode (hover a window, click to take
//! exactly it). Read at the moment the screen is captured, so the rectangles match the picture.
//!
//! | Session | Source |
//! |---|---|
//! | GNOME | the Aujar Shell extension (`busctl ... dev.aujar.Windows List`, with geometry) |
//! | Hyprland | `hyprctl clients -j` and `hyprctl activeworkspace -j` |
//! | Sway | `swaymsg -t get_tree` |
//!
//! Best effort: anything missing (an older extension without geometry, another desktop) gives no
//! windows, and the overlay simply has no window mode.

use serde_json::Value;
use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(2);

/// A window on screen, in the desktop's logical pixels (the same space as its display layout).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRect {
    pub title: String,
    pub app: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Higher is drawn on top.
    pub stack: i32,
}

/// Visible windows, topmost first. Aujar's own windows are left out.
pub fn visible_windows() -> Vec<WindowRect> {
    #[cfg(windows)]
    let mut windows = crate::win::visible_windows();
    #[cfg(target_os = "macos")]
    let mut windows = crate::macos::visible_windows();
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut windows = desktop_windows();

    windows.retain(|window| {
        window.width > 0 && window.height > 0 && !window.app.to_lowercase().contains("aujar")
    });
    windows.sort_by_key(|window| std::cmp::Reverse(window.stack));

    windows
}

/// Linux: from the compositor (Hyprland, Sway) or the Aujar Shell extension (GNOME).
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn desktop_windows() -> Vec<WindowRect> {
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());

    if var("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        let active = run("hyprctl", &["activeworkspace", "-j"])
            .and_then(|out| serde_json::from_str::<Value>(&out).ok())
            .and_then(|value| value.get("id").and_then(Value::as_i64));

        run("hyprctl", &["clients", "-j"])
            .map(|out| parse_hyprland(&out, active))
            .unwrap_or_default()
    } else if var("SWAYSOCK").is_some() {
        run("swaymsg", &["-t", "get_tree"])
            .map(|out| parse_sway(&out))
            .unwrap_or_default()
    } else if var("XDG_CURRENT_DESKTOP")
        .is_some_and(|desktop| desktop.to_uppercase().contains("GNOME"))
    {
        run(
            "busctl",
            &[
                "--user",
                "--json=short",
                "--timeout=2",
                "call",
                "dev.aujar.Windows",
                "/dev/aujar/Windows",
                "dev.aujar.Windows",
                "List",
            ],
        )
        .map(|out| parse_gnome(&out))
        .unwrap_or_default()
    } else {
        Vec::new()
    }
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();

    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });

    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    reader.join().ok()
}

fn int(value: &Value, key: &str) -> Option<i32> {
    value.get(key)?.as_i64().and_then(|n| i32::try_from(n).ok())
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// `busctl --json=short` output: `{"type":"s","data":["<JSON array>"]}`. Windows without
/// geometry (an extension from before Aujar Snip) are skipped.
pub fn parse_gnome(output: &str) -> Vec<WindowRect> {
    let Some(list) = serde_json::from_str::<Value>(output)
        .ok()
        .and_then(|reply| reply.get("data")?.get(0)?.as_str().map(str::to_owned))
        .and_then(|inner| serde_json::from_str::<Vec<Value>>(&inner).ok())
    else {
        return Vec::new();
    };

    list.iter()
        .filter(|window| window.get("visible").and_then(Value::as_bool) == Some(true))
        .filter_map(|window| {
            Some(WindowRect {
                title: text(window, "title"),
                app: text(window, "app"),
                x: int(window, "x")?,
                y: int(window, "y")?,
                width: int(window, "width")?,
                height: int(window, "height")?,
                stack: int(window, "stack").unwrap_or(0),
            })
        })
        .collect()
}

/// `hyprctl clients -j`: windows on the active workspace that are mapped and not hidden. The
/// most recently focused is on top (`focusHistoryID` 0).
pub fn parse_hyprland(output: &str, active_workspace: Option<i64>) -> Vec<WindowRect> {
    let Ok(list) = serde_json::from_str::<Vec<Value>>(output) else {
        return Vec::new();
    };

    list.iter()
        .filter(|client| client.get("mapped").and_then(Value::as_bool) != Some(false))
        .filter(|client| client.get("hidden").and_then(Value::as_bool) != Some(true))
        .filter(|client| {
            let workspace = client
                .get("workspace")
                .and_then(|w| w.get("id"))
                .and_then(Value::as_i64);

            active_workspace.is_none() || workspace == active_workspace
        })
        .filter_map(|client| {
            let at = client.get("at")?.as_array()?;
            let size = client.get("size")?.as_array()?;
            let n = |values: &Vec<Value>, i: usize| {
                values.get(i)?.as_i64().and_then(|n| i32::try_from(n).ok())
            };

            Some(WindowRect {
                title: text(client, "title"),
                app: text(client, "class"),
                x: n(at, 0)?,
                y: n(at, 1)?,
                width: n(size, 0)?,
                height: n(size, 1)?,
                stack: -int(client, "focusHistoryID").unwrap_or(1000),
            })
        })
        .collect()
}

/// `swaymsg -t get_tree`: visible application windows (tiled, then floating on top).
pub fn parse_sway(output: &str) -> Vec<WindowRect> {
    fn walk(node: &Value, floating: bool, out: &mut Vec<WindowRect>) {
        let is_window =
            node.get("pid").is_some() && node.get("visible").and_then(Value::as_bool) == Some(true);

        if is_window && let Some(rect) = node.get("rect") {
            let app = node
                .get("app_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    node.get("window_properties")
                        .and_then(|p| p.get("class"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_default();

            if let (Some(x), Some(y), Some(width), Some(height)) = (
                int(rect, "x"),
                int(rect, "y"),
                int(rect, "width"),
                int(rect, "height"),
            ) {
                out.push(WindowRect {
                    title: text(node, "name"),
                    app,
                    x,
                    y,
                    width,
                    height,
                    stack: i32::from(floating),
                });
            }
        }

        for (key, floating_child) in [("nodes", floating), ("floating_nodes", true)] {
            for child in node
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                walk(child, floating_child, out);
            }
        }
    }

    let mut out = Vec::new();

    if let Ok(tree) = serde_json::from_str::<Value>(output) {
        walk(&tree, false, &mut out);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_windows_with_geometry_only_and_visible_only() {
        let inner = r#"[
          {"id":1,"title":"Terminal","app":"Ptyxis","x":100,"y":50,"width":800,"height":600,"visible":true,"stack":3},
          {"id":2,"title":"Hidden","app":"Files","x":0,"y":0,"width":500,"height":400,"visible":false,"stack":1},
          {"id":3,"title":"Old extension","app":"Zed","workspace":0}
        ]"#;
        let output = serde_json::json!({"type": "s", "data": [inner]}).to_string();

        assert_eq!(
            parse_gnome(&output),
            [WindowRect {
                title: "Terminal".into(),
                app: "Ptyxis".into(),
                x: 100,
                y: 50,
                width: 800,
                height: 600,
                stack: 3
            }]
        );
        assert!(parse_gnome("not json").is_empty());
    }

    #[test]
    fn hyprland_active_workspace_and_focus_order() {
        let output = r#"[
          {"at":[10,20],"size":[300,200],"title":"a","class":"kitty","mapped":true,"hidden":false,"workspace":{"id":1},"focusHistoryID":1},
          {"at":[50,60],"size":[300,200],"title":"b","class":"firefox","mapped":true,"hidden":false,"workspace":{"id":1},"focusHistoryID":0},
          {"at":[0,0],"size":[100,100],"title":"c","class":"x","mapped":true,"hidden":false,"workspace":{"id":2},"focusHistoryID":2}
        ]"#;
        let windows = parse_hyprland(output, Some(1));

        assert_eq!(windows.len(), 2);
        let top = windows.iter().max_by_key(|w| w.stack).unwrap();
        assert_eq!(top.app, "firefox");
    }

    #[test]
    fn sway_visible_leaves_with_floating_on_top() {
        let output = r#"{"nodes":[{"nodes":[
            {"pid":1,"visible":true,"name":"tiled","app_id":"foot","rect":{"x":0,"y":0,"width":960,"height":1080}},
            {"pid":2,"visible":false,"name":"other ws","app_id":"foot","rect":{"x":0,"y":0,"width":960,"height":1080}}
          ],"floating_nodes":[
            {"pid":3,"visible":true,"name":"float","window_properties":{"class":"Gimp"},"rect":{"x":100,"y":100,"width":400,"height":300}}
          ]}]}"#;
        let windows = parse_sway(output);

        assert_eq!(windows.len(), 2);
        assert_eq!(windows.iter().find(|w| w.stack == 1).unwrap().app, "Gimp");
    }
}
