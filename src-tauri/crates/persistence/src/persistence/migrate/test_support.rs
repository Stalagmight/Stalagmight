//! Test support for the migration: a seeded generator of LEGACY projects,
//! built through the frozen 0.6.0 model API (`stalagmight_legacy::models`)
//! and written by the 0.6.0 save (`write_v18_with`), and the property check
//! every migrated project must pass.
//!
//! Everything is synthetic: invented geometry, names and GPS positions; the
//! files exist only in memory (and a private temp dir while SQLite edits
//! them).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use stalagmight_legacy::models::{
    self as m, ClosureStrategy as OldStrategy, GpsCoordinates, LineData, LineId, ShotData,
    StationKey, StationMerge, StationRef,
};
use stalagmight_legacy::persistence::ResolvedWarning;
use stalagmight_legacy::{
    write_v18_with, FileShape, LegacyExtras, LegacySurvey, OpenOptions, WriteOptions,
};

use crate::models::survey::ClosureStrategy;

use super::{migrate, AdjustedCheck, ExpectedClass, Migrated};
use crate::models::config::ClosureAdjustmentConfig;

/// SplitMix64: small, fast, and the same on every platform.
pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed ^ 0x5EED_0FA1_1D47_A000)
    }

    pub(super) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub(super) fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    pub(super) fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub(super) fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    pub(super) fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

/// SQL edits applied to the written file, reproducing what older builds left
/// in files the current writer no longer produces.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Degrade {
    /// Clear every `n`-th shot uuid (a pre-v6 file: the load derives them).
    pub(super) null_uuid_every: Option<i64>,
    /// Give the last shot the uuid of a shot of another line (a file written
    /// before uuids were project-unique: the load renames one copy).
    pub(super) duplicate_uuid: bool,
    /// Clear the uuid of every other ignored warning (pre-v8 rows).
    pub(super) null_ignore_uuids: bool,
}

/// One generated legacy file.
pub(super) struct LegacyCase {
    pub(super) project: m::Project,
    pub(super) extras: LegacyExtras,
    pub(super) shape: FileShape,
    pub(super) degrade: Degrade,
}

impl LegacyCase {
    /// The bytes an old build would have written.
    pub(super) fn write(&self) -> Vec<u8> {
        let options = WriteOptions {
            shape: self.shape,
            ..WriteOptions::default()
        };
        let bytes = write_v18_with(&self.project, &self.extras, &options).unwrap();
        degrade(bytes, self.degrade)
    }

    /// Write, then open with the frozen reader.
    pub(super) fn open(&self) -> LegacySurvey {
        stalagmight_legacy::open_bytes(&self.write(), &OpenOptions::default()).unwrap()
    }
}

fn degrade(bytes: Vec<u8>, degrade: Degrade) -> Vec<u8> {
    if degrade.null_uuid_every.is_none() && !degrade.duplicate_uuid && !degrade.null_ignore_uuids {
        return bytes;
    }
    edit_sqlite(&bytes, |conn| {
        if let Some(n) = degrade.null_uuid_every {
            conn.execute("UPDATE shots SET uuid = NULL WHERE id % ?1 = 0", [n])
                .unwrap();
        }
        if degrade.duplicate_uuid {
            conn.execute_batch(
                "UPDATE shots SET uuid = (
                     SELECT s2.uuid FROM shots s2
                     WHERE s2.section_id != shots.section_id AND s2.uuid IS NOT NULL
                     ORDER BY s2.id LIMIT 1)
                 WHERE id = (SELECT max(id) FROM shots)
                   AND EXISTS (SELECT 1 FROM shots s3
                               WHERE s3.section_id != shots.section_id
                                 AND s3.uuid IS NOT NULL);",
            )
            .unwrap();
        }
        if degrade.null_ignore_uuids {
            conn.execute(
                "UPDATE resolved_warnings SET shot_uuid = NULL WHERE id % 2 = 0",
                [],
            )
            .unwrap();
        }
    })
}

/// Apply `edit` to a copy of the SQLite file `bytes` and return the result.
pub(super) fn edit_sqlite(bytes: &[u8], edit: impl FnOnce(&rusqlite::Connection)) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edited.stalx");
    std::fs::write(&path, bytes).unwrap();
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        edit(&conn);
    }
    std::fs::read(&path).unwrap()
}

/// Mints deterministic shot uuids for one case.
struct Uuids {
    seed: u64,
    next: u64,
}

impl Uuids {
    fn mint(&mut self) -> String {
        self.next += 1;
        format!("{:08x}-0000-4000-8000-{:012x}", self.seed as u32, self.next)
    }
}

