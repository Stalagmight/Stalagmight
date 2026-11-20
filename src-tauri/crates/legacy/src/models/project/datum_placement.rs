//! Settling the datums after a command (#469 item 1, #741).
//!
//! A command can change WHICH datum places a merge component, or which one
//! defines the frame, without asking for it: removing a placement datum, an
//! unmerge or a delete that splits a component all re-derive the roles
//! ([`Project::datum_roles`]), so a GPS fix can become its piece's placement
//! datum. Two things then have to follow, in this order:
//!
//! 1. A datum that has just become a placement datum places its piece, exactly
//!    as [`Project::set_zero_datum`] places a subsequent datum: its rigid group
//!    is translated so its station sits at `gps_to_local(datum.gps, frame.gps)`.
//!    Without this the piece stays where the old placement put it, off its own
//!    GPS by the fix's gap, and when the removed datum was the frame every
//!    other separately placed piece ends up off its GPS by that gap too.
//! 2. When the frame changed (another station, or the same key on another
//!    datum's GPS), the whole survey is translated onto the new frame
//!    ([`Project::renormalize_frame`]).
//!
//! Step 1 measures against the frame that was in effect BEFORE the command,
//! because until step 2 runs every stored coordinate is still read against it.
//!
//! Both steps live in [`Project::settle_datum_layout`], which every command
//! epilogue calls with the [`DatumLayout`] it read before dispatch. Derived,
//! not hooked: the epilogue asks the state what changed instead of each
//! command remembering to say so.

use tracing::{info, warn};

use super::Project;
use crate::models::{gps_to_local, DatumRole, GpsCoordinates, StationKey, StationRef, ZeroDatum};

/// The datums as they were before a command: read with
/// [`Project::datum_layout`] before dispatch, handed to
/// [`Project::settle_datum_layout`] after it.
#[derive(Debug, Clone, Default)]
pub struct DatumLayout {
    /// The frame datum's station and GPS.
    frame: Option<(StationKey, GpsCoordinates)>,
    /// Every datum in list order.
    datums: Vec<DatumWas>,
}

/// One datum as it was before a command.
#[derive(Debug, Clone)]
struct DatumWas {
    key: StationKey,
    gps: GpsCoordinates,
    /// The role it had (`None`: dangling).
    role: Option<DatumRole>,
    /// Its station's horizontal position (`None`: dangling).
    at: Option<(f64, f64)>,
    /// Its stored accuracy (bits) and Entrance answer: what else tells two
    /// datums with one GPS apart.
    accuracy: Option<u64>,
    entrance: Option<bool>,
}

/// Within the offset dust of each other: the same place.
fn same_place(a: Option<(f64, f64)>, b: Option<(f64, f64)>) -> bool {
    matches!((a, b), (Some(a), Some(b))
        if (a.0 - b.0).abs() <= Project::OFFSET_DUST_M && (a.1 - b.1).abs() <= Project::OFFSET_DUST_M)
}

/// How many tiers [`likeness`] has, and how many ranks: each tier split by
/// the stored value.
const TIERS: usize = 5;
const RANKS: usize = 2 * TIERS;

/// The most earlier-by-current cells [`DatumLayout::unplaced`] aligns (a
/// byte of steps each, 4 MB, beside two rows of scores). Past it, thousands
/// of datums changed in one command, it places every placement datum among
/// the changed ones: a piece on its GPS stays where it is, but one an older
/// file left off its GPS is put on it, where below the limit it keeps its
/// gap.
const ALIGN_CELLS: usize = 1 << 22;

/// How alike an earlier datum and a current one are, as a rank from 0 (the
/// strongest) to `RANKS - 1`, or `None` when neither the station key nor the
/// GPS ties them. The tiers, strongest first:
/// 0. the same station key, GPS and position: untouched;
/// 1. the same GPS and position: re-keyed where it stands (a split, or a
///    shot delete that re-anchors the next station's datum onto the
///    removed one's key);
/// 2. the same station key and GPS: moved (a merge, a combine);
/// 3. the same GPS: re-keyed and moved (a combine);
/// 4. the same station key: its GPS was edited in place.
///
/// Within a tier the same stored accuracy and Entrance answer ranks first: a
/// command that moves or re-keys a datum does not edit it, but a Batch, which
/// settles once for several commands, can do both.
fn likeness(was: &DatumWas, now: &DatumWas) -> Option<usize> {
    let tier: usize = match (
        same_gps(&was.gps, &now.gps),
        was.key == now.key,
        same_place(was.at, now.at),
    ) {
        (true, true, true) => 0,
        (true, false, true) => 1,
        (true, true, false) => 2,
        (true, false, false) => 3,
        (false, true, _) => TIERS - 1,
        (false, false, _) => return None,
    };
    let same_value = was.accuracy == now.accuracy && was.entrance == now.entrance;
    Some(2 * tier + usize::from(!same_value))
}

