//! Migration fuzz (C24, plan 11.1 and 11.6): hostile legacy projects, built
//! through the frozen 0.6.0 API on top of the property gate's generator
//! (`test_support::random_case`) and written by the 0.6.0 save
//! (`write_v18_with`, both file shapes), then degraded with SQL the way
//! other builds and tools leave files. Every legacy feature at once, in the
//! combinations the gate does not reach: chains of merges, merges onto
//! closure ends, merge cycles and same-line merges, several datums in one
//! piece and on one merged station, closures between merged stations, hand
//! moves on merged members, drifted and moved line offsets, entrance
//! overrides, duplicate, empty and odd station names, comments, LRUD with
//! gaps, unknown tables and columns, persisted undo and redo rows, empty and
//! single-station lines, huge and tiny (NaN-free) values, shots under 20 cm
//! near a grid axis (finding C24-F2, `Knobs::degenerate_lsq`) and shots and
//! depths of 10 km and more (finding C24-F3, `Knobs::huge_lengths`); the
//! two findings were out of the default draw until plan Deviations G9
//! resolved them. The generator is deterministic per seed across processes
//! (no set or map order reaches the rng).
//!
//! Properties, per seed:
//! 1. the frozen open and the migration succeed or refuse with a typed
//!    reason, and never panic (`MigrateError::Invalid` is a migration bug);
//! 2. the migrated project passes the property gate's whole check
//!    (`test_support::check`: validate, total station map, every slot raw
//!    and in the Computed View within tolerance or in a declared class,
//!    measurements bit-identical, counts, determinism), and its Computed
//!    View moves no station further than the misclosures it distributes
//!    (`computed_view_within_its_misclosures`);
//! 3. the legacy bytes sniff and probe as legacy, `decode` refuses them as
//!    `DecodeError::Legacy`, and the legacy header counts what the file holds;
//! 4. the v2 file decodes writable, validates, equals the migrated model and
//!    re-encodes byte for byte; a second open and save is a byte fixpoint;
//!    every slot keeps its raw and Computed View bits; the probe's summary
//!    counts what the survey holds;
//! 5. the wire projection matches 0.6.0's payloads up to the declared
//!    classes (`wire_equivalence_tests::compare`);
//! 6. `legacy_origin` (plan 11.4): a random walk through the survey
//!    operations from the migrated project undoes and redoes to identical v2
//!    bytes in the session and through the persisted entries, and the
//!    entries ride the file's history (undo and redo) and step the decoded
//!    project exactly.
//!
//! Windows (farm style): `STALAGMIGHT_FUZZ_SEED_START` / `STALAGMIGHT_FUZZ_SEEDS`.
//! `migration_fuzz_window` runs a small fixed window in CI (or the env
//! window); `migration_fuzz_wide` (ignored) runs 2,000 seeds by default on
//! `STALAGMIGHT_FUZZ_THREADS` threads. A failure prints one line: the seed,
//! the test that replays it alone, and what failed.
//!
//! `migration_fuzz_explain` (ignored) prints one seed's features, report,
//! view hashes and verdict, with the knobs as env switches.
//!
//! `migration_fuzz_corpus` (ignored) runs properties 1 and 3 to 6 on every
//! `*.stalx` under `STALAGMIGHT_MIGRATION_CORPUS` (real files, on the fuzz
//! host only). It prints file-name hashes, counts and seed repros only:
//! never a name, a coordinate or a failure's text, which can quote the file.
//!
//! Synthetic data only, except the corpus mode.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveDate, Utc};
use stalagmight_legacy::models::{
    self as m, ClosureStrategy as OldStrategy, GpsCoordinates, LineData, LineId, ShotData,
    StationKey, StationMerge, StationRef,
};
use stalagmight_legacy::persistence::ResolvedWarning;
use stalagmight_legacy::{
    write_v18_with, FileShape, LegacyError, LegacySurvey, OpenOptions, WriteOptions,
};

use crate::models::config::ClosureAdjustmentConfig;
use crate::models::survey::changeset::testkit::{
    entry_text, replay, same_state, session_ids_sound, Recorded, Walk,
};
use crate::models::survey::changeset::{capture, decode_entry, redo, undo, EntryLimits};
use crate::models::survey::{Project, SlotPosition};

use super::super::stalx_v2::{
    decode, encode, probe_bytes, sniff, DecodeError, DecodeOptions, Extras, Format, History,
    IgnoredWarning, LocalSync, Mode, RawJson, SpeleoDbLink,
};
use super::test_support::{check, edit_sqlite, random_case, Rng};
use super::wire_equivalence_tests::compare as wire_compare;
use super::{legacy_header, migrate, ExpectedClass, MigrateError, Migrated, MigrationReport};

/// Seeds the CI window runs when no window is set.
const CI_SEEDS: u64 = 24;
/// Seeds the wide run takes when no window is set.
const WIDE_SEEDS: u64 = 2_000;
/// Steps of the legacy-origin walk per seed.
const WALK_STEPS: usize = 24;
/// Walk seeds per corpus file (`STALAGMIGHT_FUZZ_WALKS` overrides).
const CORPUS_WALKS: u64 = 4;
/// Steps of each corpus walk.
const CORPUS_WALK_STEPS: usize = 40;

const TEST_PATH: &str = "persistence::migrate::migrate_fuzz_tests";

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

/// The farm window: `STALAGMIGHT_FUZZ_SEED_START` and `STALAGMIGHT_FUZZ_SEEDS`.
fn window(default_count: u64) -> (u64, u64) {
    (
        env_u64("STALAGMIGHT_FUZZ_SEED_START").unwrap_or(0),
        env_u64("STALAGMIGHT_FUZZ_SEEDS").unwrap_or(default_count),
    )
}

/// What failed, by a stable class (safe to print for a private file) and a
/// detail (printed for synthetic seeds only).
#[derive(Debug)]
struct Failure {
    class: &'static str,
    detail: String,
}

