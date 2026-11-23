//! A seeded generator of valid station-graph surveys and their extras, built
//! through the M0 construction API. Fully synthetic: every name, number,
//! uuid and coordinate comes from the seed.
//!
//! Covers junctions (at line starts and mid-chain), several components,
//! datums with corrections, `via` hints and GPS fixes, placements, closures of
//! every strategy (Tie included), hand moves, Sections, credits (shared,
//! overridden, absent and empty), aliases, external ids, unicode and escape-
//! heavy text, `-0.0`, subnormals and extreme magnitudes.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};

use crate::models::survey::{
    ArchivedStation, ClosureStrategy, LineMeta, Project, StartAt, ZeroDatum, MAX_COORDINATE_M,
    MAX_MEASUREMENT_M,
};
use crate::models::{
    AzimuthReference, CorrectionSource, GpsCoordinates, NorthReference, Person, Section, ShotData,
    ShotKind, StationId, SurveyCredit, SurveyDirection, UndoMode,
};

use super::types::{Extras, History, IgnoredWarning, LocalState, LocalSync, RawJson, SpeleoDbLink};
use super::{decode, encode, DecodeOptions, Mode};

/// xorshift64*, so every run builds the same survey.
pub(super) struct Rng(pub(super) u64);

impl Rng {
    pub(super) fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub(super) fn below(&mut self, n: usize) -> usize {
        (self.next() % (n.max(1) as u64)) as usize
    }

    pub(super) fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub(super) fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    pub(super) fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    pub(super) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// A finite double from anywhere in the range: ordinary values most of
    /// the time, then the awkward ones.
    pub(super) fn awkward(&mut self, lo: f64, hi: f64) -> f64 {
        match self.below(20) {
            0 => -0.0,
            1 => 0.0,
            2 => f64::from_bits(self.next() % (1 << 52)), // subnormal
            3 => f64::MIN_POSITIVE,
            4 => 1e300 * self.range(-1.0, 1.0),
            5 => 1e-300 * self.range(-1.0, 1.0),
            6 => {
                // Any finite bit pattern.
                loop {
                    let v = f64::from_bits(self.next());
                    if v.is_finite() {
                        return v;
                    }
                }
            }
            7 => 0.1 + 0.2,
            _ => self.range(lo, hi),
        }
    }

    /// [`Self::awkward`] for a distance or coordinate the survey bounds
    /// (`MAX_MEASUREMENT_M`, `MAX_COORDINATE_M`; review finding
    /// B-derived-solver-3): a draw past `bound` becomes the bound itself, with
    /// its sign, so the edge is drawn too. Angles, the temperature and the
    /// thresholds keep the whole finite range.
    pub(super) fn awkward_within(&mut self, lo: f64, hi: f64, bound: f64) -> f64 {
        let v = self.awkward(lo, hi);
        if v.abs() > bound {
            bound.copysign(v)
        } else {
            v
        }
    }
}

/// How big and how odd a generated survey is.
#[derive(Debug, Clone, Copy)]
pub(super) struct Shape {
    pub(super) lines: usize,
    pub(super) max_shots: usize,
    /// Awkward numbers, odd text, every optional field.
    pub(super) rich: bool,
}

/// The `requires` member every file this build writes (RS 8.1), as text: a
/// test adds a feature by replacing it with a longer list.
pub(super) const REQUIRES: &str = "\"requires\":[\"archived-stations\"]";

pub(super) const SMALL: Shape = Shape {
    lines: 6,
    max_shots: 5,
    rich: true,
};

pub(super) const MEDIUM: Shape = Shape {
    lines: 40,
    max_shots: 25,
    rich: true,
};

const PREFIXES: [&str; 12] = [
    "",
    "A",
    "\u{c9}t\u{e9}-",
    "\u{6d1e}",
    "\u{1f987}",
    "q\"uote",
    "back\\slash",
    "tab\t",
    "ctl\u{1}",
    "nul\u{0}",
    "sep\u{2028}",
    " sp ",
];

