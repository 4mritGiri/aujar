//! Decides which machine owns the pointer.
//!
//! Pure state machine: callers feed it pointer positions or motion and the time, and it
//! answers with [`Transfer`]s. All I/O (grabbing devices, talking to peers) happens in the
//! engine, so every rule here is unit-tested.
//!
//! The router tracks a *virtual* cursor for the machine being controlled by integrating
//! motion over that machine's screen size. Switching back never depends on what the remote
//! desktop's pointer acceleration does to the pointer.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

use aujar_config::EasyMouse;
use aujar_core::{Position, Rect, Size};

use crate::{
    keys::Modifiers,
    layout::{Edge, Layout, MachineName},
};

/// How far inside the new screen the pointer lands, so a tiny backwards jitter does not
/// immediately cross back.
const ENTRY_INSET: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchPolicy {
    pub easy_mouse: EasyMouse,
    /// Ignore crossings that start within `corner_size` pixels of a screen corner.
    pub block_corners: bool,
    pub corner_size: i32,
    /// Minimum time between two switches.
    pub cooldown: Duration,
}

impl Default for SwitchPolicy {
    fn default() -> Self {
        Self {
            easy_mouse: EasyMouse::Enabled,
            block_corners: false,
            corner_size: 8,
            cooldown: Duration::from_millis(150),
        }
    }
}

impl SwitchPolicy {
    fn allows(&self, modifiers: Modifiers) -> bool {
        match self.easy_mouse {
            EasyMouse::Disabled => false,
            EasyMouse::Enabled => true,
            EasyMouse::RequireShift => modifiers.shift,
            EasyMouse::RequireCtrl => modifiers.ctrl,
        }
    }
}

/// Which machine currently has the keyboard and mouse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Focus {
    Local,
    Remote(MachineName),
}

/// Control moves from `from` to `to`, landing at `entry` in `to`'s coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub from: MachineName,
    pub to: MachineName,
    pub entry: Position,
}

/// Result of moving the pointer on a remote machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteStep {
    /// Still on the same machine; the virtual cursor is now at `position`.
    Moved {
        position: Position,
    },
    Switched(Transfer),
}

#[derive(Debug)]
pub struct Router {
    layout: Layout,
    local: MachineName,
    screens: BTreeMap<MachineName, Size>,
    online: BTreeSet<MachineName>,
    policy: SwitchPolicy,
    focus: Focus,
    /// Virtual cursor on the focused remote machine.
    cursor: Position,
    /// Where the pointer was when control last left each machine.
    parked: BTreeMap<MachineName, Position>,
    local_position: Position,
    last_switch: Option<Instant>,
}

impl Router {
    pub fn new(
        layout: Layout,
        local: MachineName,
        local_screen: Size,
        policy: SwitchPolicy,
    ) -> Self {
        let mut screens = BTreeMap::new();
        screens.insert(local.clone(), local_screen);

        Self {
            layout,
            local,
            screens,
            online: BTreeSet::new(),
            policy,
            focus: Focus::Local,
            cursor: Position { x: 0, y: 0 },
            parked: BTreeMap::new(),
            local_position: center(local_screen),
            last_switch: None,
        }
    }

    pub fn focus(&self) -> &Focus {
        &self.focus
    }

