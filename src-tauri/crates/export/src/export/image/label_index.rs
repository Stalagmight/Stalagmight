//! Spatial indexes that keep the map export's label placement linear (#576 B22).
//!
//! Placement asked two questions, each by scanning everything: "which drawn
//! shots touch this station?" (every shot of every section, once per labelled
//! station) and "does this box hit a label already placed?" (every placed box,
//! once per candidate). Both are local questions, so both are answered here
//! from a hash grid - and with exactly the predicates the scans used, so the
//! exported document does not change by a byte.

use std::collections::HashMap;

use crate::models::{Coordinates, Project, Station, StationRef};

/// Two points closer than this on both axes are the same physical point. The
/// tolerance the open-space scan has always used.
const COLOCATED_EPS: f64 = 1e-6;

/// Grid cell of [`ShotEnds`], in world metres. Any two colocated points fall
/// in the same or adjacent cells (floor is monotonic, and 1 m is far wider
/// than [`COLOCATED_EPS`]), so a 3x3 neighbourhood finds every candidate; the
/// exact predicate then decides.
const SHOT_END_CELL_M: f64 = 1.0;

/// Integer cell of `value` for a grid of `cell` size. Saturating: a huge
/// coordinate lands in an edge cell, which costs candidates, never answers.
fn cell_of(value: f64, cell: f64) -> i64 {
    (value / cell).floor() as i64
}

/// One end of a drawn shot: where it is, and where the shot's other end is.
struct ShotEnd {
    at: (f64, f64),
    other: (f64, f64),
}

/// Every drawn (non-virtual) shot's two ends, bucketed by position.
#[derive(Default)]
pub(super) struct ShotEnds {
    cells: HashMap<(i64, i64), Vec<ShotEnd>>,
}

impl ShotEnds {
    pub(super) fn new(
        project: &Project,
        stations: &[Station],
        station_index: &HashMap<StationRef, usize>,
    ) -> Self {
        let mut cells: HashMap<(i64, i64), Vec<ShotEnd>> = HashMap::new();
        let mut add = |at: Coordinates, other: Coordinates| {
            let key = (
                cell_of(at.x, SHOT_END_CELL_M),
                cell_of(at.y, SHOT_END_CELL_M),
            );
            cells.entry(key).or_default().push(ShotEnd {
                at: (at.x, at.y),
                other: (other.x, other.y),
            });
        };
        for section in &project.sections {
            for (i, shot) in section.shots.iter().enumerate() {
                if shot.kind.is_virtual() {
                    continue;
                }
                let from_ref = StationRef::new(section.id, i as u32);
                let to_ref = StationRef::new(section.id, (i + 1) as u32);
                let (Some(&fi), Some(&ti)) =
                    (station_index.get(&from_ref), station_index.get(&to_ref))
                else {
                    continue;
                };
                let (from, to) = (stations[fi].coordinates, stations[ti].coordinates);
                add(from, to);
                add(to, from);
            }
        }
        Self { cells }
    }

    /// Direction, in screen radians, of the bisector of the largest angular
    /// gap between the drawn shots leaving `at` - the empty wedge a label can
    /// sit in without touching a line at the station.
    ///
    /// A shot leaves `at` when either of its ends is colocated with it (within
    /// [`COLOCATED_EPS`] on both axes), which is what makes merged and
    /// aligned-closure twins - and stations that merely coincide - share their
    /// shots. Directions are measured FROM `at` to the shot's other end. If no
    /// drawn shot touches the point, `FRAC_PI_2` (screen down) is the
    /// deterministic fallback.
    pub(super) fn open_space_angle(&self, at: Coordinates) -> f64 {
        let mut dirs: Vec<f64> = Vec::new();
        let (cx, cy) = (
            cell_of(at.x, SHOT_END_CELL_M),
            cell_of(at.y, SHOT_END_CELL_M),
        );
        for dx in -1..=1_i64 {
            for dy in -1..=1_i64 {
                let key = (cx.saturating_add(dx), cy.saturating_add(dy));
                for end in self.cells.get(&key).into_iter().flatten() {
                    if (end.at.0 - at.x).abs() < COLOCATED_EPS
                        && (end.at.1 - at.y).abs() < COLOCATED_EPS
                    {
                        let dx = end.other.0 - at.x;
                        // Screen y is flipped relative to world y.
                        let dy = -(end.other.1 - at.y);
                        if dx.abs() > COLOCATED_EPS || dy.abs() > COLOCATED_EPS {
                            dirs.push(dy.atan2(dx));
                        }
                    }
                }
            }
        }
        largest_gap_bisector(dirs)
    }
}