fn gps(rng: &mut Rng) -> GpsCoordinates {
    GpsCoordinates::new(
        20.5 + rng.range(-0.01, 0.01),
        -87.3 + rng.range(-0.01, 0.01),
        0.0,
    )
    .unwrap()
}

fn shot_data(rng: &mut Rng, uuids: &mut Uuids, depth: &mut f64) -> ShotData {
    let azimuth = rng.range(0.0, 360.0);
    let length = rng.range(0.5, 15.0);
    let dz = rng.range(-0.4, 0.4) * length;
    let depth_in = *depth;
    *depth = (depth_in + dz).max(0.0);
    let optional =
        |rng: &mut Rng, p: f64, lo: f64, hi: f64| rng.chance(p).then(|| rng.range(lo, hi));
    ShotData {
        uuid: Some(uuids.mint()),
        azimuth_in: azimuth,
        azimuth_out: (azimuth + rng.range(-1.5, 1.5)).rem_euclid(360.0),
        length,
        depth_in,
        depth_out: *depth,
        pitch_in: optional(rng, 0.3, -30.0, 30.0),
        pitch_out: optional(rng, 0.2, -30.0, 30.0),
        left: optional(rng, 0.5, 0.0, 4.0),
        right: optional(rng, 0.5, 0.0, 4.0),
        up: optional(rng, 0.3, 0.0, 3.0),
        down: optional(rng, 0.3, 0.0, 3.0),
        comment: rng.chance(0.1).then(|| "note".to_string()),
        kind: match rng.below(12) {
            0 => m::ShotKind::Jump,
            1 => m::ShotKind::Reference,
            _ => m::ShotKind::Normal,
        },
        marker_index: rng.chance(0.1).then(|| rng.below(500) as u32),
        temperature: optional(rng, 0.1, 18.0, 26.0),
        ..ShotData::default()
    }
}

fn slot(project: &m::Project, rng: &mut Rng, line: LineId) -> StationRef {
    let len = project.get_line(line).map_or(0, |l| l.shots.len());
    StationRef::new(line, rng.below(len + 1) as u32)
}

fn any_slot(project: &m::Project, rng: &mut Rng) -> StationRef {
    let line = project.lines[rng.below(project.lines.len())].id;
    slot(project, rng, line)
}

fn key(project: &m::Project, sr: StationRef) -> Option<StationKey> {
    project.key_for_ref(sr)
}

/// A closure record as an older build (or a destructive adjust) stored it.
fn raw_closure(
    project: &m::Project,
    a: StationRef,
    b: StationRef,
    strategy: OldStrategy,
    aligned: bool,
) -> Option<m::LoopClosure> {
    Some(m::LoopClosure {
        id: 0,
        station_a: key(project, a)?,
        station_b: key(project, b)?,
        path_length: 10.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy,
        is_aligned: aligned,
    })
}

fn raw_merge(project: &mut m::Project, a: StationRef, b: StationRef, at: DateTime<Utc>) {
    if let (Some(station_a), Some(station_b)) = (key(project, a), key(project, b)) {
        project.station_merges.push(StationMerge {
            station_a,
            station_b,
            merged_at: at,
        });
    }
}

/// Two slots of one line that do not span exactly one shot.
fn same_line_pair(project: &m::Project, rng: &mut Rng) -> Option<(StationRef, StationRef)> {
    let candidates: Vec<&m::Line> = project
        .lines
        .iter()
        .filter(|l| l.shots.len() >= 2)
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let line = candidates[rng.below(candidates.len())];
    let n = line.shots.len() as u32;
    let i = rng.below(n as usize - 1) as u32;
    let j = i + 2 + rng.below((n - i - 1) as usize) as u32;
    Some((
        StationRef::new(line.id, i),
        StationRef::new(line.id, j.min(n)),
    ))
}

