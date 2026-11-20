//! On demand: the station-graph exporters on the characterisation scenarios'
//! final states, written in the characterisation dump's masked form so the
//! files compare with its goldens (`scripts/dev/compare-characterization.mjs`).
//!
//! Input: a directory the dump wrote with `STALAGMIGHT_CHAR_SAVE` (each
//! scenario's final project as a 0.6.0 `.stalx` plus `<scenario>.uuids.json`:
//! the uuid book and the gas / closure-adjustment settings its exports used).
//! Each project is migrated (`persistence::migrate`), exported with
//! `export::survey`, masked with the scenario's own uuid book and written to
//! `<out>/<scenario>/final-export.*`, the dump's file names.
//!
//! ```text
//! STALAGMIGHT_X_GOLDEN_IN=<saved> STALAGMIGHT_X_GOLDEN_OUT=<out> cargo test \
//!     -p stalagmight-export --lib golden_exports -- --ignored --nocapture
//! ```
//!
//! Synthetic data only (the dump builds every survey); nothing is committed.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Map, Value};

use super::{export_tml, generate_svg_string_with_positions, json_export, write_csv};
use crate::export::{ColorMode, ExportImageOptions, ImageFormat};
use crate::models::config::ClosureAdjustmentConfig;

const MASKED_KEYS: &[&str] = &[
    "created_at",
    "modified_at",
    "merged_at",
    "exported_at",
    "resolved_at",
    "timestamp",
];

/// `8-4-4-4-12` hex digits.
fn looks_like_uuid(bytes: &[u8]) -> bool {
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(k, b)| {
            if matches!(k, 8 | 13 | 18 | 23) {
                *b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

/// The dump's uuid book: `uuid#N` per uuid, new ones numbered after it.
struct Book(BTreeMap<String, usize>);

impl Book {
    fn token(&mut self, uuid: &str) -> String {
        let next = self.0.len() + 1;
        let n = *self.0.entry(uuid.to_ascii_lowercase()).or_insert(next);
        format!("uuid#{n}")
    }

    fn mask(&mut self, text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let (mut i, mut last) = (0, 0);
        while i + 36 <= bytes.len() {
            if looks_like_uuid(&bytes[i..i + 36]) {
                out.push_str(&text[last..i]);
                out.push_str(&self.token(&text[i..i + 36]));
                i += 36;
                last = i;
            } else {
                i += 1;
            }
        }
        out.push_str(&text[last..]);
        out
    }

    /// Sorted keys, masked time keys and uuids (no rounding: exports carry
    /// raw survey values).
    fn finish(&mut self, value: Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut entries: Vec<(String, Value)> = map.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                let mut out = Map::new();
                for (key, item) in entries {
                    let item = if MASKED_KEYS.contains(&key.as_str()) && !item.is_null() {
                        Value::String("<masked>".to_string())
                    } else {
                        self.finish(item)
                    };
                    out.insert(key, item);
                }
                Value::Object(out)
            }
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|v| self.finish(v)).collect())
            }
            Value::String(text) => Value::String(self.mask(&text)),
            other => other,
        }
    }
}

fn write_value(dir: &Path, file: &str, value: &Value) {
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    std::fs::write(dir.join(file), text).unwrap();
}