const COLORS: [&str; 4] = ["#4ecdc4", "#ff6b6b", "#abc", "#95e1d3"];

fn instant(rng: &mut Rng) -> DateTime<Utc> {
    let secs = 1_700_000_000 + rng.below(100_000_000) as i64;
    let nanos = rng.below(1_000_000_000) as u32;
    Utc.timestamp_opt(secs, nanos).single().unwrap()
}

fn day(rng: &mut Rng) -> NaiveDate {
    NaiveDate::from_ymd_opt(
        1990 + rng.below(40) as i32,
        1 + rng.below(12) as u32,
        1 + rng.below(28) as u32,
    )
    .unwrap()
}

fn text(rng: &mut Rng, base: &str) -> String {
    format!("{}{base}", rng.pick(&PREFIXES))
}

fn reference(rng: &mut Rng) -> AzimuthReference {
    let source = match rng.below(4) {
        0 => CorrectionSource::Unknown,
        1 => CorrectionSource::FileDeclared,
        2 => CorrectionSource::Manual,
        _ => CorrectionSource::IgrfAuto {
            epoch_year: 1900 + rng.below(200) as i32,
        },
    };
    let north = *rng.pick(&[
        NorthReference::Magnetic,
        NorthReference::True,
        NorthReference::Grid,
    ]);
    // Kept exactly as written: even a `True` reference with a correction.
    AzimuthReference {
        north,
        correction: rng.awkward(-20.0, 20.0),
        source,
    }
}

fn credit(rng: &mut Rng) -> SurveyCredit {
    let people = |n: usize, rng: &mut Rng| {
        (0..n)
            .map(|i| Person {
                name: text(rng, &format!("Diver {i}")),
                email: rng.chance(0.5).then(|| format!("d{i}@example.org")),
            })
            .collect::<Vec<_>>()
    };
    SurveyCredit {
        surveyors: people(rng.below(3), rng),
        explorers: people(rng.below(2), rng),
        date: rng.chance(0.7).then(|| day(rng)),
    }
}