/// A random legacy project for `seed`. See the module tests for the list of
/// shapes it covers.
pub(super) fn random_case(seed: u64) -> LegacyCase {
    let mut rng = Rng::new(seed);
    let mut uuids = Uuids { seed, next: 0 };
    let at = DateTime::from_timestamp(1_700_000_000 + seed as i64, 0).unwrap();
    let mut p = m::Project::new(format!("Synthetic {seed}"), None);
    p.created_at = at;
    p.modified_at = at;

    // Lines: empty, single-shot and longer ones, some with a legacy surveyor.
    let line_count = 3 + rng.below(9);
    for n in 0..line_count {
        let shots = match rng.below(10) {
            0 => 0,
            1 => 1,
            _ => 2 + rng.below(7),
        };
        let mut depth = rng.range(0.0, 30.0);
        let data = LineData {
            name: Some(format!("L{n}")),
            surveyor: rng.chance(0.2).then(|| "Ada; Bo".to_string()),
            date: None,
            color: "#336699".to_string(),
            direction: Some(m::SurveyDirection::In),
            quality: 1.0,
            shots: (0..shots)
                .map(|_| shot_data(&mut rng, &mut uuids, &mut depth))
                .collect(),
        };
        p.add_line(data);
    }

    // Hand moves before the merges (a merge holds the shown position).
    for _ in 0..rng.below(4) {
        let sr = any_slot(&p, &mut rng);
        if let Some(k) = key(&p, sr) {
            let offset = (
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
                rng.range(-0.5, 0.5),
            );
            p.station_offsets.insert(k, offset);
        }
    }

    // Merges onto earlier lines; some with a hand move on both members.
    let ids: Vec<LineId> = p.lines.iter().map(|l| l.id).collect();
    for (i, &line) in ids.iter().enumerate().skip(1) {
        for _ in 0..=usize::from(rng.chance(0.3)) {
            if !rng.chance(0.75) {
                continue;
            }
            let a = slot(&p, &mut rng, line);
            let target = ids[rng.below(i)];
            let b = slot(&p, &mut rng, target);
            if rng.chance(0.15) {
                for sr in [a, b] {
                    if let Some(k) = key(&p, sr) {
                        let offset = (
                            rng.range(-0.5, 0.5),
                            rng.range(-0.5, 0.5),
                            rng.range(-0.3, 0.3),
                        );
                        p.station_offsets.insert(k, offset);
                    }
                }
            }
            let _ = p.merge_stations(a, b);
        }
    }

    // A resurvey of two shots of a line, tied back with an aligned snap.
    if rng.chance(0.3) {
        if let Some(line) = p.lines.iter().find(|l| l.shots.len() >= 3).map(|l| l.id) {
            let source = p
                .get_line(line)
                .map(|l| l.shots[..2].to_vec())
                .unwrap_or_default();
            let shots = source
                .iter()
                .map(|s| ShotData {
                    uuid: Some(uuids.mint()),
                    ..s.to_shot_data()
                })
                .collect();
            let bypass = p
                .add_line(LineData {
                    name: Some("Bypass".to_string()),
                    surveyor: None,
                    date: None,
                    color: "#aa0000".to_string(),
                    direction: None,
                    quality: 1.0,
                    shots,
                })
                .id;
            if p.merge_stations(StationRef::new(bypass, 0), StationRef::new(line, 0))
                .is_ok()
            {
                let _ = p.create_loop_closure(
                    StationRef::new(bypass, 2),
                    StationRef::new(line, 2),
                    OldStrategy::Move,
                );
            }
        }
    }

    // Datums: placements, GPS fixes, separate datum pieces.
    for _ in 0..rng.below(4) {
        let sr = any_slot(&p, &mut rng);
        let g = gps(&mut rng);
        let _ = p.set_zero_datum(sr, g);
    }
    if let Some(datum) = p.zero_datums.first().cloned() {
        if rng.chance(0.2) {
            let _ = p.set_zero_datum_entrance(
                p.resolve_key(&datum.station_ref)
                    .unwrap_or(StationRef::new(0, 0)),
                Some(rng.chance(0.5)),
            );
        }
    }

    // Pieces moved by hand: free ones anywhere, datum pieces off their GPS
    // (an older build could leave them there; the load keeps them).
    let mut moved: BTreeSet<LineId> = BTreeSet::new();
    for line in ids.clone() {
        if moved.contains(&line) || !rng.chance(0.35) {
            continue;
        }
        let group = p.get_merged_line_ids(line);
        moved.extend(group.iter().copied());
        let (dx, dy) = (rng.range(-200.0, 200.0), rng.range(-200.0, 200.0));
        p.adjust_rigid_group_offsets(line, dx, dy);
    }
    // A drifted merge: one line nudged off its partners; the load heals it.
    if rng.chance(0.3) {
        let line = ids[rng.below(ids.len())];
        p.adjust_line_offset(line, rng.range(-0.05, 0.05), rng.range(-0.05, 0.05));
    }

    // Closures.
    for _ in 0..rng.below(3) {
        let (a, b) = (any_slot(&p, &mut rng), any_slot(&p, &mut rng));
        let strategy = if rng.chance(0.2) {
            OldStrategy::Move
        } else {
            OldStrategy::Batch
        };
        let _ = p.create_loop_closure(a, b, strategy);
    }
    if rng.chance(0.3) {
        // A legacy aligned non-Move closure (a destructive adjust's stamp).
        let (a, b) = (any_slot(&p, &mut rng), any_slot(&p, &mut rng));
        let strategy = if rng.chance(0.5) {
            OldStrategy::Batch
        } else {
            OldStrategy::LeastSquaresWeighted
        };
        if a != b && p.get_connected_line_ids(a.line_id).contains(&b.line_id) {
            if let Some(c) = raw_closure(&p, a, b, strategy, true) {
                p.loop_closures.push(c);
            }
        }
    }
    if rng.chance(0.2) {
        // A closure across one shot.
        if let Some(line) = p.lines.iter().find(|l| !l.shots.is_empty()).map(|l| l.id) {
            let i = rng.below(p.get_line(line).map_or(1, |l| l.shots.len())) as u32;
            if let Some(c) = raw_closure(
                &p,
                StationRef::new(line, i),
                StationRef::new(line, i + 1),
                OldStrategy::Batch,
                false,
            ) {
                p.loop_closures.push(c);
            }
        }
    }
    if rng.chance(0.2) {
        // An orphan closure between two pieces: the load promotes it.
        let (a, b) = (any_slot(&p, &mut rng), any_slot(&p, &mut rng));
        if !p.get_connected_line_ids(a.line_id).contains(&b.line_id) {
            if let Some(c) = raw_closure(&p, a, b, OldStrategy::Batch, false) {
                p.loop_closures.push(c);
            }
        }
    }

    // Raw closures carry id 0 until an id is assigned (combine needs them).
    p.assign_closure_ids();

    // Merge records no current command writes: cycles, same-line, self and
    // repeated merges.
    if rng.chance(0.3) {
        let (a, b) = (any_slot(&p, &mut rng), any_slot(&p, &mut rng));
        if a.line_id != b.line_id && p.get_merged_line_ids(a.line_id).contains(&b.line_id) {
            raw_merge(&mut p, a, b, at);
        }
    }
    if rng.chance(0.15) {
        if let Some((a, b)) = same_line_pair(&p, &mut rng) {
            raw_merge(&mut p, a, b, at);
        }
    }
    if rng.chance(0.1) {
        let a = any_slot(&p, &mut rng);
        raw_merge(&mut p, a, a, at);
    }
    if rng.chance(0.1) {
        if let Some(merge) = p.station_merges.first().cloned() {
            p.station_merges.push(merge);
        }
    }

    // Edits that move keys and gauges around.
    if rng.chance(0.3) {
        let line = ids[rng.below(ids.len())];
        let _ = p.reverse_line(line, rng.chance(0.5));
    }
    if rng.chance(0.25) {
        if let Some(line) = p.lines.iter().find(|l| l.shots.len() >= 3).map(|l| l.id) {
            let options = m::SplitLineOptions {
                name_a: format!("Split {line} A"),
                name_b: format!("Split {line} B"),
                color_a: "#112233".to_string(),
                color_b: "#445566".to_string(),
                comment_side: m::SplitSide::A,
                datum_side: m::SplitSide::A,
            };
            if let Ok(second) = p.split_line_at_station(line, 2, &options) {
                if rng.chance(0.5) {
                    let _ = p.combine_lines(line, second, &combine_options());
                }
            }
        }
    }
    if rng.chance(0.2) {
        let (a, b) = (ids[rng.below(ids.len())], ids[rng.below(ids.len())]);
        if a != b {
            let _ = p.combine_lines(a, b, &combine_options());
        }
    }

    // Names, comments and Sections, merged members included.
    for _ in 0..rng.below(5) {
        let sr = any_slot(&p, &mut rng);
        let name = format!("N{}", rng.below(4));
        let _ = p.set_station_name(sr, &name);
    }
    for merge in p.station_merges.iter().take(3).cloned().collect::<Vec<_>>() {
        if rng.chance(0.5) {
            let text = if rng.chance(0.5) { "sump" } else { "tight" };
            p.station_comments
                .insert(merge.station_a.clone(), text.to_string());
            let other = if rng.chance(0.5) { text } else { "low air" };
            p.station_comments
                .insert(merge.station_b.clone(), other.to_string());
        }
    }
    for _ in 0..rng.below(3) {
        let sr = any_slot(&p, &mut rng);
        if let Some(k) = key(&p, sr) {
            let text = if rng.chance(0.2) { "" } else { "restriction" };
            p.station_comments.insert(k, text.to_string());
        }
    }
    if rng.chance(0.3) {
        let section = p.add_section("North".to_string(), "#aa3300".to_string());
        let line = p.lines[0].id;
        p.set_line_section(line, Some(section));
    }

    let extras = random_extras(&p, &mut rng, at);
    let degrade = Degrade {
        null_uuid_every: rng.chance(0.15).then(|| 3 + rng.below(3) as i64),
        duplicate_uuid: rng.chance(0.15),
        null_ignore_uuids: rng.chance(0.3),
    };
    LegacyCase {
        project: p,
        extras,
        shape: if rng.chance(0.3) {
            FileShape::Release055
        } else {
            FileShape::Current
        },
        degrade,
    }
}

