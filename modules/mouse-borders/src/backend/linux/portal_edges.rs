//! Switching at screen edges on Wayland, through the desktop's input-capture portal
//! (`org.freedesktop.portal.InputCapture`, in GNOME since 45 and KDE Plasma 6).
//!
//! Wayland does not let an application read the pointer position, so the engine cannot see the
//! pointer reach an edge. The portal can: we ask it for "barriers" along the screen edges that
//! lead to another machine, and it tells us when the pointer runs into one. The portal then
//! captures *all* input (the desktop stops delivering it to any window) until we give it back
//! with `Release`. We do not need the libei stream (our own devices reader and grabber already do
//! the capturing), so the capture is handed back at once, before the engine even hears about the
//! hit: every hit costs one round trip, never a wait for the engine. Barriers only exist for
//! edges that lead to a connected machine, so every other edge keeps behaving like a plain
//! screen edge, and they stay clear of the corners.
//!
//! The portal asks the user once per session whether Aujar may do this.

use std::{os::unix::net::UnixStream, time::Duration};

use ashpd::desktop::input_capture::{
    ActivatedBarrier, Barrier, Capabilities, ConnectToEISOptions, CreateSessionOptions,
    InputCapture, ReleaseOptions, StartOptions,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use reis::{ei, event::DeviceCapability};
use tokio::sync::mpsc;

use super::super::{EdgeControl, EdgeEvent, EdgeHit, EdgeWatcher};
use crate::layout::Edge;

/// Barriers stop this far from a corner of the desktop, so a pointer in a corner is never
/// captured (it would be, over and over, while the user pushes into the corner).
const CORNER_MARGIN: i32 = 24;

/// A `Disabled` that comes this soon after we disabled capturing ourselves is our own.
const OWN_DISABLE_WINDOW: Duration = Duration::from_secs(1);

/// How close (in pixels) to a barrier the pointer must be for us to tell which one it was, when
/// the desktop does not say.
const NEAR_EDGE: f64 = 3.0;

pub struct PortalEdges;

#[async_trait]
impl EdgeWatcher for PortalEdges {
    async fn run(
        self: Box<Self>,
        events: mpsc::Sender<EdgeEvent>,
        control: mpsc::UnboundedReceiver<EdgeControl>,
    ) {
        if let Err(reason) = watch(&events, control).await {
            let _ = events.send(EdgeEvent::Unavailable(reason)).await;
        }
    }
}

fn describe(error: impl std::fmt::Display) -> String {
    format!("{error}")
}

// ---- Geometry (no desktop needed) -------------------------------------------------------

/// A piece of the desktop, in the desktop's coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Zone {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl Zone {
    fn right(self) -> i32 {
        self.x + self.width
    }

    fn bottom(self) -> i32 {
        self.y + self.height
    }
}

/// The smallest zone that holds all of them.
fn bounding_zone(zones: &[Zone]) -> Option<Zone> {
    let first = *zones.first()?;

    Some(zones.iter().fold(first, |all, zone| {
        let x = all.x.min(zone.x);
        let y = all.y.min(zone.y);

        Zone {
            x,
            y,
            width: all.right().max(zone.right()) - x,
            height: all.bottom().max(zone.bottom()) - y,
        }
    }))
}

/// One barrier to set: a line along a screen edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Planned {
    id: u32,
    edge: Edge,
    /// `x1, y1, x2, y2`
    line: (i32, i32, i32, i32),
}

