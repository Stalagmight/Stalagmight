//! The one post-edit anchor algorithm (plan 4.1) and the frame change
//! (plan 4.3, "measure, do not re-derive").
//!
//! A command takes an [`AnchorSnapshot`] before it mutates the model, mutates
//! it, then calls [`Project::settle_anchors`] with an [`EditContext`] saying
//! what kind of edit it was. Settling rewrites only anchors: datum
//! corrections and the placement list. Every station's position then follows
//! from the anchors by the solver ([`Project::raw_positions`]).
//!
//! Rules, per component of the post-edit model:
//!
//! - **Holds a datum**: its first datum in list order places it. If that datum
//!   was a placement datum before the edit (and its GPS was not edited), it
//!   keeps its correction, so the piece stays. Otherwise (a GPS fix promoted
//!   because the old placement datum left the component, a new datum, a GPS
//!   edit) the piece goes onto its GPS: correction cleared. Every datum that
//!   is a GPS fix after the edit has its correction cleared (demotion).
//! - **Holds no datum**: the *still point* `s*` is the first that applies:
//!   (0) a placement the command declares it set or moved
//!   ([`EditContext::placed`]) is authoritative; (a) a geometry edit of line L and
//!   the component holds L's pre-edit first station: that station; (b) the
//!   component holds a pre-edit placement station (preferring the merge's
//!   stationary piece's): that station; (c) the first station of the edited
//!   line with a pre-edit position; (d) its lowest station with a pre-edit
//!   position. The component is laid out with `s*` at its pre-edit position,
//!   and its placement is stored at its *placement station*: the explicit
//!   placement's, else the surviving pre-edit placement station (the merge's
//!   stationary one first), else its lowest station. So the least-squares
//!   gauge (the placement station) only changes when its station goes. A
//!   component with no pre-edit station at all (a new free piece) needs a
//!   declared placement; without one it is laid out from its lowest station
//!   at the origin.
//! - **Declared corrections** ([`EditContext::corrected`], Add .stalx) are
//!   kept as the command computed them, against the post-edit frame.
//! - **Frame change** (the first resolvable datum is another datum than
//!   before): positions are first settled in the old frame (a promoted datum
//!   onto its GPS against the old frame's GPS, as today), then everything is
//!   translated by `T = -(new frame station's position)`: placements by `T`,
//!   and every placement datum gets `correction := position + T -
//!   gps_to_local(gps, new frame gps)`, so nothing moves relative to anything
//!   else and the new frame station sits at the origin. A first datum on a
//!   survey with none translates the survey onto it; a later new datum is
//!   placed on its GPS. Removing the last datum changes no frame (nothing
//!   moves).

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{gps_to_local, GpsCoordinates, LineId, StationId};

use super::positions::Layout;
use super::{Placement, Project, RawPositions, Topology, ZeroDatum, CORRECTION_EPSILON_M};

/// The pre-edit state the anchor algorithm needs: raw positions, datums and
/// their roles, placements, line starts and the old topology.
#[derive(Debug, Clone)]
pub struct AnchorSnapshot {
    topology: Arc<Topology>,
    positions: RawPositions,
    datums: Vec<ZeroDatum>,
    was_placement: Vec<bool>,
    placements: Vec<Placement>,
    line_starts: BTreeMap<LineId, StationId>,
}

impl AnchorSnapshot {
    /// A station's pre-edit horizontal position.
    pub fn position(&self, station: StationId) -> Option<[f64; 2]> {
        self.positions.xy(station)
    }

    /// The pre-edit raw positions.
    pub fn positions(&self) -> &RawPositions {
        &self.positions
    }

    /// The pre-edit frame datum's index.
    pub fn frame(&self) -> Option<usize> {
        self.positions.frame()
    }

    /// The pre-edit topology.
    pub fn topology(&self) -> &Topology {
        &self.topology
    }

    /// The pre-edit datums.
    pub fn datums(&self) -> &[ZeroDatum] {
        &self.datums
    }

    /// The pre-edit placements.
    pub fn placements(&self) -> &[Placement] {
        &self.placements
    }
}

/// The two sides of a merge, by pre-edit station ids: a station of the piece
/// that stays and one of the piece that moves (plan 4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeSides {
    pub stationary: StationId,
    pub moving: StationId,
}

/// A GPS edit of one datum: its post-edit index and its GPS before the edit.
#[derive(Debug, Clone, Copy)]
pub struct GpsEdit {
    pub datum: usize,
    pub previous: GpsCoordinates,
}