/// The bisector of the widest gap between `dirs` around the circle, or
/// `FRAC_PI_2` for none. Order-independent: the directions are sorted first.
fn largest_gap_bisector(mut dirs: Vec<f64>) -> f64 {
    if dirs.is_empty() {
        return std::f64::consts::FRAC_PI_2;
    }
    dirs.sort_by(f64::total_cmp);
    let two_pi = std::f64::consts::TAU;
    let n = dirs.len();
    let mut best_gap = -1.0_f64;
    let mut best_mid = 0.0_f64;
    for i in 0..n {
        let a = dirs[i];
        let b = if i + 1 < n {
            dirs[i + 1]
        } else {
            dirs[0] + two_pi
        };
        let gap = b - a;
        if gap > best_gap {
            best_gap = gap;
            best_mid = a + gap * 0.5;
        }
    }
    best_mid
}

/// A label's box in viewBox units: `(x, y, width, height)`.
pub(super) type LabelBox = (f64, f64, f64, f64);

/// A box spanning more cells than this is kept aside and checked directly, so
/// one pathological label (a very long comment) cannot cost an insertion per
/// cell of its width.
const MAX_CELLS_PER_BOX: i64 = 1024;

/// The boxes every label pass has placed so far, bucketed on a grid so a
/// candidate is tested only against its neighbours.
pub(super) struct PlacedLabels {
    cell: f64,
    boxes: Vec<LabelBox>,
    cells: HashMap<(i64, i64), Vec<usize>>,
    oversized: Vec<usize>,
}

impl PlacedLabels {
    /// `cell` is the grid size in viewBox units: a few label heights.
    pub(super) fn new(cell: f64) -> Self {
        Self {
            cell,
            boxes: Vec::new(),
            cells: HashMap::new(),
            oversized: Vec::new(),
        }
    }

    /// Inclusive cell ranges a box covers, or `None` when it covers more
    /// than [`MAX_CELLS_PER_BOX`].
    fn cell_span(&self, b: LabelBox) -> Option<((i64, i64), (i64, i64))> {
        let x = (cell_of(b.0, self.cell), cell_of(b.0 + b.2, self.cell));
        let y = (cell_of(b.1, self.cell), cell_of(b.1 + b.3, self.cell));
        let count = (x.1.saturating_sub(x.0).saturating_add(1))
            .saturating_mul(y.1.saturating_sub(y.0).saturating_add(1));
        (count <= MAX_CELLS_PER_BOX).then_some((x, y))
    }

    pub(super) fn insert(&mut self, b: LabelBox) {
        let index = self.boxes.len();
        self.boxes.push(b);
        match self.cell_span(b) {
            Some(((x0, x1), (y0, y1))) => {
                for cx in x0..=x1 {
                    for cy in y0..=y1 {
                        self.cells.entry((cx, cy)).or_default().push(index);
                    }
                }
            }
            None => self.oversized.push(index),
        }
    }

