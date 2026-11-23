//! Test support for the undo engine, shared with the persistence crate's
//! size gates and v2 property tests (compiled with `test-support`). Fully
//! synthetic data, deterministic per seed.

use std::sync::Arc;

use chrono::{TimeZone, Utc};

use crate::{
    CombineJoinEnds, CombineLinesOptions, CombinePick, GpsCoordinates, LineId, LineUpdates, Person,
    Section, SectionId, ShotData, ShotId, ShotKind, ShotUpdates, SplitAt, StationId, StationRef,
    SurveyCredit, FEATURE_ID_STRIDE,
};

use super::super::{
    ArchivedStation, ClosureEnd, ClosureStrategy, Counters, LineMeta, Placement, Project,
    RangeDeleteMode, ShotPlace, SplitOptions, StartAt, ZeroDatum,
};
use super::{
    capture, decode_entry, encode_entry, redo, undo, Changeset, Direction, EntryLimits,
    HistoryEntry, HistoryRefs, RecordedPreview,
};

// ----- comparing states ------------------------------------------------------

/// The persisted model, every float by its shortest round-trip spelling:
/// equal text is equal bits (session ids and caches are not persisted).
pub fn canonical(p: &Project) -> String {
    serde_json::to_string(p).expect("a valid model serializes")
}

/// `live` stepped onto `expected`: every counter at least the recorded one
/// (ids handed out are not handed out again), and everything else equal to
/// the bit. Returns the first difference.
pub fn same_state(live: &Project, expected: &Project) -> Result<(), String> {
    let (l, e) = (live.counters, expected.counters);
    if l.next_station_id < e.next_station_id
        || l.next_closure_id < e.next_closure_id
        || l.next_section_id < e.next_section_id
    {
        return Err(format!("a counter went down: live {l:?}, recorded {e:?}"));
    }
    if super::bits::same_model_but_counters(live, expected) {
        return Ok(());
    }
    let mut normalized = live.clone();
    normalized.counters = e;
    let (a, b) = (canonical(&normalized), canonical(expected));
    if a == b {
        return Err("the models differ in a way their text does not show".to_string());
    }
    let at = a
        .bytes()
        .zip(b.bytes())
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let from = at.saturating_sub(80);
    Err(format!(
        "differs at byte {at}:\n live     ...{}\n recorded ...{}",
        a.get(from..(at + 80).min(a.len())).unwrap_or_default(),
        b.get(from..(at + 80).min(b.len())).unwrap_or_default()
    ))
}

/// Every line's session ids: unique, inside the line's window, below its
/// allocator.
pub fn session_ids_sound(p: &Project) -> Result<(), String> {
    for line in p.lines() {
        let base = line.id.saturating_mul(FEATURE_ID_STRIDE);
        let end = base.saturating_add(FEATURE_ID_STRIDE);
        let mut ids: Vec<ShotId> = line.shots.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        if ids.windows(2).any(|w| w[0] == w[1]) {
            return Err(format!("line {}: a repeated shot id", line.id));
        }
        if ids
            .iter()
            .any(|&id| id < base || id >= end || id >= line.next_shot_id)
        {
            return Err(format!("line {}: a shot id outside its window", line.id));
        }
    }
    Ok(())
}

/// The session ids of every line, for an in-session exactness check.
pub fn session_ids(p: &Project) -> Vec<(LineId, Vec<ShotId>)> {
    p.lines()
        .iter()
        .map(|l| (l.id, l.shots.iter().map(|s| s.id).collect()))
        .collect()
}

/// `changes` through the persisted form and back: what a reloaded history
/// holds (no session ids).
pub fn persisted(changes: &Changeset) -> Changeset {
    let entry = HistoryEntry {
        timestamp: Utc
            .with_ymd_and_hms(2026, 11, 20, 12, 0, 0)
            .single()
            .unwrap_or_default(),
        kind: "test".to_string(),
        label: "Test step".to_string(),
        refs: HistoryRefs::default(),
        preview: RecordedPreview::default(),
        changes: changes.clone(),
    };
    let text = encode_entry(&entry).expect("a captured changeset encodes");
    let back = decode_entry(&text, &EntryLimits::default()).expect("an encoded entry decodes");
    let again = encode_entry(&back).expect("a decoded entry encodes");
    assert_eq!(text, again, "the entry text is a fixpoint");
    back.changes
}

/// The text of one entry wrapping `changes` (for the size gates).
pub fn entry_text(changes: &Changeset, label: &str) -> String {
    encode_entry(&HistoryEntry {
        timestamp: Utc
            .with_ymd_and_hms(2026, 11, 20, 12, 0, 0)
            .single()
            .unwrap_or_default(),
        kind: "update_shot".to_string(),
        label: label.to_string(),
        refs: HistoryRefs::default(),
        preview: RecordedPreview::default(),
        changes: changes.clone(),
    })
    .expect("a captured changeset encodes")
}

// ----- the gate surveys ------------------------------------------------------

/// Shots per tree line in the gate surveys.
pub const SHOTS_PER_LINE: usize = 50;