fn combine_options() -> m::CombineLinesOptions {
    m::CombineLinesOptions {
        name: "Combined".to_string(),
        color: "#225588".to_string(),
        swap_lr: false,
        comment_pick: m::CombinePick::Both,
        name_pick: m::CombinePick::Kept,
        join: None,
    }
}

fn random_extras(p: &m::Project, rng: &mut Rng, at: DateTime<Utc>) -> LegacyExtras {
    let mut resolved = Vec::new();
    let shots: Vec<(LineId, &m::Shot)> = p
        .lines
        .iter()
        .flat_map(|l| l.shots.iter().map(move |s| (l.id, s)))
        .collect();
    if !shots.is_empty() {
        // The last shot too: the duplicate-uuid degradation renames it.
        let picks = [
            rng.below(shots.len()),
            shots.len() - 1,
            rng.below(shots.len()),
        ];
        for (n, &i) in picks.iter().enumerate() {
            let (line, shot) = shots[i];
            resolved.push(ResolvedWarning {
                line_id: line,
                shot_id: shot.id,
                warning_code: if n == 1 { "LONG_SHOT" } else { "SHORT_SHOT" }.to_string(),
                resolved_at: at,
                resolved_comment: (n == 0).then(|| "checked".to_string()),
                shot_uuid: shot.uuid.clone(),
            });
        }
    }
    LegacyExtras {
        display_settings: BTreeMap::from([("show_labels".to_string(), "true".to_string())]),
        resolved_warnings: resolved,
        speleo_link: rng.chance(0.3).then(|| m::SpeleoLink {
            endpoint: "https://example.invalid".to_string(),
            project_id: "00000000-0000-4000-8000-000000000001".to_string(),
            project_name: "Synthetic".to_string(),
            base_commit_sha: None,
            has_unpushed_edits: true,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: Some(m::UndoMode::Full),
        }),
        speleo_undo_chain: None,
        speleo_undo_watermark: None,
    }
}