/// The barriers for the wanted edges: along the outer sides of the whole desktop only, so the
/// seam between two monitors stays free, and short of the desktop's corners.
///
/// The desktop is strict about where a barrier goes (GNOME checks each one against the monitor
/// edges): a vertical line sits exactly on `x` or `x + width`, a horizontal one on `y` or
/// `y + height`; along the edge it stays inside the monitor, and a horizontal line must end one
/// pixel before the right edge (`right - 1`), or the desktop refuses it.
fn plan_barriers(zones: &[Zone], wanted: &[Edge]) -> Vec<Planned> {
    let Some(desktop) = bounding_zone(zones) else {
        return Vec::new();
    };
    let mut planned = Vec::new();

    for edge in wanted {
        for zone in zones {
            // Along the edge: from `start` to `end`, short of a corner of the desktop.
            let (on_outer_side, start, end) = match edge {
                Edge::Left | Edge::Right => (
                    if *edge == Edge::Left {
                        zone.x == desktop.x
                    } else {
                        zone.right() == desktop.right()
                    },
                    zone.y
                        + if zone.y == desktop.y {
                            CORNER_MARGIN
                        } else {
                            0
                        },
                    zone.bottom()
                        - 1
                        - if zone.bottom() == desktop.bottom() {
                            CORNER_MARGIN
                        } else {
                            0
                        },
                ),
                Edge::Top | Edge::Bottom => (
                    if *edge == Edge::Top {
                        zone.y == desktop.y
                    } else {
                        zone.bottom() == desktop.bottom()
                    },
                    zone.x
                        + if zone.x == desktop.x {
                            CORNER_MARGIN
                        } else {
                            0
                        },
                    zone.right()
                        - 1
                        - if zone.right() == desktop.right() {
                            CORNER_MARGIN
                        } else {
                            0
                        },
                ),
            };

            if !on_outer_side || end <= start {
                continue;
            }

            let line = match edge {
                Edge::Left => (zone.x, start, zone.x, end),
                Edge::Right => (zone.right(), start, zone.right(), end),
                Edge::Top => (start, zone.y, end, zone.y),
                Edge::Bottom => (start, zone.bottom(), end, zone.bottom()),
            };

            planned.push(Planned {
                id: planned.len() as u32 + 1,
                edge: *edge,
                line,
            });
        }
    }

    planned
}

/// How far along `edge` a pointer at `cursor` is: `0.0` at the top or left end, `1.0` at the
/// bottom or right end of the desktop.
fn along(edge: Edge, cursor: (f64, f64), desktop: Zone) -> f64 {
    let fraction = |at: f64, start: i32, length: i32| {
        if length <= 1 {
            0.0
        } else {
            ((at - f64::from(start)) / f64::from(length - 1)).clamp(0.0, 1.0)
        }
    };

    match edge {
        Edge::Left | Edge::Right => fraction(cursor.1, desktop.y, desktop.height),
        Edge::Top | Edge::Bottom => fraction(cursor.0, desktop.x, desktop.width),
    }
}

/// Which of the wanted edges a pointer at `cursor` is on, when the desktop did not say which
/// barrier it hit (the nearest one, if it is close enough).
fn edge_near(cursor: (f64, f64), desktop: Zone, wanted: &[Edge]) -> Option<Edge> {
    wanted
        .iter()
        .map(|edge| {
            let distance = match edge {
                Edge::Left => cursor.0 - f64::from(desktop.x),
                Edge::Right => f64::from(desktop.right() - 1) - cursor.0,
                Edge::Top => cursor.1 - f64::from(desktop.y),
                Edge::Bottom => f64::from(desktop.bottom() - 1) - cursor.1,
            };

            (*edge, distance.abs())
        })
        .filter(|(_, distance)| *distance <= NEAR_EDGE)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(edge, _)| edge)
}

// ---- The portal ---------------------------------------------------------------------------

type Session = ashpd::desktop::Session<InputCapture>;

struct Portal<'a> {
    capture: &'a InputCapture,
    session: Session,
    zones: Vec<Zone>,
    zone_set: u32,
    wanted: Vec<Edge>,
    planned: Vec<Planned>,
    /// When we last switched capturing off ourselves.
    disabled_by_us: Option<std::time::Instant>,
}