fn fail(class: &'static str, detail: impl Into<String>) -> Failure {
    Failure {
        class,
        detail: detail.into(),
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".to_string())
}

/// Counts of what a run exercised (hostile features applied, refusal
/// classes, report counters), for the coverage gate and the sweep summary.
#[derive(Debug, Default)]
struct Tally {
    counts: BTreeMap<String, usize>,
}

impl Tally {
    fn add(&mut self, key: &str, n: usize) {
        if n > 0 {
            *self.counts.entry(key.to_string()).or_default() += n;
        }
    }

    fn absorb(&mut self, other: &Tally) {
        for (k, v) in &other.counts {
            *self.counts.entry(k.clone()).or_default() += v;
        }
    }

    fn get(&self, key: &str) -> usize {
        self.counts.get(key).copied().unwrap_or(0)
    }

    fn report(&mut self, r: &MigrationReport) {
        let fields: [(&str, usize); 17] = [
            ("report.merges_collapsed", r.merges_collapsed),
            ("report.merges_demoted_cycle", r.merges_demoted_cycle),
            (
                "report.merges_demoted_same_line",
                r.merges_demoted_same_line,
            ),
            ("report.merges_dropped_self", r.merges_dropped_self),
            (
                "report.merges_dropped_redundant",
                r.merges_dropped_redundant,
            ),
            ("report.aliases_created", r.aliases_created),
            ("report.comments_joined", r.comments_joined),
            ("report.stations_renamed", r.stations_renamed),
            ("report.names_changed", r.names_changed),
            ("report.aligned_closures_to_tie", r.aligned_closures_to_tie),
            ("report.corrections_stored", r.corrections_stored),
            ("report.hand_moves_carried", r.hand_moves_carried),
            ("report.datums_kept", r.datums_kept),
            (
                "report.closures_dropped_one_station",
                r.closures_dropped_one_station,
            ),
            (
                "report.entrance_overrides_written",
                r.entrance_overrides_written,
            ),
            ("report.legacy_healed_offsets", r.legacy_healed_offsets),
            (
                "report.history_rows_dropped",
                r.history_rows_dropped as usize,
            ),
        ];
        for (k, v) in fields {
            self.add(k, v);
        }
        if let super::AdjustedCheck::Compared(a) = &r.verification.adjusted {
            for (class, stats) in &a.expected {
                self.add(&format!("report.adjusted.{}", class.as_str()), stats.slots);
            }
        }
    }
}

// ----- the hostile generator ---------------------------------------------------

/// One hostile legacy file and what went into it.
struct Hostile {
    bytes: Vec<u8>,
    features: Tally,
}

/// Odd but valid station and line names.
const ODD_NAMES: [&str; 10] = [
    "A1",
    "",
    " ",
    "q\"uote",
    "back\\slash",
    "\u{6d1e}\u{7a74}",
    "\u{1f987}",
    "tab\there",
    "nul\u{0}in",
    "sep\u{2028}line",
];

fn odd_name(rng: &mut Rng) -> String {
    if rng.chance(0.1) {
        "x".repeat(300)
    } else {
        ODD_NAMES[rng.below(ODD_NAMES.len())].to_string()
    }
}

/// A NaN-free extreme double: zeros of both signs, a subnormal, the smallest
/// normal, very small and very large magnitudes, long decimals.
fn extreme(rng: &mut Rng) -> f64 {
    const VALUES: [f64; 11] = [
        0.0,
        -0.0,
        1e-9,
        5e-324,
        f64::MIN_POSITIVE,
        1e-3,
        0.300_000_000_000_000_04,
        1e4,
        1e5,
        123_456.789,
        2.5,
    ];
    VALUES[rng.below(VALUES.len())]
}

fn odd_azimuth(rng: &mut Rng) -> f64 {
    const VALUES: [f64; 6] = [
        0.0,
        -0.0,
        359.999_999_999_999_94,
        360.0,
        180.0,
        0.300_000_000_000_000_04,
    ];
    VALUES[rng.below(VALUES.len())]
}

/// Whether `azimuth` is within 2 degrees of a grid axis (0, 90, 180, 270).
fn near_grid(azimuth: f64) -> bool {
    let off = azimuth.rem_euclid(90.0);
    off < 2.0 || 90.0 - off < 2.0
}

fn credit(rng: &mut Rng) -> Option<Arc<m::SurveyCredit>> {
    if !rng.chance(0.3) {
        return None;
    }
    let person = |rng: &mut Rng| m::Person {
        name: odd_name(rng),
        email: rng.chance(0.5).then(|| "diver@example.invalid".to_string()),
    };
    Some(Arc::new(m::SurveyCredit {
        surveyors: (0..rng.below(3)).map(|_| person(rng)).collect(),
        explorers: (0..rng.below(2)).map(|_| person(rng)).collect(),
        date: if rng.chance(0.6) {
            let (y, mo, d) = [(1900, 1, 1), (2099, 12, 31), (2024, 2, 29)][rng.below(3)];
            NaiveDate::from_ymd_opt(y, mo, d)
        } else {
            None
        },
    }))
}

/// Generator switches (the rng stream is the same whatever they say, so a
/// seed replays with any of them). The default draws everything; the env
/// switches ([`knobs_from_env`]) turn the shapes of findings C24-F2 and F3
/// off again to tell them apart while triaging.
#[derive(Debug, Clone, Copy)]
struct Knobs {
    /// Shots under 20 cm within 2 degrees of a grid axis (C24-F2).
    degenerate_lsq: bool,
    /// The hostile lines' extreme values at all.
    extremes: bool,
    /// Shots of 10 km and more, depths of 10 km and more (C24-F3); off,
    /// they are capped at about 500 m and 2.2 km.
    huge_lengths: bool,
}

const DEFAULT_KNOBS: Knobs = Knobs {
    degenerate_lsq: true,
    extremes: true,
    huge_lengths: true,
};

/// The draw wave B's default was when C24 pinned its seeds (the findings'
/// shapes off); the pinned tests replay them with it.
const C24_DRAW: Knobs = Knobs {
    degenerate_lsq: false,
    extremes: true,
    huge_lengths: false,
};

/// The default knobs with the env switches applied (the window, wide and
/// explain runs read them): `STALAGMIGHT_FUZZ_DEGENERATE_LSQ=0|1` and
/// `STALAGMIGHT_FUZZ_HUGE=0|1` set those shapes, and
/// `STALAGMIGHT_FUZZ_NO_EXTREMES=1` turns the extreme values off.
fn knobs_from_env() -> Knobs {
    let switch = |name: &str, default: bool| match std::env::var(name).as_deref() {
        Ok("0") => false,
        Ok(_) => true,
        Err(_) => default,
    };
    Knobs {
        degenerate_lsq: switch(
            "STALAGMIGHT_FUZZ_DEGENERATE_LSQ",
            DEFAULT_KNOBS.degenerate_lsq,
        ),
        extremes: DEFAULT_KNOBS.extremes
            && std::env::var_os("STALAGMIGHT_FUZZ_NO_EXTREMES").is_none(),
        huge_lengths: switch("STALAGMIGHT_FUZZ_HUGE", DEFAULT_KNOBS.huge_lengths),
    }
}

/// One shot of a hostile line. With `Knobs::degenerate_lsq` off, a shot
/// under 20 cm never lies within 2 degrees of a grid axis: such a shot had a
/// huge to infinite (or, at zero length, no) first-order least-squares
/// weight on one axis, which the solve now floors (finding C24-F2, pinned by
/// `short_shots_on_a_grid_axis_keep_the_computed_view`).
fn hostile_shot(
    rng: &mut Rng,
    uuids: &mut u64,
    seed: u64,
    extreme_values: bool,
    knobs: Knobs,
) -> ShotData {
    *uuids += 1;
    let lrud = |rng: &mut Rng| match rng.below(4) {
        0 => None,
        1 if extreme_values => Some(extreme(rng)),
        _ => Some(rng.range(0.0, 5.0)),
    };
    let (length, azimuth, depth_in, depth_out) = if extreme_values {
        (
            extreme(rng),
            odd_azimuth(rng),
            if rng.chance(0.2) { -5.0 } else { extreme(rng) },
            extreme(rng),
        )
    } else {
        let depth = rng.range(0.0, 40.0);
        (
            rng.range(0.5, 15.0),
            rng.range(0.0, 360.0),
            depth,
            depth + rng.range(-2.0, 2.0),
        )
    };
    // Over 1 km, unless asked for: 10 to 123 km become 400 to 510 m (the
    // same rng stream either way, so a seed replays with the knob on).
    let length = if !knobs.huge_lengths && length > 1e3 {
        length.log10() * 100.0
    } else {
        length
    };
    // The deepest caves are about 2.2 km deep.
    let (depth_in, depth_out) = if knobs.huge_lengths {
        (depth_in, depth_out)
    } else {
        (depth_in.min(2_200.0), depth_out.min(2_200.0))
    };
    let azimuth = if !knobs.degenerate_lsq && length.abs() < 0.2 && near_grid(azimuth) {
        (azimuth + 12.5) % 360.0
    } else {
        azimuth
    };
    ShotData {
        uuid: Some(format!(
            "{:08x}-c24c-4000-8000-{:012x}",
            seed as u32, *uuids
        )),
        azimuth_in: azimuth,
        azimuth_out: if rng.chance(0.2) {
            (azimuth + 180.0) % 360.0
        } else {
            azimuth
        },
        length,
        depth_in,
        depth_out,
        pitch_in: rng.chance(0.2).then(|| [90.0, -90.0, -0.0][rng.below(3)]),
        pitch_out: rng.chance(0.1).then(|| rng.range(-90.0, 90.0)),
        left: lrud(rng),
        right: lrud(rng),
        up: lrud(rng),
        down: lrud(rng),
        comment: rng.chance(0.2).then(|| odd_name(rng)),
        kind: match rng.below(8) {
            0 => m::ShotKind::Jump,
            1 => m::ShotKind::Reference,
            _ => m::ShotKind::Normal,
        },
        marker_index: rng.chance(0.1).then(|| [0, 1, u32::MAX][rng.below(3)]),
        temperature: rng.chance(0.1).then(|| [-2.0, 40.0, -0.0][rng.below(3)]),
        credit: credit(rng),
    }
}

fn slot_on(p: &m::Project, rng: &mut Rng, line: LineId) -> StationRef {
    let len = p.get_line(line).map_or(0, |l| l.shots.len());
    StationRef::new(line, rng.below(len + 1) as u32)
}

fn any_slot(p: &m::Project, rng: &mut Rng) -> Option<StationRef> {
    if p.lines.is_empty() {
        return None;
    }
    let line = p.lines[rng.below(p.lines.len())].id;
    Some(slot_on(p, rng, line))
}

fn any_key(p: &m::Project, rng: &mut Rng) -> Option<StationKey> {
    any_slot(p, rng).and_then(|sr| p.key_for_ref(sr))
}

fn gps(rng: &mut Rng) -> Option<GpsCoordinates> {
    GpsCoordinates::new(
        20.5 + rng.range(-0.01, 0.01),
        -87.3 + rng.range(-0.01, 0.01),
        if rng.chance(0.2) {
            rng.range(-3.0, 3.0)
        } else {
            0.0
        },
    )
    .ok()
}

fn raw_merge(p: &mut m::Project, a: StationRef, b: StationRef, at: DateTime<Utc>) -> bool {
    match (p.key_for_ref(a), p.key_for_ref(b)) {
        (Some(station_a), Some(station_b)) => {
            p.station_merges.push(StationMerge {
                station_a,
                station_b,
                merged_at: at,
            });
            true
        }
        _ => false,
    }
}

fn raw_closure(
    p: &mut m::Project,
    a: StationKey,
    b: StationKey,
    strategy: OldStrategy,
    aligned: bool,
) {
    p.loop_closures.push(m::LoopClosure {
        id: 0,
        station_a: a,
        station_b: b,
        path_length: 10.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy,
        is_aligned: aligned,
    });
}

/// The hostile layer over the gate's generator. Every step uses the frozen
/// API (refusals ignored, as a user's refused edit) or writes a record the
/// way an older build or a crafted file stored it.
#[expect(
    clippy::too_many_lines,
    reason = "one block per hostile feature; splitting it scatters the generator"
)]
fn hostile_case(seed: u64, knobs: Knobs) -> Option<Hostile> {
    let extremes = knobs.extremes;
    let base = random_case(seed);
    let mut rng = Rng::new(seed ^ 0x0C24_C24C_24C2_4C24);
    let mut p = base.project;
    let mut extras = base.extras;
    let at = p.created_at;
    let mut f = Tally::default();
    let mut uuids = 0u64;

    // Odd lines: empty, single-shot, extreme values; some hung on the survey.
    for n in 0..rng.below(4) {
        let shots = match rng.below(4) {
            0 => 0,
            1 => 1,
            _ => 2 + rng.below(5),
        };
        let extreme_values = rng.chance(0.5) && extremes;
        let data = (0..shots)
            .map(|_| hostile_shot(&mut rng, &mut uuids, seed, extreme_values, knobs))
            .collect();
        let id = p
            .add_line(LineData {
                name: rng
                    .chance(0.8)
                    .then(|| format!("H{n} {}", odd_name(&mut rng))),
                surveyor: rng.chance(0.2).then(|| odd_name(&mut rng)),
                date: rng.chance(0.2).then_some(NaiveDate::MIN),
                color: ["#abc", "#ff0000", "#00FF00", "red"][rng.below(4)].to_string(),
                direction: [
                    None,
                    Some(m::SurveyDirection::In),
                    Some(m::SurveyDirection::Out),
                ][rng.below(3)],
                quality: [1.0, 0.25, 3.0][rng.below(3)],
                shots: data,
            })
            .id;
        f.add("odd lines", 1);
        f.add("empty lines", usize::from(shots == 0));
        f.add("extreme values", usize::from(extreme_values && shots > 0));
        if rng.chance(0.5) {
            let target = p.lines[rng.below(p.lines.len())].id;
            if target != id {
                let a = slot_on(&p, &mut rng, id);
                let b = slot_on(&p, &mut rng, target);
                let _ = p.merge_stations(a, b);
            }
        }
    }

    let ids: Vec<LineId> = p.lines.iter().map(|l| l.id).collect();
    // Chains of merges: line k+1 merged onto the member line k was merged at.
    if ids.len() >= 3 && rng.chance(0.6) {
        let first = ids[rng.below(ids.len())];
        let mut at_slot = slot_on(&p, &mut rng, first);
        for _ in 0..2 + rng.below(3) {
            let next = ids[rng.below(ids.len())];
            if next == at_slot.line_id {
                continue;
            }
            let a = slot_on(&p, &mut rng, next);
            if p.merge_stations(a, at_slot).is_ok() {
                f.add("merge chains", 1);
                at_slot = a;
            }
        }
    }
    // Merges onto closure ends.
    for closure in p.loop_closures.clone().iter().take(2) {
        if !rng.chance(0.5) {
            continue;
        }
        let Some(end) = p.resolve_key(&closure.station_a) else {
            continue;
        };
        let line = ids[rng.below(ids.len())];
        if line != end.line_id {
            let a = slot_on(&p, &mut rng, line);
            if p.merge_stations(a, end).is_ok() {
                f.add("merges onto closure ends", 1);
            }
        }
    }
    // Cycle-closing, same-line, self and repeated merges (raw records).
    for _ in 0..rng.below(3) {
        let (Some(a), Some(b)) = (any_slot(&p, &mut rng), any_slot(&p, &mut rng)) else {
            break;
        };
        if a.line_id != b.line_id && p.get_merged_line_ids(a.line_id).contains(&b.line_id) {
            f.add("cycle merges", usize::from(raw_merge(&mut p, a, b, at)));
        } else if a.line_id == b.line_id && a != b {
            f.add("same-line merges", usize::from(raw_merge(&mut p, a, b, at)));
        }
    }
    if rng.chance(0.1) {
        if let Some(a) = any_slot(&p, &mut rng) {
            f.add("self merges", usize::from(raw_merge(&mut p, a, a, at)));
        }
    }

    let merges: Vec<StationMerge> = p.station_merges.iter().cloned().collect();
    // Datums: several in one piece, on both members of a merge, twice on
    // one station (raw), with entrance overrides.
    if rng.chance(0.5) {
        if let Some(sr) = any_slot(&p, &mut rng) {
            // Sorted: the set's order is random per process.
            let mut group: Vec<LineId> = p.get_merged_line_ids(sr.line_id).into_iter().collect();
            group.sort_unstable();
            for _ in 0..2 + rng.below(3) {
                let line = group[rng.below(group.len())];
                let slot = slot_on(&p, &mut rng, line);
                if let Some(g) = gps(&mut rng) {
                    f.add(
                        "datums in one piece",
                        usize::from(p.set_zero_datum(slot, g).is_ok()),
                    );
                }
            }
        }
    }
    if let Some(merge) = merges.first().filter(|_| rng.chance(0.5)) {
        for key in [&merge.station_a, &merge.station_b] {
            if let (Some(sr), Some(g)) = (p.resolve_key(key), gps(&mut rng)) {
                f.add(
                    "datums on a merged station",
                    usize::from(p.set_zero_datum(sr, g).is_ok()),
                );
            }
        }
    }
    if rng.chance(0.15) {
        if let (Some(datum), Some(g)) = (p.zero_datums.first().cloned(), gps(&mut rng)) {
            p.zero_datums.push(m::ZeroDatum::new(datum.station_ref, g));
            f.add("two datums on one key", 1);
        }
    }
    for datum in p.zero_datums.clone() {
        if rng.chance(0.3) {
            if let Some(sr) = p.resolve_key(&datum.station_ref) {
                let entrance = Some(rng.chance(0.5));
                f.add(
                    "entrance overrides",
                    usize::from(p.set_zero_datum_entrance(sr, entrance)),
                );
            }
        }
    }

    // Closures between merged stations, and between a merge member and its
    // neighbour (across one shot).
    for merge in merges.iter().take(2) {
        if rng.chance(0.4) {
            let strategy = if rng.chance(0.5) {
                OldStrategy::Batch
            } else {
                OldStrategy::Move
            };
            raw_closure(
                &mut p,
                merge.station_a.clone(),
                merge.station_b.clone(),
                strategy,
                rng.chance(0.3),
            );
            f.add("closures between merged stations", 1);
        }
    }
    for _ in 0..rng.below(3) {
        if let (Some(a), Some(b)) = (any_slot(&p, &mut rng), any_slot(&p, &mut rng)) {
            let strategy = [
                OldStrategy::Batch,
                OldStrategy::Move,
                OldStrategy::EqualDistribution,
            ][rng.below(3)];
            if p.create_loop_closure(a, b, strategy).is_ok() {
                f.add("closures", 1);
            }
        }
    }
    p.assign_closure_ids();

    // Hand moves, merged members included, tiny and large.
    for _ in 0..rng.below(5) {
        if let Some(key) = any_key(&p, &mut rng) {
            let scale = [1e-12, 0.5, 5.0][rng.below(3)];
            let offset = (
                rng.range(-1.0, 1.0) * scale,
                rng.range(-1.0, 1.0) * scale,
                rng.range(-1.0, 1.0) * scale,
            );
            p.station_offsets.insert(key, offset);
            f.add("hand moves", 1);
        }
    }
    for merge in merges.iter().take(1) {
        if rng.chance(0.3) {
            p.station_offsets
                .insert(merge.station_a.clone(), (0.25, -0.25, 0.1));
            p.station_offsets
                .insert(merge.station_b.clone(), (-0.5, 0.0, 0.0));
            f.add("hand moves on both members", 1);
        }
    }
    // Line offsets: drift (the load heals it) and moved pieces.
    for _ in 0..rng.below(3) {
        let line = ids[rng.below(ids.len())];
        let scale = if rng.chance(0.5) { 0.05 } else { 1000.0 };
        p.adjust_line_offset(
            line,
            rng.range(-1.0, 1.0) * scale,
            rng.range(-1.0, 1.0) * scale,
        );
        f.add("line offsets", 1);
    }

    // Names: duplicates on one line and across a merge, odd ones (raw).
    for _ in 0..rng.below(4) {
        if let Some(key) = any_key(&p, &mut rng) {
            p.station_names.insert(key, odd_name(&mut rng));
            f.add("odd names", 1);
        }
    }
    if rng.chance(0.5) {
        if let Some(line) = p.lines.iter().find(|l| l.shots.len() >= 2).map(|l| l.id) {
            let name = format!("dup{}", rng.below(3));
            for index in [0, 1, 2] {
                if let Some(key) = p.key_for_ref(StationRef::new(line, index)) {
                    p.station_names.insert(key, name.clone());
                }
            }
            f.add("duplicate names on a line", 1);
        }
    }
    if let Some(merge) = merges.first().filter(|_| rng.chance(0.5)) {
        p.station_names
            .insert(merge.station_a.clone(), "Junction".to_string());
        p.station_names
            .insert(merge.station_b.clone(), "Junction ".to_string());
        f.add("names across a merge", 1);
    }
    // Comments, empty ones too.
    for _ in 0..rng.below(4) {
        if let Some(key) = any_key(&p, &mut rng) {
            let text = if rng.chance(0.3) {
                String::new()
            } else {
                format!("{}\nsecond line", odd_name(&mut rng))
            };
            p.station_comments.insert(key, text);
            f.add("comments", 1);
        }
    }
    // Moving keys around after all that.
    if rng.chance(0.3) {
        let line = ids[rng.below(ids.len())];
        f.add(
            "reversed lines",
            usize::from(p.reverse_line(line, rng.chance(0.5))),
        );
    }
    if rng.chance(0.3) {
        let section = p.add_section(odd_name(&mut rng), "#123456".to_string());
        for line in ids.iter().take(1 + rng.below(3)) {
            p.set_line_section(*line, Some(section));
        }
        f.add("sections", 1);
    }

    // Per-file state.
    if rng.chance(0.4) {
        extras.speleo_link = Some(m::SpeleoLink {
            endpoint: "https://example.invalid".to_string(),
            project_id: "00000000-0000-4000-8000-0000000000c2".to_string(),
            project_name: odd_name(&mut rng),
            base_commit_sha: rng.chance(0.5).then(|| "ab".repeat(20)),
            has_unpushed_edits: rng.chance(0.5),
            last_edit_at: rng
                .chance(0.5)
                .then(|| "2026-01-02T03:04:05.123456789+02:00".to_string()),
            last_verify_at: rng.chance(0.3).then(|| "not a time".to_string()),
            undo_mode: [None, Some(m::UndoMode::Full), Some(m::UndoMode::Disabled)][rng.below(3)],
        });
        extras.speleo_undo_chain = rng.chance(0.3).then(|| "cd".repeat(20));
        extras.speleo_undo_watermark = rng
            .chance(0.3)
            .then(|| "2026-01-01T00:00:00+00:00".to_string());
        f.add("speleodb link", 1);
    }
    if rng.chance(0.5) {
        extras
            .display_settings
            .insert(format!("zz.{}", odd_name(&mut rng)), odd_name(&mut rng));
        extras
            .display_settings
            .insert("hiddenSections".to_string(), "[1,2]".to_string());
        f.add("display settings", 1);
    }
    if rng.chance(0.4) {
        let shots: Vec<(LineId, &m::Shot)> = p
            .lines
            .iter()
            .flat_map(|l| l.shots.iter().map(move |s| (l.id, s)))
            .collect();
        if !shots.is_empty() {
            let (line, shot) = shots[rng.below(shots.len())];
            if !extras
                .resolved_warnings
                .iter()
                .any(|w| w.line_id == line && w.shot_id == shot.id && w.warning_code == "PITCH")
            {
                extras.resolved_warnings.push(ResolvedWarning {
                    line_id: line,
                    shot_id: shot.id,
                    warning_code: "PITCH".to_string(),
                    resolved_at: at,
                    resolved_comment: rng.chance(0.5).then(|| odd_name(&mut rng)),
                    shot_uuid: shot.uuid.clone(),
                });
                f.add("ignores", 1);
            }
        }
    }

    // Persisted history: the reader counts it, the migration drops it.
    let row = |rng: &mut Rng| {
        let command = if rng.chance(0.5) {
            r#"{"type":"update_shot","line_id":0,"shot_id":0,"updates":{}}"#.to_string()
        } else {
            "not json \u{0}".to_string()
        };
        let at = match rng.below(3) {
            0 => None,
            1 => Some("2026-02-03T04:05:06Z".to_string()),
            _ => Some("garbage".to_string()),
        };
        (command, "{}".to_string(), at)
    };
    let (undo_rows, redo_rows) = if rng.chance(0.4) {
        f.add("history rows", 1);
        (
            (0..=rng.below(4)).map(|_| row(&mut rng)).collect(),
            (0..rng.below(3)).map(|_| row(&mut rng)).collect(),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let shape = if rng.chance(0.3) {
        FileShape::Release055
    } else {
        FileShape::Current
    };
    f.add("0.5.5 shape", usize::from(shape == FileShape::Release055));
    let options = WriteOptions {
        shape,
        undo_rows,
        redo_rows,
        ..WriteOptions::default()
    };
    let bytes = write_v18_with(&p, &extras, &options).ok()?;

    // SQL the writer never produces: the gate's degradations (pre-v6 uuids,
    // repeated uuids, pre-v8 ignores) and unknown tables, columns and keys a
    // newer or other build could leave.
    let degrade = base.degrade;
    let unknown = rng.chance(0.4);
    f.add("unknown tables and columns", usize::from(unknown));
    f.add(
        "pre-v6 uuids",
        usize::from(degrade.null_uuid_every.is_some()),
    );
    if degrade.null_uuid_every.is_none()
        && !degrade.duplicate_uuid
        && !degrade.null_ignore_uuids
        && !unknown
    {
        return Some(Hostile { bytes, features: f });
    }
    let bytes = edit_sqlite(&bytes, |conn| {
        if let Some(n) = degrade.null_uuid_every {
            let _ = conn.execute("UPDATE shots SET uuid = NULL WHERE id % ?1 = 0", [n]);
        }
        if degrade.duplicate_uuid {
            let _ = conn.execute_batch(
                "UPDATE shots SET uuid = (
                     SELECT s2.uuid FROM shots s2
                     WHERE s2.section_id != shots.section_id AND s2.uuid IS NOT NULL
                     ORDER BY s2.id LIMIT 1)
                 WHERE id = (SELECT max(id) FROM shots)
                   AND EXISTS (SELECT 1 FROM shots s3
                               WHERE s3.section_id != shots.section_id
                                 AND s3.uuid IS NOT NULL);",
            );
        }
        if degrade.null_ignore_uuids {
            let _ = conn.execute(
                "UPDATE resolved_warnings SET shot_uuid = NULL WHERE id % 2 = 0",
                [],
            );
        }
        if unknown {
            let _ = conn.execute_batch(
                "CREATE TABLE zz_future_feature (id INTEGER PRIMARY KEY, payload TEXT);
                 INSERT INTO zz_future_feature (payload) VALUES ('{\"a\":1}'), (NULL);
                 ALTER TABLE shots ADD COLUMN zz_future REAL;
                 UPDATE shots SET zz_future = 1.5;
                 ALTER TABLE sections ADD COLUMN zz_note TEXT;
                 ALTER TABLE project ADD COLUMN zz_extra TEXT;
                 CREATE INDEX zz_shot_length ON shots (length);
                 INSERT OR REPLACE INTO project_settings (key, value)
                     VALUES ('zz_future_setting', '{\"x\":[1,2]}');
                 INSERT OR REPLACE INTO display_settings (key, value)
                     VALUES ('zz.from.another.build', 'yes');",
            );
        }
    });
    Some(Hostile { bytes, features: f })
}

// ----- the properties ----------------------------------------------------------

/// The file's per-file content as the app maps it (`FileState::from_migrated`):
/// display settings, ignores, the link identity and sync state.
fn extras_of(migrated: &Migrated) -> Extras {
    let e = &migrated.extras;
    let time = |t: Option<&String>| {
        t.and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.with_timezone(&Utc))
    };
    let mut extras = Extras {
        ignored_warnings: e.ignored_warnings.clone(),
        ..Extras::default()
    };
    extras.local.display.clone_from(&e.display_settings);
    if let Some(link) = &e.speleo_link {
        extras.speleodb = Some(SpeleoDbLink {
            endpoint: link.endpoint.clone(),
            project_id: link.project_id.clone(),
            project_name: link.project_name.clone(),
            undo_mode: link.undo_mode,
        });
        extras.local.speleodb = LocalSync {
            base_commit: link.base_commit_sha.clone(),
            unpushed_edits: link.has_unpushed_edits,
            last_edit_at: time(link.last_edit_at.as_ref()),
            last_verify_at: time(link.last_verify_at.as_ref()),
            unwound_from: e.speleo_undo_chain.clone(),
            inherited_through: time(e.speleo_undo_watermark.as_ref()),
            preserved: Vec::new(),
        };
    }
    extras
}