/// A valid survey and extras from `seed`.
pub(super) fn generate(seed: u64, shape: Shape) -> (Project, Extras) {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let rich = shape.rich;
    let mut p = Project::new_at(
        if rich {
            text(&mut rng, "Synthetic cave")
        } else {
            "Synthetic cave".to_string()
        },
        rng.chance(0.7).then(|| "Nowhere, synthetic".to_string()),
        instant(&mut rng),
    );
    p.modified_at = instant(&mut rng);

    for i in 0..rng.below(4) {
        let id = p.mint_section_id().unwrap();
        p.sections.push(Section {
            id,
            name: format!("Section {i}"),
            color: rng.pick(&COLORS).to_string(),
        });
    }
    if rich {
        p.settings.validation.warn_shot_length_min = rng.awkward(0.0, 1.0);
        p.settings.surface_elevation = rng.chance(0.5).then(|| rng.awkward(-10.0, 3000.0));
        p.settings.default_azimuth_reference = rng.chance(0.5).then(|| reference(&mut rng));
    }

    let shared: Vec<Arc<SurveyCredit>> = (0..3).map(|_| Arc::new(credit(&mut rng))).collect();
    let mut uuid_seq = 0u64;
    for i in 0..shape.lines.max(1) {
        let line_credit = rng.chance(0.7).then(|| Arc::clone(rng.pick(&shared)));
        let n = rng.below(shape.max_shots + 1);
        let shots: Vec<ShotData> = (0..n)
            .map(|_| {
                uuid_seq += 1;
                shot(&mut rng, rich, seed, uuid_seq, line_credit.as_ref())
            })
            .collect();
        let meta = LineMeta {
            name: rng
                .chance(0.8)
                .then(|| text(&mut rng, &format!("Line {i}"))),
            color: rng.pick(&COLORS).to_string(),
            direction: *rng.pick(&[SurveyDirection::In, SurveyDirection::Out]),
            quality: if rich && rng.chance(0.3) {
                rng.range(0.1, 3.0)
            } else {
                1.0
            },
            azimuth_reference: (rich && rng.chance(0.3)).then(|| reference(&mut rng)),
            section_id: (!p.sections.is_empty() && rng.chance(0.5))
                .then(|| rng.pick(&p.sections).id),
            legacy_surveyor: (rich && rng.chance(0.2)).then(|| text(&mut rng, "Old surveyor")),
            legacy_date: (rich && rng.chance(0.2)).then(|| day(&mut rng)),
        };
        let stations: Vec<StationId> = p.stations.keys().copied().collect();
        if i == 0 || stations.is_empty() || rng.chance(0.2) {
            p.add_line(meta, StartAt::New, shots).unwrap();
        } else if n >= 2 && rng.chance(0.25) {
            // A junction mid-chain: the line passes through a station of
            // another piece (two pieces join; anchors are fixed at the end).
            let topology = p.topology_arc();
            let through = *rng.pick(&stations);
            let fresh_component = topology.component_of(through);
            let at = 1 + rng.below(n - 1);
            let mut chain = Vec::with_capacity(n + 1);
            let start = p.mint_station("s").unwrap();
            chain.push(start);
            for k in 1..=n {
                if k == at {
                    chain.push(through);
                } else {
                    chain.push(p.mint_station(format!("n{k}")).unwrap());
                }
            }
            let _ = fresh_component;
            p.add_line_on_stations(meta, &chain, shots).unwrap();
        } else {
            let start = *rng.pick(&stations);
            p.add_line(meta, StartAt::Station(start), shots).unwrap();
        }
    }

    // Unique, awkward station names (unique project-wide, so on every line).
    let ids: Vec<StationId> = p.stations.keys().copied().collect();
    for &id in &ids {
        let prefix = if rich { *rng.pick(&PREFIXES) } else { "S" };
        let name = format!("{prefix}{id}");
        let comment = (rich && rng.chance(0.1)).then(|| text(&mut rng, "tie-off\non a rock"));
        let external = (rich && rng.chance(0.05)).then(|| format!("EXT-{id}"));
        let station = p.station_mut(id).unwrap();
        station.name = name;
        station.comment = comment;
        station.external = external;
    }
    if rich {
        for _ in 0..rng.below(3) {
            let host = *rng.pick(&ids);
            let alias = p.mint_station_id().unwrap();
            let name = format!("old{alias}");
            let comment = rng.chance(0.5).then(|| format!("note {alias}"));
            // Half of them name the first line through a junction host (the
            // line whose own station it was), keeping a native line there
            // (A3, A4); the others are history.
            let lines = p.topology().lines_of(host);
            let named = p.stations[&host]
                .archived
                .iter()
                .any(|r| !r.lines.is_empty());
            let lines = if rng.chance(0.5) && lines.len() > 1 && !named {
                vec![lines[0]]
            } else {
                Vec::new()
            };
            p.station_mut(host).unwrap().archived.push(ArchivedStation {
                id: alias,
                name,
                comment,
                external: None,
                into: None,
                lines,
            });
        }
        // An empty credit object (`{}`) is a value of its own; `from_data`
        // never stores one, so set it on the model directly.
        if let Some(line) = p.lines().iter().find(|l| !l.shots.is_empty()).map(|l| l.id) {
            p.line_mut(line).unwrap().shots[0].credit = Some(Arc::new(SurveyCredit::default()));
        }
        // Legacy hand moves on a few (line, station) slots.
        let line_ids: Vec<_> = p.lines().iter().map(|l| l.id).collect();
        for _ in 0..rng.below(4) {
            let line = *rng.pick(&line_ids);
            let chain = p.line_stations(line).unwrap().to_vec();
            let station = *rng.pick(&chain);
            let hand = [
                rng.awkward_within(-1.0, 1.0, MAX_COORDINATE_M),
                rng.awkward_within(-1.0, 1.0, MAX_COORDINATE_M),
                rng.awkward_within(-1.0, 1.0, MAX_COORDINATE_M),
            ];
            p.line_mut(line).unwrap().hand_moves.insert(station, hand);
        }
    }

    // Anchors: some pieces get datums (the first places, later ones are
    // fixes), every other piece exactly one placement.
    let topology = p.topology_arc();
    let mut datum_pieces = Vec::new();
    for (index, component) in topology.components().iter().enumerate() {
        if rng.chance(0.4) {
            for _ in 0..=usize::from(rng.chance(0.3)) {
                let station = *rng.pick(&component.stations);
                let lines = topology.lines_of(station);
                p.push_datum(ZeroDatum {
                    station,
                    via: (rng.chance(0.5) && !lines.is_empty()).then(|| *rng.pick(&lines)),
                    gps: GpsCoordinates {
                        latitude: rng.range(-89.0, 89.0),
                        longitude: rng.range(-180.0, 180.0),
                        depth_reference: rng.awkward(-100.0, 100.0),
                    },
                    accuracy_m: rng.chance(0.5).then(|| rng.range(0.1, 30.0)),
                    entrance: rng.chance(0.3).then(|| rng.chance(0.5)),
                    correction: rng.chance(0.5).then(|| {
                        [
                            rng.awkward_within(-5.0, 5.0, MAX_COORDINATE_M),
                            rng.awkward_within(-5.0, 5.0, MAX_COORDINATE_M),
                        ]
                    }),
                });
            }
            datum_pieces.push(index);
        } else {
            let station = *rng.pick(&component.stations);
            p.placements.push(crate::models::survey::Placement {
                station,
                x: rng.awkward_within(-1000.0, 1000.0, MAX_COORDINATE_M),
                y: rng.awkward_within(-1000.0, 1000.0, MAX_COORDINATE_M),
            });
        }
    }

    // Closures inside a piece, and between two datum pieces.
    let strategies = [
        ClosureStrategy::Batch,
        ClosureStrategy::Move,
        ClosureStrategy::Tie,
        ClosureStrategy::LeastSquaresWeighted,
        ClosureStrategy::ProportionalByLength,
        ClosureStrategy::EqualDistribution,
    ];
    for _ in 0..rng.below(shape.lines / 2 + 2) {
        let component = rng.pick(topology.components());
        if component.stations.len() < 2 {
            continue;
        }
        let a = *rng.pick(&component.stations);
        let b = *rng.pick(&component.stations);
        if a == b {
            continue;
        }
        let a_lines = topology.lines_of(a);
        let a_via = rng.chance(0.5).then(|| *rng.pick(&a_lines));
        p.push_closure(a, a_via, b, None, *rng.pick(&strategies));
    }
    if datum_pieces.len() >= 2 {
        let a = topology.components()[datum_pieces[0]].stations[0];
        let b = topology.components()[datum_pieces[1]].stations[0];
        p.push_closure(a, None, b, None, ClosureStrategy::Batch);
    }
    if rich {
        for k in 0..rng.below(3) {
            p.imported_file_hashes
                .push(format!("{:064x}", u128::from(seed as u32) + k as u128));
        }
        for k in 0..rng.below(3) {
            p.imported_line_hashes.push(format!("line-{seed}-{k}"));
        }
        p.counters.next_closure_id += rng.below(3) as u64;
        p.counters.next_station_id += rng.below(3) as u32;
    }

    let violations = p.validate();
    assert!(
        violations.is_empty(),
        "the generator built an invalid survey: {violations:?}"
    );
    let extras = extras_for(&p, &mut rng, rich);
    (p, extras)
}