/// What kind of edit ran, for [`Project::settle_anchors`].
#[derive(Debug, Clone, Default)]
pub struct EditContext {
    /// A geometry edit of this line (UpdateShot, insert, delete, prepend,
    /// split shot at distance, range delete): rule (a).
    pub geometry_line: Option<LineId>,
    /// The line the edit worked on: rule (c). [`Self::geometry`] sets it too.
    pub edited_line: Option<LineId>,
    /// A merge: the stationary piece's placement station is preferred.
    pub merge: Option<MergeSides>,
    /// Stations whose datums now sit on another id, `(old, new)`: a merge's
    /// retired station and its survivor, a detach's station and the new one.
    /// Used to recognise a datum across the edit.
    pub renamed: Vec<(StationId, StationId)>,
    /// A GPS edit of one datum (its correction is cleared).
    pub gps_edit: Option<GpsEdit>,
    /// Stations whose placement the command set or moved itself (Move
    /// Segment, an import's manual placement, Add .stalx): authoritative, in
    /// post-edit coordinates. Any other entry of the placement list is only
    /// pre-edit state (a re-keyed or stale entry is ignored).
    pub placed: Vec<StationId>,
    /// Datums (post-edit indices) whose correction the command set itself
    /// (Add .stalx): kept as is, in post-edit coordinates.
    pub corrected: Vec<usize>,
}

impl EditContext {
    /// No particular edit (topology changes with no line to favour).
    pub fn none() -> Self {
        Self::default()
    }

    /// A geometry edit of `line`.
    pub fn geometry(line: LineId) -> Self {
        Self {
            geometry_line: Some(line),
            edited_line: Some(line),
            ..Self::default()
        }
    }

    /// A non-geometry edit centred on `line` (detach, split, combine).
    pub fn line(line: LineId) -> Self {
        Self {
            edited_line: Some(line),
            ..Self::default()
        }
    }

    /// A merge whose `moving` side moved onto `stationary`.
    pub fn merge(stationary: StationId, moving: StationId) -> Self {
        Self {
            merge: Some(MergeSides { stationary, moving }),
            ..Self::default()
        }
    }

    /// Record that datums on `old` now sit on `new`.
    #[must_use]
    pub fn with_renamed(mut self, old: StationId, new: StationId) -> Self {
        self.renamed.push((old, new));
        self
    }

    /// Record a GPS edit of post-edit datum `datum`, whose GPS was `previous`.
    #[must_use]
    pub fn with_gps_edit(mut self, datum: usize, previous: GpsCoordinates) -> Self {
        self.gps_edit = Some(GpsEdit { datum, previous });
        self
    }

    /// Record that the command set the placement on `station` itself.
    #[must_use]
    pub fn with_placed(mut self, station: StationId) -> Self {
        self.placed.push(station);
        self
    }

    /// Record that the command set the correction of post-edit datum `datum`.
    #[must_use]
    pub fn with_corrected(mut self, datum: usize) -> Self {
        self.corrected.push(datum);
        self
    }
}

/// What [`Project::settle_anchors`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnchorReport {
    /// The frame datum changed (another datum, or a first one).
    pub frame_changed: bool,
    /// The translation applied by a frame change (zero otherwise).
    pub translation: [f64; 2],
    /// Datums (post-edit indices) whose piece was put onto its GPS: promoted
    /// fixes, new placement datums, GPS-edited placement datums.
    pub placed_on_gps: Vec<usize>,
    /// Datums whose correction was cleared because they are GPS fixes now.
    pub demoted: Vec<usize>,
    /// Pre-edit placement stations no longer used as a placement.
    pub placements_dropped: Vec<StationId>,
}

impl Project {
    /// Record what [`Self::settle_anchors`] needs from the state before an
    /// edit.
    pub fn anchor_snapshot(&self) -> AnchorSnapshot {
        let topology = self.topology_arc();
        let positions = self.raw_positions();
        let mut was_placement = vec![false; self.datums.len()];
        for anchor in positions.anchors() {
            if let Some(datum) = anchor.datum() {
                was_placement[datum] = true;
            }
        }
        AnchorSnapshot {
            topology,
            positions,
            datums: self.datums.clone(),
            was_placement,
            placements: self.placements.clone(),
            line_starts: self.lines.iter().map(|l| (l.id, l.start)).collect(),
        }
    }

    /// The correction that holds placement datum `datum`'s station at local
    /// `at` in the current frame: `at - gps_to_local(gps, frame gps)` (the
    /// frame datum: `at`), `None` when it is below [`CORRECTION_EPSILON_M`]
    /// or there is no frame. The formula of a frame change and of Add .stalx.
    pub fn correction_to_hold(&self, datum: usize, at: [f64; 2]) -> Option<[f64; 2]> {
        let frame = self.frame_datum()?;
        let d = self.datums.get(datum)?;
        let base = if datum == frame {
            [0.0, 0.0]
        } else {
            let (x, y) = gps_to_local(&d.gps, &self.datums[frame].gps);
            [x, y]
        };
        significant([at[0] - base[0], at[1] - base[1]])
    }