impl Portal<'_> {
    async fn refresh_zones(&mut self) -> Result<(), String> {
        let zones = self
            .capture
            .zones(&self.session, Default::default())
            .await
            .map_err(describe)?
            .response()
            .map_err(describe)?;

        self.zone_set = zones.zone_set();
        self.zones = zones
            .regions()
            .iter()
            .map(|region| Zone {
                x: region.x_offset(),
                y: region.y_offset(),
                width: region.width() as i32,
                height: region.height() as i32,
            })
            .collect();

        Ok(())
    }

    /// Barriers for `wanted`, replacing the ones set before. The portal wants capturing off while
    /// barriers change, and on again afterwards.
    async fn watch_edges(&mut self, wanted: Vec<Edge>) -> Result<Option<String>, String> {
        let _ = self
            .capture
            .disable(&self.session, Default::default())
            .await;

        self.wanted = wanted;
        self.planned = plan_barriers(&self.zones, &self.wanted);

        if self.planned.is_empty() {
            return Ok(None);
        }

        let barriers: Vec<Barrier> = self
            .planned
            .iter()
            .filter_map(|plan| {
                std::num::NonZeroU32::new(plan.id).map(|id| Barrier::new(id, plan.line))
            })
            .collect();

        let response = self
            .capture
            .set_pointer_barriers(&self.session, &barriers, self.zone_set, Default::default())
            .await
            .map_err(describe)?
            .response()
            .map_err(describe)?;

        let failed = response.failed_barriers().len();

        self.capture
            .enable(&self.session, Default::default())
            .await
            .map_err(describe)?;

        Ok((failed > 0).then(|| {
            format!(
                "the desktop refused {failed} of {} screen edge barriers",
                barriers.len()
            )
        }))
    }

    /// Give the captured input back to the desktop. Only valid while a capture is active (the
    /// desktop says "Capture not active" otherwise, which is harmless). The pointer stays where
    /// it is: warping it somewhere could land it on a barrier again.
    async fn release(&self, token: u32) -> Result<(), String> {
        self.capture
            .release(
                &self.session,
                ReleaseOptions::default().set_activation_id(token),
            )
            .await
            .map_err(describe)
    }

    fn desktop(&self) -> Option<Zone> {
        bounding_zone(&self.zones)
    }
}

async fn create_session(
    capture: &InputCapture,
    capabilities: ashpd::enumflags2::BitFlags<Capabilities>,
) -> Result<Session, String> {
    // Newer portals first (`CreateSession2` and `Start`), the original call otherwise.
    match capture.create_session2(Default::default()).await {
        Ok(session) => {
            capture
                .start(
                    &session,
                    None,
                    StartOptions::default().set_capabilities(capabilities),
                )
                .await
                .map_err(describe)?
                .response()
                .map_err(describe)?;

            Ok(session)
        }
        Err(ashpd::Error::RequiresVersion(..)) => capture
            .create_session(
                None,
                CreateSessionOptions::default().set_capabilities(capabilities),
            )
            .await
            .map(|(session, _)| session)
            .map_err(describe),
        Err(error) => Err(describe(error)),
    }
}

