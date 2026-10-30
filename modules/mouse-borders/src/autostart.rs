//! Starting the agent when the user logs in, on each platform.
//!
//! The *plan* for each OS (which file to write, which command to run) is built by pure
//! functions and tested everywhere; applying it just writes the files and runs the commands
//! (never through a shell).
//!
//! | OS      | Mechanism                                                        |
//! |---------|------------------------------------------------------------------|
//! | Linux   | a systemd **user** unit, `~/.config/systemd/user/aujar-mwb.service` |
//! | macOS   | a LaunchAgent, `~/Library/LaunchAgents/com.aujar.mwb.plist`       |
//! | Windows | a value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` |

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use aujar_config::platform::{self, Os};

use crate::error::{Error, Result};

const LABEL: &str = "com.aujar.mwb";
const UNIT: &str = "aujar-mwb.service";
const RUN_VALUE: &str = "AujarMwb";
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// A file to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileToWrite {
    pub path: PathBuf,
    pub contents: String,
}

/// What enabling or disabling autostart consists of.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub files: Vec<FileToWrite>,
    /// Commands to run after writing the files, as program and arguments.
    pub commands: Vec<Vec<String>>,
    /// Files to delete (disabling).
    pub remove: Vec<PathBuf>,
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Quote a path for a systemd `ExecStart=` line.
fn systemd_quote(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");

    format!("\"{escaped}\"")
}

pub fn systemd_unit(exe: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=Aujar Mouse Borders (share one keyboard and mouse between your computers)\n\
         After=graphical-session.target network-online.target\n\
         PartOf=graphical-session.target\n\n\
         [Service]\n\
         ExecStart={} run\n\
         Restart=on-failure\n\
         RestartSec=3\n\n\
         [Install]\n\
         WantedBy=graphical-session.target\n",
        systemd_quote(&exe.display().to_string())
    )
}

pub fn launch_agent_plist(exe: &Path, log: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{LABEL}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>{}</string>\n\
         \t\t<string>run</string>\n\
         \t</array>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>KeepAlive</key>\n\
         \t<dict>\n\
         \t\t<key>SuccessfulExit</key>\n\
         \t\t<false/>\n\
         \t</dict>\n\
         \t<key>ProcessType</key>\n\
         \t<string>Interactive</string>\n\
         \t<key>StandardOutPath</key>\n\
         \t<string>{}</string>\n\
         \t<key>StandardErrorPath</key>\n\
         \t<string>{}</string>\n\
         </dict>\n\
         </plist>\n",
        escape_xml(&macos_path(exe)),
        escape_xml(&macos_path(log)),
        escape_xml(&macos_path(log)),
    )
}