fn shot(
    rng: &mut Rng,
    rich: bool,
    seed: u64,
    seq: u64,
    line_credit: Option<&Arc<SurveyCredit>>,
) -> ShotData {
    let num = |rng: &mut Rng, lo: f64, hi: f64| {
        if rich {
            rng.awkward(lo, hi)
        } else {
            (rng.range(lo, hi) * 10.0).round() / 10.0
        }
    };
    // A distance the survey bounds (B-derived-solver-3).
    let dist = |rng: &mut Rng, lo: f64, hi: f64| {
        if rich {
            rng.awkward_within(lo, hi, MAX_MEASUREMENT_M)
        } else {
            (rng.range(lo, hi) * 10.0).round() / 10.0
        }
    };
    let lrud = rng.below(4);
    let cell = |rng: &mut Rng| match lrud {
        0 => None,
        1 => Some(dist(rng, 0.1, 6.0)),
        _ => rng.chance(0.6).then(|| dist(rng, 0.1, 6.0)),
    };
    let left = cell(rng);
    let right = cell(rng);
    let up = cell(rng);
    let down = cell(rng);
    let own_credit = match rng.below(10) {
        0 if rich => None,
        1 if rich => Some(Arc::new(SurveyCredit::default())),
        2 => Some(Arc::new(credit(rng))),
        _ => line_credit.map(Arc::clone),
    };
    ShotData {
        uuid: Some(if rich && rng.chance(0.05) {
            format!("{}-{seq}", "x".repeat(100))
        } else {
            format!("{seed:08x}-{seq:06}-4000-8000-000000000000")
        }),
        azimuth_in: num(rng, 0.0, 360.0),
        azimuth_out: num(rng, 0.0, 360.0),
        length: dist(rng, 0.3, 30.0),
        depth_in: dist(rng, 0.0, 60.0),
        depth_out: dist(rng, 0.0, 60.0),
        pitch_in: (rich && rng.chance(0.2)).then(|| num(rng, -90.0, 90.0)),
        pitch_out: (rich && rng.chance(0.2)).then(|| num(rng, -90.0, 90.0)),
        left,
        right,
        up,
        down,
        comment: rng
            .chance(0.08)
            .then(|| text(rng, "lead on the left, \"silt\"")),
        kind: if rich {
            *rng.pick(&[
                ShotKind::Normal,
                ShotKind::Normal,
                ShotKind::Jump,
                ShotKind::Reference,
            ])
        } else {
            ShotKind::Normal
        },
        marker_index: rng.chance(0.05).then(|| rng.below(5) as u32),
        temperature: rng.chance(0.3).then(|| num(rng, 18.0, 28.0)),
        credit: own_credit,
    }
}

