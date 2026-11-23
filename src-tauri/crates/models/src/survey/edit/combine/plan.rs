//! Combine Lines, the decisions (plan 5.4 steps 1, 2, 3 and 6): which side
//! moves, by how much, and which junctions are torn. Read-only: the tears are
//! tried on scratch copies to see the two sides they leave.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::{KeptMovesReason, LineEnd, LineId, StationId};

use super::super::super::ops_detach::DetachMode;
use super::super::super::{ClosureStrategy, Project, RawPositions};
use super::{CombineJoin, TornTieReason};

/// Below this in each axis a gap is float dust and nothing moves (today's
/// `Project::OFFSET_DUST_M`).
pub(super) const DUST_M: f64 = 1e-6;

/// One junction the combine tears: `line` leaves `station`, which stays on
/// its other lines (`toward` is the one the far side is reached through).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Detach {
    pub(super) line: LineId,
    pub(super) station: StationId,
    pub(super) toward: Option<LineId>,
    pub(super) reason: TornTieReason,
}

/// Everything the combine decided before it changes anything.
#[derive(Debug, Clone)]
pub(super) struct Decision {
    pub(super) join: CombineJoin,
    pub(super) moving: LineId,
    pub(super) shifting: bool,
    /// The translation of the moving side.
    pub(super) shift: [f64; 2],
    pub(super) kept_moves_because: Option<KeptMovesReason>,
    /// The junctions torn, in order.
    pub(super) detaches: Vec<Detach>,
    /// The joined station keeps the kept line's hand move (else the other's:
    /// the fixed side's wins).
    pub(super) kept_hand_wins: bool,
}

/// The tears tried for one choice of moving line, and the moving side they
/// leave (in the scratch copy's station ids).
struct Trial {
    detaches: Vec<Detach>,
    scratch: Project,
    /// The moving side's stations, and where each pre-edit datum sits in the
    /// copy the side was read from.
    side: BTreeSet<StationId>,
    side_datums: Vec<StationId>,
}

impl Trial {
    /// Does the moving side hold one of `placements` (pre-edit indices)?
    fn held(&self, placements: &[usize]) -> bool {
        placements.iter().any(|&d| {
            self.side_datums
                .get(d)
                .is_some_and(|station| self.side.contains(station))
        })
    }
}

/// The moving side of `m` in `p`, whose join station is `jm`: its component,
/// except for a merged join (`jm` is also on the fixed line), where it is
/// what hangs off `m` without passing the join station (the lines there are
/// branches of the joined station, on neither side). Returns the side and the
/// station of every datum in the copy it was read from.
pub(super) fn moving_side(
    p: &Project,
    m: LineId,
    jm: StationId,
    merged: bool,
) -> (BTreeSet<StationId>, Vec<StationId>) {
    let component = |q: &Project, s: StationId| -> BTreeSet<StationId> {
        let topology = q.topology();
        topology
            .component_of(s)
            .and_then(|c| topology.component(c))
            .map(|c| c.stations.iter().copied().collect())
            .unwrap_or_default()
    };
    let datums = |q: &Project| q.datums.iter().map(|d| d.station).collect::<Vec<_>>();
    if merged {
        let mut virtual_detach = p.clone();
        if let Ok(fresh) = virtual_detach
            .detach_ids(jm, m, DetachMode::Tear)
            .map(|r| r.new_station)
        {
            return (component(&virtual_detach, fresh), datums(&virtual_detach));
        }
    }
    (component(p, jm), datums(p))
}

/// The station at `end` of `line` in `p`.
pub(super) fn end_station(p: &Project, line: LineId, end: LineEnd) -> Option<StationId> {
    let l = p.line(line)?;
    Some(match end {
        LineEnd::Start => l.start,
        LineEnd::End => l.shots.last().map_or(l.start, |s| s.to),
    })
}

