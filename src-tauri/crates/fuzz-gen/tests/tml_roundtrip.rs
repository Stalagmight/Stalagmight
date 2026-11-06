//! Ariane round trips over generated surveys (stable Rust, part of the normal
//! `cargo test --workspace`), on the model + renderer split of `gen::tml`:
//!
//! - `metres_and_feet_import_alike` (unit metamorphic): one survey written in
//!   metres and in feet - each with its "inherit" `DepthIn` marker spelled as
//!   files in that unit carry it - must import as the same shots. Finding HFI-1
//!   broke exactly this: a metric file whose `-1` marker a feet-to-metre
//!   conversion had turned into `-0.3048` (`-0.3` as Ariane writes it) imported
//!   every inheriting start depth as a literal -0.3 m.
//! - `export_round_trip_keeps_every_shot`: a survey imported through the
//!   app's path, exported with `export_tml` and read back keeps every shot,
//!   modulo the losses docs/ariane-export.md documents.
//!
//! Shots are matched by identity (a `UUID` per row, which the importer keeps
//! and the exporter writes back), not by position: which sections the importer
//! cuts a survey into is a separate matter from what each shot measures.
//!
//! Long runs on demand (the fuzz farm uses these, as in tests/fixed_seeds.rs):
//!   STALAGMIGHT_FUZZ_SEEDS=<count per property>   (defaults below)
//!   STALAGMIGHT_FUZZ_SEED_START=<first seed>       (default 0)
//!   STALAGMIGHT_FUZZ_VERBOSE=1                     (one line per seed)
//! A failing seed prints its number; replay it with
//! `STALAGMIGHT_FUZZ_SEED_START=<seed> STALAGMIGHT_FUZZ_SEEDS=1`.

#![expect(clippy::print_stderr)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use stalagmight_export::export::export_tml;
use stalagmight_fuzz_gen::fileset::{with_scratch_dir, FileSet};
use stalagmight_fuzz_gen::gen::tml::{self, DepthIn, RowType, TmlModel, Units};
use stalagmight_fuzz_gen::gen::{Faults, LengthUnit};
use stalagmight_fuzz_gen::source::{seed_bytes, ByteSource};
use stalagmight_models::section::ImportSectionConfig;
use stalagmight_models::{average_azimuth_of, Project, SectionId, Shot, ShotData};
use stalagmight_parsers::parsers::tml::{
    convert_tml_with_options, parse_tml, TmlFile, TmlImportOptions,
};

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn verbose() -> bool {
    std::env::var_os("STALAGMIGHT_FUZZ_VERBOSE").is_some()
}

/// The seeds a property runs: `default` from 0, or the env knobs.
fn seeds(default: u64) -> (u64, u64) {
    (
        env_u64("STALAGMIGHT_FUZZ_SEED_START", 0),
        env_u64("STALAGMIGHT_FUZZ_SEEDS", default),
    )
}

/// The model decisions of seed `seed`, their length varied as
/// `targets::seed_input` varies it so seeds cover small and large surveys.
fn decisions(seed: u64) -> Vec<u8> {
    let len = 64 + ((seed.wrapping_mul(2_654_435_761) >> 7) % 6000) as usize;
    seed_bytes(seed, len)
}

/// The rendering decisions of seed `seed`: one stream for every rendering of
/// it, so two renderings differ in the numbers alone.
fn style(seed: u64) -> Vec<u8> {
    seed_bytes(seed ^ 0x7A11_5EED, 512)
}

/// What an import produced, by the file and by section.
struct Imported {
    file: TmlFile,
    configs: Vec<ImportSectionConfig>,
}

fn read_back(path: &Path, opts: &TmlImportOptions) -> Result<Imported, String> {
    let file = parse_tml(path).map_err(|e| format!("parse: {e:#}"))?;
    let (configs, _, _) =
        convert_tml_with_options(&file, opts).map_err(|e| format!("convert: {e:#}"))?;
    Ok(Imported { file, configs })
}