    /// Re-anchor every component after an edit (plan 4.1, 4.3). See the
    /// module documentation for the rules.
    pub fn settle_anchors(&mut self, before: &AnchorSnapshot, ctx: &EditContext) -> AnchorReport {
        let topology = self.topology_arc();
        let origins = self.datum_origins(before, ctx);
        let placement_datums = self.placement_datums();
        let frame_new = self.frame_datum();
        let frame_old = before.frame();
        let frame_changed = match (frame_old, frame_new) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(old), Some(new)) => origins[new] != Some(old),
        };
        let old_frame_gps = frame_old.map(|i| before.datums[i].gps);
        let mut report = AnchorReport {
            frame_changed,
            ..AnchorReport::default()
        };

        // Placement datums: does each stay where it was, and where is it in
        // the old frame?
        let mut settled: Vec<(usize, bool, Option<[f64; 2]>)> = Vec::new();
        for &datum in placement_datums.iter().flatten() {
            let gps_edited = ctx.gps_edit.is_some_and(|edit| edit.datum == datum);
            let origin = origins[datum];
            let stays = !gps_edited && origin.is_some_and(|i| before.was_placement[i]);
            let old_frame_position = if stays {
                origin.and_then(|i| before.position(before.datums[i].station))
            } else if let Some(gps) = old_frame_gps {
                let (x, y) = gps_to_local(&self.datums[datum].gps, &gps);
                Some([x, y])
            } else if Some(datum) == frame_new {
                // A first datum: the survey moves onto it as one body.
                before.position(self.datums[datum].station)
            } else {
                None
            };
            if !stays && !ctx.corrected.contains(&datum) {
                report.placed_on_gps.push(datum);
            }
            settled.push((datum, stays, old_frame_position));
        }

        let translation = if frame_changed {
            let frame = frame_new.unwrap_or_default();
            let at = settled
                .iter()
                .find(|(datum, _, _)| *datum == frame)
                .and_then(|(_, _, at)| *at)
                .unwrap_or([0.0, 0.0]);
            [-at[0], -at[1]]
        } else {
            [0.0, 0.0]
        };
        report.translation = translation;

        for &(datum, stays, old_frame_position) in &settled {
            if ctx.corrected.contains(&datum) {
                // The command computed it against the post-edit frame.
                continue;
            }
            let correction = if frame_changed {
                match old_frame_position {
                    Some(at) if Some(datum) != frame_new => self.correction_to_hold(
                        datum,
                        [at[0] + translation[0], at[1] + translation[1]],
                    ),
                    _ => None,
                }
            } else if stays {
                self.datums[datum].correction
            } else {
                None
            };
            self.datums[datum].correction = correction;
        }

        // Demotions: a datum that is not its component's placement datum
        // carries no correction.
        let is_placement: Vec<bool> = {
            let mut flags = vec![false; self.datums.len()];
            for &datum in placement_datums.iter().flatten() {
                flags[datum] = true;
            }
            flags
        };
        for (index, datum) in self.datums.iter_mut().enumerate() {
            if !is_placement[index]
                && topology.contains(datum.station)
                && datum.correction.take().is_some()
            {
                report.demoted.push(index);
            }
        }

        let placements = self.settle_placements(&topology, before, ctx, translation);
        for old in &before.placements {
            if !placements.iter().any(|p| p.station == old.station) {
                report.placements_dropped.push(old.station);
            }
        }
        self.placements = placements;
        report
    }

    /// Match each post-edit datum to the pre-edit datum it was: same GPS bits
    /// (the previous GPS for a GPS edit) and the same station or a renamed
    /// one. First unused match in list order.
    fn datum_origins(&self, before: &AnchorSnapshot, ctx: &EditContext) -> Vec<Option<usize>> {
        let mut used = vec![false; before.datums.len()];
        let mut out = Vec::with_capacity(self.datums.len());
        for (index, datum) in self.datums.iter().enumerate() {
            let gps = match ctx.gps_edit {
                Some(edit) if edit.datum == index => edit.previous,
                _ => datum.gps,
            };
            let found = before.datums.iter().enumerate().position(|(i, old)| {
                !used[i]
                    && old.same_gps_bits(&gps)
                    && (old.station == datum.station
                        || ctx.renamed.contains(&(old.station, datum.station)))
            });
            if let Some(i) = found {
                used[i] = true;
            }
            out.push(found);
        }
        out
    }

    /// The placement list after an edit: one per datum-less component.
    fn settle_placements(
        &self,
        topology: &Topology,
        before: &AnchorSnapshot,
        ctx: &EditContext,
        translation: [f64; 2],
    ) -> Vec<Placement> {
        let components = topology.components();
        let placement_datums = self.placement_datums();
        let shift = |p: [f64; 2]| [p[0] + translation[0], p[1] + translation[1]];
        let stationary_component = ctx
            .merge
            .and_then(|merge| before.topology.component_of(merge.stationary));

        // (0) Placements the command declares it set or moved, by component
        // (the last entry of the list for that station wins).
        let mut explicit: Vec<Option<Placement>> = vec![None; components.len()];
        for placement in self.placements.iter().rev() {
            if !ctx.placed.contains(&placement.station) {
                continue;
            }
            if let Some(c) = topology.component_of(placement.station) {
                explicit[c].get_or_insert(*placement);
            }
        }

        // Pre-edit placements surviving in each component: (index in the old
        // list, placement), the merge's stationary one first, else list order.
        let mut surviving: Vec<Option<(usize, Placement)>> = vec![None; components.len()];
        for (index, old) in before.placements.iter().enumerate() {
            let Some(c) = topology.component_of(old.station) else {
                continue;
            };
            let stationary = stationary_component.is_some()
                && before.topology.component_of(old.station) == stationary_component;
            match surviving[c] {
                None => surviving[c] = Some((index, *old)),
                Some((_, current)) => {
                    let current_stationary = stationary_component.is_some()
                        && before.topology.component_of(current.station) == stationary_component;
                    if stationary && !current_stationary {
                        surviving[c] = Some((index, *old));
                    }
                }
            }
        }

        let mut layout = Layout::new(topology, self.lines.len());
        let mut chosen: Vec<(usize, usize, Placement)> = Vec::new();
        for (c, component) in components.iter().enumerate() {
            if placement_datums[c].is_some() {
                continue;
            }
            let has_position = |s: StationId| before.position(s).is_some();
            let still: Option<(StationId, [f64; 2])> = explicit[c]
                .map(|p| (p.station, [p.x, p.y]))
                .or_else(|| {
                    ctx.geometry_line
                        .and_then(|line| before.line_starts.get(&line).copied())
                        .filter(|&s| component.contains(s) && has_position(s))
                        .and_then(|s| before.position(s).map(|p| (s, shift(p))))
                })
                .or_else(|| {
                    surviving[c].and_then(|(_, p)| {
                        before.position(p.station).map(|at| (p.station, shift(at)))
                    })
                })
                .or_else(|| {
                    let line = ctx.edited_line.or(ctx.geometry_line)?;
                    let chain = topology.chain(line)?;
                    chain
                        .iter()
                        .copied()
                        .find(|&s| component.contains(s) && has_position(s))
                        .and_then(|s| before.position(s).map(|p| (s, shift(p))))
                })
                .or_else(|| {
                    component
                        .stations
                        .iter()
                        .copied()
                        .find(|&s| has_position(s))
                        .and_then(|s| before.position(s).map(|p| (s, shift(p))))
                });
            let (s_star, at) = match still {
                Some(found) => found,
                None => (component.lowest_station().unwrap_or_default(), [0.0, 0.0]),
            };
            self.walk_component(topology, s_star, at, &mut layout);

            let station = match (explicit[c], surviving[c]) {
                (Some(p), _) => p.station,
                (None, Some((_, p))) => p.station,
                (None, None) => component.lowest_station().unwrap_or_default(),
            };
            // List order: a pre-edit placement's slot, then the command's own
            // placements in their order, then new ones by component.
            let order = before
                .placements
                .iter()
                .position(|p| p.station == station)
                .or_else(|| {
                    self.placements
                        .iter()
                        .position(|p| p.station == station)
                        .map(|i| before.placements.len() + i)
                })
                .unwrap_or(usize::MAX);
            let at = topology.dense(station).map_or([0.0, 0.0], |d| layout.xy[d]);
            chosen.push((
                order,
                c,
                Placement {
                    station,
                    x: at[0],
                    y: at[1],
                },
            ));
        }
        // Surviving placements keep their list order; new ones follow, by
        // component.
        chosen.sort_by_key(|&(order, c, _)| (order, c));
        chosen.into_iter().map(|(_, _, p)| p).collect()
    }
}

/// `v`, or `None` when its magnitude is below [`CORRECTION_EPSILON_M`] or it
/// is not finite.
fn significant(v: [f64; 2]) -> Option<[f64; 2]> {
    let magnitude = (v[0] * v[0] + v[1] * v[1]).sqrt();
    (magnitude >= CORRECTION_EPSILON_M).then_some(v)
}
