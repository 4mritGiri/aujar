//! Editing the layout in the settings, for the settings window and the command line.
//!
//! Plain functions over [`MouseBorders`] so every front end changes the layout the same way
//! and all of it is tested without a window.

use aujar_config::{Machine, MatrixMode, MouseBorders};

use crate::{
    config::hostname_as_machine_name,
    error::{Error, Result},
    layout::{MAX_MACHINES, MachineName, PLANE_COLS, PLANE_ROWS, PLANE_SLOTS},
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

/// How many slots the current arrangement has.
fn slot_count(matrix: MatrixMode) -> usize {
    match matrix {
        MatrixMode::Row | MatrixMode::Grid => MAX_MACHINES,
        MatrixMode::Free => PLANE_SLOTS,
    }
}

fn first_free_slot(settings: &MouseBorders) -> Option<u8> {
    if settings.machines.len() >= MAX_MACHINES {
        return None;
    }

    (0..slot_count(settings.matrix) as u8)
        .find(|slot| !settings.machines.iter().any(|machine| machine.slot == *slot))
}

/// Where a slot sits on the 4 x 3 board, as `(column, row)`, for the arrangement `matrix`.
pub fn board_position(matrix: MatrixMode, slot: u8) -> (u8, u8) {
    match matrix {
        MatrixMode::Row => (slot % PLANE_COLS, 0),
        MatrixMode::Grid => (slot % 2, slot / 2),
        MatrixMode::Free => (slot % PLANE_COLS, slot / PLANE_COLS),
    }
}

/// The machine at each cell of the 4 x 3 board, `row * 4 + column`, whatever the arrangement.
pub fn board(settings: &MouseBorders) -> [Option<&Machine>; PLANE_SLOTS] {
    let mut board = [None; PLANE_SLOTS];

    for machine in &settings.machines {
        let (col, row) = board_position(settings.matrix, machine.slot);

        if col < PLANE_COLS && row < PLANE_ROWS {
            board[usize::from(row) * usize::from(PLANE_COLS) + usize::from(col)] = Some(machine);
        }
    }

    board
}

/// Switch to the free board without moving anyone: every machine keeps its place.
pub fn make_free(settings: &mut MouseBorders) {
    if settings.matrix == MatrixMode::Free {
        return;
    }

    let from = settings.matrix;

    for machine in &mut settings.machines {
        let (col, row) = board_position(from, machine.slot);

        machine.slot = row * PLANE_COLS + col;
    }

    settings.matrix = MatrixMode::Free;
}

/// Put a machine on the board cell `target` (`row * 4 + column`). An occupied cell swaps
/// places with the machine being moved. Switches to the free board first. Returns false for
/// an unknown machine, a cell off the board or no change.
pub fn place_machine(settings: &mut MouseBorders, name: &str, target: u8) -> bool {
    if usize::from(target) >= PLANE_SLOTS {
        return false;
    }

    make_free(settings);

    let Some(from) = settings
        .machines
        .iter()
        .find(|machine| machine.name == name)
        .map(|machine| machine.slot)
    else {
        return false;
    };

    if from == target {
        return false;
    }

    for machine in &mut settings.machines {
        if machine.name == name {
            machine.slot = target;
        } else if machine.slot == target {
            machine.slot = from;
        }
    }

    true
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

    if !(0..slot_count(settings.matrix) as i16).contains(&to) {
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
    fn making_a_row_free_keeps_every_machine_in_place() {
        let mut settings = MouseBorders::default();
        add_machine(&mut settings, &name("a"), None).unwrap();
        add_machine(&mut settings, &name("b"), None).unwrap();

        make_free(&mut settings);

        assert_eq!(settings.matrix, MatrixMode::Free);
        assert_eq!(settings.machines[0].slot, 0);
        assert_eq!(settings.machines[1].slot, 1);
    }

    #[test]
    fn making_a_grid_free_keeps_the_second_row_below() {
        let mut settings = MouseBorders {
            matrix: MatrixMode::Grid,
            ..MouseBorders::default()
        };
        for letter in ["a", "b", "c", "d"] {
            add_machine(&mut settings, &name(letter), None).unwrap();
        }

        make_free(&mut settings);

        let slot_of = |who: &str| {
            settings
                .machines
                .iter()
                .find(|machine| machine.name == who)
                .unwrap()
                .slot
        };

        // a b . .      slots 0 1 2 3
        // c d . .      slots 4 5 6 7
        assert_eq!([slot_of("a"), slot_of("b"), slot_of("c"), slot_of("d")], [0, 1, 4, 5]);
    }

    #[test]
    fn placing_moves_to_an_empty_cell_or_swaps() {
        let mut settings = MouseBorders::default();
        add_machine(&mut settings, &name("a"), None).unwrap();
        add_machine(&mut settings, &name("b"), None).unwrap();

        // Below `a`: row 1, column 0.
        assert!(place_machine(&mut settings, "b", 4));
        assert_eq!(settings.matrix, MatrixMode::Free);
        assert_eq!(board(&settings)[4].unwrap().name, "b");
        assert!(board(&settings)[1].is_none());

        // Dropping `a` on `b` swaps them.
        assert!(place_machine(&mut settings, "a", 4));
        assert_eq!(board(&settings)[0].unwrap().name, "b");
        assert_eq!(board(&settings)[4].unwrap().name, "a");

        // Same cell, off the board, unknown machine: nothing happens.
        assert!(!place_machine(&mut settings, "a", 4));
        assert!(!place_machine(&mut settings, "a", PLANE_SLOTS as u8));
        assert!(!place_machine(&mut settings, "nobody", 3));
    }

    #[test]
    fn free_boards_hold_four_machines_in_any_cells() {
        let mut settings = MouseBorders {
            matrix: MatrixMode::Free,
            ..MouseBorders::default()
        };

        for letter in ["a", "b", "c", "d"] {
            add_machine(&mut settings, &name(letter), None).unwrap();
        }

        assert!(add_machine(&mut settings, &name("e"), None).is_err());
        assert!(place_machine(&mut settings, "d", 11));
        assert_eq!(board(&settings)[11].unwrap().name, "d");
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