/// Parse and convert `files` as the import dialog does.
fn import(files: &FileSet, opts: &TmlImportOptions) -> Result<Imported, String> {
    with_scratch_dir(|dir| -> Result<Imported, String> {
        let path = files
            .materialize(dir)
            .map_err(|e| format!("stage: {e:#}"))?;
        read_back(&path, opts)
    })
    .map_err(|e| format!("scratch dir: {e:#}"))?
}

/// A shot reduced to what the properties compare.
#[derive(Debug, Clone, PartialEq)]
struct Measured {
    length: f64,
    depth_in: f64,
    depth_out: f64,
    /// Both forward readings.
    azimuth: (f64, f64),
    lrud: [Option<f64>; 4],
    /// Absent and level are one value: Ariane's wire has no null pitch.
    pitch: f64,
    is_virtual: bool,
    comment: Option<String>,
}

impl Measured {
    fn of(shot: &ShotData) -> Self {
        Self {
            length: shot.length,
            depth_in: shot.depth_in,
            depth_out: shot.depth_out,
            azimuth: (shot.azimuth_in, shot.azimuth_out),
            lrud: [shot.left, shot.right, shot.up, shot.down],
            pitch: shot.pitch_in.unwrap_or(0.0),
            is_virtual: shot.kind.is_virtual(),
            comment: shot.comment.clone(),
        }
    }
}

const LRUD_NAMES: [&str; 4] = ["left", "right", "up", "down"];

fn circular_gap(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// How a shot's depths must come back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Depths {
    /// Both absolute depths, and the tape.
    Absolute,
    /// [#576 TML-DEPTH-D1] A shot the Ariane export may have refitted: one
    /// touching a junction whose depth readings disagree (exported at their
    /// mean), one starting away from where the file's chain puts it, or broken
    /// data (a drop longer than its tape, exported vertical with its drop as
    /// its length, TML-DEPTH-Q2-D1). Depth readings are true, so its END depth
    /// still comes back exactly unless `end_may_move` (its end station is such
    /// a junction), and its horizontal run - the plan - always comes back; its
    /// start and its tape follow from the depth change the file now states
    /// (docs/ariane-export.md, "Junction depths").
    Refitted { end_may_move: bool },
}

/// A shot's horizontal run, as the model derives it (`ShotData::horizontal_length`).
fn horizontal_run(length: f64, change: f64) -> f64 {
    (length * length - change * change).max(0.0).sqrt()
}

/// Each way `got` differs from `want`: lengths and depths by more than `tol`
/// metres (plus a part per billion of the magnitude), angles by more than
/// 1e-5 degrees, anything else at all.
fn differences(want: &Measured, got: &Measured, tol: f64, depths: Depths) -> Vec<String> {
    let close = |a: f64, b: f64| (a - b).abs() <= tol + 1e-9 * a.abs().max(b.abs());
    let mut out = Vec::new();
    let mut pairs = Vec::new();
    match depths {
        Depths::Absolute => {
            pairs.push(("length", want.length, got.length));
            pairs.push(("depth_in", want.depth_in, got.depth_in));
            pairs.push(("depth_out", want.depth_out, got.depth_out));
        }
        Depths::Refitted { end_may_move } => {
            if !end_may_move {
                pairs.push(("depth_out", want.depth_out, got.depth_out));
            }
            // The run as the file states it: six decimals on the tape and on
            // both depths, and the tape rounded up by up to two micrometres
            // more, so a steep shot's run is held only to what those can move
            // it by (a vertical one by about the square root of that).
            let run = horizontal_run(want.length, want.depth_out - want.depth_in);
            let back = horizontal_run(got.length, got.depth_out - got.depth_in);
            let slack = (got.length.abs() + (got.depth_out - got.depth_in).abs()) * 8e-6;
            let allowed = (run * run + slack).sqrt() - run + tol;
            if (back - run).abs() > allowed {
                out.push(format!(
                    "horizontal run {run} vs {back} (allowed {allowed})"
                ));
            }
        }
    }
    for (name, a, b) in pairs {
        if !close(a, b) {
            out.push(format!("{name} {a} vs {b}"));
        }
    }
    for (name, a, b) in [
        ("azimuth_in", want.azimuth.0, got.azimuth.0),
        ("azimuth_out", want.azimuth.1, got.azimuth.1),
        ("pitch", want.pitch, got.pitch),
    ] {
        if circular_gap(a, b) > 1e-5 {
            out.push(format!("{name} {a} vs {b}"));
        }
    }
    for ((name, a), b) in LRUD_NAMES.iter().zip(&want.lrud).zip(&got.lrud) {
        let same = match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => close(*a, *b),
            _ => false,
        };
        if !same {
            out.push(format!("{name} {a:?} vs {b:?}"));
        }
    }
    if want.is_virtual != got.is_virtual {
        out.push(format!("virtual {} vs {}", want.is_virtual, got.is_virtual));
    }
    if want.comment != got.comment {
        out.push(format!("comment {:?} vs {:?}", want.comment, got.comment));
    }
    out
}