/// Every slot's position as bits: the raw view, then the Computed View.
fn position_bits(p: &Project) -> Vec<(SlotPosition, [u64; 3])> {
    let raw = p.raw_positions();
    let adjusted = p.adjusted_positions_from(&raw, &ClosureAdjustmentConfig::default());
    raw.slots(p)
        .into_iter()
        .chain(adjusted.slots(p))
        .map(|s| (s, [s.x.to_bits(), s.y.to_bits(), s.z.to_bits()]))
        .collect()
}

/// Property 3: the legacy bytes are recognised as legacy everywhere, and the
/// legacy header counts what the file holds.
fn legacy_recognised(bytes: &[u8], legacy: &LegacySurvey, dir: &Path) -> Result<(), Failure> {
    if sniff(bytes) != Format::Legacy {
        return Err(fail(
            "legacy-sniff",
            "a legacy file does not sniff as legacy",
        ));
    }
    if probe_bytes(bytes).format != Format::Legacy {
        return Err(fail(
            "legacy-probe",
            "a legacy file does not probe as legacy",
        ));
    }
    match decode(bytes, &DecodeOptions::default()) {
        Err(DecodeError::Legacy) => {}
        other => {
            return Err(fail(
                "legacy-decode",
                format!("decode of a legacy file: {:?}", other.err()),
            ))
        }
    }
    let path = dir.join("legacy.stalx");
    std::fs::write(&path, bytes).map_err(|e| fail("io", e.to_string()))?;
    let header = legacy_header(&path).map_err(|e| fail("legacy-header", e.to_string()))?;
    let summary = header
        .summary
        .ok_or_else(|| fail("legacy-header", "no summary"))?;
    let lp = &legacy.project;
    let shots: usize = lp.lines.iter().map(|l| l.shots.len()).sum();
    if summary.lines != lp.lines.len() as u64 || summary.shots != shots as u64 {
        return Err(fail(
            "legacy-header",
            format!(
                "header counts {} lines {} shots, the file holds {} and {shots}",
                summary.lines,
                summary.shots,
                lp.lines.len()
            ),
        ));
    }
    Ok(())
}