/// LaunchAgent files are interpreted by macOS, even when their plan is generated in a
/// cross-platform test run. Keep their paths in the POSIX form macOS expects rather than
/// serialising Windows separators into the XML.
fn macos_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn home(os: Os, env: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> Result<PathBuf> {
    platform::home_dir_for(os, env)
        .ok_or_else(|| Error::Config("the home directory is unknown".into()))
}

fn unit_path(os: Os, env: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> PathBuf {
    platform::config_dir_for(os, env)
        .parent()
        .map(|dir| dir.join("systemd").join("user").join(UNIT))
        .unwrap_or_else(|| PathBuf::from(UNIT))
}

/// What it takes to start `exe` at login. `uid` is the numeric user id (macOS needs it to
/// address the user's launchd domain).
pub fn plan_enable(
    os: Os,
    exe: &Path,
    uid: &str,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<Plan> {
    let exe_text = exe.display().to_string();

    Ok(match os {
        Os::Linux => Plan {
            files: vec![FileToWrite {
                path: unit_path(os, env),
                contents: systemd_unit(exe),
            }],
            commands: vec![
                vec!["systemctl".into(), "--user".into(), "daemon-reload".into()],
                vec![
                    "systemctl".into(),
                    "--user".into(),
                    "enable".into(),
                    "--now".into(),
                    UNIT.into(),
                ],
            ],
            remove: Vec::new(),
        },
        Os::MacOs => {
            let home = home(os, env)?;
            let plist = home
                .join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist"));
            let log = home.join("Library/Logs/aujar-mwb.log");

            Plan {
                files: vec![FileToWrite {
                    path: plist.clone(),
                    contents: launch_agent_plist(exe, &log),
                }],
                commands: vec![vec![
                    "launchctl".into(),
                    "bootstrap".into(),
                    format!("gui/{uid}"),
                    plist.display().to_string(),
                ]],
                remove: Vec::new(),
            }
        }
        Os::Windows => Plan {
            files: Vec::new(),
            commands: vec![vec![
                "reg".into(),
                "add".into(),
                RUN_KEY.into(),
                "/v".into(),
                RUN_VALUE.into(),
                "/t".into(),
                "REG_SZ".into(),
                "/d".into(),
                format!("\"{exe_text}\" run --background"),
                "/f".into(),
            ]],
            remove: Vec::new(),
        },
    })
}

/// What it takes to stop starting at login.
pub fn plan_disable(
    os: Os,
    uid: &str,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<Plan> {
    Ok(match os {
        Os::Linux => Plan {
            files: Vec::new(),
            commands: vec![vec![
                "systemctl".into(),
                "--user".into(),
                "disable".into(),
                "--now".into(),
                UNIT.into(),
            ]],
            remove: vec![unit_path(os, env)],
        },
        Os::MacOs => {
            let home = home(os, env)?;

            Plan {
                files: Vec::new(),
                commands: vec![vec![
                    "launchctl".into(),
                    "bootout".into(),
                    format!("gui/{uid}/{LABEL}"),
                ]],
                remove: vec![
                    home.join("Library/LaunchAgents")
                        .join(format!("{LABEL}.plist")),
                ],
            }
        }
        Os::Windows => Plan {
            files: Vec::new(),
            commands: vec![vec![
                "reg".into(),
                "delete".into(),
                RUN_KEY.into(),
                "/v".into(),
                RUN_VALUE.into(),
                "/f".into(),
            ]],
            remove: Vec::new(),
        },
    })
}

fn current_uid() -> String {
    // `id -u` on Unix. Windows has no use for it.
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|text| text.trim().to_owned())
        .unwrap_or_default()
}

fn apply(plan: &Plan, ignore_command_failures: bool) -> Result<()> {
    for file in &plan.files {
        if let Some(parent) = file.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&file.path, &file.contents)?;
    }

    for command in &plan.commands {
        let (program, args) = command.split_first().expect("a command has a program");
        let status = Command::new(program)
            .args(args)
            .status()
            .map_err(|error| Error::Other(format!("cannot run `{program}`: {error}")))?;

        if !status.success() && !ignore_command_failures {
            return Err(Error::Other(format!(
                "`{}` failed ({status})",
                command.join(" ")
            )));
        }
    }

    for path in &plan.remove {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    Ok(())
}

/// Start `exe run` automatically at login.
pub fn enable(exe: &Path) -> Result<String> {
    let os = Os::current();
    let plan = plan_enable(os, exe, &current_uid(), &|name| std::env::var_os(name))?;

    apply(&plan, false)?;

    Ok(match os {
        Os::Linux => format!("Enabled: systemd user unit {UNIT}"),
        Os::MacOs => format!("Enabled: LaunchAgent {LABEL}"),
        Os::Windows => format!("Enabled: {RUN_KEY}\\{RUN_VALUE}"),
    })
}

/// Stop starting at login. Missing pieces are not an error.
pub fn disable() -> Result<String> {
    let os = Os::current();
    let plan = plan_disable(os, &current_uid(), &|name| std::env::var_os(name))?;

    apply(&plan, true)?;

    Ok("Disabled".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn linux_gets_a_systemd_user_unit() {
        let environment = env(&[("HOME", "/home/me")]);
        let plan = plan_enable(
            Os::Linux,
            Path::new("/usr/bin/aujar-mwb"),
            "1000",
            &environment,
        )
        .unwrap();

        assert_eq!(plan.files.len(), 1);
        assert_eq!(
            plan.files[0].path,
            Path::new("/home/me/.config/systemd/user/aujar-mwb.service")
        );
        assert!(
            plan.files[0]
                .contents
                .contains("ExecStart=\"/usr/bin/aujar-mwb\" run")
        );
        assert!(plan.files[0].contents.contains("Restart=on-failure"));
        assert_eq!(
            plan.commands[1].join(" "),
            "systemctl --user enable --now aujar-mwb.service"
        );
    }

    #[test]
    fn systemd_quoting_survives_spaces_and_percent_signs() {
        let unit = systemd_unit(Path::new("/opt/my apps/100%/aujar-mwb"));

        assert!(
            unit.contains("ExecStart=\"/opt/my apps/100%%/aujar-mwb\" run"),
            "{unit}"
        );
    }

    #[test]
    fn macos_gets_a_launch_agent_in_the_users_domain() {
        let environment = env(&[("HOME", "/Users/me")]);
        let plan = plan_enable(
            Os::MacOs,
            Path::new("/Applications/Aujar/aujar-mwb"),
            "501",
            &environment,
        )
        .unwrap();
        let plist = &plan.files[0];

        assert_eq!(
            plist.path,
            Path::new("/Users/me/Library/LaunchAgents/com.aujar.mwb.plist")
        );
        assert!(plist.contents.contains("<string>com.aujar.mwb</string>"));
        assert!(
            plist
                .contents
                .contains("<string>/Applications/Aujar/aujar-mwb</string>")
        );
        assert!(plist.contents.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(
            plist
                .contents
                .contains("/Users/me/Library/Logs/aujar-mwb.log")
        );
        assert_eq!(
            plan.commands[0],
            [
                "launchctl",
                "bootstrap",
                "gui/501",
                "/Users/me/Library/LaunchAgents/com.aujar.mwb.plist"
            ]
        );
    }

    #[test]
    fn plist_values_are_xml_escaped() {
        let plist = launch_agent_plist(
            Path::new("/Apps/R&D <test>/aujar-mwb"),
            Path::new("/tmp/log"),
        );

        assert!(
            plist.contains("/Apps/R&amp;D &lt;test&gt;/aujar-mwb"),
            "{plist}"
        );
        assert!(!plist.contains("R&D"));
    }

    #[test]
    fn windows_uses_the_run_key_with_a_quoted_path() {
        let plan = plan_enable(
            Os::Windows,
            Path::new(r"C:\Program Files\Aujar\aujar-mwb.exe"),
            "",
            &env(&[]),
        )
        .unwrap();

        assert!(plan.files.is_empty());
        assert_eq!(plan.commands[0][0], "reg");
        assert!(
            plan.commands[0]
                .contains(&r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run".to_owned())
        );
        assert!(
            plan.commands[0]
                .contains(&r#""C:\Program Files\Aujar\aujar-mwb.exe" run --background"#.to_owned())
        );
        assert_eq!(plan.commands[0].last().unwrap(), "/f");
    }

    #[test]
    fn disabling_undoes_each_platforms_enabling() {
        let linux = plan_disable(Os::Linux, "1000", &env(&[("HOME", "/home/me")])).unwrap();
        let mac = plan_disable(Os::MacOs, "501", &env(&[("HOME", "/Users/me")])).unwrap();
        let windows = plan_disable(Os::Windows, "", &env(&[])).unwrap();

        assert_eq!(
            linux.remove,
            [PathBuf::from(
                "/home/me/.config/systemd/user/aujar-mwb.service"
            )]
        );
        assert_eq!(
            linux.commands[0].join(" "),
            "systemctl --user disable --now aujar-mwb.service"
        );
        assert_eq!(
            mac.commands[0],
            ["launchctl", "bootout", "gui/501/com.aujar.mwb"]
        );
        assert_eq!(mac.remove.len(), 1);
        assert_eq!(windows.commands[0][1], "delete");
    }

    #[test]
    fn a_missing_home_directory_is_reported_not_guessed() {
        assert!(plan_enable(Os::MacOs, Path::new("/x"), "1", &env(&[])).is_err());
    }

    #[test]
    fn applying_a_plan_writes_files_runs_commands_and_removes_files() {
        let dir = tempfile::tempdir().unwrap();
        let written = dir.path().join("nested").join("unit");
        let doomed = dir.path().join("old");

        std::fs::write(&doomed, "x").unwrap();

        apply(
            &Plan {
                files: vec![FileToWrite {
                    path: written.clone(),
                    contents: "hello".into(),
                }],
                commands: vec![vec!["true".into()]],
                remove: vec![doomed.clone(), dir.path().join("never-existed")],
            },
            false,
        )
        .unwrap();

        assert_eq!(std::fs::read_to_string(written).unwrap(), "hello");
        assert!(!doomed.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_command_is_an_error_unless_failures_are_ignored() {
        let plan = Plan {
            commands: vec![vec!["false".into()]],
            ..Plan::default()
        };

        assert!(apply(&plan, false).is_err());
        assert!(apply(&plan, true).is_ok());
    }
}