/// An alignment's pairs, then its pairs of each rank, strongest rank first:
/// compared lexicographically.
///
/// The count comes first. A command edits, removes or appends a datum or
/// two, so the alignment that keeps the most datums is the likeliest story,
/// however alike a crossing pair looks: a new GPS on the frame datum copied
/// from a fix on its spot, accuracy and all, makes the frame look like that
/// fix re-keyed where it stands, and pairing them would drop every datum in
/// between. Among alignments that keep as many, one more pair of a stronger
/// rank outweighs any number of weaker ones.
type Score = [u32; RANKS + 1];

/// What an alignment does with an earlier datum and a current one.
#[derive(Clone, Copy)]
enum Step {
    /// They are one datum.
    Pair,
    /// The earlier one is gone (removed by the command).
    Dropped,
    /// The current one is new (appended by the command).
    Added,
}

/// The best order-preserving pairing of `before` earlier datums with `after`
/// current ones, `rank(i, j)` being [`likeness`]: the most pairs, then the
/// most of rank 0, then of rank 1, and so on ([`Score`]). Returns, for each
/// current datum, the earlier datum it is, or `None` for a new one.
///
/// On a tie it pairs rather than drop, and drops rather than add: a command
/// appends a new datum at the end of the list, so a current datum is left
/// new only when no earlier one explains it as well.
///
/// `O(before * after)` time, and as many steps (one byte each).
pub(super) fn align(
    before: usize,
    after: usize,
    rank: impl Fn(usize, usize) -> Option<usize>,
) -> Vec<Option<usize>> {
    let mut steps = vec![Step::Added; before * after];
    // `next[j]` is the best score of earlier datums `i + 1..` against current
    // ones `j..`, and `row[j]` that of `i..`: the suffixes, from the end.
    let mut next: Vec<Score> = vec![[0; RANKS + 1]; after + 1];
    let mut row = next.clone();
    for i in (0..before).rev() {
        for j in (0..after).rev() {
            let mut best = (next[j], Step::Dropped);
            if let Some(r) = rank(i, j) {
                debug_assert!(r < RANKS, "likeness rank {r} is past RANKS");
                let mut paired = next[j + 1];
                let [pairs, ranks @ ..] = &mut paired;
                *pairs += 1;
                if let Some(count) = ranks.get_mut(r) {
                    *count += 1;
                }
                if paired >= best.0 {
                    best = (paired, Step::Pair);
                }
            }
            if row[j + 1] > best.0 {
                best = (row[j + 1], Step::Added);
            }
            row[j] = best.0;
            steps[i * after + j] = best.1;
        }
        std::mem::swap(&mut next, &mut row);
    }
    let mut earlier = vec![None; after];
    let (mut i, mut j) = (0, 0);
    while i < before && j < after {
        match steps[i * after + j] {
            Step::Pair => {
                earlier[j] = Some(i);
                i += 1;
                j += 1;
            }
            Step::Dropped => i += 1,
            Step::Added => j += 1,
        }
    }
    earlier
}