/// Property 4: the v2 file of a migrated project.
fn v2_round_trip(migrated: &Migrated) -> Result<Vec<u8>, Failure> {
    let extras = extras_of(migrated);
    let bytes = encode(&migrated.project, &extras, Mode::Normal)
        .map_err(|e| fail("v2-encode", e.to_string()))?;
    if sniff(&bytes) != Format::V2 {
        return Err(fail(
            "v2-sniff",
            "the encoder's output does not sniff as v2",
        ));
    }
    let decoded =
        decode(&bytes, &DecodeOptions::default()).map_err(|e| fail("v2-decode", e.to_string()))?;
    if let Some(why) = decoded.read_only() {
        return Err(fail("v2-read-only", why.to_string()));
    }
    if !decoded.project.validate().is_empty() {
        return Err(fail("v2-validate", "the decoded project is invalid"));
    }
    if decoded.project != migrated.project {
        return Err(fail(
            "v2-model",
            "the decoded model differs from the migrated one",
        ));
    }
    let mut ignores: Vec<IgnoredWarning> = extras.ignored_warnings.clone();
    ignores.sort_by(|a, b| (&a.shot, &a.code).cmp(&(&b.shot, &b.code)));
    ignores.dedup_by(|b, a| a.shot == b.shot && a.code == b.code);
    if decoded.extras.ignored_warnings != ignores
        || decoded.extras.local.display != extras.local.display
        || decoded.extras.speleodb != extras.speleodb
    {
        return Err(fail(
            "v2-extras",
            "the per-file state changed in a round trip",
        ));
    }
    let second = encode(&decoded.project, &decoded.extras, Mode::Normal)
        .map_err(|e| fail("v2-encode", e.to_string()))?;
    if second != bytes {
        return Err(fail("v2-fixpoint", "a second encode gives other bytes"));
    }
    let reopened =
        decode(&second, &DecodeOptions::default()).map_err(|e| fail("v2-decode", e.to_string()))?;
    let third = encode(&reopened.project, &reopened.extras, Mode::Normal)
        .map_err(|e| fail("v2-encode", e.to_string()))?;
    if third != second {
        return Err(fail("v2-fixpoint", "open and save is not a byte fixpoint"));
    }
    if position_bits(&decoded.project) != position_bits(&migrated.project) {
        return Err(fail(
            "v2-positions",
            "a slot moved through the v2 round trip",
        ));
    }
    let header = probe_bytes(&bytes);
    let summary = header
        .summary
        .ok_or_else(|| fail("v2-probe", "no summary"))?;
    let p = &migrated.project;
    let shots: usize = p.lines().iter().map(|l| l.shots.len()).sum();
    if !header.complete
        || header.format != Format::V2
        || summary.lines != p.lines().len() as u64
        || summary.stations != p.stations.len() as u64
        || summary.shots != shots as u64
        || summary.name != p.name
    {
        return Err(fail(
            "v2-probe",
            "the probe's summary does not count the survey",
        ));
    }
    Ok(bytes)
}

fn v2_of(p: &Project) -> Result<Vec<u8>, String> {
    encode(p, &Extras::default(), Mode::Normal).map_err(|e| e.to_string())
}

/// `live` stepped onto `expected`: the same v2 bytes once the counters (which
/// may stand higher) are set to the recorded ones.
fn same_v2(live: &Project, expected: &Project) -> Result<(), String> {
    same_state(live, expected)?;
    session_ids_sound(live)?;
    let mut normalized = live.clone();
    normalized.counters = expected.counters;
    if v2_of(&normalized)? != v2_of(expected)? {
        return Err("the v2 bytes differ".to_string());
    }
    Ok(())
}

/// Property 6: a random walk from a migrated project (`legacy_origin`).
/// Returns the steps that changed the project.
fn legacy_origin_walk(start: &Project, seed: u64, steps: usize) -> Result<usize, Failure> {
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<usize, String> {
        let mut walk = Walk::new(seed);
        walk.project = start.clone();
        let mut recorded = Recorded {
            states: vec![walk.project.clone()],
            changes: Vec::new(),
            ops: Vec::new(),
        };
        for _ in 0..steps {
            let before = walk.project.clone();
            if let Some(op) = walk.step() {
                recorded.changes.push(capture(&before, &walk.project));
                recorded.states.push(walk.project.clone());
                recorded.ops.push(op);
            }
        }
        let mut first_error: Option<String> = None;
        for persisted in [false, true] {
            replay(&recorded, persisted, |live, expected, what| {
                if first_error.is_none() {
                    if let Err(e) = same_v2(live, expected) {
                        first_error = Some(format!("persisted {persisted}, {what}: {e}"));
                    }
                }
            });
        }
        if let Some(e) = first_error {
            return Err(e);
        }
        history_rides_the_file(&recorded, seed)?;
        Ok(recorded.changes.len())
    }));
    match outcome {
        Ok(Ok(n)) => Ok(n),
        Ok(Err(e)) => Err(fail("legacy-origin-walk", e)),
        Err(payload) => Err(fail("legacy-origin-walk", panic_text(payload.as_ref()))),
    }
}