fn extras_for(p: &Project, rng: &mut Rng, rich: bool) -> Extras {
    let mut extras = Extras {
        generator: "Stalagmight test".to_string(),
        ..Extras::default()
    };
    if rng.chance(0.5) {
        extras.speleodb = Some(SpeleoDbLink {
            endpoint: "https://speleodb.example".to_string(),
            project_id: format!("{:032x}", rng.next()),
            project_name: text(rng, "Shared cave"),
            undo_mode: *rng.pick(&[None, Some(UndoMode::Full), Some(UndoMode::Disabled)]),
        });
        let base_commit = rng.chance(0.7).then(|| format!("{:040x}", rng.next()));
        extras.local.speleodb = LocalSync {
            // No draw of its own: every later draw stays where seeds expect it.
            base_survey: base_commit
                .as_ref()
                .map(|base| format!("sha256:{:0>64}", &base[..8])),
            base_commit,
            base_survey_edit: None,
            unpushed_edits: rng.chance(0.5),
            last_edit_at: rng.chance(0.5).then(|| instant(rng)),
            last_verify_at: rng.chance(0.5).then(|| instant(rng)),
            unwound_from: rng.chance(0.2).then(|| format!("{:040x}", rng.next())),
            inherited_through: rng.chance(0.3).then(|| instant(rng)),
            preserved: Vec::new(),
        };
        // No draw either: the digest's vouched edit time, where there is both.
        let sync = &mut extras.local.speleodb;
        sync.base_survey_edit = sync.base_survey.as_ref().and(sync.last_edit_at);
    }
    if rng.chance(0.7) {
        let mut display = BTreeMap::new();
        display.insert("grid".to_string(), "true".to_string());
        display.insert("hiddenSections".to_string(), "[1,2]".to_string());
        extras.local = LocalState {
            display,
            ..extras.local
        };
    }
    let uuids: Vec<String> = p
        .lines()
        .iter()
        .flat_map(|l| l.shots.iter().map(|s| s.uuid.clone()))
        .collect();
    if !uuids.is_empty() {
        for _ in 0..rng.below(4) {
            extras.ignored_warnings.push(IgnoredWarning {
                shot: rng.pick(&uuids).clone(),
                code: rng.pick(&["SHORT_SHOT", "PITCH_DEVIATION"]).to_string(),
                at: instant(rng),
                comment: rng.chance(0.5).then(|| text(rng, "checked")),
            });
        }
    }
    extras.ignored_warnings.sort();
    extras
        .ignored_warnings
        .dedup_by(|b, a| a.shot == b.shot && a.code == b.code);
    if rich || rng.chance(0.5) {
        let (undo, redo) = (rng.below(6), rng.below(3));
        extras.history = history(rng, undo, redo);
    }
    extras
}

