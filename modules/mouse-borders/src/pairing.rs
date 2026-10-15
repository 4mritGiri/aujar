//! Pairing codes: everything another machine needs to join, as one line you can copy.
//!
//! A code carries the security key and the layout (shape, machines, slots, addresses), so
//! pasting it on a second machine configures that machine completely, with the same layout
//! as the first. Typing 39-character keys and addresses is what this avoids.
//!
//! Format (`;`-separated, plain text so it is easy to eyeball):
//!
//! ```text
//! aujar-pair1;row;K7QD-2XJP-…;desk@0@192.168.1.10:15101;laptop@1@
//! ```
//!
//! The code contains the security key: treat it like a password. Anyone who sees it can
//! connect to your machines.

use std::net::IpAddr;

use aujar_config::{Machine, MatrixMode, MouseBorders};

use crate::{
    error::{Error, Result},
    layout::{MAX_MACHINES, MachineName},
    net,
    secure::SecurityKey,
};

const PREFIX: &str = "aujar-pair1";
const MAX_CODE_LEN: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedMachine {
    pub name: MachineName,
    pub slot: u8,
    /// Empty when the machine is found by its name.
    pub address: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode {
    pub matrix: MatrixMode,
    pub key: SecurityKey,
    pub machines: Vec<PairedMachine>,
}

/// What joining changed on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    /// This machine was not in the adopted layout and had to be added.
    pub added_local: bool,
    pub machines: usize,
}

fn is_address_text(address: &str) -> bool {
    address.len() <= 255
        && address
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']'))
}

impl PairingCode {
    /// Build the code to hand to other machines.
    ///
    /// `local_host` fills in this machine's own address when the layout has none for it, so
    /// the others know where to reach it. The saved settings are not changed.
    pub fn from_settings(
        settings: &MouseBorders,
        key: SecurityKey,
        local: &MachineName,
        local_host: Option<IpAddr>,
    ) -> Result<Self> {
        let mut machines = Vec::new();

        for machine in &settings.machines {
            let name = MachineName::new(&machine.name)?;
            let mut address = machine.address.trim().to_owned();

            if name == *local
                && address.is_empty()
                && let Some(host) = local_host
            {
                address = match host {
                    IpAddr::V4(v4) => format!("{v4}:{}", settings.port),
                    IpAddr::V6(v6) => format!("[{v6}]:{}", settings.port),
                };
            }

            if !is_address_text(&address) {
                return Err(Error::Config(format!(
                    "the address `{address}` of `{name}` cannot go into a pairing code"
                )));
            }

            machines.push(PairedMachine {
                name,
                slot: machine.slot,
                address,
            });
        }

        if !machines.iter().any(|machine| machine.name == *local) {
            return Err(Error::Config(
                "add this machine to the layout before creating a pairing code".into(),
            ));
        }

        Ok(Self {
            matrix: settings.matrix,
            key,
            machines,
        })
    }

    pub fn encode(&self) -> String {
        let matrix = match self.matrix {
            MatrixMode::Row => "row",
            MatrixMode::Grid => "grid",
        };
        let mut code = format!("{PREFIX};{matrix};{}", self.key.to_display());

        for machine in &self.machines {
            code.push_str(&format!(
                ";{}@{}@{}",
                machine.name, machine.slot, machine.address
            ));
        }

        code
    }

    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let bad = |why: &str| Error::Config(format!("not a valid pairing code: {why}"));

        if text.len() > MAX_CODE_LEN {
            return Err(bad("too long"));
        }

        let mut parts = text.split(';');

        if parts.next() != Some(PREFIX) {
            return Err(bad("it should start with `aujar-pair1`"));
        }

        let matrix = match parts.next() {
            Some("row") => MatrixMode::Row,
            Some("grid") => MatrixMode::Grid,
            _ => return Err(bad("unknown layout shape")),
        };
        let key = SecurityKey::parse(parts.next().ok_or_else(|| bad("no key"))?)?;

        let mut machines: Vec<PairedMachine> = Vec::new();