/// Every imported shot by its identity: (section, index in it, measurements).
fn by_uuid(
    configs: &[ImportSectionConfig],
) -> Result<HashMap<String, (usize, usize, Measured)>, String> {
    let mut out = HashMap::new();
    for (si, config) in configs.iter().enumerate() {
        for (i, shot) in config.shots.iter().enumerate() {
            let uuid = shot
                .uuid
                .clone()
                .ok_or_else(|| format!("section {si} shot {i} has no uuid"))?;
            if out
                .insert(uuid.clone(), (si, i, Measured::of(shot)))
                .is_some()
            {
                return Err(format!("shot {uuid} imported twice"));
            }
        }
    }
    Ok(out)
}

/// The sections an import cut, as names and shot identities.
fn partition(configs: &[ImportSectionConfig]) -> Vec<(Option<String>, Vec<Option<String>>)> {
    configs
        .iter()
        .map(|c| {
            let shots = c.shots.iter().map(|s| s.uuid.clone()).collect();
            (c.name.clone(), shots)
        })
        .collect()
}

/// Up to the first `max` of `items`, one per line, and how many were left out.
fn listing(items: &[String], max: usize) -> String {
    let mut out = String::new();
    for item in items.iter().take(max) {
        let _ = write!(out, "\n    {item}");
    }
    if items.len() > max {
        let _ = write!(out, "\n    ... and {} more", items.len() - max);
    }
    out
}

// ============================================================================
// (a) Metres and feet import alike
// ============================================================================

/// [#576 HFI-1] -1 ft written in metres at the precisions a converter keeps
/// (docs/ariane-import.md, "A 'same as previous' marker scaled from feet").
const SCALED_MARKERS: [f64; 4] = [-0.3, -0.304, -0.305, -0.3048];

/// Two decimals of a foot (Ariane writes two) are off by up to 0.005 ft =
/// 1.524 mm; the model's lengths are whole centimetres.
const UNIT_TOLERANCE_M: f64 = 0.002;

/// Make a clean model one that metres and feet both carry: every spelled-out
/// start depth to the centimetre, and none within 2 cm of a value the reader
/// takes for the marker in one unit but not the other (-1 m, -1 ft = -0.3048
/// m, the scaled spellings). The reader treats those differently per unit BY
/// DESIGN (the marker is a spelling, docs/ariane-import.md:277-279), so they
/// are not a fact this property can test.
fn unit_stable(model: &mut TmlModel) {
    for row in &mut model.rows {
        if let DepthIn::Value(v) = &mut row.depth_in {
            *v = (*v * 100.0).round() / 100.0;
            let near_a_marker = [-1.0, -0.3048]
                .iter()
                .chain(&SCALED_MARKERS)
                .any(|m| (*v - m).abs() < 0.02);
            if near_a_marker {
                *v += 0.05;
            }
        }
    }
}