/// The gate surveys' shot: today's `undo_size_gate_tests.rs` values, with a
/// deterministic uuid in the usual 36-character form.
pub fn gate_shot(line: usize, i: usize) -> ShotData {
    let azimuth = ((line * 37 + i * 11) % 360) as f64 + 0.25;
    let uuid = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!("gate-{line}-{i}").as_bytes(),
    );
    ShotData {
        uuid: Some(uuid.to_string()),
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length: 3.0 + ((line + i * 7) % 9) as f64 * 1.125,
        depth_in: 5.0 + (i as f64) * 0.1,
        depth_out: 5.1 + (i as f64) * 0.1,
        left: Some(1.5),
        right: Some(2.0),
        up: Some(0.75),
        down: Some(1.25),
        comment: i
            .is_multiple_of(10)
            .then(|| format!("station note {line}-{i}")),
        marker_index: Some(i as u32),
        ..ShotData::default()
    }
}

/// A gate survey and the handles its actions use.
pub struct GateSurvey {
    pub project: Project,
    /// The tree's lines, in creation order.
    pub lines: Vec<LineId>,
    /// An unconnected 30-shot line.
    pub island: LineId,
    /// A one-shot line hanging off the root line's far end.
    pub leaf: LineId,
    /// The long unconnected line, when asked for.
    pub long: Option<LineId>,
    /// The Section grouping the first ten lines.
    pub section: SectionId,
}

impl GateSurvey {
    /// A tree line in the middle that no closure touches.
    pub fn mid(&self) -> LineId {
        self.lines[self.mid_index()]
    }

    pub fn mid_index(&self) -> usize {
        self.lines.len() / 2 + 1
    }

    /// Station `index` of `line`.
    pub fn station(&self, line: LineId, index: usize) -> StationId {
        station_of(&self.project, line, index)
    }

    /// The session id of shot `index` of `line`.
    pub fn shot_id(&self, line: LineId, index: usize) -> ShotId {
        self.project.line(line).expect("a gate line").shots[index].id
    }
}

fn station_of(p: &Project, line: LineId, index: usize) -> StationId {
    p.line_stations(line).expect("a gate line")[index]
}

fn meta(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: "#4ecdc4".to_string(),
        ..LineMeta::default()
    }
}

/// The gate survey of today's size gate, built with the survey API: a tree
/// of `lines` lines of [`SHOTS_PER_LINE`] shots (each line starting on a
/// station of its parent, a junction), a snap closure every 25 lines, a GPS
/// datum at the entrance, a Section over the first ten lines, an unconnected
/// 30-shot island and a one-shot leaf off the root line's far end (so the two
/// can combine); with `long`, an unconnected line of that many shots too.
pub fn gate_survey(lines: usize, long: Option<usize>) -> GateSurvey {
    let mut p = Project::new_at(
        "Gate",
        None,
        Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
            .single()
            .unwrap_or_default(),
    );
    let shots = |line: usize, n: usize| (0..n).map(|i| gate_shot(line, i)).collect::<Vec<_>>();
    let mut ids: Vec<LineId> = Vec::with_capacity(lines);
    for s in 0..lines {
        let start = if s == 0 {
            StartAt::New
        } else {
            let parent = ids[(s - 1) / 2];
            StartAt::Station(station_of(&p, parent, 1 + (s * 7) % (SHOTS_PER_LINE - 1)))
        };
        let id = p
            .add_line(meta(&format!("Line {s}")), start, shots(s, SHOTS_PER_LINE))
            .expect("a gate line");
        ids.push(id);
    }
    for s in (25..lines).step_by(25) {
        let a = station_of(&p, ids[s], SHOTS_PER_LINE);
        let b = station_of(&p, ids[s - 3], 2);
        p.push_closure(a, Some(ids[s]), b, Some(ids[s - 3]), ClosureStrategy::Move);
    }
    let entrance = station_of(&p, ids[0], 0);
    p.push_datum(ZeroDatum {
        via: Some(ids[0]),
        ..ZeroDatum::new(entrance, gps(20.5, -87.25))
    });
    let section = p.mint_section_id().expect("a Section id");
    p.sections.push(Section {
        id: section,
        name: "Upper level".to_string(),
        color: "#ff6b6b".to_string(),
    });
    for id in ids.iter().take(10) {
        p.line_mut(*id).expect("a gate line").section_id = Some(section);
    }
    let free = |p: &mut Project, name: &str, line: usize, n: usize, at: f64| -> LineId {
        let id = p
            .add_line(meta(name), StartAt::New, shots(line, n))
            .expect("a free line");
        let start = station_of(p, id, 0);
        p.placements.push(Placement {
            station: start,
            x: at,
            y: -at,
        });
        id
    };
    let long = long.map(|n| free(&mut p, "Long", 8_000, n, 900.0));
    let island = free(&mut p, "Island", 9_000, 30, 400.0);
    let root_end = station_of(&p, ids[0], SHOTS_PER_LINE);
    let leaf = p
        .add_line(meta("Leaf"), StartAt::Station(root_end), shots(9_001, 1))
        .expect("the leaf");
    let violations = p.validate();
    assert!(
        violations.is_empty(),
        "the gate survey is valid: {violations:?}"
    );
    GateSurvey {
        project: p,
        lines: ids,
        island,
        leaf,
        long,
        section,
    }
}