        for part in parts {
            let mut fields = part.splitn(3, '@');
            let (Some(name), Some(slot), Some(address)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return Err(bad("a machine entry is incomplete"));
            };

            let slot: u8 = slot.parse().map_err(|_| bad("a slot is not a number"))?;

            if usize::from(slot) >= MAX_MACHINES {
                return Err(bad("a slot is out of range"));
            }

            if !is_address_text(address) {
                return Err(bad("an address has unusual characters"));
            }

            if !address.is_empty() {
                net::split_host_port(address, 1)?;
            }

            let name = MachineName::new(name)?;

            if machines
                .iter()
                .any(|existing| existing.name == name || existing.slot == slot)
            {
                return Err(bad("a machine or slot appears twice"));
            }

            machines.push(PairedMachine {
                name,
                slot,
                address: address.to_owned(),
            });
        }

        if machines.is_empty() {
            return Err(bad("no machines"));
        }

        Ok(Self {
            matrix,
            key,
            machines,
        })
    }

    /// Adopt this code's layout into `settings` (the caller stores the key). This machine is
    /// added at the first free slot if the layout does not contain it. Enables Mouse
    /// Borders; other options are left alone.
    pub fn apply(&self, settings: &mut MouseBorders, local: &MachineName) -> Result<Joined> {
        let mut machines: Vec<Machine> = self
            .machines
            .iter()
            .map(|machine| Machine {
                name: machine.name.to_string(),
                slot: machine.slot,
                address: machine.address.clone(),
            })
            .collect();

        let added_local = !self.machines.iter().any(|machine| machine.name == *local);

        if added_local {
            let slot = (0..MAX_MACHINES as u8)
                .find(|slot| !machines.iter().any(|machine| machine.slot == *slot))
                .ok_or_else(|| {
                    Error::Layout(format!(
                        "the layout already has {MAX_MACHINES} machines; there is no room for `{local}`"
                    ))
                })?;

            machines.push(Machine {
                name: local.to_string(),
                slot,
                address: String::new(),
            });
        }

        settings.matrix = self.matrix;
        settings.machine_name = local.to_string();
        settings.machines = machines;
        settings.enabled = true;

        Ok(Joined {
            added_local,
            machines: settings.machines.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(value: &str) -> MachineName {
        MachineName::new(value).unwrap()
    }

    fn settings() -> MouseBorders {
        MouseBorders {
            machine_name: "desk".into(),
            machines: vec![
                Machine {
                    name: "desk".into(),
                    slot: 0,
                    address: String::new(),
                },
                Machine {
                    name: "laptop".into(),
                    slot: 1,
                    address: "laptop.local".into(),
                },
            ],
            ..MouseBorders::default()
        }
    }

    fn code() -> PairingCode {
        PairingCode::from_settings(
            &settings(),
            SecurityKey::generate().unwrap(),
            &name("desk"),
            Some("192.168.1.10".parse().unwrap()),
        )
        .unwrap()
    }

    #[test]
    fn a_code_round_trips_and_looks_like_text() {
        let code = code();
        let text = code.encode();

        assert!(text.starts_with("aujar-pair1;row;"), "{text}");
        assert!(text.contains(";desk@0@192.168.1.10:15101;"), "{text}");
        assert!(text.ends_with(";laptop@1@laptop.local"), "{text}");
        assert!(!text.contains(char::is_whitespace));
        assert_eq!(PairingCode::parse(&text).unwrap(), code);
        // Pasting usually brings a trailing newline or spaces.
        assert_eq!(PairingCode::parse(&format!("  {text}\n")).unwrap(), code);
    }

    #[test]
    fn this_machines_own_address_is_filled_in_for_others_to_reach() {
        let code = code();
        let desk = code
            .machines
            .iter()
            .find(|m| m.name == name("desk"))
            .unwrap();

        assert_eq!(desk.address, "192.168.1.10:15101");
        // Without a known address it stays empty (found by name).
        let bare = PairingCode::from_settings(
            &settings(),
            SecurityKey::generate().unwrap(),
            &name("desk"),
            None,
        )
        .unwrap();

        assert_eq!(bare.machines[0].address, "");
    }

    #[test]
    fn ipv6_hosts_are_bracketed() {
        let code = PairingCode::from_settings(
            &settings(),
            SecurityKey::generate().unwrap(),
            &name("desk"),
            Some("fd00::5".parse().unwrap()),
        )
        .unwrap();

        assert_eq!(code.machines[0].address, "[fd00::5]:15101");
        assert!(PairingCode::parse(&code.encode()).is_ok());
    }

    #[test]
    fn a_code_cannot_be_made_without_this_machine_in_the_layout() {
        let error = PairingCode::from_settings(
            &settings(),
            SecurityKey::generate().unwrap(),
            &name("stranger"),
            None,
        )
        .unwrap_err();

        assert!(error.to_string().contains("add this machine"));
    }

    #[test]
    fn malformed_codes_are_rejected_with_a_reason() {
        let key = SecurityKey::generate().unwrap().to_display();
        let cases = [
            ("", "start with"),
            ("hello", "start with"),
            ("aujar-pair2;row;x", "start with"),
            ("aujar-pair1;diagonal;x", "shape"),
            ("aujar-pair1;row", "key"),
            (&format!("aujar-pair1;row;{key}"), "no machines"),
            (&format!("aujar-pair1;row;{key};desk"), "incomplete"),
            (&format!("aujar-pair1;row;{key};desk@x@"), "not a number"),
            (&format!("aujar-pair1;row;{key};desk@9@"), "out of range"),
            (&format!("aujar-pair1;row;{key};a@0@;a@1@"), "twice"),
            (&format!("aujar-pair1;row;{key};a@0@;b@0@"), "twice"),
            (&format!("aujar-pair1;row;{key};a b@0@"), "machine name"),
            (&format!("aujar-pair1;row;{key};a@0@host name"), "unusual"),
            (&format!("aujar-pair1;row;{key};a@0@host/../x"), "unusual"),
            (&format!("aujar-pair1;row;{key};a@0@host:notaport"), "port"),
            ("aujar-pair1;row;SHORT;a@0@", "characters"),
        ];

        for (text, expected) in cases {
            let error = PairingCode::parse(text).unwrap_err().to_string();

            assert!(
                error.contains(expected),
                "{text:?} gave `{error}`, wanted `{expected}`"
            );
        }

        assert!(PairingCode::parse(&"x".repeat(5000)).is_err());
    }

    #[test]
    fn joining_adopts_the_layout_and_enables_the_feature() {
        let mut mine = MouseBorders {
            wrap_mouse: true,
            ..MouseBorders::default()
        };

        let joined = code().apply(&mut mine, &name("laptop")).unwrap();

        assert_eq!(
            joined,
            Joined {
                added_local: false,
                machines: 2
            }
        );
        assert!(mine.enabled);
        assert_eq!(mine.machine_name, "laptop");
        assert_eq!(mine.machines.len(), 2);
        assert_eq!(mine.machines[0].address, "192.168.1.10:15101");
        // Unrelated options survive.
        assert!(mine.wrap_mouse);
    }

    #[test]
    fn a_new_machine_is_appended_at_the_first_free_slot() {
        let mut mine = MouseBorders::default();

        let joined = code().apply(&mut mine, &name("tv")).unwrap();

        assert!(joined.added_local);
        assert_eq!(joined.machines, 3);
        let tv = mine.machines.iter().find(|m| m.name == "tv").unwrap();
        assert_eq!(tv.slot, 2);
    }

    #[test]
    fn joining_a_full_layout_says_there_is_no_room() {
        let mut full = settings();
        full.machines.push(Machine {
            name: "a".into(),
            slot: 2,
            address: String::new(),
        });
        full.machines.push(Machine {
            name: "b".into(),
            slot: 3,
            address: String::new(),
        });
        let code = PairingCode::from_settings(
            &full,
            SecurityKey::generate().unwrap(),
            &name("desk"),
            None,
        )
        .unwrap();

        let mut mine = MouseBorders::default();
        let error = code.apply(&mut mine, &name("tv")).unwrap_err().to_string();

        assert!(error.contains("no room"), "{error}");
        assert!(
            mine.machines.is_empty(),
            "a failed join must change nothing"
        );
        assert!(!mine.enabled);
    }

    #[test]
    fn the_adopted_layout_is_a_valid_engine_configuration() {
        let mut mine = MouseBorders {
            receive_dir: "/tmp/aujar-pairing-test".into(),
            ..MouseBorders::default()
        };

        code().apply(&mut mine, &name("laptop")).unwrap();

        assert!(crate::config::EngineConfig::from_settings(&mine).is_ok());
    }
}