    /// Whether `b` overlaps any placed box - the strict-inequality test the
    /// linear scan used. Two boxes that overlap share a cell: the larger of
    /// their left edges lies inside both x-spans (and likewise in y), and
    /// `floor` is monotonic, so its cell is in both cell ranges.
    pub(super) fn overlaps(&self, b: LabelBox) -> bool {
        let hit =
            |p: &LabelBox| b.0 < p.0 + p.2 && b.0 + b.2 > p.0 && b.1 < p.1 + p.3 && b.1 + b.3 > p.1;
        if self.oversized.iter().any(|&i| hit(&self.boxes[i])) {
            return true;
        }
        let Some(((x0, x1), (y0, y1))) = self.cell_span(b) else {
            return self.boxes.iter().any(hit);
        };
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                if let Some(members) = self.cells.get(&(cx, cy)) {
                    if members.iter().any(|&i| hit(&self.boxes[i])) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::models::{SectionData, ShotData, ShotKind, StationMerge};

    /// The open-space angle as it was computed before the index (#576 B22):
    /// a scan of every drawn shot of every section. Kept as the oracle.
    fn open_space_angle_by_scan(
        station_idx: usize,
        stations: &[Station],
        project: &Project,
        station_index: &HashMap<StationRef, usize>,
    ) -> f64 {
        let si_pos = stations[station_idx].coordinates;
        let eps = 1e-6_f64;
        let mut dirs: Vec<f64> = Vec::new();
        let push_dir = |dirs: &mut Vec<f64>, sx: f64, sy: f64, ox: f64, oy: f64| {
            let dx = ox - sx;
            let dy = -(oy - sy);
            if dx.abs() > eps || dy.abs() > eps {
                dirs.push(dy.atan2(dx));
            }
        };
        for section in &project.sections {
            for (i, shot) in section.shots.iter().enumerate() {
                if shot.kind.is_virtual() {
                    continue;
                }
                let from_ref = StationRef::new(section.id, i as u32);
                let to_ref = StationRef::new(section.id, (i + 1) as u32);
                let (Some(&fi), Some(&ti)) =
                    (station_index.get(&from_ref), station_index.get(&to_ref))
                else {
                    continue;
                };
                let from_pos = stations[fi].coordinates;
                let to_pos = stations[ti].coordinates;
                if (from_pos.x - si_pos.x).abs() < eps && (from_pos.y - si_pos.y).abs() < eps {
                    push_dir(&mut dirs, si_pos.x, si_pos.y, to_pos.x, to_pos.y);
                }
                if (to_pos.x - si_pos.x).abs() < eps && (to_pos.y - si_pos.y).abs() < eps {
                    push_dir(&mut dirs, si_pos.x, si_pos.y, from_pos.x, from_pos.y);
                }
            }
        }
        if dirs.is_empty() {
            return std::f64::consts::FRAC_PI_2;
        }
        dirs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let two_pi = std::f64::consts::TAU;
        let n = dirs.len();
        let mut best_gap = -1.0_f64;
        let mut best_mid = 0.0_f64;
        for i in 0..n {
            let a = dirs[i];
            let b = if i + 1 < n {
                dirs[i + 1]
            } else {
                dirs[0] + two_pi
            };
            let gap = b - a;
            if gap > best_gap {
                best_gap = gap;
                best_mid = a + gap * 0.5;
            }
        }
        best_mid
    }

    /// Deterministic xorshift, so the property tests need no dependency.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1_u64 << 53) as f64
        }
    }