/// The libei connection the portal needs: finish the handshake, ask for the devices, and then
/// let the events pass (the engine reads the real devices itself).
async fn eis_task(stream: UnixStream) {
    let Ok(context) = ei::Context::new(stream) else {
        return;
    };
    let Ok((_connection, mut events)) = context
        .handshake_tokio("aujar-mouse-borders", ei::handshake::ContextType::Receiver)
        .await
    else {
        return;
    };

    while let Some(event) = events.next().await {
        match event {
            Ok(reis::event::EiEvent::SeatAdded(added)) => {
                added.seat.bind_capabilities(
                    DeviceCapability::Pointer
                        | DeviceCapability::PointerAbsolute
                        | DeviceCapability::Keyboard
                        | DeviceCapability::Scroll
                        | DeviceCapability::Button,
                );
                let _ = context.flush();
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

async fn watch(
    events: &mpsc::Sender<EdgeEvent>,
    mut control: mpsc::UnboundedReceiver<EdgeControl>,
) -> Result<(), String> {
    let capture = InputCapture::new()
        .await
        .map_err(|error| format!("the input-capture portal is not available ({error})"))?;
    let capabilities = Capabilities::Keyboard | Capabilities::Pointer;
    let session = create_session(&capture, capabilities)
        .await
        .map_err(|error| format!("the desktop did not allow input capture ({error})"))?;

    let fd = capture
        .connect_to_eis(&session, ConnectToEISOptions::default())
        .await
        .map_err(describe)?;

    // The libei events are not `Send`, so that connection lives on a thread of its own.
    let stream = UnixStream::from(fd);

    std::thread::Builder::new()
        .name("aujar-eis".into())
        .spawn(move || {
            if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                runtime.block_on(eis_task(stream));
            }
        })
        .map_err(describe)?;

    let mut portal = Portal {
        capture: &capture,
        session,
        zones: Vec::new(),
        zone_set: 0,
        wanted: Vec::new(),
        planned: Vec::new(),
        disabled_by_us: None,
    };

    portal.refresh_zones().await?;

    let mut activated = capture.receive_activated().await.map_err(describe)?;
    let mut disabled = capture.receive_disabled().await.map_err(describe)?;
    let mut zones_changed = capture.receive_zones_changed().await.map_err(describe)?;

    if events.send(EdgeEvent::Ready).await.is_err() {
        return Ok(());
    }

    loop {
        tokio::select! {
            Some(hit) = activated.next() => {
                let Some(token) = hit.activation_id() else { continue };
                let cursor = hit
                    .cursor_position()
                    .map_or((0.0, 0.0), |(x, y)| (f64::from(x), f64::from(y)));

                // Everything is held until this is given back, so do that first.
                let given_back = portal.release(token).await;

                let Some(desktop) = portal.desktop() else { continue };
                let edge = match hit.barrier_id() {
                    Some(ActivatedBarrier::Barrier(id)) => portal
                        .planned
                        .iter()
                        .find(|plan| plan.id == id.get())
                        .map(|plan| plan.edge),
                    _ => None,
                }
                .or_else(|| edge_near(cursor, desktop, &portal.wanted));

                if let Err(reason) = given_back {
                    let _ = events
                        .send(EdgeEvent::Note(format!("cannot hand the pointer back: {reason}")))
                        .await;
                }

                if let Some(edge) = edge {
                    let hit = EdgeHit {
                        edge,
                        along: along(edge, cursor, desktop),
                    };

                    if events.send(EdgeEvent::Hit(hit)).await.is_err() {
                        return Ok(());
                    }
                }
            }
            Some(message) = control.recv() => match message {
                EdgeControl::Watch(edges) => {
                    match portal.watch_edges(edges).await {
                        Ok(Some(note)) => {
                            let _ = events.send(EdgeEvent::Note(note)).await;
                        }
                        Ok(None) => {}
                        Err(reason) => {
                            let _ = events
                                .send(EdgeEvent::Note(format!("cannot set screen edge barriers: {reason}")))
                                .await;
                        }
                    }
                }
            },
            Some(_) = disabled.next() => {
                // The desktop switched capturing off by itself (the session may have been
                // reset): barriers again, with the edges the engine last asked for. Our own
                // `disable` (when the barriers change) is not that: leave it alone, or every
                // change would start the next one.
                let own = portal
                    .disabled_by_us
                    .is_some_and(|at| at.elapsed() < OWN_DISABLE_WINDOW);

                if !own {
                    let wanted = portal.wanted.clone();

                    let _ = portal.watch_edges(wanted).await;
                }
            }
            Some(_) = zones_changed.next() => {
                // Monitors came or went: the barriers must follow the new screen edges.
                if portal.refresh_zones().await.is_ok() {
                    let wanted = portal.wanted.clone();

                    let _ = portal.watch_edges(wanted).await;
                }
            }
            else => break,
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: Zone = Zone {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };

    #[test]
    fn one_screen_gets_barriers_on_the_edge_coordinates_short_of_the_corners() {
        let plan = plan_barriers(
            &[LAPTOP],
            &[Edge::Left, Edge::Right, Edge::Top, Edge::Bottom],
        );

        let lines: Vec<_> = plan
            .iter()
            .map(|planned| (planned.edge, planned.line))
            .collect();

        // Vertical lines sit on x = 0 and x = width, horizontal ones on y = 0 and y = height
        // (that is what the desktop accepts); every line stops CORNER_MARGIN short of a corner,
        // and horizontal lines end at `right - 1 - margin`.
        assert_eq!(
            lines,
            vec![
                (Edge::Left, (0, 24, 0, 1055)),
                (Edge::Right, (1920, 24, 1920, 1055)),
                (Edge::Top, (24, 0, 1895, 0)),
                (Edge::Bottom, (24, 1080, 1895, 1080)),
            ]
        );

        // Ids are unique and not zero (the portal needs that).
        let mut ids: Vec<_> = plan.iter().map(|planned| planned.id).collect();

        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids, vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_horizontal_barrier_never_reaches_the_right_edge_of_the_desktop() {
        // The desktop refuses a horizontal line that touches `x = right`.
        for planned in plan_barriers(&[LAPTOP], &[Edge::Top, Edge::Bottom]) {
            assert!(planned.line.2 < LAPTOP.right(), "{planned:?}");
        }
    }

    #[test]
    fn only_wanted_edges_get_barriers() {
        let plan = plan_barriers(&[LAPTOP], &[Edge::Right]);

        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].edge, Edge::Right);
        assert!(plan_barriers(&[LAPTOP], &[]).is_empty());
        assert!(plan_barriers(&[], &[Edge::Right]).is_empty());
    }

    #[test]
    fn the_seam_between_two_monitors_stays_free() {
        // Laptop screen with a monitor to its right, the monitor a little higher.
        let monitor = Zone {
            x: 1920,
            y: -200,
            width: 2560,
            height: 1440,
        };
        let plan = plan_barriers(&[LAPTOP, monitor], &[Edge::Left, Edge::Right]);

        // Left: only the laptop (x = 0). It reaches neither the top nor the bottom of the
        // desktop (the monitor is taller), so it is not trimmed. Right: only the monitor (the
        // far side), trimmed at both its corners.
        assert_eq!(
            plan.iter()
                .map(|planned| (planned.edge, planned.line))
                .collect::<Vec<_>>(),
            vec![
                (Edge::Left, (0, 0, 0, 1079)),
                (Edge::Right, (4480, -176, 4480, 1215)),
            ]
        );
    }

    #[test]
    fn a_zone_too_small_for_the_margins_gets_no_barrier() {
        let tiny = Zone {
            x: 0,
            y: 0,
            width: 40,
            height: 40,
        };

        assert!(plan_barriers(&[tiny], &[Edge::Left, Edge::Top]).is_empty());
    }

    #[test]
    fn the_desktop_is_the_box_around_all_zones() {
        let monitor = Zone {
            x: 1920,
            y: -200,
            width: 2560,
            height: 1440,
        };

        assert_eq!(
            bounding_zone(&[LAPTOP, monitor]),
            Some(Zone {
                x: 0,
                y: -200,
                width: 4480,
                height: 1440
            })
        );
        assert_eq!(bounding_zone(&[]), None);
    }

    #[test]
    fn how_far_along_an_edge_is_a_fraction_of_the_desktop() {
        assert_eq!(along(Edge::Right, (1919.0, 0.0), LAPTOP), 0.0);
        assert_eq!(along(Edge::Right, (1919.0, 1079.0), LAPTOP), 1.0);
        assert!((along(Edge::Left, (0.0, 539.5), LAPTOP) - 0.5).abs() < 1e-6);
        assert!((along(Edge::Top, (959.5, 0.0), LAPTOP) - 0.5).abs() < 1e-6);

        // Outside the desktop stays on the edge.
        assert_eq!(along(Edge::Left, (0.0, 5000.0), LAPTOP), 1.0);
        assert_eq!(along(Edge::Left, (0.0, -40.0), LAPTOP), 0.0);
    }

    #[test]
    fn the_edge_is_guessed_from_the_position_when_the_desktop_does_not_say() {
        let wanted = [Edge::Left, Edge::Right];

        assert_eq!(
            edge_near((1919.0, 400.0), LAPTOP, &wanted),
            Some(Edge::Right)
        );
        assert_eq!(edge_near((1.0, 400.0), LAPTOP, &wanted), Some(Edge::Left));
        // Not near a wanted edge (the bottom is not wanted, the middle is nowhere).
        assert_eq!(edge_near((900.0, 1079.0), LAPTOP, &wanted), None);
        assert_eq!(edge_near((900.0, 500.0), LAPTOP, &wanted), None);
    }
}