/// Opaque history entries shaped like stream U's (`t` first).
pub(super) fn history(rng: &mut Rng, undo: usize, redo: usize) -> History {
    let mut at = Utc.timestamp_opt(1_750_000_000, 0).single().unwrap();
    let mut entry = |rng: &mut Rng| {
        at += chrono::Duration::seconds(1 + rng.below(100) as i64);
        RawJson::parse(&format!(
            r#"{{"t": "{}", "k": "update_shot", "label": "Edit shot é {}", "ops": [{{"x": -0.0}}, 1e5]}}"#,
            at.to_rfc3339(),
            rng.below(100)
        ))
        .unwrap()
    };
    History {
        undo: (0..undo).map(|_| entry(rng)).collect(),
        redo: (0..redo).map(|_| entry(rng)).collect(),
    }
}

/// Encode normally, decode, and require the same model, the same extras and
/// the same bytes again. Returns the bytes.
pub(super) fn assert_round_trip(p: &Project, extras: &Extras) -> Vec<u8> {
    let bytes = encode(p, extras, Mode::Normal).unwrap();
    let decoded = match decode(&bytes, &DecodeOptions::default()) {
        Ok(d) => d,
        Err(e) => panic!("decode failed: {e}\n{}", String::from_utf8_lossy(&bytes)),
    };
    let mut expected = p.clone();
    expected.assign_session_shot_ids();
    assert!(
        decoded.project == expected,
        "the project changed in a round trip"
    );
    assert_eq!(
        &decoded.extras, extras,
        "the extras changed in a round trip"
    );
    assert!(
        decoded.notes.is_empty(),
        "notes on a clean file: {:?}",
        decoded.notes
    );
    let again = encode(&decoded.project, &decoded.extras, Mode::Normal).unwrap();
    assert!(again == bytes, "re-encoding changed the bytes");
    bytes
}

/// The text of encoded bytes.
pub(super) fn text_of(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

/// `text` with `edit` applied to the first line header whose id is `id`.
pub(super) fn edit_line_header(text: &str, id: u32, edit: impl Fn(&str) -> String) -> String {
    let needle = format!("{{\"id\":{id},");
    match text
        .lines()
        .find(|l| l.starts_with(&needle) && l.contains("\"start\":"))
    {
        Some(header) => text.replacen(header, &edit(header), 1),
        None => text.to_string(),
    }
}

/// `text` with `insert` (one or more records, no trailing comma) added at the
/// end of the `lines` array.
pub(super) fn append_to_lines(text: &str, insert: &str) -> String {
    let start = text.find("\n\"lines\":[\n").unwrap();
    let end = start + text[start..].find("\n],\n").unwrap();
    format!("{},\n{insert}{}", &text[..end], &text[end..])
}