/// The properties every migration of a legacy survey must have (plan 11.1,
/// E7 7.1). `Err` names the first one that failed.
pub(super) fn check(legacy: &LegacySurvey, migrated: &Migrated) -> Result<(), String> {
    let lp = &legacy.project;
    let p = &migrated.project;
    let r = &migrated.report;

    let violations = p.validate();
    if !violations.is_empty() {
        return Err(format!("validate: {}", violations[0]));
    }

    // Lines, chains, the total station map.
    if p.lines().len() != lp.lines.len() {
        return Err("line count".to_string());
    }
    let mut station_of: HashMap<(LineId, u32), u32> = HashMap::new();
    for (old, new) in lp.lines.iter().zip(p.lines()) {
        if old.id != new.id || old.shots.len() != new.shots.len() {
            return Err(format!("line {} shape", old.id));
        }
        for (index, station) in new.stations().enumerate() {
            if !p.stations.contains_key(&station) {
                return Err(format!("slot {}:{index} has no station", old.id));
            }
            station_of.insert((old.id, index as u32), station);
        }
        // Measurements bit-identical.
        for (i, (a, b)) in old.shots.iter().zip(&new.shots).enumerate() {
            if !same_measurements(a, b) {
                return Err(format!("shot {}:{i} measurements", old.id));
            }
        }
    }
    let at = |sr: StationRef| station_of.get(&(sr.line_id, sr.station_index)).copied();

    // Every line shows each of its stations by the name 0.6.0 showed there
    // (its stored name, else its index), or the change is declared in the
    // conversion notice's list (RS 8.3: a junction shows its kept station on
    // every line, and a repeat takes a letter). RS-F: the 8.3 oracle (rule K,
    // rule L never lettering an identity shown on two or more lines, hidden
    // comments).
    let declared: BTreeSet<(String, String, String)> = migrated
        .extras
        .names_changed
        .iter()
        .map(|c| (c.line.clone(), c.from.clone(), c.to.clone()))
        .collect();
    if r.names_changed != migrated.extras.names_changed.len() {
        return Err(format!(
            "names changed: {} reported, {} listed",
            r.names_changed,
            migrated.extras.names_changed.len()
        ));
    }
    for line in &lp.lines {
        for index in 0..=line.shots.len() {
            let sr = StationRef::new(line.id, index as u32);
            let Some(station) = at(sr) else {
                continue;
            };
            let was = lp
                .station_name_at(sr)
                .map_or_else(|| index.to_string(), str::to_string);
            let now = p.shown_name(station).unwrap_or_default();
            let label = p.line_label_of(line.id);
            if now != was && !declared.contains(&(label, was.clone(), now.to_string())) {
                return Err(format!(
                    "slot {}:{index} reads {now:?}, 0.6.0 showed {was:?}, undeclared",
                    line.id
                ));
            }
        }
    }

    // Merged members are one station, or tied by a Tie closure.
    let ties: BTreeSet<(u32, u32)> = p
        .closures
        .iter()
        .filter(|c| c.strategy == ClosureStrategy::Tie)
        .map(|c| (c.a.min(c.b), c.a.max(c.b)))
        .collect();
    for merge in &lp.station_merges {
        let (Some(a), Some(b)) = (
            lp.resolve_key(&merge.station_a),
            lp.resolve_key(&merge.station_b),
        ) else {
            continue;
        };
        let (Some(sa), Some(sb)) = (at(a), at(b)) else {
            return Err("merge end unmapped".to_string());
        };
        if sa != sb && !ties.contains(&(sa.min(sb), sa.max(sb))) {
            return Err(format!("merge {a}~{b}: neither one station nor tied"));
        }
    }

    // Closures preserved (same id, mapped ends) or reported.
    let by_id: HashMap<u64, &crate::models::survey::LoopClosure> =
        p.closures.iter().map(|c| (c.id, c)).collect();
    let mut missing = 0;
    for closure in &lp.loop_closures {
        let ends = lp
            .resolve_key(&closure.station_a)
            .zip(lp.resolve_key(&closure.station_b))
            .and_then(|(a, b)| at(a).zip(at(b)));
        match (by_id.get(&closure.id), ends) {
            (Some(c), Some((a, b))) if c.a == a && c.b == b => {
                let expected = if closure.strategy != OldStrategy::Move && closure.is_aligned {
                    ClosureStrategy::Tie
                } else {
                    super::convert::closure_strategy(closure.strategy)
                };
                if c.strategy != expected {
                    return Err(format!("closure {} strategy", closure.id));
                }
            }
            _ => missing += 1,
        }
    }
    let dropped = r.closures_dropped_dangling
        + r.closures_dropped_one_station
        + r.closures_dropped_unanchored
        + r.closure_ids_reminted;
    if missing != dropped {
        return Err(format!("closures: {missing} missing, {dropped} reported"));
    }

    // Counts.
    if p.stations.len() + r.merges_collapsed != r.slots {
        return Err("stations + collapsed merges != slots".to_string());
    }
    let datums = lp
        .zero_datums
        .iter()
        .filter(|d| lp.resolve_key(&d.station_ref).is_some())
        .count();
    if p.datums.len() != datums {
        return Err("datum count".to_string());
    }
    let shot_uuids_changed = lp
        .lines
        .iter()
        .zip(p.lines())
        .flat_map(|(a, b)| a.shots.iter().zip(&b.shots))
        .filter(|(a, b)| a.uuid.as_deref() != Some(b.uuid.as_str()))
        .count();
    if shot_uuids_changed != r.shot_uuids_minted {
        return Err("shot uuids changed without being reported".to_string());
    }
    // Every legacy Station ID survives, as a station or an archived record.
    let mut ids: BTreeSet<u32> = BTreeSet::new();
    for station in p.stations.values() {
        ids.insert(station.id);
        ids.extend(station.archived.iter().map(|r| r.id));
    }
    let lost = lp
        .station_ids
        .values()
        .filter(|id| !ids.contains(id))
        .count();
    if lost != r.station_ids_dropped {
        return Err(format!(
            "{lost} station ids lost, {} reported",
            r.station_ids_dropped
        ));
    }

    // Datums keep their order and their member's line as the `via` hint.
    let legacy_datums = lp
        .zero_datums
        .iter()
        .filter_map(|d| lp.resolve_key(&d.station_ref));
    for (sr, datum) in legacy_datums.zip(&p.datums) {
        if at(sr) != Some(datum.station) || datum.via != Some(sr.line_id) {
            return Err(format!("datum at {sr}: station or via hint"));
        }
    }
    // Every datum-less piece is placed at its legacy gauge station (the
    // station the old least-squares solve held), never by the fallback.
    if r.placements_fallback != 0 {
        return Err("a piece had no legacy gauge".to_string());
    }
    let topology = p.topology();
    for gauge in legacy.datumless_gauges() {
        let Some(station) = at(gauge.station) else {
            return Err("gauge slot unmapped".to_string());
        };
        let component = topology.component_of(station);
        let placed = p
            .placements
            .iter()
            .find(|pl| topology.component_of(pl.station) == component);
        if placed.map(|pl| pl.station) != Some(station) {
            return Err(format!(
                "the piece held at {} is placed elsewhere",
                gauge.station
            ));
        }
    }

    // A settled legacy file needs none of the repairs for broken references.
    let repairs = [
        ("collapsed torn merges", r.merges_collapsed_torn),
        ("changed datum roles", r.datum_roles_changed),
        (
            "dropped datums",
            r.datums_dropped_dangling + r.datums_dropped_invalid,
        ),
        ("dangling merges", r.merges_dropped_dangling),
        ("dangling station keys", r.station_keys_dangling),
    ];
    if let Some((what, n)) = repairs.iter().find(|(_, n)| *n > 0) {
        return Err(format!("{n} {what} in a settled file"));
    }

    // Positions.
    let v = &r.verification;
    if let Some(m) = v.mismatches.first() {
        return Err(format!(
            "{} unexplained position mismatches, first {}:{} {:?} {:e} m",
            v.mismatches.len(),
            m.line_id,
            m.station_index,
            m.axis,
            m.displacement_m
        ));
    }
    if v.slots_checked != r.slots {
        return Err("not every slot was verified".to_string());
    }
    if v.expected.contains_key(&ExpectedClass::TornMergeCollapsed) {
        return Err("a merge the legacy load kept torn was collapsed".to_string());
    }

    // The Closures Computed View (plan 7.2.7): every slot within tolerance
    // of 0.6.0's, or explained by a declared rule (D10, D11); the same
    // components fall back to measured.
    let AdjustedCheck::Compared(a) = &v.adjusted else {
        return Err("the Closures Computed View was not compared".to_string());
    };
    if let Some(m) = a.mismatches.first() {
        return Err(format!(
            "{} unexplained Computed View mismatches, first {}:{} {:?} {:e} m",
            a.mismatches.len(),
            m.line_id,
            m.station_index,
            m.axis,
            m.displacement_m
        ));
    }
    if let Some(lines) = a.failure_mismatches.first() {
        return Err(format!(
            "{} least-squares fallbacks differ, first on lines {lines:?}",
            a.failure_mismatches.len()
        ));
    }
    if a.slots_checked != r.slots {
        return Err("not every slot's Computed View position was verified".to_string());
    }
    let ties = r.merges_demoted_cycle + r.merges_demoted_same_line + r.aligned_closures_to_tie;
    if a.expected.contains_key(&ExpectedClass::TieSolved) && ties == 0 {
        return Err("a Computed View difference blamed on a Tie, without one".to_string());
    }
    if a.expected.contains_key(&ExpectedClass::TieNoLongerFails)
        && r.merges_demoted_cycle + r.merges_demoted_same_line == 0
    {
        return Err(
            "a Computed View difference blamed on a demoted merge, without one".to_string(),
        );
    }
    if a.expected.contains_key(&ExpectedClass::MergeTieSkipped) {
        // A `Tie` the legacy file did not have as a closure is a demoted merge.
        let legacy_ids: BTreeSet<u64> = legacy.project.loop_closures.iter().map(|c| c.id).collect();
        let p = &migrated.project;
        let skips_a_merge = p
            .adjusted_positions(&ClosureAdjustmentConfig::default())
            .skipped()
            .iter()
            .any(|s| {
                !legacy_ids.contains(&s.closure_id)
                    && p.closures
                        .iter()
                        .any(|c| c.id == s.closure_id && c.strategy == ClosureStrategy::Tie)
            });
        if !skips_a_merge {
            return Err(
                "a Computed View difference blamed on a skipped merge, without one".to_string(),
            );
        }
    }
    legacy_precision_classes_hold(legacy, a)?;

    // Deterministic.
    let again = migrate(legacy, &ClosureAdjustmentConfig::default())
        .map_err(|e| format!("second migrate: {e}"))?;
    if again.project != migrated.project
        || again.report != migrated.report
        || again.extras != migrated.extras
    {
        return Err("migrating twice gave different results".to_string());
    }
    let json = |project: &crate::models::survey::Project| {
        serde_json::to_string(project).map_err(|e| format!("serialize: {e}"))
    };
    let (a, b) = (json(&again.project)?, json(&migrated.project)?);
    if a != b {
        return Err("migrating twice serialized differently".to_string());
    }
    Ok(())
}