impl DatumLayout {
    /// The datums of `now` (this project's layout after a command, `self`
    /// the one before it) that are placement datums now and were GPS fixes
    /// or dangling datums before, by index into `zero_datums`.
    ///
    /// Datums carry no id, but their order in the list is their identity: a
    /// command edits a datum in place, removes it, or appends a new one, and
    /// never reorders the list (the order is what gives each datum its role,
    /// [`Project::datum_roles`]). So the datums that survive a command keep
    /// their order, and the earlier and current lists are matched by the best
    /// order-preserving alignment ([`align`] over [`likeness`]). That tells
    /// twins apart (two datums with one GPS, copied coordinates) that a
    /// combine re-keys one onto the other's key, even with one stored value.
    /// Untouched datums at either end of the lists pair as they stand, so
    /// only the few a command changed are aligned.
    ///
    /// Key and GPS alone are not enough. Deleting a line's first shot drops
    /// the datum on its first station and re-anchors the next station's datum
    /// onto that key, and a GPS copied from another datum matches that datum
    /// too: either way the promoted datum would pass for the placement datum
    /// it replaced. The position settles it, since neither moves a station.
    ///
    /// A placement datum paired, not untouched, with an earlier placement
    /// datum still counts as unplaced when the alignment left out an earlier
    /// datum that was not a placement datum and could be it as well: one as
    /// alike, or its twin (key and position say little once twins are both
    /// re-keyed and moved). Placing it moves nothing when its piece already
    /// sits on that GPS. A datum that matches no earlier one is new (its
    /// command placed it already).
    fn unplaced(&self, now: &DatumLayout) -> Vec<usize> {
        let (was, is) = (&self.datums, &now.datums);
        let rank = |i: usize, j: usize| likeness(&was[i], &is[j]);
        let untouched = |i: usize, j: usize| rank(i, j).is_some_and(|r| r < 2);
        let head = (0..was.len().min(is.len()))
            .take_while(|&k| untouched(k, k))
            .count();
        let tail = (0..(was.len() - head).min(is.len() - head))
            .take_while(|&k| untouched(was.len() - 1 - k, is.len() - 1 - k))
            .count();
        let (was_end, is_end) = (was.len() - tail, is.len() - tail);
        let mut earlier: Vec<Option<usize>> = (0..is.len())
            .map(|j| {
                if j < head {
                    Some(j)
                } else if j >= is_end {
                    Some(j - is_end + was_end)
                } else {
                    None
                }
            })
            .collect();
        let (middle_was, middle_is) = (was_end - head, is_end - head);
        let aligned = middle_was.saturating_mul(middle_is) <= ALIGN_CELLS;
        if aligned {
            let middle = align(middle_was, middle_is, |i, j| rank(head + i, head + j));
            for (j, i) in middle.into_iter().enumerate() {
                earlier[head + j] = i.map(|i| head + i);
            }
        } else {
            warn!(
                earlier = middle_was,
                current = middle_is,
                purpose = "match the datums before and after a command",
                "Too many changed datums to align; placing every placement datum among them"
            );
        }
        let mut paired = vec![false; was.len()];
        for i in earlier.iter().flatten() {
            paired[*i] = true;
        }
        let placement = |d: &DatumWas| d.role == Some(DatumRole::Placement);
        let doubted = |i: usize, j: usize| {
            (0..was.len()).any(|o| {
                !paired[o]
                    && !placement(&was[o])
                    && rank(o, j)
                        .is_some_and(|r| same_gps(&was[o].gps, &is[j].gps) || Some(r) <= rank(i, j))
            })
        };
        (0..is.len())
            .filter(|&j| placement(&is[j]))
            .filter(|&j| match earlier[j] {
                Some(i) => !placement(&was[i]) || (!untouched(i, j) && doubted(i, j)),
                None => !aligned && (head..is_end).contains(&j),
            })
            .collect()
    }

    /// Whether the frame datum is not the one it was: another station, or
    /// another GPS (a promoted datum re-keyed onto the old frame's key).
    ///
    /// No frame now (`Some -> None`, the last datum removed) is no change:
    /// there is nothing to re-frame onto. A first frame (`None -> Some`) is
    /// one, though [`Project::set_zero_datum`] has already put it on the
    /// origin, so the measured re-frame is a no-op there.
    fn frame_changed(&self, now: Option<&ZeroDatum>) -> bool {
        match (&self.frame, now) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some((key, gps)), Some(zd)) => zd.station_ref != *key || !same_gps(&zd.gps, gps),
        }
    }
}

/// Bit-for-bit the same stored GPS: identity, not proximity.
pub(super) fn same_gps(a: &GpsCoordinates, b: &GpsCoordinates) -> bool {
    a.latitude.to_bits() == b.latitude.to_bits() && a.longitude.to_bits() == b.longitude.to_bits()
}

impl Project {
    /// The datums as they are now, for [`Self::settle_datum_layout`] after a
    /// command: a walk of `zero_datums` with a `resolve_key` and a shot walk
    /// to the station each.
    pub fn datum_layout(&self) -> DatumLayout {
        DatumLayout {
            frame: self
                .frame_datum()
                .map(|zd| (zd.station_ref.clone(), zd.gps)),
            datums: self
                .zero_datums
                .iter()
                .zip(self.datum_roles())
                .map(|(zd, role)| DatumWas {
                    key: zd.station_ref.clone(),
                    gps: zd.gps,
                    role,
                    at: self.datum_position(zd),
                    accuracy: zd.accuracy_m.map(f64::to_bits),
                    entrance: zd.entrance,
                })
                .collect(),
        }
    }

    /// The horizontal position of `zd`'s station, `None` when it dangles.
    /// The shot walk only: no colocation search, which would cost a pass over
    /// every merge per datum for a depth this never reads.
    fn datum_position(&self, zd: &ZeroDatum) -> Option<(f64, f64)> {
        self.compute_station_raw_xy(self.resolve_key(&zd.station_ref)?)
    }