    /// Branchy random surveys with merges onto earlier stations, sections
    /// that start at the origin unmerged (colocated twins), zero-length and
    /// virtual shots, and coordinates on and near cell boundaries.
    fn random_project(rng: &mut Rng, sections: usize) -> Project {
        let mut project = Project::new("Random".to_string(), None);
        for _ in 0..sections {
            let shots = (0..=(rng.next() % 12))
                .map(|_| ShotData {
                    azimuth_in: (rng.next() % 8) as f64 * 45.0,
                    azimuth_out: (rng.next() % 8) as f64 * 45.0,
                    length: [0.0, 1.0, 0.5, 2.0, 1e-7][(rng.next() % 5) as usize],
                    kind: if rng.next().is_multiple_of(6) {
                        ShotKind::Reference
                    } else {
                        ShotKind::Normal
                    },
                    ..Default::default()
                })
                .collect();
            project.add_section(SectionData {
                color: "#000000".to_string(),
                shots,
                ..Default::default()
            });
        }
        let ids: Vec<_> = project
            .sections
            .iter()
            .map(|s| (s.id, s.shots.len()))
            .collect();
        let merged_at = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        for k in 1..ids.len() {
            if rng.next().is_multiple_of(3) {
                continue; // left unmerged: starts at the origin
            }
            let (parent, len) = ids[(rng.next() as usize) % k];
            project.station_merges.push(StationMerge {
                station_a: StationRef::new(parent, (rng.next() as usize % (len + 1)) as u32).into(),
                station_b: StationRef::new(ids[k].0, 0).into(),
                merged_at,
            });
        }
        project
    }

    /// The index answers exactly what the scan answered, bit for bit, for
    /// every station of many random surveys.
    #[test]
    fn open_space_angle_matches_the_scan_bit_for_bit() {
        let mut rng = Rng(0x5eed_1234_abcd_ef01);
        let mut checked = 0;
        for _ in 0..40 {
            let sections = 1 + (rng.next() % 25) as usize;
            let project = random_project(&mut rng, sections);
            let stations = project.compute_station_coordinates();
            let station_index: HashMap<StationRef, usize> = stations
                .iter()
                .enumerate()
                .map(|(i, s)| (s.station_ref, i))
                .collect();
            let index = ShotEnds::new(&project, &stations, &station_index);
            for i in 0..stations.len() {
                let fast = index.open_space_angle(stations[i].coordinates);
                let slow = open_space_angle_by_scan(i, &stations, &project, &station_index);
                assert_eq!(fast.to_bits(), slow.to_bits(), "station {i}");
                checked += 1;
            }
        }
        assert!(checked > 1000, "{checked}");
    }

    /// The grid answers exactly what the linear scan over every placed box
    /// answered, for random boxes of every size including ones that only
    /// touch (strict inequality: touching is not overlapping) and ones too
    /// wide for the grid.
    #[test]
    fn placed_labels_overlap_matches_the_linear_scan() {
        let mut rng = Rng(0x0dd_c0ff_ee12_3456);
        let mut grid = PlacedLabels::new(0.5);
        let mut all: Vec<LabelBox> = Vec::new();
        let mut hits = 0;
        for k in 0..4000 {
            let w = if k % 997 == 0 {
                900.0 // spans more than MAX_CELLS_PER_BOX cells
            } else {
                0.05 + rng.unit() * 2.0
            };
            let b: LabelBox = (
                (rng.unit() * 40.0 - 20.0).round() / 4.0,
                (rng.unit() * 40.0 - 20.0).round() / 4.0,
                w,
                0.05 + rng.unit(),
            );
            let linear = all
                .iter()
                .any(|p| b.0 < p.0 + p.2 && b.0 + b.2 > p.0 && b.1 < p.1 + p.3 && b.1 + b.3 > p.1);
            assert_eq!(grid.overlaps(b), linear, "box {k}: {b:?}");
            hits += usize::from(linear);
            if !linear || k % 4 == 0 {
                grid.insert(b);
                all.push(b);
            }
        }
        assert!(hits > 100 && hits < 3900, "{hits}");

        // Touching edges do not overlap, in either direction.
        let mut grid = PlacedLabels::new(1.0);
        grid.insert((0.0, 0.0, 1.0, 1.0));
        assert!(!grid.overlaps((1.0, 0.0, 1.0, 1.0)));
        assert!(!grid.overlaps((0.0, -1.0, 1.0, 1.0)));
        assert!(grid.overlaps((0.999, 0.999, 1.0, 1.0)));
    }
}