/// The walk's entries in the file's history, split between undo and redo the
/// way a session leaves them after undoing the last `k`: the decoded project
/// redoes to the end and undoes to the start, landing on every state.
fn history_rides_the_file(recorded: &Recorded, seed: u64) -> Result<(), String> {
    let n = recorded.changes.len();
    let entries: Vec<RawJson> = recorded
        .changes
        .iter()
        .zip(&recorded.ops)
        .map(|(cs, op)| RawJson::parse(&entry_text(cs, op)))
        .collect::<Result<_, _>>()?;
    let undone = if n == 0 { 0 } else { (seed as usize) % (n + 1) };
    let at = n - undone;
    let mut redo_list: Vec<RawJson> = entries[at..].to_vec();
    redo_list.reverse();
    let extras = Extras {
        history: History {
            undo: entries[..at].to_vec(),
            redo: redo_list,
        },
        ..Extras::default()
    };
    let bytes = encode(&recorded.states[at], &extras, Mode::Normal).map_err(|e| e.to_string())?;
    let decoded = decode(&bytes, &DecodeOptions::default()).map_err(|e| e.to_string())?;
    if encode(&decoded.project, &decoded.extras, Mode::Normal).map_err(|e| e.to_string())? != bytes
    {
        return Err("the file with its history is not a fixpoint".to_string());
    }
    let step = |text: &RawJson| {
        decode_entry(text.as_str(), &EntryLimits::default()).map_err(|e| e.to_string())
    };
    let mut live = decoded.project;
    for (k, text) in decoded.extras.history.redo.iter().rev().enumerate() {
        let entry = step(text)?;
        redo(&mut live, &entry.changes).map_err(|e| format!("file redo {k}: {e}"))?;
        same_v2(&live, &recorded.states[at + k + 1]).map_err(|e| format!("file redo {k}: {e}"))?;
    }
    for k in (0..n).rev() {
        let entry = step(&entries[k])?;
        undo(&mut live, &entry.changes).map_err(|e| format!("file undo {k}: {e}"))?;
        same_v2(&live, &recorded.states[k]).map_err(|e| format!("file undo {k}: {e}"))?;
    }
    Ok(())
}

/// The maximum principle of the Closures Computed View: per axis the
/// least squares is a resistor network (shots weighted `1 / sigma^2`) driven
/// only by the misclosures it distributes: every tied closure's raw gap,
/// every GPS fix's offset from its raw station, and any shot the raw layout
/// does not satisfy (none in a valid model: its shots form a forest). A
/// network with one such source keeps every displacement within that
/// source's size, so by superposition (and the triangle inequality across
/// the axes) no station moves further than the sum of all of them. A
/// degenerate solve breaks this by orders of magnitude (finding C24-F2:
/// 13.8 km and 11,550 km against a few hundred metres of misclosure).
///
/// Why it is a true property of the solve as built (sigma floor, `Tie`
/// closures, held datums and fixes, spurs), not a heuristic: per axis the
/// solve minimises, over station displacements `d` from the raw layout,
/// `sum w_s (d_t - d_u - m_s)^2` over shots (`m_s` the raw layout's misfit
/// of shot `u -> t`) plus `w_f (d_f - o_f)^2` per soft fix, with `d_a - d_b
/// = raw_b - raw_a` for every closure it ties (its ends share one point),
/// `d = o_f` at a hard fix and `d = 0` at a held datum or gauge. Every
/// weight is positive (the floor raises small sigmas; it never changes a
/// sign), so this is a resistor network: shots are resistors with a series
/// source `m_s`, ties are ideal sources of their raw gap, fixes are sources
/// to ground. The minimiser is linear in the sources. With one source alone
/// (a tie of zero gap is a short, a fix of zero offset is ground), a node
/// that is not one of its two ends carries no injected current, so its
/// value is a weighted mean of its neighbours' and the extremes sit at the
/// source's ends; the ground (all held nodes, one potential) carries no net
/// current either (a source returns what it sends), so 0 lies between those
/// extremes and every value is within the source's size. Superposing, `|d|
/// <= sum |source|` per axis, and across the axes `hypot` of the sums is at
/// most the sum of the sources' `hypot`s (the budget below). Spurs are laid
/// out from their measured shots, which is exactly the minimiser there (a
/// leaf carries no current). So a station past the budget is the solve's
/// error, never the model's: farm seed 34022454 (809 m against 37 m; the
/// exact minimiser moves it 10.7 m) was the absolute-coordinate solve's
/// rounding, fixed in `models::survey::lsq`. Seeds 34014784 and 34016414
/// attain the bound (one source moves a station by all of it), so the slack
/// covers only the solve's rounding: `1e-6 m` plus `1e-9` of the largest
/// raw coordinate. The corrected solve agrees with a 100-digit solve of the
/// farm's seeds within 5e-9 m.
fn computed_view_within_its_misclosures(project: &Project) -> Result<(), String> {
    use crate::models::{gps_to_local, DatumRole};
    let raw = project.raw_positions();
    let adjusted = project.adjusted_positions_from(&raw, &ClosureAdjustmentConfig::default());
    let gap = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let mut budget = 0.0_f64;
    for c in &project.closures {
        if let (Some(a), Some(b)) = (raw.xy(c.a), raw.xy(c.b)) {
            budget += gap(a, b);
        }
    }
    for line in project.lines() {
        let reference = project.azimuth_reference_for(line);
        let hand = |s| line.hand_moves.get(&s).copied().unwrap_or([0.0; 3]);
        for (k, shot) in line.shots.iter().enumerate() {
            let (Some(from), Some(to)) = (line.station(k), line.station(k + 1)) else {
                continue;
            };
            let (Some(a), Some(b)) = (raw.xy(from), raw.xy(to)) else {
                continue;
            };
            let v = shot.vector(reference);
            let (ha, hb) = (hand(from), hand(to));
            let observed = [a[0] + v[0] + hb[0] - ha[0], a[1] + v[1] + hb[1] - ha[1]];
            budget += gap(b, observed);
        }
    }
    if let Some(frame) = project.frame_datum().and_then(|f| project.datums.get(f)) {
        for (datum, role) in project.datums.iter().zip(project.datum_roles()) {
            if role != Some(DatumRole::GpsFix) {
                continue;
            }
            let (east, north) = gps_to_local(&datum.gps, &frame.gps);
            if let (true, Some(r)) = (east.is_finite() && north.is_finite(), raw.xy(datum.station))
            {
                budget += gap([east, north], r);
            }
        }
    }
    let scale = raw
        .iter()
        .map(|(_, p)| p[0].abs().max(p[1].abs()))
        .fold(0.0_f64, f64::max);
    let slack = 1e-6 + 1e-9 * scale;
    for (station, [x, y, _]) in adjusted.iter() {
        let Some(r) = raw.xy(station) else {
            continue;
        };
        let moved = gap([x, y], r);
        // NaN on either side fails too.
        let within = matches!(
            moved.partial_cmp(&(budget + slack)),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        );
        if !within {
            return Err(format!(
                "station {station} moves {moved:e} m in the Computed View; its misclosures add up to {budget:e} m"
            ));
        }
    }
    Ok(())
}

/// The corpus mode's subset of `test_support::check`: what must hold for a
/// real file, which may need repairs a settled synthetic file never needs.
fn check_real(legacy: &LegacySurvey, migrated: &Migrated) -> Result<(), Failure> {
    if !migrated.project.validate().is_empty() {
        return Err(fail("validate", "the migrated project is invalid"));
    }
    let v = &migrated.report.verification;
    if !v.mismatches.is_empty() {
        return Err(fail("raw-mismatch", format!("{}", v.mismatches.len())));
    }
    if let super::AdjustedCheck::Compared(a) = &v.adjusted {
        if !a.mismatches.is_empty() || !a.failure_mismatches.is_empty() {
            return Err(fail("adjusted-mismatch", format!("{}", a.mismatches.len())));
        }
    }
    if computed_view_within_its_misclosures(&migrated.project).is_err() {
        return Err(fail(
            "computed-view-bound",
            "a station moved past its misclosures",
        ));
    }
    let again = migrate(legacy, &ClosureAdjustmentConfig::default())
        .map_err(|e| fail("second-migrate", e.class()))?;
    if again.project != migrated.project || again.report != migrated.report {
        return Err(fail(
            "determinism",
            "migrating twice gave different results",
        ));
    }
    Ok(())
}

/// What a seed or file came to.
enum Outcome {
    /// Migrated; every property held.
    Migrated(Tally),
    /// A typed refusal (by class).
    Refused(&'static str, Tally),
    /// The generator produced nothing (the legacy writer refused).
    Unbuilt,
}

/// The frozen open and the migration, never panicking (property 1).
fn open_and_migrate(
    bytes: &[u8],
) -> Result<Result<(LegacySurvey, Migrated), &'static str>, Failure> {
    let opened = catch_unwind(AssertUnwindSafe(|| {
        stalagmight_legacy::open_bytes(bytes, &OpenOptions::default())
    }))
    .map_err(|p| fail("legacy-open-panic", panic_text(p.as_ref())))?;
    let legacy = match opened {
        Ok(legacy) => legacy,
        Err(e) => return Ok(Err(MigrateError::Legacy(e).class())),
    };
    let migrated = catch_unwind(AssertUnwindSafe(|| {
        migrate(&legacy, &ClosureAdjustmentConfig::default())
    }))
    .map_err(|p| fail("migrate-panic", panic_text(p.as_ref())))?;
    match migrated {
        Ok(m) => Ok(Ok((legacy, m))),
        Err(MigrateError::Invalid(v)) => Err(fail(
            "migrate-invalid",
            v.first().map(ToString::to_string).unwrap_or_default(),
        )),
        Err(e) => Ok(Err(e.class())),
    }
}

