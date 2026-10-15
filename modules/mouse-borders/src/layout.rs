//! The physical arrangement of machines and which one lies beyond each screen edge.

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// At most this many machines share one keyboard and mouse.
pub const MAX_MACHINES: usize = 4;

const MAX_NAME_LEN: usize = 63;

/// A validated machine name: 1 to 63 characters of `A-Z a-z 0-9 . - _`.
///
/// The restriction keeps names safe to log, to use in file names and to resolve as host
/// names, whatever a peer claims to be called.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MachineName(String);

impl MachineName {
    pub fn new(name: impl AsRef<str>) -> Result<Self> {
        let name = name.as_ref();

        if name.is_empty() || name.len() > MAX_NAME_LEN {
            return Err(Error::Layout(format!(
                "machine name `{name}` must be 1 to {MAX_NAME_LEN} characters"
            )));
        }

        if let Some(bad) = name
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
        {
            return Err(Error::Layout(format!(
                "machine name `{name}` contains `{}`; use letters, digits, `.`, `-` and `_`",
                bad.escape_default()
            )));
        }

        Ok(Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MachineName {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl From<MachineName> for String {
    fn from(name: MachineName) -> Self {
        name.0
    }
}

impl fmt::Display for MachineName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub const fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }

    const fn is_horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    /// `-1` towards the origin, `+1` away from it.
    const fn step(self) -> i8 {
        match self {
            Self::Left | Self::Top => -1,
            Self::Right | Self::Bottom => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Up to four machines side by side.
    Row,
    /// Two rows of two.
    Grid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cell {
    col: u8,
    row: u8,
    slot: u8,
}

impl Shape {
    fn cell(self, slot: u8) -> Cell {
        match self {
            Self::Row => Cell {
                col: slot,
                row: 0,
                slot,
            },
            Self::Grid => Cell {
                col: slot % 2,
                row: slot / 2,
                slot,
            },
        }
    }
}

/// Where each machine sits, and the rules for moving between them.
#[derive(Debug, Clone)]
pub struct Layout {
    shape: Shape,
    wrap: bool,
    cells: BTreeMap<MachineName, Cell>,
}

impl Layout {
    /// Build a layout from `(machine, slot)` pairs.
    pub fn new(
        shape: Shape,
        wrap: bool,
        machines: impl IntoIterator<Item = (MachineName, u8)>,
    ) -> Result<Self> {
        let mut cells = BTreeMap::new();
        let mut used_slots = [false; MAX_MACHINES];

        for (name, slot) in machines {
            let index = usize::from(slot);

            if index >= MAX_MACHINES {
                return Err(Error::Layout(format!(
                    "machine `{name}` has slot {slot}; slots are 0 to {}",
                    MAX_MACHINES - 1
                )));
            }

            if std::mem::replace(&mut used_slots[index], true) {
                return Err(Error::Layout(format!("slot {slot} is used twice")));
            }

            if cells.insert(name.clone(), shape.cell(slot)).is_some() {
                return Err(Error::Layout(format!("machine `{name}` is listed twice")));
            }
        }

        if cells.is_empty() {
            return Err(Error::Layout("the layout has no machines".into()));
        }

        Ok(Self { shape, wrap, cells })
    }

    pub fn shape(&self) -> Shape {
        self.shape
    }

    pub fn contains(&self, name: &MachineName) -> bool {
        self.cells.contains_key(name)
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Machines in slot order.
    pub fn machines(&self) -> Vec<&MachineName> {
        let mut machines: Vec<_> = self.cells.iter().collect();
        machines.sort_by_key(|(_, cell)| cell.slot);

        machines.into_iter().map(|(name, _)| name).collect()
    }

    pub fn slot_of(&self, name: &MachineName) -> Option<u8> {
        self.cells.get(name).map(|cell| cell.slot)
    }

    pub fn machine_at_slot(&self, slot: u8) -> Option<&MachineName> {
        self.cells
            .iter()
            .find(|(_, cell)| cell.slot == slot)
            .map(|(name, _)| name)
    }

    /// The machine reached by leaving `from` through `edge`: the nearest machine in that
    /// direction on the same row or column. With wrapping on, leaving the last machine
    /// continues at the other end. `None` means the edge is a wall.
    pub fn neighbor(&self, from: &MachineName, edge: Edge) -> Option<&MachineName> {
        let origin = *self.cells.get(from)?;

        // Machines in the same row (for Left/Right) or column (for Top/Bottom), ordered
        // along the axis of travel.
        let mut line: Vec<(&MachineName, i8)> = self
            .cells
            .iter()
            .filter(|(_, cell)| {
                if edge.is_horizontal() {
                    cell.row == origin.row
                } else {
                    cell.col == origin.col
                }
            })
            .map(|(name, cell)| {
                let position = if edge.is_horizontal() {
                    cell.col
                } else {
                    cell.row
                };

                (name, position as i8)
            })
            .collect();
        line.sort_by_key(|(_, position)| *position);

        let here = line.iter().position(|(name, _)| *name == from)?;
        let next = here as isize + isize::from(edge.step());

        if (0..line.len() as isize).contains(&next) {
            return Some(line[next as usize].0);
        }

        if self.wrap && line.len() > 1 {
            let wrapped = if edge.step() > 0 { 0 } else { line.len() - 1 };

            return Some(line[wrapped].0);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(value: &str) -> MachineName {
        MachineName::new(value).unwrap()
    }

    fn row(wrap: bool) -> Layout {
        Layout::new(
            Shape::Row,
            wrap,
            [("a", 0), ("b", 1), ("c", 2)].map(|(n, s)| (name(n), s)),
        )
        .unwrap()
    }

    fn grid(wrap: bool) -> Layout {
        Layout::new(
            Shape::Grid,
            wrap,
            [("a", 0), ("b", 1), ("c", 2), ("d", 3)].map(|(n, s)| (name(n), s)),
        )
        .unwrap()
    }

    #[test]
    fn names_are_validated() {
        assert!(MachineName::new("desk-01.lan").is_ok());
        assert!(MachineName::new("").is_err());
        assert!(MachineName::new("a b").is_err());
        assert!(MachineName::new("../etc").is_err());
        assert!(MachineName::new("line\nbreak").is_err());
        assert!(MachineName::new("x".repeat(64)).is_err());
    }

    #[test]
    fn row_neighbors() {
        let layout = row(false);

        assert_eq!(layout.neighbor(&name("a"), Edge::Right), Some(&name("b")));
        assert_eq!(layout.neighbor(&name("b"), Edge::Left), Some(&name("a")));
        assert_eq!(layout.neighbor(&name("c"), Edge::Right), None);
        assert_eq!(layout.neighbor(&name("a"), Edge::Left), None);
        assert_eq!(layout.neighbor(&name("b"), Edge::Top), None);
        assert_eq!(layout.neighbor(&name("b"), Edge::Bottom), None);
    }

    #[test]
    fn row_wraps_to_the_other_end() {
        let layout = row(true);

        assert_eq!(layout.neighbor(&name("c"), Edge::Right), Some(&name("a")));
        assert_eq!(layout.neighbor(&name("a"), Edge::Left), Some(&name("c")));
        // No vertical wrap in a single row: there is nobody above or below.
        assert_eq!(layout.neighbor(&name("a"), Edge::Top), None);
    }

    #[test]
    fn gaps_are_skipped() {
        let layout = Layout::new(Shape::Row, false, [(name("a"), 0), (name("c"), 2)]).unwrap();

        assert_eq!(layout.neighbor(&name("a"), Edge::Right), Some(&name("c")));
    }

    #[test]
    fn grid_neighbors_stay_in_their_row_and_column() {
        let layout = grid(false);

        assert_eq!(layout.neighbor(&name("a"), Edge::Right), Some(&name("b")));
        assert_eq!(layout.neighbor(&name("a"), Edge::Bottom), Some(&name("c")));
        assert_eq!(layout.neighbor(&name("d"), Edge::Top), Some(&name("b")));
        assert_eq!(layout.neighbor(&name("d"), Edge::Left), Some(&name("c")));
        assert_eq!(layout.neighbor(&name("b"), Edge::Right), None);
        assert_eq!(layout.neighbor(&name("c"), Edge::Bottom), None);
    }

    #[test]
    fn grid_wraps_within_row_and_column() {
        let layout = grid(true);

        assert_eq!(layout.neighbor(&name("b"), Edge::Right), Some(&name("a")));
        assert_eq!(layout.neighbor(&name("c"), Edge::Bottom), Some(&name("a")));
        assert_eq!(layout.neighbor(&name("a"), Edge::Top), Some(&name("c")));
    }

    #[test]
    fn single_machine_never_wraps_onto_itself() {
        let layout = Layout::new(Shape::Row, true, [(name("a"), 0)]).unwrap();

        assert_eq!(layout.neighbor(&name("a"), Edge::Right), None);
    }

    #[test]
    fn rejects_invalid_layouts() {
        let dup_slot = Layout::new(Shape::Row, false, [(name("a"), 0), (name("b"), 0)]);
        let dup_name = Layout::new(Shape::Row, false, [(name("a"), 0), (name("a"), 1)]);
        let bad_slot = Layout::new(Shape::Row, false, [(name("a"), 4)]);
        let empty = Layout::new(Shape::Row, false, std::iter::empty());

        assert!(dup_slot.is_err());
        assert!(dup_name.is_err());
        assert!(bad_slot.is_err());
        assert!(empty.is_err());
    }

    #[test]
    fn machines_come_back_in_slot_order() {
        let layout = Layout::new(
            Shape::Row,
            false,
            [(name("z"), 0), (name("a"), 2), (name("m"), 1)],
        )
        .unwrap();
        let order: Vec<_> = layout.machines().into_iter().map(|m| m.as_str()).collect();

        assert_eq!(order, ["z", "m", "a"]);
        assert_eq!(layout.machine_at_slot(2), Some(&name("a")));
        assert_eq!(layout.slot_of(&name("m")), Some(1));
    }
}