    /// Place every piece whose placement datum changed in a command, then
    /// re-frame when the frame changed (see the module doc). `before` is what
    /// [`Self::datum_layout`] returned before the command. Returns true when
    /// anything moved.
    ///
    /// Run it ONCE per command, on the forward path only: the placement step
    /// measures against `before`'s frame, so a second call with the same
    /// `before` after a re-frame would place against a frame that is no longer
    /// the origin. Undo restores; it never settles.
    pub fn settle_datum_layout(&mut self, before: &DatumLayout) -> bool {
        let mut moved = false;
        // With no frame before, nothing was placed relative to anything, and
        // the re-frame below is the whole answer.
        if let Some((_, frame_gps)) = before.frame {
            let promoted: Vec<(StationRef, GpsCoordinates)> = before
                .unplaced(&self.datum_layout())
                .into_iter()
                .filter_map(|j| {
                    let zd = self.zero_datums.get(j)?;
                    Some((self.resolve_key(&zd.station_ref)?, zd.gps))
                })
                .collect();
            for (station_ref, gps) in promoted {
                moved |= self.place_group_at(
                    station_ref,
                    gps_to_local(&gps, &frame_gps),
                    "place a piece by its promoted datum",
                );
            }
        }
        // Compared by key AND GPS: a promoted datum can inherit the old
        // frame's key (see `DatumLayout::unplaced`), and the frame has
        // still changed. Re-framing is measured, so an unchanged frame is a
        // no-op anyway.
        if before.frame_changed(self.frame_datum()) {
            moved |= self.renormalize_frame();
        }
        moved
    }

    /// Move every placement datum's piece other than the frame's by the frame
    /// correction, after the frame datum got a new GPS on the same station
    /// (#769): `gps_to_local(its gps, new frame gps) - gps_to_local(its gps,
    /// old frame gps)`. The frame stays on the origin, so without this every
    /// other placed piece is off its own GPS by the edit. A piece on its GPS
    /// stays on it; one an older file left off its GPS keeps its gap, as an
    /// accuracy-only edit leaves it. GPS fixes ride with their piece. Returns
    /// true when anything moved.
    pub(super) fn place_pieces_on_frame(&mut self, old_frame_gps: &GpsCoordinates) -> bool {
        let Some((frame_key, frame_gps)) = self
            .frame_datum()
            .map(|zd| (zd.station_ref.clone(), zd.gps))
        else {
            return false;
        };
        let pieces: Vec<(StationRef, (f64, f64))> = self
            .placement_datums()
            .into_iter()
            .filter(|zd| zd.station_ref != frame_key)
            .filter_map(|zd| {
                let station_ref = self.resolve_key(&zd.station_ref)?;
                let current = self.compute_station_raw_coords(station_ref)?;
                let (new_x, new_y) = gps_to_local(&zd.gps, &frame_gps);
                let (old_x, old_y) = gps_to_local(&zd.gps, old_frame_gps);
                Some((
                    station_ref,
                    (current.x + new_x - old_x, current.y + new_y - old_y),
                ))
            })
            .collect();
        let mut moved = false;
        for (station_ref, target) in pieces {
            moved |= self.place_group_at(station_ref, target, "move a piece with the frame's GPS");
        }
        moved
    }

    /// Translate `station_ref`'s rigid group so the station sits at `target`,
    /// then cascade through aligned closures: the same placement
    /// [`Self::set_zero_datum`] applies to a subsequent datum. `purpose` names
    /// the caller in the log. Returns true when anything moved.
    fn place_group_at(
        &mut self,
        station_ref: StationRef,
        target: (f64, f64),
        purpose: &'static str,
    ) -> bool {
        let Some(current) = self.compute_station_raw_coords(station_ref) else {
            return false;
        };
        let (dx, dy) = (target.0 - current.x, target.1 - current.y);
        if !dx.is_finite() || !dy.is_finite() {
            warn!(
                ?station_ref,
                purpose, "Datum placement is not finite; leaving the piece where it is"
            );
            return false;
        }
        if dx.abs() <= Self::OFFSET_DUST_M && dy.abs() <= Self::OFFSET_DUST_M {
            return false;
        }
        self.adjust_rigid_group_offsets(station_ref.line_id, dx, dy);
        self.cascade_connection_offsets(&[station_ref.line_id]);
        info!(
            ?station_ref,
            delta_x = dx,
            delta_y = dy,
            purpose,
            "Moved a piece so its datum station sits on its target"
        );
        true
    }
}