/// `a - b`.
fn minus(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn beyond_dust(v: [f64; 2]) -> bool {
    v[0].abs() > DUST_M || v[1].abs() > DUST_M
}

impl Project {
    /// The chain position of `station` on `line`: its shown position less
    /// the line's hand move there.
    pub(super) fn chain_position(
        &self,
        positions: &RawPositions,
        line: LineId,
        station: StationId,
    ) -> Option<[f64; 2]> {
        let [x, y] = positions.xy(station)?;
        let hand = self.line(line)?.hand(station);
        Some([x - hand[0], y - hand[1]])
    }

    /// The pre-edit placement datums (index into `datums`) of every
    /// component.
    fn placement_datum_indices(&self) -> Vec<usize> {
        self.placement_datums().into_iter().flatten().collect()
    }

    /// Does `line` carry placement datum `datum` on one of its own stations
    /// (its station is on the line, and its `via` is unset or the line)?
    fn line_carries(&self, line: LineId, datum: usize) -> bool {
        let Some(d) = self.datums.get(datum) else {
            return false;
        };
        self.topology().slot_on(d.station, line).is_some() && d.via.is_none_or(|v| v == line)
    }

    /// Why combining at `join` is refused, if it is (today's reasons and
    /// words): both join stations hold a datum, both lines carry their own
    /// placement datum, or the combined line would hold too many shots.
    pub(super) fn combine_blocked_reason(&self, join: &CombineJoin) -> Option<String> {
        let (k, o) = (join.kept, join.other);
        let holds = |s: StationId| self.datums.iter().any(|d| d.station == s);
        if join.kept_station != join.other_station
            && holds(join.kept_station)
            && holds(join.other_station)
        {
            let index = |line: LineId, s: StationId| {
                self.topology()
                    .slot_on(s, line)
                    .map_or(0, |slot| slot.station_index)
            };
            return Some(format!(
                "{} #{} and {} #{} each hold a zero datum, and combining makes them one station: \
                 remove one of the two datums first",
                self.line_label_of(k),
                index(k, join.kept_station),
                self.line_label_of(o),
                index(o, join.other_station)
            ));
        }
        let placements = self.placement_datum_indices();
        let carried_by = |line: LineId| -> Vec<usize> {
            placements
                .iter()
                .copied()
                .filter(|&d| self.line_carries(line, d))
                .collect()
        };
        let (ck, co) = (carried_by(k), carried_by(o));
        if ck.iter().any(|d| co.iter().any(|e| e != d)) {
            return Some(format!(
                "{} and {} are each placed by their own zero datum: merge them first, so the \
                 later datum becomes a GPS fix",
                self.line_label_of(k),
                self.line_label_of(o)
            ));
        }
        let budget = crate::FEATURE_ID_STRIDE.saturating_sub(1) as usize;
        let total =
            self.line(k).map_or(0, |l| l.shots.len()) + self.line(o).map_or(0, |l| l.shots.len());
        (total > budget)
            .then(|| format!("The combined line would have {total} shots (at most {budget})"))
    }

    /// Decide the combine at `join` (module docs; plan 5.4 steps 1, 2, 3, 6).
    /// `Err` is a refusal in today's words.
    pub(super) fn decide_combine(
        &self,
        join: &CombineJoin,
        positions: &RawPositions,
    ) -> Result<Decision, String> {
        let (k, o) = (join.kept, join.other);
        let (Some(ck), Some(co)) = (
            self.chain_position(positions, k, join.kept_station),
            self.chain_position(positions, o, join.other_station),
        ) else {
            return Err(self.cannot_combine_message(k, o));
        };
        let (Some(sk), Some(so)) = (
            positions.xy(join.kept_station),
            positions.xy(join.other_station),
        ) else {
            return Err(self.cannot_combine_message(k, o));
        };
        let gap = minus(ck, co);
        let shown = minus(sk, so);
        let finite = gap.iter().chain(&shown).all(|v| v.is_finite());
        if !finite {
            return Err(self.cannot_combine_message(k, o));
        }
        let merged = join.kept_station == join.other_station;
        let topology = self.topology();
        let third = |s: StationId| topology.lines_of(s).iter().any(|l| *l != k && *l != o);
        let shifting = beyond_dust(gap)
            || (!merged
                && beyond_dust(shown)
                && (third(join.kept_station) || third(join.other_station)));
        if !shifting {
            let trial = self.trial(join, o);
            return Ok(Decision {
                join: *join,
                moving: o,
                shifting: false,
                shift: [0.0, 0.0],
                kept_moves_because: None,
                detaches: trial.detaches,
                kept_hand_wins: true,
            });
        }
        let (moving, detaches, because) = self.pick_moving_side(join)?;
        // Today's "idle" plan: the other line was picked only to carry what
        // sits at its join station (the joined station keeps the kept line's
        // hand move), the chain gap is dust, and nothing else is there to
        // carry: nothing moves after all.
        if moving == o && !beyond_dust(gap) && !third(join.other_station) {
            let trial = self.trial(join, o);
            return Ok(Decision {
                join: *join,
                moving: o,
                shifting: false,
                shift: [0.0, 0.0],
                kept_moves_because: None,
                detaches: trial.detaches,
                kept_hand_wins: true,
            });
        }
        let shift = if moving == o { gap } else { [-gap[0], -gap[1]] };
        Ok(Decision {
            join: *join,
            moving,
            shifting: true,
            shift,
            kept_moves_because: because,
            detaches,
            kept_hand_wins: moving == o,
        })
    }

    /// `Lines "A" and "B" cannot be combined`.
    pub(super) fn cannot_combine_message(&self, kept: LineId, other: LineId) -> String {
        let name = |id: LineId| {
            if self.line(id).is_some() {
                format!("\"{}\"", self.line_label_of(id))
            } else {
                "a missing line".to_string()
            }
        };
        format!(
            "Lines {} and {} cannot be combined",
            name(kept),
            name(other)
        )
    }

    /// Step 1 with steps 2, 3 and 6 for each candidate: the moving line, its
    /// tears, and why the kept line moves when it does.
    fn pick_moving_side(
        &self,
        join: &CombineJoin,
    ) -> Result<(LineId, Vec<Detach>, Option<KeptMovesReason>), String> {
        let (k, o) = (join.kept, join.other);
        let placements = self.placement_datum_indices();
        let anchor = self.combine_gauge(join);
        let mut free: Vec<(LineId, Trial, bool)> = Vec::new();
        for m in [o, k] {
            let trial = self.trial(join, m);
            if !trial.held(&placements) {
                let holds = anchor.is_some_and(|g| trial.side.contains(&g));
                free.push((m, trial, holds));
            }
        }
        let pick = free
            .iter()
            .position(|(_, _, holds)| !holds)
            .or((!free.is_empty()).then_some(0));
        if let Some(pick) = pick {
            let other_free = free.iter().any(|(m, _, _)| *m == o);
            let (m, trial, _) = free.swap_remove(pick);
            let because = (m == k).then_some(if other_free {
                KeptMovesReason::AdjustmentAnchor
            } else {
                KeptMovesReason::ZeroDatum
            });
            return Ok((m, trial.detaches, because));
        }
        // Both sides held: the moving line (one that carries no placement
        // datum itself) is detached from the parts that hold one.
        for m in [o, k] {
            if placements.iter().any(|&d| self.line_carries(m, d)) {
                continue;
            }
            let mut trial = self.trial(join, m);
            let join_station =
                (join.kept_station == join.other_station).then_some(join.kept_station);
            let parts = trial.scratch.datum_held_parts(m, &placements, join_station);
            for (station, toward) in parts {
                if trial.scratch.detach_ids(station, m, DetachMode::Tear).is_ok() {
                    trial.detaches.push(Detach {
                        line: m,
                        station,
                        toward,
                        reason: TornTieReason::ZeroDatum,
                    });
                }
            }
            let because = (m == k).then_some(KeptMovesReason::ZeroDatum);
            return Ok((m, trial.detaches, because));
        }
        Err(
            "Both lines are placed by a zero datum on one of their stations: merge them first, \
             so the later datum becomes a GPS fix"
                .to_string(),
        )
    }

    /// The junctions of `line` whose far part (reached without `line`'s
    /// shots) holds a datum of `placements` (pre-edit indices), with the line
    /// the part is reached through.
    fn datum_held_parts(
        &self,
        line: LineId,
        placements: &[usize],
        skip: Option<StationId>,
    ) -> Vec<(StationId, Option<LineId>)> {
        let topology = self.topology_arc();
        let Some(chain) = topology.chain(line) else {
            return Vec::new();
        };
        let held: BTreeSet<StationId> = placements
            .iter()
            .filter_map(|&d| self.datums.get(d).map(|datum| datum.station))
            .collect();
        let mut out = Vec::new();
        for &station in chain {
            if skip == Some(station) {
                continue;
            }
            let others: Vec<LineId> = topology
                .lines_of(station)
                .into_iter()
                .filter(|l| *l != line)
                .collect();
            if others.is_empty() {
                continue;
            }
            let excluded = |l: LineId, _: usize| l == line;
            let part = &self.reach_without_shots(&[station], &excluded, false)[0];
            if !held.iter().any(|s| part.contains(s)) {
                continue;
            }
            // The line the held part is reached along (the first one through
            // the junction whose own chain reaches a held station, else the
            // first one).
            let toward = others
                .iter()
                .copied()
                .find(|l| {
                    let excluded = |x: LineId, _: usize| x == line;
                    let from = topology
                        .chain(*l)
                        .and_then(|c| c.iter().copied().find(|s| *s != station));
                    from.is_some_and(|from| {
                        self.reach_without_shots(&[from], &excluded, false)[0]
                            .iter()
                            .any(|s| held.contains(s) && *s != station)
                    })
                })
                .unwrap_or(others[0]);
            out.push((station, Some(toward)));
        }
        out
    }

    /// The tears (steps 6 and 2) for moving line `m`, tried on a scratch copy.
    fn trial(&self, join: &CombineJoin, m: LineId) -> Trial {
        let (k, o) = (join.kept, join.other);
        let f = if m == o { k } else { o };
        let mut scratch = self.clone();
        let mut detaches = Vec::new();
        let (mut jm, jf) = if m == o {
            (join.other_station, join.kept_station)
        } else {
            (join.kept_station, join.other_station)
        };
        // Step 6: a station both lines pass besides the join.
        let chain_m: Vec<StationId> = self.line_stations(m).unwrap_or(&[]).to_vec();
        let on_f: BTreeSet<StationId> = self
            .line_stations(f)
            .unwrap_or(&[])
            .iter()
            .copied()
            .collect();
        for s in chain_m {
            let is_join = join.kept_station == join.other_station && s == join.kept_station;
            if !on_f.contains(&s) || is_join {
                continue;
            }
            if let Ok(fresh) = scratch.detach_ids(s, m, DetachMode::Tear).map(|r| r.new_station) {
                if s == jm {
                    jm = fresh;
                }
                detaches.push(Detach {
                    line: m,
                    station: s,
                    toward: Some(f),
                    reason: TornTieReason::PassesTwice,
                });
            }
        }
        // Step 2: still connected, so tear the junction nearest M on the path.
        if jm != jf && scratch.topology().connected(jm, jf) {
            if let Some((station, toward)) = scratch.first_junction_off(m, jm, jf) {
                if let Ok(fresh) = scratch.detach_ids(station, m, DetachMode::Tear).map(|r| r.new_station) {
                    if station == jm {
                        jm = fresh;
                    }
                    detaches.push(Detach {
                        line: m,
                        station,
                        toward: Some(toward),
                        reason: TornTieReason::Loop,
                    });
                }
            }
        }
        let merged = join.kept_station == join.other_station;
        let (side, side_datums) = moving_side(&scratch, m, jm, merged);
        Trial {
            detaches,
            scratch,
            side,
            side_datums,
        }
    }

    /// On the tree path from `from` (on line `m`) to `to`: the station where
    /// the path leaves `m`'s chain, and the line it leaves along.
    fn first_junction_off(
        &self,
        m: LineId,
        from: StationId,
        to: StationId,
    ) -> Option<(StationId, LineId)> {
        let mut adjacency: BTreeMap<StationId, Vec<(StationId, LineId)>> = BTreeMap::new();
        for line in self.lines() {
            let mut a = line.start;
            for shot in &line.shots {
                adjacency.entry(a).or_default().push((shot.to, line.id));
                adjacency.entry(shot.to).or_default().push((a, line.id));
                a = shot.to;
            }
        }
        let mut parent: BTreeMap<StationId, (StationId, LineId)> = BTreeMap::new();
        let mut seen = BTreeSet::from([from]);
        let mut queue = VecDeque::from([from]);
        while let Some(at) = queue.pop_front() {
            if at == to {
                break;
            }
            for &(next, line) in adjacency.get(&at).into_iter().flatten() {
                if seen.insert(next) {
                    parent.insert(next, (at, line));
                    queue.push_back(next);
                }
            }
        }
        // Walk back from `to`, then forwards from `from`.
        let mut path: Vec<(StationId, LineId)> = Vec::new();
        let mut at = to;
        while at != from {
            let &(prev, line) = parent.get(&at)?;
            path.push((prev, line));
            at = prev;
        }
        path.reverse();
        path.into_iter().find(|(_, line)| *line != m)
    }

    /// The adjusted-view gauge the anchor rule protects (step 1): the station
    /// the Closures Computed View holds the kept join station's solve
    /// component at ([`Project::solve_hold`]: its first placement datum's
    /// station, else its lowest piece's `Placement` station), when the view
    /// solves the joined survey; `None` otherwise (the plain rule applies).
    /// A datum station is never on a free side, so with a datum in the
    /// component no side holds the gauge, as today.
    fn combine_gauge(&self, join: &CombineJoin) -> Option<StationId> {
        let hold = self.solve_hold(join.kept_station)?;
        (hold.solved || self.plain_combine_solves(join)).then_some(hold.anchor)
    }

    /// Would the plain plan (the other line moves, its loop tears only) give
    /// the joined survey something to solve? Only when it tears a tie: a
    /// dry run of that combine, then [`Project::solve_hold`] on the joined
    /// station.
    fn plain_combine_solves(&self, join: &CombineJoin) -> bool {
        let trial = self.trial(join, join.other);
        let snaps = self.snaps_crossing(join, &trial);
        if trial.detaches.is_empty() && !snaps {
            return false;
        }
        let decision = Decision {
            join: *join,
            moving: join.other,
            shifting: true,
            shift: [0.0, 0.0],
            kept_moves_because: None,
            detaches: trial.detaches,
            kept_hand_wins: true,
        };
        let mut copy = self.clone();
        let options = super::apply::plain_options();
        if copy.run_combine(&decision, &options).is_err() {
            return false;
        }
        end_station(&copy, join.kept, join.kept_end)
            .and_then(|joined| copy.solve_hold(joined))
            .is_some_and(|hold| hold.solved)
    }

    /// Is a snap (aligned `Move` closure) across the two sides of `trial`?
    fn snaps_crossing(&self, _join: &CombineJoin, trial: &Trial) -> bool {
        let side = &trial.side;
        let positions = self.raw_positions();
        self.closures.iter().any(|c| {
            (c.strategy == ClosureStrategy::Tie
                || (c.strategy == ClosureStrategy::Move && self.closure_is_aligned(&positions, c)))
                && trial
                    .scratch
                    .closures
                    .iter()
                    .find(|t| t.id == c.id)
                    .is_some_and(|t| side.contains(&t.a) != side.contains(&t.b))
        })
    }
}