/// A GPS position (depth reference 0).
pub fn gps(latitude: f64, longitude: f64) -> GpsCoordinates {
    GpsCoordinates {
        latitude,
        longitude,
        depth_reference: 0.0,
    }
}

/// `Surveyor` on shots `range` of `line` (the credit edit the core makes).
pub fn set_credit(p: &mut Project, line: LineId, range: std::ops::Range<usize>, name: &str) {
    let credit = Arc::new(SurveyCredit {
        surveyors: vec![Person {
            name: name.to_string(),
            email: None,
        }],
        ..SurveyCredit::default()
    });
    let l = p.line_mut(line).expect("a line");
    for shot in &mut l.shots[range] {
        shot.credit = Some(Arc::clone(&credit));
    }
}

// ----- a random walk of survey operations ------------------------------------

/// splitmix64: small, seedable, the same everywhere.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x5DEE_CE66_D1CE_4E5B)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> Option<T> {
        (!items.is_empty()).then(|| items[self.below(items.len())])
    }
}

/// A random walk through the survey operations (the M1/M2 commands through
/// their public API, plus the direct edits the command layer makes: station
/// names and comments, credits, Sections, settings, import hashes).
pub struct Walk {
    pub project: Project,
    pub rng: Rng,
    seed: u64,
    uuids: u64,
    steps: usize,
}

impl Walk {
    pub fn new(seed: u64) -> Self {
        Self {
            project: Project::new_at(
                "Walk",
                None,
                Utc.with_ymd_and_hms(2026, 3, 4, 5, 6, 7)
                    .single()
                    .unwrap_or_default(),
            ),
            rng: Rng::new(seed),
            seed,
            uuids: 0,
            steps: 0,
        }
    }

    fn uuid(&mut self) -> String {
        self.uuids += 1;
        format!("w{}-{}", self.seed, self.uuids)
    }

    /// Awkward but valid numbers now and then: `-0.0`, a subnormal, a long
    /// decimal.
    fn awkward(&mut self, typical: f64) -> f64 {
        match self.rng.below(12) {
            0 => -0.0,
            1 => 0.1 + 0.2,
            2 => typical + f64::EPSILON * 3.0,
            _ => typical,
        }
    }

    fn shot(&mut self) -> ShotData {
        let azimuth = (self.rng.below(360) as f64) + self.rng.unit();
        let depth_in = self.rng.unit() * 20.0;
        let depth_out = depth_in + self.rng.unit() - 0.5;
        let left = self.rng.chance(0.6).then(|| self.awkward(1.25));
        let right = self.rng.chance(0.6).then(|| self.rng.unit() * 3.0);
        ShotData {
            uuid: Some(self.uuid()),
            azimuth_in: azimuth,
            azimuth_out: if self.rng.chance(0.3) {
                (azimuth + 179.0) % 360.0
            } else {
                azimuth
            },
            length: 1.0 + self.rng.unit() * 20.0,
            depth_in,
            depth_out,
            pitch_in: self.rng.chance(0.2).then(|| self.rng.unit() * 40.0 - 20.0),
            pitch_out: self.rng.chance(0.2).then(|| self.awkward(-3.5)),
            left,
            right,
            up: self.rng.chance(0.3).then_some(0.5),
            down: self.rng.chance(0.3).then_some(1.0),
            comment: self
                .rng
                .chance(0.2)
                .then(|| format!("note \u{e9}\"{}", self.uuids)),
            kind: if self.rng.chance(0.1) {
                ShotKind::Jump
            } else {
                ShotKind::Normal
            },
            marker_index: self.rng.chance(0.2).then_some(7),
            temperature: self.rng.chance(0.1).then_some(24.5),
            ..ShotData::default()
        }
    }

    fn line(&mut self) -> Option<LineId> {
        let ids: Vec<LineId> = self.project.lines().iter().map(|l| l.id).collect();
        self.rng.pick(&ids)
    }

    fn station(&mut self) -> Option<StationId> {
        let ids: Vec<StationId> = self.project.stations.keys().copied().collect();
        self.rng.pick(&ids)
    }

    fn shot_of(&mut self, line: LineId) -> Option<(usize, ShotId)> {
        let n = self.project.line(line)?.shots.len();
        if n == 0 {
            return None;
        }
        let i = self.rng.below(n);
        Some((i, self.project.line(line)?.shots[i].id))
    }