/// Whether well over half of the rows the importer puts in sections (every
/// row but START) inherit their start depth: the reader takes a scaled
/// spelling for the marker only when MORE THAN HALF of a file's shots carry
/// it (docs/ariane-import.md:279), so only such a survey can carry it.
fn marker_majority(model: &TmlModel) -> bool {
    let sectioned = model.rows.iter().filter(|r| r.row_type != RowType::Start);
    let (rows, inherit) = sectioned.fold((0usize, 0usize), |(n, i), r| {
        (n + 1, i + usize::from(r.depth_in == DepthIn::Inherit))
    });
    inherit * 20 > rows * 11
}

struct UnitCase {
    scaled: Option<f64>,
    shots: usize,
    partition_differs: bool,
}

fn unit_case(seed: u64) -> Result<UnitCase, String> {
    let bytes = decisions(seed);
    let mut src = ByteSource::new(&bytes);
    let mut model = tml::model(&mut src, Faults::none());
    unit_stable(&mut model);
    for (i, row) in model.rows.iter_mut().enumerate() {
        row.uuid = Some(format!("fz-{i}"));
    }
    let zipped = src.bool();
    let opts = TmlImportOptions {
        include_excluded: src.bool(),
        ..TmlImportOptions::default()
    };
    // The metric file carries the HFI-1 spelling whenever it can.
    let scaled =
        (marker_majority(&model) && !src.chance(32)).then(|| src.pick_or(&SCALED_MARKERS, -0.3));
    let metric_units = scaled.map_or_else(Units::metres, Units::metres_with_marker);
    let style = style(seed);
    let metric = tml::render(
        &model,
        &metric_units,
        zipped,
        Faults::none(),
        &mut ByteSource::new(&style),
    );
    let feet = tml::render(
        &model,
        &Units::feet(),
        zipped,
        Faults::none(),
        &mut ByteSource::new(&style),
    );
    let a = import(&metric, &opts).map_err(|e| format!("the metric file: {e}"))?;
    let b = import(&feet, &opts).map_err(|e| format!("the feet file: {e}"))?;
    let marker = scaled.map_or_else(|| "-1".to_string(), |m| format!("{m} (scaled)"));
    let context = format!("metric marker {marker}, metric vs feet");

    let mut diffs = Vec::new();
    let (ea, eb) = (
        a.file.first_start_absolute_elevation,
        b.file.first_start_absolute_elevation,
    );
    if (ea - eb).abs() > UNIT_TOLERANCE_M + 1e-9 * ea.abs() {
        diffs.push(format!("entrance elevation {ea} vs {eb}"));
    }
    let gps = |f: &TmlFile| f.geo_coding.map(|g| (g.latitude, g.longitude));
    if gps(&a.file) != gps(&b.file) {
        diffs.push(format!("GPS {:?} vs {:?}", gps(&a.file), gps(&b.file)));
    }
    let shots_a = by_uuid(&a.configs)?;
    let shots_b = by_uuid(&b.configs)?;
    if shots_a.len() != shots_b.len() {
        diffs.push(format!("{} shots vs {}", shots_a.len(), shots_b.len()));
    }
    let mut ids: Vec<&String> = shots_a.keys().collect();
    ids.sort_unstable();
    for uuid in ids {
        let Some((si, i, want)) = shots_a.get(uuid) else {
            continue;
        };
        let name = a.configs.get(*si).and_then(|c| c.name.clone());
        match shots_b.get(uuid) {
            None => diffs.push(format!("{uuid} ({name:?} shot {i}) missing in feet")),
            Some((_, _, got)) => {
                let d = differences(want, got, UNIT_TOLERANCE_M, Depths::Absolute);
                if !d.is_empty() {
                    diffs.push(format!("{uuid} ({name:?} shot {i}): {}", d.join(", ")));
                }
            }
        }
    }
    if !diffs.is_empty() {
        return Err(format!("{context}:{}", listing(&diffs, 6)));
    }
    Ok(UnitCase {
        scaled,
        shots: shots_a.len(),
        partition_differs: partition(&a.configs) != partition(&b.configs),
    })
}

