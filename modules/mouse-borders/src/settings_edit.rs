//! Editing the layout in the settings, for the settings window and the command line.
//!
//! Plain functions over [`MouseBorders`] so every front end changes the layout the same way
//! and all of it is tested without a window.

use aujar_config::{Machine, MouseBorders};

use crate::{
    config::hostname_as_machine_name,
    error::{Error, Result},
    layout::{MAX_MACHINES, MachineName},
};

/// This machine's name: the configured one, or the hostname.
pub fn local_name(settings: &MouseBorders) -> MachineName {
    match settings.machine_name.trim() {
        "" => hostname_as_machine_name(),
        name => MachineName::new(name).unwrap_or_else(|_| hostname_as_machine_name()),
    }
}

/// The machines by slot (`None` for empty slots).
pub fn slots(settings: &MouseBorders) -> [Option<&Machine>; MAX_MACHINES] {
    let mut slots = [None; MAX_MACHINES];

    for machine in &settings.machines {
        if let Some(entry) = slots.get_mut(usize::from(machine.slot)) {
            *entry = Some(machine);
        }
    }

    slots
}

fn first_free_slot(settings: &MouseBorders) -> Option<u8> {
    slots(settings)
        .iter()
        .position(Option::is_none)
        .map(|slot| slot as u8)
}

/// Add a machine at the first free slot. Adding one that exists only updates its address
/// (when given). Returns its slot.
pub fn add_machine(
    settings: &mut MouseBorders,
    name: &MachineName,
    address: Option<&str>,
) -> Result<u8> {
    if let Some(existing) = settings
        .machines
        .iter_mut()
        .find(|machine| machine.name == name.as_str())
    {
        if let Some(address) = address {
            existing.address = address.trim().to_owned();
        }

        return Ok(existing.slot);
    }

    let slot = first_free_slot(settings).ok_or_else(|| {
        Error::Layout(format!(
            "the layout already has {MAX_MACHINES} machines; remove one first"
        ))
    })?;

    settings.machines.push(Machine {
        name: name.to_string(),
        slot,
        address: address.unwrap_or_default().trim().to_owned(),
    });

    Ok(slot)
}

pub fn remove_machine(settings: &mut MouseBorders, name: &str) -> bool {
    let before = settings.machines.len();

    settings.machines.retain(|machine| machine.name != name);

    settings.machines.len() != before
}

/// Move a machine one slot left (`-1`) or right (`+1`), swapping with whoever is there.
/// Returns false at the ends or for an unknown machine.
pub fn move_machine(settings: &mut MouseBorders, name: &str, direction: i8) -> bool {
    let Some(from) = settings
        .machines
        .iter()
        .find(|machine| machine.name == name)
        .map(|machine| machine.slot)
    else {
        return false;
    };

    let to = i16::from(from) + i16::from(direction.signum());

    if !(0..MAX_MACHINES as i16).contains(&to) {
        return false;
    }

    let to = to as u8;

    for machine in &mut settings.machines {
        if machine.name == name {
            machine.slot = to;
        } else if machine.slot == to {
            machine.slot = from;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(value: &str) -> MachineName {
        MachineName::new(value).unwrap()
    }

    fn names(settings: &MouseBorders) -> Vec<String> {
        slots(settings)
            .iter()
            .map(|slot| slot.map_or("-".to_owned(), |machine| machine.name.clone()))
            .collect()
    }

    #[test]
    fn machines_fill_the_first_free_slots() {
        let mut settings = MouseBorders::default();

        assert_eq!(add_machine(&mut settings, &name("a"), None).unwrap(), 0);
        assert_eq!(
            add_machine(&mut settings, &name("b"), Some(" b.lan ")).unwrap(),
            1
        );

        remove_machine(&mut settings, "a");

        assert_eq!(add_machine(&mut settings, &name("c"), None).unwrap(), 0);
        assert_eq!(names(&settings), ["c", "b", "-", "-"]);
        assert_eq!(settings.machines[0].address, "b.lan");
    }

    #[test]
    fn adding_an_existing_machine_only_updates_its_address() {
        let mut settings = MouseBorders::default();

        add_machine(&mut settings, &name("a"), None).unwrap();
        let slot = add_machine(&mut settings, &name("a"), Some("10.0.0.5")).unwrap();

        assert_eq!(slot, 0);
        assert_eq!(settings.machines.len(), 1);
        assert_eq!(settings.machines[0].address, "10.0.0.5");

        // No address given: keep the one we have.
        add_machine(&mut settings, &name("a"), None).unwrap();
        assert_eq!(settings.machines[0].address, "10.0.0.5");
    }

    #[test]
    fn a_fifth_machine_is_refused() {
        let mut settings = MouseBorders::default();

        for letter in ["a", "b", "c", "d"] {
            add_machine(&mut settings, &name(letter), None).unwrap();
        }

        assert!(add_machine(&mut settings, &name("e"), None).is_err());
        assert_eq!(settings.machines.len(), 4);
    }

    #[test]
    fn moving_swaps_with_the_neighbour_or_takes_an_empty_slot() {
        let mut settings = MouseBorders::default();
        add_machine(&mut settings, &name("a"), None).unwrap();
        add_machine(&mut settings, &name("b"), None).unwrap();

        assert!(move_machine(&mut settings, "a", 1));
        assert_eq!(names(&settings), ["b", "a", "-", "-"]);

        assert!(move_machine(&mut settings, "a", 1));
        assert_eq!(names(&settings), ["b", "-", "a", "-"]);

        assert!(move_machine(&mut settings, "a", -1));
        assert!(move_machine(&mut settings, "a", -1));
        assert_eq!(names(&settings), ["a", "b", "-", "-"]);
    }

    #[test]
    fn moving_past_the_ends_or_an_unknown_machine_does_nothing() {
        let mut settings = MouseBorders::default();
        add_machine(&mut settings, &name("a"), None).unwrap();

        assert!(!move_machine(&mut settings, "a", -1));
        assert!(!move_machine(&mut settings, "nobody", 1));

        move_machine(&mut settings, "a", 1);
        move_machine(&mut settings, "a", 1);
        move_machine(&mut settings, "a", 1);

        assert!(!move_machine(&mut settings, "a", 1));
        assert_eq!(names(&settings), ["-", "-", "-", "a"]);
    }

    #[test]
    fn removing_reports_whether_anything_changed() {
        let mut settings = MouseBorders::default();
        add_machine(&mut settings, &name("a"), None).unwrap();

        assert!(remove_machine(&mut settings, "a"));
        assert!(!remove_machine(&mut settings, "a"));
    }

    #[test]
    fn the_local_name_is_the_setting_or_the_hostname() {
        let mut settings = MouseBorders::default();

        assert_eq!(local_name(&settings), hostname_as_machine_name());

        settings.machine_name = " desk ".into();
        assert_eq!(local_name(&settings).as_str(), "desk");

        // An invalid configured name does not panic; the hostname is the fallback.
        settings.machine_name = "not valid!".into();
        assert_eq!(local_name(&settings), hostname_as_machine_name());
    }
}