/// The G9 classes (C24 F2 and F3) against the whole legacy project: a class
/// blamed on 0.6.0's weights or rewrite needs a shot that has them, and the
/// solve-precision class stays within the project-wide bound (the largest
/// weight spread and coordinate, raw or a datum's GPS, so at least any
/// component's).
fn legacy_precision_classes_hold(
    legacy: &LegacySurvey,
    a: &super::AdjustedVerification,
) -> Result<(), String> {
    use super::verify::{
        legacy_rewrite_precision_m, legacy_solve_precision_m, ADJUSTED_TOLERANCE_M, DUST_PER_HOP_M,
    };
    use crate::models::survey::ShotSigmas;
    let config = ClosureAdjustmentConfig::default();
    let lp = &legacy.project;
    let (mut floored, mut rewrite) = (false, 0.0_f64);
    let mut weights = (f64::INFINITY, 0.0_f64);
    for line in &lp.lines {
        let reference = lp.azimuth_reference_for(line);
        for shot in &line.shots {
            let azimuth = shot.average_azimuth_true_rad(reference);
            let sigmas = ShotSigmas::of(&config, line.quality, shot.length, azimuth);
            floored |= sigmas.floored();
            for sigma in sigmas.axes {
                let w = 1.0 / (sigma * sigma);
                if w.is_finite() && w > 0.0 {
                    weights = (weights.0.min(w), weights.1.max(w));
                }
            }
            rewrite += legacy_rewrite_precision_m(
                shot.length,
                shot.depth_change(),
                shot.horizontal_length(),
            );
        }
    }
    if a.expected
        .contains_key(&ExpectedClass::LegacyWeightsDegenerate)
        && !floored
    {
        return Err(
            "a Computed View difference blamed on 0.6.0's weights, without a shot under the floor"
                .to_string(),
        );
    }
    if a.expected.contains_key(&ExpectedClass::LegacyRewriteFailed)
        && !(rewrite > ADJUSTED_TOLERANCE_M
            && legacy
                .adjustment_failures
                .iter()
                .any(|f| f.error.contains("inconsistent line translation")))
    {
        return Err(
            "a Computed View difference blamed on 0.6.0's rewrite, without a failed check it explains"
                .to_string(),
        );
    }
    if let Some(stats) = a.expected.get(&ExpectedClass::LegacySolvePrecision) {
        let datums = lp.frame_datum().map_or_else(Vec::new, |frame| {
            lp.zero_datums
                .iter()
                .map(|d| {
                    let (east, north) =
                        stalagmight_legacy::models::gps_to_local(&d.gps, &frame.gps);
                    east.abs().max(north.abs())
                })
                .collect()
        });
        let scale = legacy
            .raw_positions
            .values()
            .map(|p| p[0].abs().max(p[1].abs()))
            .chain(datums)
            .filter(|m| m.is_finite())
            .fold(0.0_f64, f64::max);
        let hops = f64::from(u32::try_from(lp.lines.len()).unwrap_or(u32::MAX));
        let bound =
            ADJUSTED_TOLERANCE_M + DUST_PER_HOP_M * hops + legacy_solve_precision_m(weights, scale);
        // NaN on either side fails too.
        let within = matches!(
            stats.max_displacement_m.partial_cmp(&bound),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        );
        if !within {
            return Err(format!(
                "a Computed View difference of {:e} m blamed on 0.6.0's solve precision, over its bound {bound:e} m",
                stats.max_displacement_m
            ));
        }
    }
    Ok(())
}