fn export_scenario(stalx: &Path, out: &Path) {
    let scenario = stalx.file_stem().unwrap().to_string_lossy().to_string();
    let meta: Value =
        serde_json::from_str(&std::fs::read_to_string(stalx.with_extension("uuids.json")).unwrap())
            .unwrap();
    let mut book = Book(serde_json::from_value(meta["uuids"].clone()).unwrap());
    let config: ClosureAdjustmentConfig =
        serde_json::from_value(meta["closure_adjustment"].clone()).unwrap();
    let include_reference_legs = meta["include_reference_legs"].as_bool().unwrap_or(false);

    let migrated = stalagmight_persistence::persistence::migrate::migrate_bytes(
        &std::fs::read(stalx).unwrap(),
    )
    .unwrap();
    let project = migrated.project;
    let dir = out.join(&scenario);
    std::fs::create_dir_all(&dir).unwrap();
    let tmp = tempfile::TempDir::new().unwrap();

    let mut csv = Vec::new();
    write_csv(&project, &mut csv).unwrap();
    std::fs::write(
        dir.join("final-export.csv"),
        book.mask(&String::from_utf8(csv).unwrap()),
    )
    .unwrap();

    let json = serde_json::to_value(json_export(&project, Utc::now())).unwrap();
    let json = book.finish(json);
    write_value(&dir, "final-export.json", &json);

    let tmlu = tmp.path().join("export.tmlu");
    let notes = export_tml(&project, &tmlu).unwrap();
    std::fs::write(
        dir.join("final-export.tmlu"),
        book.mask(&std::fs::read_to_string(&tmlu).unwrap()),
    )
    .unwrap();
    // The file re-imports to every shot, by uuid.
    let parsed = crate::parsers::tml::parse_tml(&tmlu).unwrap();
    let (configs, _, _) =
        crate::parsers::tml::convert_tml_to_import_configs(&parsed, false, true).unwrap();
    let mut back: Vec<String> = configs
        .iter()
        .flat_map(|c| c.shots.iter().filter_map(|s| s.uuid.clone()))
        .collect();
    let mut want: Vec<String> = project
        .lines()
        .iter()
        .flat_map(|l| l.shots.iter().map(|s| s.uuid.clone()))
        .collect();
    back.sort();
    want.sort();
    assert_eq!(
        back, want,
        "{scenario}: the Ariane export does not re-import to the same shots"
    );
    let notes = book.finish(serde_json::to_value(&notes).unwrap());
    write_value(&dir, "final-export-tml-notes.json", &notes);

    let tml = tmp.path().join("export.tml");
    export_tml(&project, &tml).unwrap();
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&tml).unwrap()).unwrap();
    let mut entries = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).unwrap();
        let mut text = String::new();
        entry.read_to_string(&mut text).unwrap();
        entries.push((entry.name().to_string(), text));
    }
    entries.sort();
    let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
    write_value(
        &dir,
        "final-export-tml-entries.json",
        &serde_json::to_value(&names).unwrap(),
    );
    for (name, text) in &entries {
        let file = if name == "Data.xml" {
            "final-export.tml.xml".to_string()
        } else {
            format!("final-export.tml.{}", name.replace('/', "_"))
        };
        std::fs::write(dir.join(file), book.mask(text)).unwrap();
    }

    let raw = project.raw_positions();
    let adjusted = project.adjusted_positions_from(&raw, &config);
    // Per-slot positions (full precision), raw and Computed View, beside the
    // exports, so a rendering difference can be told from a layout one.
    let slots = |positions: &[crate::models::survey::SlotPosition]| -> Value {
        positions
            .iter()
            .map(|p| {
                serde_json::json!({
                    "slot": [p.slot.line_id, p.slot.station_index],
                    "station": p.station,
                    "xyz": [p.x, p.y, p.z],
                })
            })
            .collect()
    };
    let positions = serde_json::json!({
        "raw": slots(&raw.slots(&project)),
        "adjusted": slots(&adjusted.slots(&project)),
    });
    let diagnostics = out.join("positions");
    std::fs::create_dir_all(&diagnostics).unwrap();
    write_value(&diagnostics, &format!("{scenario}.json"), &positions);
    for (file, ids) in [
        ("final-export-station-ids.svg", true),
        ("final-export-station-names.svg", false),
    ] {
        let options = ExportImageOptions {
            format: ImageFormat::Svg,
            dpi: 96,
            color_mode: ColorMode::Line,
            show_station_labels: ids,
            show_station_names: !ids,
            show_line_names: true,
            show_station_comments: true,
            show_shot_comments: true,
            show_shot_data: false,
            include_reference_legs,
            ..Default::default()
        };
        let svg =
            generate_svg_string_with_positions(&project, &options, &adjusted, &raw).unwrap();
        std::fs::write(dir.join(file), book.mask(&svg)).unwrap();
    }
}

#[test]
#[ignore = "on demand: needs STALAGMIGHT_X_GOLDEN_IN (a dump's STALAGMIGHT_CHAR_SAVE dir) and _OUT"]
fn golden_exports() {
    let input = PathBuf::from(std::env::var("STALAGMIGHT_X_GOLDEN_IN").unwrap());
    let out = PathBuf::from(std::env::var("STALAGMIGHT_X_GOLDEN_OUT").unwrap());
    let mut scenarios: Vec<PathBuf> = std::fs::read_dir(&input)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "stalx"))
        .collect();
    scenarios.sort();
    assert!(!scenarios.is_empty(), "no .stalx in {}", input.display());
    for stalx in &scenarios {
        export_scenario(stalx, &out);
    }
}