/// Properties 4 to 6 on one migrated project.
fn transition_properties(
    what: &str,
    seed: u64,
    bytes: &[u8],
    (legacy, migrated): (&LegacySurvey, &Migrated),
    walks: &[(u64, usize)],
    tally: &mut Tally,
) -> Result<(), Failure> {
    v2_round_trip(migrated)?;
    let _ = bytes;
    let wire = catch_unwind(AssertUnwindSafe(|| {
        wire_compare(what.to_string(), seed, legacy, migrated)
    }))
    .map_err(|p| fail("wire-panic", panic_text(p.as_ref())))?;
    if let Some(first) = wire.unexplained.first() {
        return Err(fail(
            "wire-unexplained",
            format!("{} unexplained, first: {first}", wire.unexplained.len()),
        ));
    }
    for (class, n) in &wire.classes {
        tally.add(&format!("wire.{class}"), *n);
    }
    for &(walk_seed, steps) in walks {
        let changed = legacy_origin_walk(&migrated.project, walk_seed, steps)?;
        tally.add("walk steps", changed);
    }
    Ok(())
}

fn run_seed(seed: u64) -> Result<Outcome, Failure> {
    let knobs = knobs_from_env();
    let built = catch_unwind(|| hostile_case(seed, knobs))
        .map_err(|p| fail("generator-panic", panic_text(p.as_ref())))?;
    let Some(hostile) = built else {
        return Ok(Outcome::Unbuilt);
    };
    let mut tally = hostile.features;
    let (legacy, migrated) = match open_and_migrate(&hostile.bytes)? {
        Ok(pair) => pair,
        Err(class) => return Ok(Outcome::Refused(class, tally)),
    };
    check(&legacy, &migrated).map_err(|e| fail("migration-check", e))?;
    computed_view_within_its_misclosures(&migrated.project)
        .map_err(|e| fail("computed-view-bound", e))?;
    let dir = tempfile::tempdir().map_err(|e| fail("io", e.to_string()))?;
    legacy_recognised(&hostile.bytes, &legacy, dir.path())?;
    tally.report(&migrated.report);
    transition_properties(
        &format!("seed {seed}"),
        seed,
        &hostile.bytes,
        (&legacy, &migrated),
        &[(seed, WALK_STEPS)],
        &mut tally,
    )?;
    Ok(Outcome::Migrated(tally))
}

/// A run's totals.
#[derive(Default)]
struct Totals {
    seeds: u64,
    migrated: u64,
    refused: BTreeMap<&'static str, u64>,
    unbuilt: u64,
    failures: Vec<(u64, Failure)>,
    tally: Tally,
}

fn run_window(start: u64, count: u64, threads: usize) -> Totals {
    let next = AtomicU64::new(start);
    let totals = Mutex::new(Totals::default());
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| loop {
                let seed = next.fetch_add(1, Ordering::Relaxed);
                if seed >= start + count {
                    break;
                }
                let outcome = run_seed(seed);
                let mut t = totals
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                t.seeds += 1;
                match outcome {
                    Ok(Outcome::Migrated(tally)) => {
                        t.migrated += 1;
                        t.tally.absorb(&tally);
                    }
                    Ok(Outcome::Refused(class, tally)) => {
                        *t.refused.entry(class).or_default() += 1;
                        t.tally.absorb(&tally);
                    }
                    Ok(Outcome::Unbuilt) => t.unbuilt += 1,
                    Err(failure) => t.failures.push((seed, failure)),
                }
            });
        }
    });
    totals
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn report_and_assert(totals: &Totals, test: &str) {
    let mut failures: Vec<&(u64, Failure)> = totals.failures.iter().collect();
    failures.sort_by_key(|(seed, _)| *seed);
    for (seed, failure) in &failures {
        eprintln!(
            "C24 migration fuzz FAILED seed={seed} class={} replay=\"STALAGMIGHT_FUZZ_SEED_START={seed} \
             STALAGMIGHT_FUZZ_SEEDS=1 cargo test -p stalagmight-persistence --lib {TEST_PATH}::{test} \
             -- --include-ignored\" detail={}",
            failure.class,
            failure.detail.replace('\n', " | ")
        );
    }
    eprintln!(
        "C24 migration fuzz: seeds={} migrated={} refused={:?} unbuilt={} failed={}",
        totals.seeds,
        totals.migrated,
        totals.refused,
        totals.unbuilt,
        totals.failures.len()
    );
    assert!(
        totals.failures.is_empty(),
        "{} seed(s) failed, see the lines above",
        totals.failures.len()
    );
}

#[test]
fn migration_fuzz_window() {
    let (start, count) = window(CI_SEEDS);
    let totals = run_window(start, count, 2);
    report_and_assert(&totals, "migration_fuzz_window");
    if env_u64("STALAGMIGHT_FUZZ_SEEDS").is_some() {
        return;
    }
    // The fixed window must reach the hostile features it is meant to.
    let t = &totals.tally;
    let wanted = [
        "odd lines",
        "empty lines",
        "extreme values",
        "merge chains",
        "cycle merges",
        "datums in one piece",
        "datums on a merged station",
        "entrance overrides",
        "closures between merged stations",
        "hand moves",
        "line offsets",
        "odd names",
        "duplicate names on a line",
        "comments",
        "history rows",
        "unknown tables and columns",
        "0.5.5 shape",
        "speleodb link",
        "report.merges_demoted_cycle",
        // Every merged member's line keeps showing the member's name, by an
        // alias naming it (I-3), so `names_changed` is 0 on every seed
        // (`test_support::check` fails a seed that changes a name) and the
        // window gates the aliases that do it instead.
        "report.aliases_created",
        "report.comments_joined",
        "report.hand_moves_carried",
        "report.history_rows_dropped",
        "walk steps",
    ];
    let missing: Vec<&str> = wanted.into_iter().filter(|k| t.get(k) == 0).collect();
    assert!(
        missing.is_empty(),
        "the CI window never exercised {missing:?}; counts: {:#?}",
        t.counts
    );
    assert!(
        totals.migrated >= count / 2,
        "only {} of {count} seeds migrated",
        totals.migrated
    );
}

/// `cargo test -p stalagmight-persistence --lib migration_fuzz_wide --
/// --ignored --nocapture`: 2,000 seeds (or the env window) on
/// `STALAGMIGHT_FUZZ_THREADS` threads (default 4); prints the coverage.
#[test]
#[ignore = "wide run: 2,000 seeds by default; set STALAGMIGHT_FUZZ_SEED_START / STALAGMIGHT_FUZZ_SEEDS"]
fn migration_fuzz_wide() {
    let (start, count) = window(WIDE_SEEDS);
    let threads = env_u64("STALAGMIGHT_FUZZ_THREADS").map_or(4, |n| n as usize);
    let totals = run_window(start, count, threads);
    for (key, n) in &totals.tally.counts {
        eprintln!("coverage: {key} = {n}");
    }
    report_and_assert(&totals, "migration_fuzz_wide");
}

// ----- the private corpus --------------------------------------------------------

/// A short, stable, non-reversible tag for a file name (the corpus runner's).
fn name_hash(name: &str) -> String {
    const NAMESPACE: uuid::Uuid = uuid::Uuid::from_u128(0x1b0e_4a2c_77d9_4c31_9f05_2e6a_c8d4_5b17);
    uuid::Uuid::new_v5(&NAMESPACE, name.as_bytes())
        .simple()
        .to_string()[..12]
        .to_string()
}

fn stalx_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            stalx_files(&path, out);
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("stalx"))
        {
            out.push(path);
        }
    }
}

/// `STALAGMIGHT_MIGRATION_CORPUS=<dir> cargo test -p stalagmight-persistence
/// --lib migration_fuzz_corpus -- --ignored --nocapture`: every `*.stalx`
/// under `<dir>`: the open and the migration (typed or ok, never a panic),
/// the legacy and v2 recognition, the v2 round trip and fixpoint, the wire
/// classes, and `STALAGMIGHT_FUZZ_WALKS` seeded walks from the migrated
/// project with undo and redo exactness (seeds from
/// `STALAGMIGHT_FUZZ_SEED_START`). `STALAGMIGHT_CORPUS_FILE=<hash>` runs
/// one file. Prints hashes, counts and classes only.
#[test]
#[ignore = "runs on a private corpus: set STALAGMIGHT_MIGRATION_CORPUS"]
fn migration_fuzz_corpus() {
    let Some(dir) = std::env::var_os("STALAGMIGHT_MIGRATION_CORPUS") else {
        eprintln!("STALAGMIGHT_MIGRATION_CORPUS is not set");
        return;
    };
    let dir = PathBuf::from(dir);
    let only = std::env::var("STALAGMIGHT_CORPUS_FILE").ok();
    let walks = env_u64("STALAGMIGHT_FUZZ_WALKS").unwrap_or(CORPUS_WALKS);
    let first_walk = env_u64("STALAGMIGHT_FUZZ_SEED_START").unwrap_or(0);
    let mut files = Vec::new();
    stalx_files(&dir, &mut files);
    // A panic message could quote a file: silence the hook for the run.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let scratch = tempfile::tempdir();
    let mut totals = Totals::default();
    let mut failed_files = 0usize;
    for path in &files {
        let relative = path.strip_prefix(&dir).unwrap_or(path);
        let tag = name_hash(&relative.to_string_lossy());
        if only.as_deref().is_some_and(|h| h != tag) {
            continue;
        }
        totals.seeds += 1;
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("file={tag} status=unreadable");
            continue;
        };
        let mut tally = Tally::default();
        let result = (|| -> Result<Option<&'static str>, Failure> {
            let (legacy, migrated) = match open_and_migrate(&bytes)? {
                Ok(pair) => pair,
                Err(class) => return Ok(Some(class)),
            };
            check_real(&legacy, &migrated)?;
            if let Ok(scratch) = &scratch {
                legacy_recognised(&bytes, &legacy, scratch.path())?;
            }
            tally.report(&migrated.report);
            // Never keep a failure's text: it can quote the file.
            transition_properties(&tag, 0, &bytes, (&legacy, &migrated), &[], &mut tally).map_err(
                |f| Failure {
                    class: f.class,
                    detail: String::new(),
                },
            )?;
            for walk_seed in first_walk..first_walk + walks {
                legacy_origin_walk(&migrated.project, walk_seed, CORPUS_WALK_STEPS).map_err(
                    |f| Failure {
                        class: f.class,
                        detail: format!("walk_seed={walk_seed}"),
                    },
                )?;
                tally.add("walk seeds", 1);
            }
            Ok(None)
        })();
        match result {
            Ok(None) => {
                totals.migrated += 1;
                eprintln!(
                    "file={tag} status=ok walks={} stations_renamed={} merges_demoted_cycle={}",
                    tally.get("walk seeds"),
                    tally.get("report.stations_renamed"),
                    tally.get("report.merges_demoted_cycle")
                );
                totals.tally.absorb(&tally);
            }
            Ok(Some(class)) => {
                *totals.refused.entry(class).or_default() += 1;
                eprintln!("file={tag} status=refused class={class}");
            }
            Err(failure) => {
                failed_files += 1;
                // Only the class and, for a walk, its seed: never the text.
                let seed_start = failure
                    .detail
                    .strip_prefix("walk_seed=")
                    .map_or(String::new(), |s| {
                        format!("STALAGMIGHT_FUZZ_SEED_START={s} STALAGMIGHT_FUZZ_WALKS=1 ")
                    });
                eprintln!(
                    "file={tag} status=FAILED class={} replay=\"STALAGMIGHT_MIGRATION_CORPUS=<dir> \
                     STALAGMIGHT_CORPUS_FILE={tag} {seed_start}cargo test -p stalagmight-persistence \
                     --lib {TEST_PATH}::migration_fuzz_corpus -- --ignored --nocapture\"",
                    failure.class
                );
            }
        }
    }
    std::panic::set_hook(hook);
    eprintln!(
        "corpus: files={} ok={} refused={:?} failed={failed_files}",
        totals.seeds, totals.migrated, totals.refused
    );
    for (key, n) in &totals.tally.counts {
        eprintln!("corpus: {key} = {n}");
    }
    let lost: u64 = totals
        .refused
        .iter()
        .filter(|(class, _)| !class.starts_with("legacy-"))
        .map(|(_, n)| n)
        .sum();
    assert_eq!(failed_files, 0, "files failed a transition property");
    assert_eq!(lost, 0, "files the legacy reader opened failed to migrate");
}