fn bits(v: Option<f64>) -> Option<u64> {
    v.map(f64::to_bits)
}

/// Every measurement of a legacy shot equals the migrated one, bit for bit.
fn same_measurements(a: &m::Shot, b: &crate::models::survey::Shot) -> bool {
    let fixed = |x: f64| x.to_bits();
    let credit_a = a.credit.as_ref().filter(|c| !c.is_empty()).map(|c| {
        (
            c.surveyors
                .iter()
                .map(|p| (p.name.clone(), p.email.clone()))
                .collect::<Vec<_>>(),
            c.explorers
                .iter()
                .map(|p| (p.name.clone(), p.email.clone()))
                .collect::<Vec<_>>(),
            // As the file holds it (F10): out-of-range dates are clamped.
            c.date.map(super::super::stalx_v2::normalized_date),
        )
    });
    let credit_b = b.credit.as_ref().map(|c| {
        (
            c.surveyors
                .iter()
                .map(|p| (p.name.clone(), p.email.clone()))
                .collect::<Vec<_>>(),
            c.explorers
                .iter()
                .map(|p| (p.name.clone(), p.email.clone()))
                .collect::<Vec<_>>(),
            c.date,
        )
    });
    fixed(a.azimuth_in) == fixed(b.azimuth_in)
        && fixed(a.azimuth_out) == fixed(b.azimuth_out)
        && fixed(a.length) == fixed(b.length)
        && fixed(a.depth_in) == fixed(b.depth_in)
        && fixed(a.depth_out) == fixed(b.depth_out)
        && bits(a.pitch_in) == bits(b.pitch_in)
        && bits(a.pitch_out) == bits(b.pitch_out)
        && bits(a.left) == bits(b.left)
        && bits(a.right) == bits(b.right)
        && bits(a.up) == bits(b.up)
        && bits(a.down) == bits(b.down)
        && bits(a.temperature) == bits(b.temperature)
        && a.comment == b.comment
        && super::convert::shot_kind(a.kind) == b.kind
        && a.marker_index == b.marker_index
        && credit_a == credit_b
}