    /// One random operation. Returns its name when it changed the project; a
    /// refused operation must leave the project exactly as it was.
    pub fn step(&mut self) -> Option<&'static str> {
        self.steps += 1;
        let before = self.project.clone();
        let (name, ok) = self.operation();
        if !ok {
            assert_eq!(
                canonical(&self.project),
                canonical(&before),
                "seed {}: a refused {name} changed the project",
                self.seed
            );
            return None;
        }
        let violations = self.project.validate();
        assert!(
            violations.is_empty(),
            "seed {} step {}: invalid after {name}: {violations:?}",
            self.seed,
            self.steps
        );
        (canonical(&self.project) != canonical(&before)).then_some(name)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per operation; splitting it scatters the walk"
    )]
    fn operation(&mut self) -> (&'static str, bool) {
        let n = self.steps;
        let empty = self.project.lines().is_empty();
        let choice = if empty { 0 } else { self.rng.below(30) };
        match choice {
            0..=2 => {
                let count = 1 + self.rng.below(5);
                let shots = (0..count).map(|_| self.shot()).collect();
                let start = match self.station() {
                    Some(s) if self.rng.chance(0.5) => StartAt::Station(s),
                    _ => StartAt::New,
                };
                let at = [self.rng.unit() * 200.0, self.rng.unit() * 200.0];
                let r = self
                    .project
                    .create_line(meta(&format!("L{n}")), start, shots, Some(at));
                ("create line", r.is_ok())
            }
            3 | 4 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let data = self.shot();
                let place = match (self.rng.below(4), self.shot_of(line)) {
                    (0, Some((_, id))) => ShotPlace::After(id),
                    (1, Some((_, id))) => ShotPlace::Before(id),
                    (2, _) => ShotPlace::Start,
                    _ => ShotPlace::End,
                };
                (
                    "create shot",
                    self.project.create_shot(line, data, place).is_ok(),
                )
            }
            5 | 6 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let Some((_, id)) = self.shot_of(line) else {
                    return ("skip", false);
                };
                let updates = match self.rng.below(4) {
                    0 => ShotUpdates {
                        length: Some(1.0 + self.rng.unit() * 30.0),
                        azimuth_in: Some(self.rng.unit() * 359.0),
                        ..ShotUpdates::default()
                    },
                    1 => ShotUpdates {
                        comment: Some(Some(format!("c{n}"))),
                        ..ShotUpdates::default()
                    },
                    2 => ShotUpdates {
                        left: Some(Some(self.awkward(2.0))),
                        down: Some(None),
                        ..ShotUpdates::default()
                    },
                    _ => ShotUpdates {
                        depth_out: Some(self.rng.unit() * 30.0),
                        pitch_in: Some(Some(-0.0)),
                        ..ShotUpdates::default()
                    },
                };
                (
                    "update shot",
                    self.project.update_shot(line, id, &updates).is_ok(),
                )
            }
            7 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let Some((_, id)) = self.shot_of(line) else {
                    return ("skip", false);
                };
                let kind = if self.rng.chance(0.5) {
                    ShotKind::Reference
                } else {
                    ShotKind::Jump
                };
                (
                    "set kind",
                    self.project.set_shot_kind(line, id, kind).is_ok(),
                )
            }
            8 | 9 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let Some((_, id)) = self.shot_of(line) else {
                    return ("skip", false);
                };
                ("delete shot", self.project.delete_shot(line, id).is_ok())
            }
            10 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let Some((i, id)) = self.shot_of(line) else {
                    return ("skip", false);
                };
                let length = self.project.line(line).map_or(0.0, |l| l.shots[i].length);
                let distance = length * (0.2 + 0.6 * self.rng.unit());
                let uuid = self.uuid();
                let r = self
                    .project
                    .split_shot_at_distance(line, id, distance, Some(uuid));
                ("split shot", r.is_ok())
            }
            11 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let (Some((ia, a)), Some((ib, b))) = (self.shot_of(line), self.shot_of(line))
                else {
                    return ("skip", false);
                };
                let (a, b) = if ia <= ib { (a, b) } else { (b, a) };
                let mode = if self.rng.chance(0.5) {
                    RangeDeleteMode::Shift
                } else {
                    RangeDeleteMode::Split {
                        name_a: format!("A{n}"),
                        name_b: format!("B{n}"),
                    }
                };
                let r = self.project.delete_shot_range(line, a, b, &mode);
                ("delete range", r.is_ok())
            }
            12 => {
                if self.project.lines().len() < 3 {
                    return ("skip", false);
                }
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                ("delete line", self.project.delete_line(line).is_ok())
            }
            13 | 14 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let swap = self.rng.chance(0.5);
                ("reverse", self.project.reverse_line(line, swap).is_ok())
            }
            15 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let count = self.project.line(line).map_or(0, |l| l.shots.len());
                let at = SplitAt::Station {
                    station_index: self.rng.below(count + 1) as u32,
                };
                let options = SplitOptions {
                    name_a: format!("SA{n}"),
                    name_b: format!("SB{n}"),
                    color_a: "#000000".to_string(),
                    color_b: "#ffffff".to_string(),
                };
                (
                    "split line",
                    self.project.split_line(line, at, &options).is_ok(),
                )
            }
            16 | 17 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let count = self.project.line(line).map_or(0, |l| l.shots.len());
                let end = if self.rng.chance(0.5) { 0 } else { count };
                let slot = StationRef::new(line, end as u32);
                let offered: Vec<_> = self
                    .project
                    .combine_candidates(slot)
                    .into_iter()
                    .filter(|c| c.blocked_reason.is_none())
                    .collect();
                let Some(candidate) =
                    (!offered.is_empty()).then(|| offered[self.rng.below(offered.len())].clone())
                else {
                    return ("skip", false);
                };
                let keep_a = self.rng.chance(0.5);
                let (kept, other, join) = if keep_a {
                    (
                        candidate.line_a,
                        candidate.line_b,
                        CombineJoinEnds {
                            kept: candidate.end_a,
                            other: candidate.end_b,
                        },
                    )
                } else {
                    (
                        candidate.line_b,
                        candidate.line_a,
                        CombineJoinEnds {
                            kept: candidate.end_b,
                            other: candidate.end_a,
                        },
                    )
                };
                let options = CombineLinesOptions {
                    name: format!("C{n}"),
                    color: "#123456".to_string(),
                    swap_lr: self.rng.chance(0.5),
                    comment_pick: CombinePick::Both,
                    name_pick: CombinePick::Kept,
                    join: Some(join),
                };
                (
                    "combine",
                    self.project.combine_lines(kept, other, &options).is_ok(),
                )
            }
            18 => {
                let (Some(a), Some(b)) = (self.station(), self.station()) else {
                    return ("skip", false);
                };
                // Either station kept (RS 4.1); the collisions answered by
                // the preview's rule-L suggestions, or (sometimes) not at
                // all, which refuses with nothing changed.
                let keep = if self.rng.chance(0.5) { a } else { b };
                let renames: Vec<(StationId, String)> = if self.rng.chance(0.8) {
                    self.project
                        .merge_preview(a, b, keep)
                        .map(|preview| {
                            preview
                                .collisions
                                .into_iter()
                                .map(|c| (c.suggestion.station, c.suggestion.to))
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                (
                    "merge",
                    self.project.merge_stations(a, b, keep, &renames).is_ok(),
                )
            }
            19 => {
                // A junction, when there is one.
                let topology = self.project.topology();
                let junctions: Vec<StationId> = topology
                    .stations()
                    .iter()
                    .copied()
                    .filter(|s| topology.is_junction(*s))
                    .collect();
                let Some(station) = self.rng.pick(&junctions) else {
                    return ("skip", false);
                };
                let lines = self.project.topology().lines_of(station);
                let Some(line) = self.rng.pick(&lines) else {
                    return ("skip", false);
                };
                // The remaining junction's record picked at random (RS 4.2
                // case 2), the collisions answered as the dialog would.
                let Ok(naming) = self.project.detach_naming(station, line, None) else {
                    return ("detach", false);
                };
                let rest_keep = self.rng.pick(&naming.rest_choices);
                let renames: Vec<(StationId, String)> = self
                    .project
                    .detach_naming(station, line, rest_keep)
                    .map(|n| {
                        n.collisions
                            .into_iter()
                            .map(|c| (c.station, c.to))
                            .collect()
                    })
                    .unwrap_or_default();
                (
                    "detach",
                    self.project
                        .detach_line(station, line, &renames, rest_keep)
                        .is_ok(),
                )
            }
            20 => {
                let (Some(a), Some(b)) = (self.station(), self.station()) else {
                    return ("skip", false);
                };
                let strategy = if self.rng.chance(0.5) {
                    ClosureStrategy::Batch
                } else {
                    ClosureStrategy::Move
                };
                // An allowed end kept, or "Keep both names" (RS 5.2); the
                // collisions answered by the preview's rule-L suggestions,
                // or (sometimes) not at all, which refuses with nothing
                // changed.
                let mut keeps: Vec<Option<StationId>> = vec![None];
                keeps.extend(
                    self.project
                        .closure_keep_choices(a, b)
                        .allowed
                        .into_iter()
                        .map(Some),
                );
                let keep = self.rng.pick(&keeps).flatten();
                let renames: Vec<(StationId, String)> = if self.rng.chance(0.8) {
                    self.project
                        .closure_naming((a, None), (b, None), strategy, keep)
                        .map(|n| {
                            n.collisions
                                .into_iter()
                                .map(|c| (c.station, c.to))
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let r = self
                    .project
                    .create_closure(a, None, b, None, strategy, keep, &renames);
                ("create closure", r.is_ok())
            }
            21 => {
                let ids: Vec<u64> = self.project.closures.iter().map(|c| c.id).collect();
                let Some(id) = self.rng.pick(&ids) else {
                    return ("skip", false);
                };
                // What comes back answered as the Remove Closure dialog
                // would (RS 5.3), or (sometimes) not at all.
                let renames: Vec<(StationId, String)> = if self.rng.chance(0.8) {
                    self.project
                        .closure_removal(id)
                        .map(|n| {
                            n.collisions
                                .into_iter()
                                .map(|c| (c.station, c.to))
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                (
                    "remove closure",
                    self.project.remove_closure(id, &renames).is_ok(),
                )
            }
            22 => {
                let Some(station) = self.station() else {
                    return ("skip", false);
                };
                let lat = 20.0 + self.rng.unit() * 0.01;
                let lon = -87.0 - self.rng.unit() * 0.01;
                let accuracy = self.rng.chance(0.5).then_some(3.0);
                let r = self
                    .project
                    .set_datum(station, None, gps(lat, lon), accuracy);
                ("set datum", r.is_ok())
            }
            23 => {
                if self.project.datums.is_empty() {
                    return ("skip", false);
                }
                let i = self.rng.below(self.project.datums.len());
                if self.rng.chance(0.3) {
                    let entrance = Some(self.rng.chance(0.5));
                    (
                        "datum entrance",
                        self.project.set_datum_entrance(i, entrance).is_ok(),
                    )
                } else {
                    ("remove datum", self.project.remove_datum(i).is_ok())
                }
            }
            24 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let offset = [
                    self.rng.unit() * 10.0 - 5.0,
                    self.rng.unit() * 10.0 - 5.0,
                    0.0,
                ];
                if self.rng.chance(0.5) {
                    (
                        "move segment",
                        self.project.move_segment(line, offset).is_ok(),
                    )
                } else {
                    let Some(station) = self.station() else {
                        return ("skip", false);
                    };
                    let offset = [offset[0] * 0.1, offset[1] * 0.1, 0.25];
                    (
                        "move station",
                        self.project.move_station(station, offset).is_ok(),
                    )
                }
            }
            25 => {
                let mut order: Vec<LineId> = self.project.lines().iter().map(|l| l.id).collect();
                if order.len() < 2 {
                    return ("skip", false);
                }
                let (i, j) = (self.rng.below(order.len()), self.rng.below(order.len()));
                let moved = order.remove(i);
                order.insert(j, moved);
                ("reorder lines", self.project.reorder_lines(&order).is_ok())
            }
            26 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let updates = LineUpdates {
                    name: Some(Some(format!("Renamed {n}"))),
                    color: self.rng.chance(0.5).then(|| "#abcdef".to_string()),
                    quality: self.rng.chance(0.3).then_some(2.5),
                    ..LineUpdates::default()
                };
                (
                    "update line",
                    self.project.update_line_meta(line, &updates).is_ok(),
                )
            }
            27 => self.station_edit(n),
            28 => self.direct_edit(n),
            _ => {
                let source = self.source();
                ("append", self.project.append_project(&source).is_ok())
            }
        }
    }

    /// A station rename or comment (the command layer edits the station).
    fn station_edit(&mut self, n: usize) -> (&'static str, bool) {
        let Some(id) = self.station() else {
            return ("skip", false);
        };
        if self.rng.chance(0.5) {
            let name = format!("S{n}");
            if !self.project.conflicts_with_name(id, &name).is_empty() {
                return ("rename station", false);
            }
            self.project.set_shown_name(id, name);
            ("rename station", true)
        } else {
            let comment = self.rng.chance(0.7).then(|| format!("comment {n}"));
            if let Some(s) = self.project.station_mut(id) {
                s.comment = comment;
            }
            ("station comment", true)
        }
    }

    /// Credits, Sections, settings and import hashes (direct edits).
    fn direct_edit(&mut self, n: usize) -> (&'static str, bool) {
        match self.rng.below(8) {
            0 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let count = self.project.line(line).map_or(0, |l| l.shots.len());
                if count == 0 {
                    return ("skip", false);
                }
                let a = self.rng.below(count);
                let b = a + 1 + self.rng.below(count - a);
                set_credit(&mut self.project, line, a..b, &format!("Diver {}", n % 3));
                ("credit", true)
            }
            1 => {
                let Ok(id) = self.project.mint_section_id() else {
                    return ("skip", false);
                };
                self.project.sections.push(Section {
                    id,
                    name: format!("Level {n}"),
                    color: "#336699".to_string(),
                });
                if let Some(line) = self.line() {
                    if let Some(l) = self.project.line_mut(line) {
                        l.section_id = Some(id);
                    }
                }
                ("create section", true)
            }
            2 => {
                if self.project.sections.is_empty() {
                    return ("skip", false);
                }
                let i = self.rng.below(self.project.sections.len());
                let id = self.project.sections[i].id;
                if self.rng.chance(0.5) {
                    self.project.sections[i].name = format!("Renamed level {n}");
                } else {
                    self.project.sections.remove(i);
                    for line in self.project.lines_mut() {
                        if line.section_id == Some(id) {
                            line.section_id = None;
                        }
                    }
                }
                ("edit section", true)
            }
            3 => {
                self.project.settings.surface_elevation = Some(self.rng.unit() * 100.0);
                self.project.name = format!("Walk {n}");
                ("settings", true)
            }
            4 => {
                // An azimuth reference: on a line (a geometry edit), or the
                // project default with the location.
                if self.rng.chance(0.5) {
                    let Some(line) = self.line() else {
                        return ("skip", false);
                    };
                    let reference = self.rng.chance(0.7).then(|| {
                        crate::AzimuthReference::magnetic(
                            self.rng.unit() * 10.0 - 5.0,
                            crate::CorrectionSource::Manual,
                        )
                    });
                    let ok = self
                        .project
                        .set_line_azimuth_reference(line, reference)
                        .is_ok();
                    ("azimuth reference", ok)
                } else {
                    self.project.settings.default_azimuth_reference = Some(
                        crate::AzimuthReference::true_north(crate::CorrectionSource::FileDeclared),
                    );
                    self.project.location = Some(format!("Site {n}"));
                    ("azimuth reference", true)
                }
            }
            5 => {
                // A legacy hand move (only migrated files carry them).
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let chain = self
                    .project
                    .line_stations(line)
                    .unwrap_or_default()
                    .to_vec();
                let Some(at) = self.rng.pick(&chain) else {
                    return ("skip", false);
                };
                let hand = [self.rng.unit() - 0.5, self.awkward(0.25), -0.0];
                if let Some(l) = self.project.line_mut(line) {
                    l.hand_moves.insert(at, hand);
                }
                ("hand move", true)
            }
            6 => {
                // An importer's station identity.
                let Some(id) = self.station() else {
                    return ("skip", false);
                };
                if let Some(s) = self.project.station_mut(id) {
                    s.external = Some(format!("EXT{n}"));
                }
                ("external id", true)
            }
            _ => {
                self.project.imported_line_hashes.push(format!("hash-{n}"));
                if self.rng.chance(0.5) {
                    self.project
                        .imported_file_hashes
                        .insert(0, format!("file-{n}"));
                }
                ("import hash", true)
            }
        }
    }

    /// A small valid source project for Add .stalx.
    fn source(&mut self) -> Project {
        let mut s = Project::new_at(
            "Source",
            None,
            Utc.with_ymd_and_hms(2026, 5, 6, 7, 8, 9)
                .single()
                .unwrap_or_default(),
        );
        let count = 1 + self.rng.below(3);
        let shots: Vec<ShotData> = (0..count).map(|_| self.shot()).collect();
        if let Ok(line) = s.add_line(meta("Added"), StartAt::New, shots) {
            if let Some(&start) = s.line_stations(line).and_then(|c| c.first()) {
                s.placements.push(Placement {
                    station: start,
                    x: 10.0,
                    y: 20.0,
                });
            }
        }
        s
    }
}

// ----- recording and replaying a walk ----------------------------------------

/// A walk's states and the changesets captured between them: `states[k + 1]`
/// is `states[k]` after `changes[k]`.
pub struct Recorded {
    pub states: Vec<Project>,
    pub changes: Vec<Changeset>,
    pub ops: Vec<&'static str>,
}

/// Run `steps` random operations from `seed`, capturing after each one that
/// changed the project.
pub fn record_walk(seed: u64, steps: usize) -> Recorded {
    let mut walk = Walk::new(seed);
    let mut out = Recorded {
        states: vec![walk.project.clone()],
        changes: Vec::new(),
        ops: Vec::new(),
    };
    for _ in 0..steps {
        let before = walk.project.clone();
        if let Some(op) = walk.step() {
            out.changes.push(capture(&before, &walk.project));
            out.states.push(walk.project.clone());
            out.ops.push(op);
        }
    }
    out
}

/// Undo every recorded step from the last state, then redo them all,
/// calling `check(live, expected, what)` at every depth. With `persisted`,
/// the steps go through the encoded entries (no session ids); otherwise the
/// session ids must come back exactly too.
pub fn replay(
    recorded: &Recorded,
    persisted_form: bool,
    mut check: impl FnMut(&Project, &Project, &str),
) {
    let changes: Vec<Changeset> = if persisted_form {
        recorded.changes.iter().map(persisted).collect()
    } else {
        recorded.changes.clone()
    };
    let last = recorded.states.len() - 1;
    let mut live = recorded.states[last].clone();
    for k in (0..changes.len()).rev() {
        undo(&mut live, &changes[k])
            .unwrap_or_else(|e| panic!("undo of step {k} ({}): {e}", recorded.ops[k]));
        let expected = &recorded.states[k];
        check(
            &live,
            expected,
            &format!("undo of step {k} ({})", recorded.ops[k]),
        );
        if !persisted_form {
            assert_eq!(
                session_ids(&live),
                session_ids(expected),
                "undo of step {k} ({}): session ids",
                recorded.ops[k]
            );
        }
    }
    for (k, change) in changes.iter().enumerate() {
        redo(&mut live, change)
            .unwrap_or_else(|e| panic!("redo of step {k} ({}): {e}", recorded.ops[k]));
        let expected = &recorded.states[k + 1];
        check(
            &live,
            expected,
            &format!("redo of step {k} ({})", recorded.ops[k]),
        );
        if !persisted_form {
            assert_eq!(
                session_ids(&live),
                session_ids(expected),
                "redo of step {k} ({}): session ids",
                recorded.ops[k]
            );
        }
    }
}

// ----- tampering -------------------------------------------------------------

/// Change, behind the engine's back, one thing a step in `direction` through
/// `changes` checks. Returns what was changed, or `None` when the changeset
/// touches nothing that can be changed this way (counters only). The
/// project may be left invalid: the step must refuse all the same.
pub fn tamper(p: &mut Project, changes: &Changeset, direction: Direction) -> Option<&'static str> {
    let source_after = direction == Direction::Undo;
    let unused_line = p.lines().iter().map(|l| l.id).max().map_or(1, |m| m + 1);
    for change in &changes.lines {
        let has_source = match direction {
            Direction::Undo => change.after.is_some(),
            Direction::Redo => change.before.is_some(),
        };
        if !change.exists(source_after) {
            // A line it creates: put one with that id in its way.
            let mut blocker = p.lines().first()?.clone();
            blocker.id = change.id;
            blocker.shots.clear();
            p.lines_mut().push(blocker);
            return Some("a line where it creates one");
        }
        if let Some(line) = p.line_mut(change.id) {
            if has_source {
                match line.shots.first_mut() {
                    Some(shot) => shot.comment = Some("changed behind its back".to_string()),
                    None => line.color.push('!'),
                }
            } else {
                // Metadata only: every field, so the recorded one differs.
                line.name = Some(format!("{:?}!", line.name));
                line.color.push('!');
                line.quality += 1.0;
                line.direction = match line.direction {
                    crate::SurveyDirection::In => crate::SurveyDirection::Out,
                    crate::SurveyDirection::Out => crate::SurveyDirection::In,
                };
                line.section_id = Some(line.section_id.map_or(0, |s| s.wrapping_add(1)));
                line.start = line.start.wrapping_add(1);
                line.legacy_surveyor = Some("changed".to_string());
                line.hand_moves.insert(u32::MAX, [1.0, 1.0, 1.0]);
                line.azimuth_reference = Some(crate::AzimuthReference::magnetic(
                    1.5,
                    crate::CorrectionSource::Manual,
                ));
            }
            return Some("a line it records");
        }
    }
    let s = &changes.stations;
    let (removed, inserted) = if source_after {
        (&s.add, &s.del)
    } else {
        (&s.del, &s.add)
    };
    if let Some(row) = inserted.first() {
        p.stations
            .insert(row.0.id, crate::survey::Station::new(row.0.id, "blocker"));
        return Some("a station where it creates one");
    }
    let edited = [
        s.name.as_ref().and_then(|c| c.0.ids().next()),
        s.comment.as_ref().and_then(|c| c.0.ids().next()),
        s.archived.as_ref().and_then(|c| c.0.ids().next()),
        s.external.as_ref().and_then(|c| c.0.ids().next()),
        removed.first().map(|r| r.0.id),
    ];
    if let Some(id) = edited.into_iter().flatten().next() {
        if let Some(st) = p.station_mut(id) {
            st.name.push('!');
            st.comment = Some(format!("{:?}!", st.comment));
            st.external = Some(format!("{:?}!", st.external));
            st.archived.push(ArchivedStation {
                id: u32::MAX,
                name: "tampered".to_string(),
                comment: Some("changed behind its back".to_string()),
                external: None,
                into: None,
                lines: Vec::new(),
            });
            if let Some(record) = st.archived.first_mut() {
                record.name.push('!');
                record.lines.push(u32::MAX);
                record.into = Some(record.into.map_or(u32::MAX - 1, |o| o ^ 1));
            }
            return Some("a station it records");
        }
    }
    // Lists: a different length is always seen.
    if changes.datums.is_some() {
        p.datums.push(ZeroDatum::new(0, gps(1.0, 1.0)));
        return Some("the datums");
    }
    if changes.placements.is_some() {
        p.placements.push(Placement {
            station: 0,
            x: 1.0,
            y: 1.0,
        });
        return Some("the placements");
    }
    if let Some(change) = &changes.closures {
        // A closure the step replaces: its archive flipped (RS 10), which a
        // whole-list comparison sees; else one closure more.
        let first = change.h.first();
        let source = first.map_or(0, |h| match direction {
            Direction::Redo => h.1.len(),
            Direction::Undo => h.2.len(),
        });
        if let Some(closure) = first
            .filter(|_| source > 0)
            .and_then(|h| p.closures.get_mut(h.0 as usize))
        {
            closure.archived = match closure.archived {
                None => Some(ClosureEnd::A),
                Some(_) => None,
            };
            return Some("a closure's archive");
        }
        p.closures.push(crate::survey::LoopClosure {
            id: u64::MAX,
            a: 0,
            a_via: None,
            b: 1,
            b_via: None,
            strategy: ClosureStrategy::Batch,
            archived: None,
        });
        return Some("the closures");
    }
    if changes.sections.is_some() {
        p.sections.push(Section {
            id: u32::MAX,
            name: "Extra".to_string(),
            color: "#000".to_string(),
        });
        return Some("the Sections");
    }
    if changes.line_hashes.is_some() || changes.file_hashes.is_some() {
        p.imported_line_hashes.push("extra".to_string());
        p.imported_file_hashes.push("extra".to_string());
        return Some("the import hashes");
    }
    // The name is not a precondition (`apply::prepare`): only the other
    // project fields can make a step refuse.
    let checked = &changes.project;
    if checked.location.is_some() || checked.created.is_some() || checked.settings.is_some() {
        p.name.push('!');
        p.settings.surface_elevation = Some(-1.0);
        p.location = Some("elsewhere".to_string());
        p.created_at += chrono::Duration::seconds(1);
        return Some("the project fields");
    }
    if changes.order.is_some() {
        let mut extra = p.lines().first()?.clone();
        extra.id = unused_line;
        extra.shots.clear();
        p.lines_mut().push(extra);
        return Some("the line order");
    }
    None
}

/// The counters as recorded (for an exact comparison of everything else).
pub fn counters(p: &Project) -> Counters {
    p.counters
}