/// A refusal the frozen reader gives must be typed, never a panic: a file
/// that is not SQLite, a truncated SQLite header, a newer schema.
#[test]
fn refusals_of_broken_legacy_bytes_are_typed() {
    let Some(hostile) = hostile_case(3, DEFAULT_KNOBS) else {
        panic!("seed 3 builds");
    };
    let bytes = hostile.bytes;
    let mut newer = edit_sqlite(&bytes, |conn| {
        let _ = conn.execute_batch("PRAGMA user_version = 99;");
    });
    let cases: [(&str, Vec<u8>); 4] = [
        ("empty", Vec::new()),
        ("sqlite magic only", b"SQLite format 3\0".to_vec()),
        ("half a file", bytes[..bytes.len() / 2].to_vec()),
        ("newer schema", std::mem::take(&mut newer)),
    ];
    for (what, input) in cases {
        match open_and_migrate(&input) {
            Ok(Err(class)) => assert!(class.starts_with("legacy-"), "{what}: {class}"),
            Ok(Ok(_)) => panic!("{what}: migrated"),
            Err(f) => panic!("{what}: {} {}", f.class, f.detail),
        }
    }
    // The class of a refusal names no content.
    assert_eq!(
        MigrateError::Legacy(LegacyError::NotSqlite).class(),
        "legacy-not-sqlite"
    );
}

/// One pinned seed of a knob through the whole migration check, plus the
/// Computed View's maximum principle; the classes the seed's Computed View
/// differences fell into.
fn pinned_seed(seed: u64, knobs: Knobs) -> BTreeMap<ExpectedClass, usize> {
    let Some(hostile) = hostile_case(seed, knobs) else {
        panic!("seed {seed} builds");
    };
    let (legacy, migrated) = match open_and_migrate(&hostile.bytes) {
        Ok(Ok(pair)) => pair,
        Ok(Err(class)) => panic!("seed {seed}: refused {class}"),
        Err(f) => panic!("seed {seed}: {} {}", f.class, f.detail),
    };
    if let Err(e) = check(&legacy, &migrated) {
        panic!("seed {seed}: {e}");
    }
    if let Err(e) = computed_view_within_its_misclosures(&migrated.project) {
        panic!("seed {seed}: {e}");
    }
    match &migrated.report.verification.adjusted {
        super::AdjustedCheck::Compared(a) => a
            .expected
            .iter()
            .map(|(class, stats)| (*class, stats.slots))
            .collect(),
        super::AdjustedCheck::NoLegacyPositions => panic!("seed {seed}: not compared"),
    }
}

/// Finding C24-F2 (plan Deviations G9): a shot under about 20 cm within 2
/// degrees of a grid axis (a zero-length "0 0 0" connector at azimuth 0 is
/// the common one) had a huge, infinite or no least-squares weight on one
/// axis (sigma = length x sigma_az, against sin(azimuth) x sigma_length), in
/// 0.6.0 and in the port alike, and both Closures Computed View solves went
/// wrong, each its own way: 0.6.0 moved a station 11,550 km in seed 46, the
/// port 13.8 km in seed 98; in seeds 0 and 19 the two disagreed by 3.7 m and
/// 90 m. The port now floors each axis's sigma at sigma_len x sigma_az
/// (`ShotSigmas`), so its solve stays within the misclosures it distributes,
/// and the migration declares the difference from 0.6.0's degenerate solve
/// (`LegacyWeightsDegenerate`, detected from the legacy shots). Without the
/// floor seed 98's port moves a station 4.7 km against 1.7 km of
/// misclosure; without the class all four seeds report unexplained slots.
#[test]
fn short_shots_on_a_grid_axis_keep_the_computed_view() {
    let knobs = Knobs {
        degenerate_lsq: true,
        ..C24_DRAW
    };
    for seed in [0, 19, 46, 98] {
        let classes = pinned_seed(seed, knobs);
        assert!(
            classes.contains_key(&ExpectedClass::LegacyWeightsDegenerate),
            "seed {seed}: {classes:?}"
        );
    }
}

/// Finding C24-F3 (plan Deviations G9): shots and depth changes of 10 km and
/// more (a unit mistake on import: centimetres read as metres) reach the
/// precision of 0.6.0's Computed View, which the station graph's does not
/// share, in two ways, each declared from the legacy inputs:
/// - 0.6.0 writes its solve back into the shots as slope lengths; on a
///   near-vertical shot with a depth change of kilometres the horizontal it
///   re-derives cannot hold its 1e-6 m line-translation check, which fails,
///   and 0.6.0 shows the piece measured (seeds 878 and 1186; flattening those
///   shots makes the failure go):
///   `LegacyRewriteFailed`;
/// - 0.6.0 placed an all-spur line by its attachment group's position from
///   the normal equations, whose rounding grows with the spread of the shot
///   weights (a 100 km shot's cross-axis weight is 1e-7): the line lands 1e-5
///   to 2e-4 m off where the port lays it out from its measured shots
///   (seeds 1001, 1104 and 1642): `LegacySolvePrecision`, bounded by
///   eps x weight spread x coordinate scale.
#[test]
fn km_scale_shots_keep_the_computed_view() {
    let knobs = Knobs {
        huge_lengths: true,
        ..C24_DRAW
    };
    for (seed, class) in [
        (878, ExpectedClass::LegacyRewriteFailed),
        (1001, ExpectedClass::LegacySolvePrecision),
        (1104, ExpectedClass::LegacySolvePrecision),
        (1186, ExpectedClass::LegacyRewriteFailed),
        (1642, ExpectedClass::LegacySolvePrecision),
    ] {
        let classes = pinned_seed(seed, knobs);
        assert!(classes.contains_key(&class), "seed {seed}: {classes:?}");
    }
}

/// A legacy credit or line date outside 0001-9999 (an import of a garbled
/// date, or a crafted file) is clamped by the migration with the format's
/// own rule, so the migrated model is exactly what its v2 file holds: before
/// the fix the session kept year -262143 and the reopened file said 0001
/// (fuzz seeds 2, 7 and 13).
#[test]
fn out_of_range_legacy_dates_migrate_as_the_file_holds_them() {
    for seed in [2, 7, 13] {
        let Some(hostile) = hostile_case(seed, C24_DRAW) else {
            panic!("seed {seed} builds");
        };
        let (legacy, migrated) = match open_and_migrate(&hostile.bytes) {
            Ok(Ok(pair)) => pair,
            Ok(Err(class)) => panic!("seed {seed}: refused {class}"),
            Err(f) => panic!("seed {seed}: {} {}", f.class, f.detail),
        };
        assert!(
            migrated.report.dates_clamped > 0,
            "seed {seed} clamps a date"
        );
        if let Err(f) = v2_round_trip(&migrated) {
            panic!("seed {seed}: {} {}", f.class, f.detail);
        }
        if let Err(e) = check(&legacy, &migrated) {
            panic!("seed {seed}: {e}");
        }
    }
}

/// `STALAGMIGHT_FUZZ_SEED_START=<s> [STALAGMIGHT_FUZZ_SEEDS=<n>]
/// [STALAGMIGHT_FUZZ_NO_EXTREMES=1] [STALAGMIGHT_FUZZ_HUGE=0]
/// [STALAGMIGHT_FUZZ_DEGENERATE_LSQ=0] cargo test -p stalagmight-persistence
/// --lib migration_fuzz_explain -- --ignored --nocapture`: for each seed of
/// the window, the hostile features, the migration report, a hash of each
/// Computed View, the largest move each makes and the check's verdict
/// (synthetic data), to triage a failing seed.
#[test]
#[ignore = "triage aid: prints one seed's features, report and verdict"]
fn migration_fuzz_explain() {
    let (start, count) = window(1);
    let knobs = knobs_from_env();
    for seed in start..start + count {
        let Some(hostile) = hostile_case(seed, knobs) else {
            eprintln!("seed {seed}: the legacy writer refused the case");
            continue;
        };
        eprintln!("seed {seed} features: {:?}", hostile.features.counts);
        match open_and_migrate(&hostile.bytes) {
            Ok(Ok((legacy, migrated))) => {
                eprintln!("{}", migrated.report.summary());
                // Which side of a Computed View difference moves between
                // runs: the bits of 0.6.0's view and of the port's.
                let hash = |bits: Vec<u64>| {
                    use std::hash::{Hash as _, Hasher as _};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    bits.hash(&mut h);
                    h.finish()
                };
                let old_view = legacy.adjusted_positions.as_ref().map(|v| {
                    hash(
                        v.values()
                            .flat_map(|p| p.iter().map(|x| x.to_bits()))
                            .collect(),
                    )
                });
                let new_view = hash(
                    position_bits(&migrated.project)
                        .into_iter()
                        .flat_map(|(_, b)| b)
                        .collect(),
                );
                eprintln!("seed {seed} views: legacy {old_view:?} port {new_view}");
                // How far each view moves a slot from its raw position.
                let raw = migrated.project.raw_positions();
                let adjusted = migrated
                    .project
                    .adjusted_positions_from(&raw, &ClosureAdjustmentConfig::default());
                let moved = |a: &[SlotPosition], b: &[SlotPosition]| {
                    a.iter()
                        .zip(b)
                        .map(|(p, q)| (p.x - q.x).hypot(p.y - q.y))
                        .fold(0.0_f64, f64::max)
                };
                let port_move = moved(
                    &adjusted.slots(&migrated.project),
                    &raw.slots(&migrated.project),
                );
                let legacy_move = legacy.adjusted_positions.as_ref().map(|v| {
                    v.iter()
                        .filter_map(|(slot, a)| {
                            legacy
                                .raw_positions
                                .get(slot)
                                .map(|r| (a[0] - r[0]).hypot(a[1] - r[1]))
                        })
                        .fold(0.0_f64, f64::max)
                });
                eprintln!(
                    "seed {seed} largest Computed View move: legacy {legacy_move:?} m, port {port_move} m"
                );
                eprintln!("seed {seed} check: {:?}", check(&legacy, &migrated));
            }
            Ok(Err(class)) => eprintln!("seed {seed}: refused {class}"),
            Err(f) => eprintln!("seed {seed}: {} {}", f.class, f.detail),
        }
    }
}