/// Farm seeds that once failed, run on every run as well as the window:
/// 363281 (#576 W27) cut a section at a junction in feet but not in metres,
/// so an excluded leg was dropped mid-section in one import and as a lead in
/// the other, and the shot after it took two different start depths.
const UNIT_REGRESSION_SEEDS: [u64; 1] = [363_281];

#[test]
fn metres_and_feet_import_alike() {
    let (start, count) = seeds(64);
    let mut failures = Vec::new();
    let mut scaled = 0u64;
    let mut cut_differently = 0u64;
    for seed in (start..start.saturating_add(count)).chain(UNIT_REGRESSION_SEEDS) {
        match unit_case(seed) {
            Ok(case) => {
                scaled += u64::from(case.scaled.is_some());
                cut_differently += u64::from(case.partition_differs);
                if verbose() {
                    eprintln!(
                        "tml unit seed {seed}: shots={} scaled_marker={:?} sections_differ={}",
                        case.shots, case.scaled, case.partition_differs
                    );
                }
            }
            Err(e) => failures.push(format!("seed {seed}: {e}")),
        }
    }
    eprintln!(
        "tml unit metamorphic: {count} seeds from {start}, {scaled} with the marker scaled from \
         feet, {cut_differently} sectioned differently, {} failed",
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "metres and feet imported differently on {} seed(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
    // The fixed set must keep exercising the HFI-1 spelling.
    assert!(
        count < 16 || scaled > 0,
        "no seed wrote the scaled marker: the property no longer covers HFI-1"
    );
}

// ============================================================================
// (b) Import, export, re-import
// ============================================================================

/// The export writes six decimals (`export::tml::format_f64`); a genuine -1.0
/// start depth is nudged 1e-6 off the marker (docs/ariane-export.md:99).
const EXPORT_TOLERANCE_M: f64 = 1e-5;

/// A comment as the text format can carry it: characters XML 1.0 cannot hold
/// become U+FFFD (docs/ariane-export.md:357-365) and whitespace at either end
/// of the field is trimmed on read (docs/ariane-import.md:384-393).
fn as_written(comment: Option<&str>) -> Option<String> {
    let replaced: String = comment?
        .chars()
        .map(|c| {
            let legal = matches!(c,
                '\u{9}' | '\u{A}' | '\u{D}'
                | '\u{20}'..='\u{D7FF}'
                | '\u{E000}'..='\u{FFFD}'
                | '\u{10000}'..='\u{10FFFF}');
            if legal {
                c
            } else {
                '\u{FFFD}'
            }
        })
        .collect();
    let trimmed = replaced.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// What the export is documented to hand back for `shot`:
/// - ONE azimuth, the circular mean of the two readings, on both
///   (docs/ariane-export.md:96, the `Azimut` row);
/// - six decimals, so an LRUD value that rounds to zero comes back absent -
///   the one wire `0.0` an unmeasured dimension also is (docs/ariane-export.md
///   :213-233, "The LRUD zero/null collision");
/// - an absent and a level pitch as the one wire `0.0`, read back as either by
///   the implied-pitch rule (docs/ariane-import.md:348-382), so the pitch is
///   compared with absent as 0 ([`Measured::pitch`]);
/// - Jump and Reference both as VIRTUAL (docs/ariane-export.md:103), so only
///   virtual-or-not is compared.
fn expected_after_export(shot: &Shot) -> Measured {
    let azimuth = average_azimuth_of(shot.azimuth_in, shot.azimuth_out);
    let lrud = [shot.left, shot.right, shot.up, shot.down].map(|v| v.filter(|x| x.abs() >= 0.5e-6));
    Measured {
        length: shot.length,
        depth_in: shot.depth_in,
        depth_out: shot.depth_out,
        azimuth: (azimuth, azimuth),
        lrud,
        pitch: shot.pitch_in.unwrap_or(0.0),
        is_virtual: shot.kind.is_virtual(),
        comment: as_written(shot.comment.as_deref()),
    }
}

/// Make every shot start where the station it hangs off ends (inheriting its
/// start depth), drop less than its tape by at least a centimetre, and be
/// included: a survey Ariane's chain holds exactly, so no junction disagrees,
/// the export has nothing to refit and the absolute depths must come back. Rows are minted
/// parent-first, so in id order every parent is settled before its children.
fn make_continuous(model: &mut TmlModel) {
    let mut order: Vec<usize> = (0..model.rows.len()).collect();
    order.sort_by_key(|&i| model.rows.get(i).map_or(0, |r| r.id));
    let mut depth_of: HashMap<i64, f64> = HashMap::new();
    for i in order {
        let Some(row) = model.rows.get_mut(i) else {
            continue;
        };
        row.excluded = false;
        match row.row_type {
            RowType::Start => {
                depth_of.insert(row.id, row.depth);
            }
            RowType::Closure => {}
            RowType::Real | RowType::Virtual => {
                // A parent the file does not have reads as depth 0
                // (`TmlSection::start_depth`).
                let start = depth_of.get(&row.from_id).copied().unwrap_or(0.0);
                // In whole centimetres, a centimetre inside the tape: a feet
                // rendering's rounding must not push the drop past it.
                let tape_cm = ((row.length * 100.0).round() as i64 - 1).max(0);
                let drop_cm =
                    (((row.depth - start) * 100.0).round() as i64).clamp(-tape_cm, tape_cm);
                row.depth = start + drop_cm as f64 / 100.0;
                row.depth_in = DepthIn::Inherit;
                depth_of.insert(row.id, row.depth);
            }
        }
    }
}

/// [#576 TML-DEPTH-D1] How each shot of `project` must come back from the
/// Ariane export, by (section id, shot index): [`Depths::Absolute`] for one the
/// export leaves exactly as measured, [`Depths::Refitted`] for one it may
/// refit. Worked out from the project alone and on the safe side:
/// - a merged station whose readings (every arriving and departing depth of
///   every station merged into it) disagree is a junction that may be
///   exported at their mean;
/// - a shot dropping further than its tape is broken data and may be
///   exported vertical, its drop as its length; both its depths, and so the
///   next shot, stay as measured (TML-DEPTH-Q2-D1);
/// - a shot that does not start where the one before it ended (a second
///   reading inside its section) may be refitted.
///
/// Everything else - every station of an ordinary chain - must come back
/// exactly: depths, tape and all.
fn expected_depths(project: &Project) -> HashMap<(SectionId, usize), Depths> {
    type Station = (SectionId, u32);
    let mut group: HashMap<Station, Station> = HashMap::new();
    fn root(group: &mut HashMap<Station, Station>, s: Station) -> Station {
        let mut r = s;
        while let Some(&up) = group.get(&r) {
            if up == r {
                break;
            }
            r = up;
        }
        group.insert(s, r);
        r
    }
    for merge in &project.station_merges {
        let (Some(a), Some(b)) = (
            project.resolve_key(&merge.station_a),
            project.resolve_key(&merge.station_b),
        ) else {
            continue;
        };
        let ra = root(&mut group, (a.section_id, a.station_index));
        let rb = root(&mut group, (b.section_id, b.station_index));
        group.insert(ra, rb);
        group.entry(rb).or_insert(rb);
    }
    let broken = |shot: &Shot| (shot.depth_out - shot.depth_in).abs() > shot.length + 1e-9;
    // Per merge group: (lowest reading, highest reading).
    let mut readings: HashMap<Station, (f64, f64)> = HashMap::new();
    let members: Vec<Station> = group.keys().copied().collect();
    for station in members {
        let Some(section) = project.sections.iter().find(|s| s.id == station.0) else {
            continue;
        };
        let i = station.1 as usize;
        let arriving = i.checked_sub(1).and_then(|k| section.shots.get(k));
        let departing = section.shots.get(i);
        let key = root(&mut group, station);
        let entry = readings
            .entry(key)
            .or_insert((f64::INFINITY, f64::NEG_INFINITY));
        for depth in [arriving.map(|s| s.depth_out), departing.map(|s| s.depth_in)]
            .into_iter()
            .flatten()
        {
            entry.0 = entry.0.min(depth);
            entry.1 = entry.1.max(depth);
        }
    }
    // Whether a station is a merged one whose readings disagree.
    let mut disagrees = |station: Station| -> bool {
        if !group.contains_key(&station) {
            return false;
        }
        let key = root(&mut group, station);
        let (low, high) = readings.get(&key).copied().unwrap_or((0.0, 0.0));
        high - low > 1e-9
    };
    let mut out = HashMap::new();
    for section in &project.sections {
        for (i, shot) in section.shots.iter().enumerate() {
            let from_disagrees = disagrees((section.id, i as u32));
            let to_disagrees = disagrees((section.id, i as u32 + 1));
            let follows_chain = match i.checked_sub(1).and_then(|k| section.shots.get(k)) {
                Some(before) => (before.depth_out - shot.depth_in).abs() < 1e-9,
                None => true,
            };
            let plain = !from_disagrees && !to_disagrees && !broken(shot) && follows_chain;
            let depths = if plain {
                Depths::Absolute
            } else {
                Depths::Refitted {
                    end_may_move: to_disagrees,
                }
            };
            out.insert((section.id, i), depths);
        }
    }
    out
}

/// One checked round trip.
struct RoundTrip {
    shots: usize,
    /// Shots whose end depth the export moved (see [`Depths::Refitted`]).
    moved: usize,
    continuous: bool,
}

/// `Ok(None)` when the app refused the generated file (not this property's
/// business: the fuzz targets cover refusals).
fn round_trip_case(seed: u64) -> Result<Option<RoundTrip>, String> {
    let bytes = decisions(seed);
    let mut src = ByteSource::new(&bytes);
    let mut model = tml::model(&mut src, Faults::none());
    // A zero-length shot fails the whole import by specification
    // (docs/SPEC.md:2593, "Zero length shot | Error"): not this property's
    // subject, and it would only cost the seed.
    for row in &mut model.rows {
        if matches!(row.row_type, RowType::Real | RowType::Virtual) && row.length.abs() < 0.005 {
            row.length = 0.01;
        }
    }
    let continuous = src.bool();
    if continuous {
        make_continuous(&mut model);
    }
    // Unique identities: the export re-mints a repeated one (a documented
    // repair, docs/ariane-export.md "Shot UUIDs"), and the comparison keys
    // on them. A row without one gets a fresh mint at import.
    for (i, row) in model.rows.iter_mut().enumerate() {
        if row.uuid.is_some() {
            row.uuid = Some(format!("00000000-0000-4000-8000-{i:012}"));
        }
    }
    let units = if src.bool() {
        Units::feet()
    } else {
        Units::metres()
    };
    let zipped = src.bool();
    let opts = TmlImportOptions {
        include_excluded: src.bool(),
        ..TmlImportOptions::default()
    };
    let files = tml::render(
        &model,
        &units,
        zipped,
        Faults::none(),
        &mut ByteSource::new(&style(seed)),
    );
    let project = match tml::import_project(&files, &opts) {
        Ok(Ok(project)) => project,
        Ok(Err(reason)) => {
            if verbose() {
                eprintln!("tml round trip seed {seed}: refused: {reason}");
            }
            return Ok(None);
        }
        Err(violation) => return Err(format!("harness: {violation}")),
    };
    // Out through the other container than the survey came in by.
    let ext = if zipped { "tmlu" } else { "tml" };
    let back = with_scratch_dir(|dir| -> Result<Imported, String> {
        let path = dir.join(format!("roundtrip.{ext}"));
        export_tml(&project, &path)
            .map_err(|e| format!("export refused a project the import built: {e:#}"))?;
        let all = TmlImportOptions {
            include_excluded: true,
            ..TmlImportOptions::default()
        };
        read_back(&path, &all).map_err(|e| format!("re-import of the export: {e}"))
    })
    .map_err(|e| format!("scratch dir: {e:#}"))??;

    let got = by_uuid(&back.configs)?;
    let refits = (!continuous).then(|| expected_depths(&project));
    let mut diffs = Vec::new();
    let (mut shots, mut moved) = (0usize, 0usize);
    for (si, section) in project.sections.iter().enumerate() {
        for (i, shot) in section.shots.iter().enumerate() {
            shots += 1;
            let at = format!("{:?} shot {i}", section.name);
            let Some(uuid) = shot.uuid.as_deref() else {
                diffs.push(format!("{at} (section {si}) has no uuid in the project"));
                continue;
            };
            let Some((_, _, back)) = got.get(uuid) else {
                diffs.push(format!("{at} ({uuid}) did not come back"));
                continue;
            };
            let mut back = back.clone();
            back.comment = as_written(back.comment.as_deref());
            if (back.depth_out - shot.depth_out).abs() > EXPORT_TOLERANCE_M {
                moved += 1;
            }
            let depths = refits
                .as_ref()
                .and_then(|r| r.get(&(section.id, i)).copied())
                .unwrap_or(Depths::Absolute);
            let d = differences(
                &expected_after_export(shot),
                &back,
                EXPORT_TOLERANCE_M,
                depths,
            );
            if !d.is_empty() {
                diffs.push(format!("{at} ({uuid}): {}", d.join(", ")));
            }
        }
    }
    if got.len() != shots {
        diffs.push(format!(
            "the re-import holds {} shots, the project {shots}",
            got.len()
        ));
    }
    if diffs.is_empty() {
        Ok(Some(RoundTrip {
            shots,
            moved,
            continuous,
        }))
    } else {
        let survey = if continuous { "continuous" } else { "free" };
        Err(format!(
            "{survey} survey {} {ext}:{}",
            units_name(&units),
            listing(&diffs, 6)
        ))
    }
}

fn units_name(units: &Units) -> &'static str {
    match units.scale {
        LengthUnit::Meters => "imported from metres, exported as",
        LengthUnit::Feet => "imported from feet, exported as",
    }
}

#[test]
fn export_round_trip_keeps_every_shot() {
    let (start, count) = seeds(32);
    let mut failures = Vec::new();
    let (mut checked, mut refused, mut moved) = (0u64, 0u64, 0usize);
    for seed in start..start.saturating_add(count) {
        match round_trip_case(seed) {
            Ok(Some(case)) => {
                checked += 1;
                moved += case.moved;
                if verbose() {
                    eprintln!(
                        "tml round trip seed {seed}: {} shots came back, continuous={}, \
                         end depth moved by the export={}",
                        case.shots, case.continuous, case.moved
                    );
                }
            }
            Ok(None) => refused += 1,
            Err(e) => failures.push(format!("seed {seed}: {e}")),
        }
    }
    eprintln!(
        "tml export round trip: {count} seeds from {start}, {checked} checked, {refused} refused \
         at import, {moved} shots' end depth moved by the export, {} failed",
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "the export round trip lost or changed shots on {} seed(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(
        count < 8 || checked * 2 >= count,
        "most seeds must reach the export: {refused} of {count} were refused at import"
    );
}