    pub fn local(&self) -> &MachineName {
        &self.local
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn cursor(&self) -> Position {
        self.cursor
    }

    pub fn policy(&self) -> &SwitchPolicy {
        &self.policy
    }

    pub fn set_policy(&mut self, policy: SwitchPolicy) {
        self.policy = policy;
    }

    pub fn is_online(&self, name: &MachineName) -> bool {
        *name == self.local || self.online.contains(name)
    }

    pub fn screen(&self, name: &MachineName) -> Option<Size> {
        self.screens.get(name).copied()
    }

    /// A peer connected (or changed resolution): it can now be switched to.
    pub fn peer_up(&mut self, name: MachineName, screen: Size) {
        if name == self.local || !self.layout.contains(&name) {
            return;
        }

        self.set_screen(&name, screen);
        self.online.insert(name);
    }

    /// The local screen changed size.
    pub fn set_local_screen(&mut self, screen: Size) {
        let local = self.local.clone();

        self.set_screen(&local, screen);
    }

    fn set_screen(&mut self, name: &MachineName, screen: Size) {
        self.screens.insert(name.clone(), screen);

        let bounds = bounds(screen);

        if let Some(position) = self.parked.get_mut(name) {
            *position = bounds.clamp(*position);
        }

        if *name == self.local {
            self.local_position = bounds.clamp(self.local_position);
        } else if self.focus == Focus::Remote(name.clone()) {
            self.cursor = bounds.clamp(self.cursor);
        }
    }

    /// A peer disconnected. If it had focus, control returns to this machine.
    pub fn peer_down(&mut self, name: &MachineName, now: Instant) -> Option<Transfer> {
        self.online.remove(name);

        if self.focus == Focus::Remote(name.clone()) {
            let entry = self.local_position;
            self.apply(
                Focus::Remote(name.clone()),
                Focus::Local,
                entry,
                None,
                Some(now),
            );

            return Some(Transfer {
                from: name.clone(),
                to: self.local.clone(),
                entry,
            });
        }

        None
    }

    /// The real local pointer is at `position` and just moved by `delta`. Returns a
    /// transfer when it pushed against an edge that leads to another machine.
    pub fn on_local_motion(
        &mut self,
        position: Position,
        delta: (i32, i32),
        modifiers: Modifiers,
        now: Instant,
    ) -> Option<Transfer> {
        if self.focus != Focus::Local {
            return None;
        }

        let screen = self.screens[&self.local];
        let position = bounds(screen).clamp(position);

        self.local_position = position;

        let edge = self.crossing(position, delta, screen, modifiers, now)?;
        let to = self.target(&self.local, edge)?;
        let entry = self.entry_for(position, edge, screen, &to);

        self.apply(
            Focus::Local,
            Focus::Remote(to.clone()),
            entry,
            Some(position),
            Some(now),
        );

        Some(Transfer {
            from: self.local.clone(),
            to,
            entry,
        })
    }

    /// The pointer moved by `delta` while a remote machine has focus. Returns `None` when
    /// this machine has focus.
    pub fn on_remote_motion(
        &mut self,
        delta: (i32, i32),
        modifiers: Modifiers,
        now: Instant,
    ) -> Option<RemoteStep> {
        let Focus::Remote(machine) = self.focus.clone() else {
            return None;
        };

        let screen = self.screens.get(&machine).copied()?;
        let moved = Position {
            x: saturating_add(self.cursor.x, delta.0),
            y: saturating_add(self.cursor.y, delta.1),
        };
        let clamped = bounds(screen).clamp(moved);

        if let Some(edge) = escaped_edge(moved, screen)
            && self.permitted(clamped, screen, modifiers, now)
            && let Some(to) = self.target(&machine, edge)
        {
            let entry = self.entry_for(clamped, edge, screen, &to);
            let focus = if to == self.local {
                Focus::Local
            } else {
                Focus::Remote(to.clone())
            };

            self.apply(
                Focus::Remote(machine.clone()),
                focus,
                entry,
                Some(clamped),
                Some(now),
            );

            return Some(RemoteStep::Switched(Transfer {
                from: machine,
                to,
                entry,
            }));
        }

        self.cursor = clamped;

        Some(RemoteStep::Moved { position: clamped })
    }

    /// Jump straight to `to` (keyboard shortcut). `None` if it is already focused, unknown
    /// or offline.
    pub fn jump(&mut self, to: &MachineName, now: Instant) -> Option<Transfer> {
        let from = self.focused_machine().clone();

        if *to == from || !self.layout.contains(to) || !self.is_online(to) {
            return None;
        }

        let screen = self.screens.get(to).copied()?;
        let entry = bounds(screen).clamp(
            self.parked
                .get(to)
                .copied()
                .or_else(|| (*to == self.local).then_some(self.local_position))
                .unwrap_or_else(|| center(screen)),
        );
        let new_focus = if *to == self.local {
            Focus::Local
        } else {
            Focus::Remote(to.clone())
        };
        let parked_from = match &self.focus {
            Focus::Local => self.local_position,
            Focus::Remote(_) => self.cursor,
        };

        self.apply(
            self.focus.clone(),
            new_focus,
            entry,
            Some(parked_from),
            Some(now),
        );

        Some(Transfer {
            from,
            to: to.clone(),
            entry,
        })
    }

    pub fn focused_machine(&self) -> &MachineName {
        match &self.focus {
            Focus::Local => &self.local,
            Focus::Remote(name) => name,
        }
    }

    fn apply(
        &mut self,
        from: Focus,
        to: Focus,
        entry: Position,
        left_at: Option<Position>,
        now: Option<Instant>,
    ) {
        let from_name = match from {
            Focus::Local => self.local.clone(),
            Focus::Remote(name) => name,
        };

        if let Some(position) = left_at {
            self.parked.insert(from_name, position);
        }

        match &to {
            Focus::Local => self.local_position = entry,
            Focus::Remote(_) => self.cursor = entry,
        }

        self.focus = to;

        if let Some(now) = now {
            self.last_switch = Some(now);
        }
    }

    fn cooling_down(&self, now: Instant) -> bool {
        self.last_switch
            .is_some_and(|at| now.saturating_duration_since(at) < self.policy.cooldown)
    }

    /// Edge the real pointer is pressing against, if a switch is allowed there.
    fn crossing(
        &self,
        position: Position,
        delta: (i32, i32),
        screen: Size,
        modifiers: Modifiers,
        now: Instant,
    ) -> Option<Edge> {
        let edge = pushed_edge(position, delta, screen)?;

        self.permitted(position, screen, modifiers, now)
            .then_some(edge)
    }

    /// Whether policy, timing and corner rules allow a switch starting at `position`.
    fn permitted(
        &self,
        position: Position,
        screen: Size,
        modifiers: Modifiers,
        now: Instant,
    ) -> bool {
        self.policy.allows(modifiers)
            && !self.cooling_down(now)
            && !(self.policy.block_corners && in_corner(position, screen, self.policy.corner_size))
    }

    /// The machine beyond `edge` of `from`, if it is connected.
    fn target(&self, from: &MachineName, edge: Edge) -> Option<MachineName> {
        let to = self.layout.neighbor(from, edge)?;

        (self.is_online(to) && self.screens.contains_key(to)).then(|| to.clone())
    }

    fn entry_for(&self, exit: Position, edge: Edge, from: Size, to: &MachineName) -> Position {
        map_entry(exit, edge, from, self.screens[to])
    }
}

fn saturating_add(value: i32, delta: i32) -> i32 {
    value.saturating_add(delta)
}

fn bounds(size: Size) -> Rect {
    Rect::new(0, 0, size.width, size.height)
}

fn center(size: Size) -> Position {
    Position {
        x: (size.width / 2) as i32,
        y: (size.height / 2) as i32,
    }
}

fn max_x(size: Size) -> i32 {
    (size.width as i32 - 1).max(0)
}

fn max_y(size: Size) -> i32 {
    (size.height as i32 - 1).max(0)
}

/// The edge `position` is pressing against, given the motion `delta`. Pressing against two
/// edges (a corner) picks the axis the user is moving fastest along.
fn pushed_edge(position: Position, delta: (i32, i32), screen: Size) -> Option<Edge> {
    let horizontal = if position.x <= 0 && delta.0 < 0 {
        Some(Edge::Left)
    } else if position.x >= max_x(screen) && delta.0 > 0 {
        Some(Edge::Right)
    } else {
        None
    };
    let vertical = if position.y <= 0 && delta.1 < 0 {
        Some(Edge::Top)
    } else if position.y >= max_y(screen) && delta.1 > 0 {
        Some(Edge::Bottom)
    } else {
        None
    };

    match (horizontal, vertical) {
        (Some(h), Some(v)) => Some(if delta.0.abs() >= delta.1.abs() { h } else { v }),
        (h, v) => h.or(v),
    }
}

/// The edge an unclamped position left the screen through. Leaving through a corner picks
/// the axis it overshot furthest.
fn escaped_edge(moved: Position, screen: Size) -> Option<Edge> {
    let over_x = if moved.x < 0 {
        moved.x
    } else {
        (moved.x - max_x(screen)).max(0)
    };
    let over_y = if moved.y < 0 {
        moved.y
    } else {
        (moved.y - max_y(screen)).max(0)
    };

    let horizontal = match over_x {
        0 => None,
        x if x < 0 => Some((Edge::Left, -x)),
        x => Some((Edge::Right, x)),
    };
    let vertical = match over_y {
        0 => None,
        y if y < 0 => Some((Edge::Top, -y)),
        y => Some((Edge::Bottom, y)),
    };

    match (horizontal, vertical) {
        (Some(h), Some(v)) => Some(if h.1 >= v.1 { h.0 } else { v.0 }),
        (h, v) => h.or(v).map(|(edge, _)| edge),
    }
}

fn in_corner(position: Position, screen: Size, size: i32) -> bool {
    let near_x = position.x < size || position.x > max_x(screen) - size;
    let near_y = position.y < size || position.y > max_y(screen) - size;

    near_x && near_y
}

fn scale(value: i32, from: u32, to: u32) -> i32 {
    if from <= 1 {
        return (to / 2) as i32;
    }

    let scaled = i64::from(value) * i64::from(to.saturating_sub(1)) / i64::from(from - 1);

    scaled.clamp(0, i64::from(to.saturating_sub(1))) as i32
}

/// Where the pointer appears on the new screen: the same relative position along the shared
/// edge, a few pixels inside the opposite one.
fn map_entry(exit: Position, edge: Edge, from: Size, to: Size) -> Position {
    let inset_x = ENTRY_INSET.min(max_x(to));
    let inset_y = ENTRY_INSET.min(max_y(to));

    match edge {
        Edge::Right => Position {
            x: inset_x,
            y: scale(exit.y, from.height, to.height),
        },
        Edge::Left => Position {
            x: max_x(to) - inset_x,
            y: scale(exit.y, from.height, to.height),
        },
        Edge::Bottom => Position {
            x: scale(exit.x, from.width, to.width),
            y: inset_y,
        },
        Edge::Top => Position {
            x: scale(exit.x, from.width, to.width),
            y: max_y(to) - inset_y,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Shape;

    const FHD: Size = Size {
        width: 1920,
        height: 1080,
    };
    const QHD: Size = Size {
        width: 2560,
        height: 1440,
    };

    fn name(value: &str) -> MachineName {
        MachineName::new(value).unwrap()
    }

    fn pos(x: i32, y: i32) -> Position {
        Position { x, y }
    }

    fn router(shape: Shape, wrap: bool, slots: &[(&str, u8)]) -> Router {
        let layout = Layout::new(shape, wrap, slots.iter().map(|(n, s)| (name(n), *s))).unwrap();

        Router::new(layout, name("a"), FHD, SwitchPolicy::default())
    }

    /// a | b | c with every peer online.
    fn trio() -> Router {
        let mut router = router(Shape::Row, false, &[("a", 0), ("b", 1), ("c", 2)]);
        router.peer_up(name("b"), QHD);
        router.peer_up(name("c"), FHD);

        router
    }

    fn later(now: Instant, ms: u64) -> Instant {
        now + Duration::from_millis(ms)
    }

    #[test]
    fn pushing_past_the_right_edge_hands_over_with_proportional_entry() {
        let mut router = trio();
        let now = Instant::now();

        let transfer = router
            .on_local_motion(pos(1919, 540), (5, 0), Modifiers::default(), now)
            .unwrap();

        assert_eq!(transfer.to, name("b"));
        // Halfway down a 1080p screen is halfway down the 1440p one.
        assert_eq!(transfer.entry, pos(ENTRY_INSET, 540 * 1439 / 1079));
        assert_eq!(router.focus(), &Focus::Remote(name("b")));
    }

    #[test]
    fn staying_inside_or_moving_away_does_not_switch() {
        let mut router = trio();
        let now = Instant::now();

        assert!(
            router
                .on_local_motion(pos(1000, 500), (5, 0), Modifiers::default(), now)
                .is_none()
        );
        // At the right edge but moving left.
        assert!(
            router
                .on_local_motion(pos(1919, 500), (-5, 0), Modifiers::default(), now)
                .is_none()
        );
        // Wall on the left of the first machine.
        assert!(
            router
                .on_local_motion(pos(0, 500), (-5, 0), Modifiers::default(), now)
                .is_none()
        );
        assert_eq!(router.focus(), &Focus::Local);
    }

    #[test]
    fn offline_neighbour_is_a_wall() {
        let mut router = trio();
        let now = Instant::now();

        router.peer_down(&name("b"), now);

        assert!(
            router
                .on_local_motion(pos(1919, 500), (5, 0), Modifiers::default(), now)
                .is_none()
        );
    }

    #[test]
    fn easy_mouse_modes() {
        let now = Instant::now();
        let push = |router: &mut Router, modifiers| {
            router.on_local_motion(pos(1919, 500), (5, 0), modifiers, now)
        };
        let with = |mode, modifiers| {
            let mut router = trio();
            router.set_policy(SwitchPolicy {
                easy_mouse: mode,
                ..SwitchPolicy::default()
            });

            push(&mut router, modifiers).is_some()
        };
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };

        assert!(!with(EasyMouse::Disabled, Modifiers::default()));
        assert!(!with(EasyMouse::Disabled, shift));
        assert!(with(EasyMouse::Enabled, Modifiers::default()));
        assert!(!with(EasyMouse::RequireShift, Modifiers::default()));
        assert!(with(EasyMouse::RequireShift, shift));
        assert!(!with(EasyMouse::RequireShift, ctrl));
        assert!(!with(EasyMouse::RequireCtrl, Modifiers::default()));
        assert!(with(EasyMouse::RequireCtrl, ctrl));
    }

    #[test]
    fn corner_blocking_only_applies_in_the_corner_zone() {
        let now = Instant::now();
        let mut router = trio();
        router.set_policy(SwitchPolicy {
            block_corners: true,
            ..SwitchPolicy::default()
        });

        assert!(
            router
                .on_local_motion(pos(1919, 3), (5, 0), Modifiers::default(), now)
                .is_none()
        );
        assert!(
            router
                .on_local_motion(pos(1919, 1078), (5, 0), Modifiers::default(), now)
                .is_none()
        );
        assert!(
            router
                .on_local_motion(pos(1919, 540), (5, 0), Modifiers::default(), now)
                .is_some()
        );
    }

    #[test]
    fn moving_back_returns_to_the_local_machine() {
        let mut router = trio();
        let now = Instant::now();

        router
            .on_local_motion(pos(1919, 540), (5, 0), Modifiers::default(), now)
            .unwrap();

        // Entered at x=2; move left past the edge.
        let step = router
            .on_remote_motion((-10, 0), Modifiers::default(), later(now, 500))
            .unwrap();

        let RemoteStep::Switched(transfer) = step else {
            panic!("expected a switch, got {step:?}");
        };

        assert_eq!(transfer.from, name("b"));
        assert_eq!(transfer.to, name("a"));
        assert_eq!(transfer.entry.x, 1919 - ENTRY_INSET);
        assert_eq!(router.focus(), &Focus::Local);
    }

    #[test]
    fn crossing_a_middle_machine_goes_straight_to_the_next_one() {
        let mut router = trio();
        let now = Instant::now();

        router
            .on_local_motion(pos(1919, 700), (5, 0), Modifiers::default(), now)
            .unwrap();
        router
            .on_remote_motion((2557, 0), Modifiers::default(), later(now, 200))
            .unwrap();
        // Now at the right edge of b (x = 2559). Push on.
        let step = router
            .on_remote_motion((10, 0), Modifiers::default(), later(now, 400))
            .unwrap();

        let RemoteStep::Switched(transfer) = step else {
            panic!("expected a switch");
        };

        assert_eq!((transfer.from.as_str(), transfer.to.as_str()), ("b", "c"));
        assert_eq!(router.focus(), &Focus::Remote(name("c")));
    }

    #[test]
    fn motion_inside_a_remote_screen_moves_the_virtual_cursor_and_clamps() {
        let mut router = trio();
        let now = Instant::now();

        router
            .on_local_motion(pos(1919, 500), (5, 0), Modifiers::default(), now)
            .unwrap();
        let start = router.cursor();

        let step = router
            .on_remote_motion((30, 20), Modifiers::default(), later(now, 200))
            .unwrap();

        assert_eq!(
            step,
            RemoteStep::Moved {
                position: pos(start.x + 30, start.y + 20)
            }
        );

        // Walls clamp instead of switching: the bottom of b has no neighbour.
        let step = router
            .on_remote_motion((0, 100_000), Modifiers::default(), later(now, 300))
            .unwrap();

        assert_eq!(
            step,
            RemoteStep::Moved {
                position: pos(start.x + 30, 1439)
            }
        );
    }

    #[test]
    fn cooldown_prevents_ping_pong() {
        let mut router = trio();
        let now = Instant::now();

        router
            .on_local_motion(pos(1919, 500), (5, 0), Modifiers::default(), now)
            .unwrap();

        // Immediately pushing back is ignored (and clamped)...
        let step = router
            .on_remote_motion((-50, 0), Modifiers::default(), later(now, 10))
            .unwrap();
        assert!(matches!(step, RemoteStep::Moved { .. }));
        assert_eq!(router.focus(), &Focus::Remote(name("b")));

        // ...but works once the cooldown has passed.
        let step = router
            .on_remote_motion((-50, 0), Modifiers::default(), later(now, 400))
            .unwrap();
        assert!(matches!(step, RemoteStep::Switched(_)));
    }

    #[test]
    fn wrap_connects_the_ends() {
        let mut router = router(Shape::Row, true, &[("a", 0), ("b", 1)]);
        router.peer_up(name("b"), FHD);
        let now = Instant::now();

        let transfer = router
            .on_local_motion(pos(0, 300), (-5, 0), Modifiers::default(), now)
            .unwrap();

        assert_eq!(transfer.to, name("b"));
        assert_eq!(transfer.entry.x, 1919 - ENTRY_INSET);
    }

    #[test]
    fn grid_vertical_crossing_maps_the_x_coordinate() {
        let mut router = router(
            Shape::Grid,
            false,
            &[("a", 0), ("b", 1), ("c", 2), ("d", 3)],
        );
        router.peer_up(name("c"), QHD);
        let now = Instant::now();

        let transfer = router
            .on_local_motion(pos(960, 1079), (0, 4), Modifiers::default(), now)
            .unwrap();

        assert_eq!(transfer.to, name("c"));
        assert_eq!(transfer.entry, pos(960 * 2559 / 1919, ENTRY_INSET));
    }

    #[test]
    fn corner_push_follows_the_dominant_axis() {
        // Bottom-right corner of `a` with c below and b to the right.
        let mut router = router(Shape::Grid, false, &[("a", 0), ("b", 1), ("c", 2)]);
        router.peer_up(name("b"), FHD);
        router.peer_up(name("c"), FHD);
        let now = Instant::now();

        let transfer = router
            .on_local_motion(pos(1919, 1079), (2, 9), Modifiers::default(), now)
            .unwrap();

        assert_eq!(transfer.to, name("c"));
    }

    #[test]
    fn jump_uses_the_last_position_or_the_centre() {
        let mut router = trio();
        let now = Instant::now();

        let first = router.jump(&name("c"), now).unwrap();
        assert_eq!(first.entry, pos(960, 540));
        assert_eq!(router.focus(), &Focus::Remote(name("c")));

        router
            .on_remote_motion((100, 50), Modifiers::default(), later(now, 200))
            .unwrap();
        router.jump(&name("b"), later(now, 400)).unwrap();
        let back = router.jump(&name("c"), later(now, 600)).unwrap();

        assert_eq!(back.entry, pos(1060, 590));
    }

    #[test]
    fn jump_home_restores_the_local_pointer() {
        let mut router = trio();
        let now = Instant::now();

        router.on_local_motion(pos(700, 300), (1, 0), Modifiers::default(), now);
        router.jump(&name("b"), now).unwrap();
        let home = router.jump(&name("a"), later(now, 300)).unwrap();

        assert_eq!(home.entry, pos(700, 300));
        assert_eq!(router.focus(), &Focus::Local);
    }

    #[test]
    fn jump_ignores_unknown_offline_and_current_machines() {
        let mut router = trio();
        let now = Instant::now();

        assert!(router.jump(&name("a"), now).is_none());
        assert!(router.jump(&name("nobody"), now).is_none());

        router.peer_down(&name("c"), now);
        assert!(router.jump(&name("c"), now).is_none());
    }

    #[test]
    fn losing_the_focused_peer_returns_control() {
        let mut router = trio();
        let now = Instant::now();

        router.jump(&name("b"), now).unwrap();
        let transfer = router.peer_down(&name("b"), later(now, 1)).unwrap();

        assert_eq!(transfer.to, name("a"));
        assert_eq!(router.focus(), &Focus::Local);
        assert!(router.peer_down(&name("b"), later(now, 2)).is_none());
    }

    #[test]
    fn peers_outside_the_layout_are_ignored() {
        let mut router = trio();

        router.peer_up(name("stranger"), FHD);

        assert!(!router.is_online(&name("stranger")));
    }

    #[test]
    fn screen_resize_keeps_the_cursor_on_screen() {
        let mut router = trio();
        let now = Instant::now();

        router.jump(&name("b"), now).unwrap();
        router
            .on_remote_motion((1000, 500), Modifiers::default(), later(now, 200))
            .unwrap();
        router.peer_up(
            name("b"),
            Size {
                width: 800,
                height: 600,
            },
        );

        assert_eq!(router.cursor(), pos(799, 599));
    }

    #[test]
    fn mapping_handles_degenerate_screens() {
        let tiny = Size {
            width: 1,
            height: 1,
        };

        assert_eq!(map_entry(pos(5, 5), Edge::Right, FHD, tiny), pos(0, 0));
        assert_eq!(scale(10, 1, 100), 50);
        assert_eq!(scale(-4, 100, 50), 0);
    }
}