// ----- farm seeds (wave B's draw), plan Deviations G10 -------------------

/// 0.6.0 united every merge before its across-one-shot check (D11); a
/// demoted same-line or cycle-closing merge spanning one shot is a `Tie` the
/// migrated solve skips (and reports). Where that union made 0.6.0 fail the
/// component with two placement datums in one constraint group (farm seeds
/// 34000999, 34013608), the failure is `TieNoLongerFails`; where 0.6.0
/// solved the merge's two stations as one point and the migrated solve holds
/// them apart (a GPS fix or the loop pulls them: 34003170 by 1.9 m, 34007292
/// by 694 m, 34011271 by 1.3 km), `MergeTieSkipped`. Both are detected from
/// the demoted merges and the migrated solve's skips. Without the classes
/// all five seeds report unexplained Computed View slots.
#[test]
fn merges_united_before_the_across_one_shot_check_are_declared() {
    for (seed, class) in [
        (34_000_999, ExpectedClass::TieNoLongerFails),
        (34_013_608, ExpectedClass::TieNoLongerFails),
        (34_003_170, ExpectedClass::MergeTieSkipped),
        (34_007_292, ExpectedClass::MergeTieSkipped),
        (34_011_271, ExpectedClass::MergeTieSkipped),
    ] {
        let classes = pinned_seed(seed, C24_DRAW);
        assert!(classes.contains_key(&class), "seed {seed}: {classes:?}");
    }
}

/// Farm seeds whose wire projection the comparison misread:
/// - 34000253: 0.6.0 failed a component for a demoted merge (D11) whose
///   migrated solve moves nothing beyond the tolerance, so no slot is in a
///   class; the verification's own failure comparison explains it;
/// - 34003575: a range delete lists the closure its sweep removes once the
///   sweep's promotion joins its ends (W6, N2 #738);
/// - 34008560: a gap-close brings an unaligned `Move` closure's ends within
///   1 cm, so the derived alignment (plan 3.6) ties it and the delete removes
///   it across one shot, where 0.6.0 judged it by its flag from the open;
/// - 34004586, 34014759: deleting a line's end shot at a junction, a closure
///   the line kept there that another closure doubles from the junction's
///   replacement member leaves with the line, as 0.6.0 dropped it.
#[test]
fn farm_seeds_project_to_todays_payloads() {
    for seed in [34_000_253, 34_003_575, 34_008_560, 34_004_586, 34_014_759] {
        let Some(hostile) = hostile_case(seed, C24_DRAW) else {
            panic!("seed {seed} builds");
        };
        let Ok(Ok((legacy, migrated))) = open_and_migrate(&hostile.bytes) else {
            panic!("seed {seed} migrates");
        };
        let tally = wire_compare(format!("seed {seed}"), seed, &legacy, &migrated);
        assert!(tally.unexplained.is_empty(), "{:?}", tally.unexplained);
    }
}

/// Fuzz seed 34277750 (the default draw): a GPS fix sits where the survey
/// puts its station, 1.8 km from the frame, so its gap is rounding (2.3e-13
/// m, one ulp there) and its bearing noise: 270 degrees in 0.6.0's payload,
/// 284 on the wire. The comparison leaves out the bearing of a gap under its
/// floor (`wire_equivalence_tests::GAP_BEARING_MIN_REL`, 1.9 mm there);
/// comparing it reports the gap unexplained.
#[test]
fn a_gps_fix_gap_of_rounding_has_no_bearing_to_compare() {
    let seed = 34_277_750;
    let Some(hostile) = hostile_case(seed, DEFAULT_KNOBS) else {
        panic!("seed {seed} builds");
    };
    let Ok(Ok((legacy, migrated))) = open_and_migrate(&hostile.bytes) else {
        panic!("seed {seed} migrates");
    };
    assert!(
        migrated
            .project
            .gps_fix_gaps()
            .iter()
            .any(|(_, gap)| gap.gap_m < 1e-9),
        "seed {seed}: no GPS fix sits on its station any more"
    );
    let tally = wire_compare(format!("seed {seed}"), seed, &legacy, &migrated);
    assert!(tally.unexplained.is_empty(), "{:?}", tally.unexplained);
}

/// Farm seeds 34008291 and 34014759: a line's end shot at a junction is
/// deleted while two closures tie the junction to the same far slot, one via
/// the line and one via the junction's replacement member (the remaining
/// slot with the lowest line id and index). 0.6.0 dropped the line's closure
/// (`closure_rewire_skipped`) and its command's sweep then judged the other
/// closures in list order; the station graph kept it and promoted it first,
/// which in 34014759 put two lines 18.8 m from where 0.6.0 put them. Both
/// deletes run here, 0.6.0's with its command epilogue's sweep, and every
/// slot must sit where 0.6.0 puts it.
#[test]
fn a_doubled_closure_leaves_with_the_slot_as_today() {
    for (seed, line, last) in [(34_008_291, 0, false), (34_014_759, 1, true)] {
        let Some(hostile) = hostile_case(seed, C24_DRAW) else {
            panic!("seed {seed} builds");
        };
        let Ok(Ok((legacy, migrated))) = open_and_migrate(&hostile.bytes) else {
            panic!("seed {seed} migrates");
        };
        let (mut old, mut new) = (legacy.project, migrated.project);
        let Some(shot) = old.get_line(line).and_then(|l| {
            if last {
                l.shots.last()
            } else {
                l.shots.first()
            }
            .map(|s| s.id)
        }) else {
            panic!("seed {seed}: line {line} has a shot");
        };
        if let Err(e) = old.remove_shot_with_rewire_reporting(line, shot) {
            panic!("seed {seed}: 0.6.0's delete: {e}");
        }
        old.convert_orphan_closures_to_merges();
        if let Err(e) = new.delete_shot(line, shot) {
            panic!("seed {seed}: the station graph's delete: {e}");
        }
        let today: BTreeMap<crate::models::StationRef, [f64; 2]> = old
            .compute_station_coordinates()
            .into_iter()
            .map(|s| {
                let slot = crate::models::StationRef::new(
                    s.station_ref.line_id,
                    s.station_ref.station_index,
                );
                (slot, [s.coordinates.x, s.coordinates.y])
            })
            .collect();
        let slots = new.raw_positions().slots(&new);
        assert_eq!(slots.len(), today.len(), "seed {seed}: slot count");
        for slot in slots {
            let Some([x, y]) = today.get(&slot.slot).copied() else {
                panic!("seed {seed}: {:?} is not a slot today", slot.slot);
            };
            assert!(
                (slot.x - x).hypot(slot.y - y) <= 1e-6,
                "seed {seed}: {:?} at ({}, {}), today ({x}, {y})",
                slot.slot,
                slot.x,
                slot.y
            );
        }
    }
}

/// Farm seeds of the default draw (extreme values, 100 km shots and
/// zero-length shots on lines of quality 0.25: weights 1e14 apart) whose
/// Computed View the solve got wrong, and one 0.6.0 got wrong:
/// - 34022454: a part of the survey hanging off one 100 km shot; the exact
///   least squares moves no station there more than 10.7 m (the shot
///   carries nothing), the solve for absolute coordinates moved one 809 m
///   (807 m from the exact solution: rounding of `weight x coordinate`
///   currents and a diagonal leaking to ground, which the weak shot
///   amplified);
/// - 34014784, 34016414 (the farm's finding on the window from 34013171
///   shrinks to 34014784):
///   a station moves by exactly its misclosures (one GPS fix 72 km off, or
///   one closure), so the oracle's bound is attained and the solve's
///   rounding, 9.4e-4 m and 0.022 m, broke it;
/// - 34019254: 0.6.0 placed a spur beyond a merge by its group's solved
///   position, 3.4 mm off where its measured shots put it (GPS fixes hold
///   the 17 m survey 2 km away, under weights 2e11 apart); the port lays it
///   out from its shots and agrees with a 100-digit solve to 1e-13 m:
///   `LegacySolvePrecision`, whose scale now counts the datums' GPS
///   positions.
///
/// The solve now finds corrections to the raw layout from misclosure
/// currents and refines them (`models::survey::lsq`); a 100-digit solve of
/// each seed's least squares agrees with it within 5e-9 m. Injections: the
/// absolute solve fails 34022454, 34014784 and 34016414 on the bound (one
/// pass without the refinement is caught by the synthetic
/// `models::survey::adjust_precision_tests` instead); the precision scale
/// without the datums leaves 34019254's four slots unexplained.
#[test]
fn farm_seeds_keep_the_computed_view_within_its_misclosures() {
    for (seed, class) in [
        (34_022_454, None),
        (34_014_784, None),
        (34_016_414, None),
        (34_019_254, Some(ExpectedClass::LegacySolvePrecision)),
    ] {
        let classes = pinned_seed(seed, DEFAULT_KNOBS);
        if let Some(class) = class {
            assert!(classes.contains_key(&class), "seed {seed}: {classes:?}");
        }
    }
}
