//! The import oracle: run one [`FileSet`] through the same parse -> preview ->
//! convert (-> apply) pipeline the app's import commands run, and check the
//! invariants that must hold for ANY input, however hostile.
//!
//! A panic anywhere in the pipeline is itself a finding (the fuzz target lets
//! it unwind to libFuzzer; the fixed-seed tests catch it per seed). Beyond
//! "no panic", a run returns a [`Violation`] when:
//! - a preview the import dialog receives cannot be serialised for IPC;
//! - an import that the app accepted left a shot with a non-finite number in
//!   the project, or a station whose computed position is non-finite.
//!
//! An `Err` from a parser or converter, or a refused apply, is NOT a violation:
//! rejecting a bad file with a message is the correct outcome.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use stalagmight_core::core::events::EventSink;
use stalagmight_core::core::{AppState, VirtualShotMode};
use stalagmight_models::section::{ImportClosure, ImportSectionConfig};
use stalagmight_parsers::parsers::{compass, gpx, tml, walls};

use crate::fileset::{with_scratch_dir, FileSet};

/// Every importer the app offers, by the file the user opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// mNemo `.dmp`.
    Dmp,
    /// Ariane `.tml` (a zip holding the XML).
    Tml,
    /// Ariane `.tmlu` (plain XML).
    Tmlu,
    /// Walls `.srv` survey opened on its own.
    Srv,
    /// Walls `.wpj` project with linked `.srv` files.
    Wpj,
    /// Compass `.dat` survey opened on its own.
    Dat,
    /// Compass `.mak` project with linked `.dat` files.
    Mak,
    /// GPX.
    Gpx,
}

impl Format {
    pub const ALL: [Self; 8] = [
        Self::Dmp,
        Self::Tml,
        Self::Tmlu,
        Self::Srv,
        Self::Wpj,
        Self::Dat,
        Self::Mak,
        Self::Gpx,
    ];

    /// The file name the opened file is written under (the importers dispatch
    /// on the extension).
    pub fn entry_name(self) -> &'static str {
        match self {
            Self::Dmp => "survey.dmp",
            Self::Tml => "survey.tml",
            Self::Tmlu => "survey.tmlu",
            Self::Srv => "survey.srv",
            Self::Wpj => "project.wpj",
            Self::Dat => "survey.dat",
            Self::Mak => "project.mak",
            Self::Gpx => "track.gpx",
        }
    }

    /// Short stable name (fuzz target and corpus directory suffix).
    pub fn slug(self) -> &'static str {
        match self {
            Self::Dmp => "dmp",
            Self::Tml => "tml",
            Self::Tmlu => "tmlu",
            Self::Srv => "srv",
            Self::Wpj => "wpj",
            Self::Dat => "dat",
            Self::Mak => "mak",
            Self::Gpx => "gpx",
        }
    }
}

/// How far down the pipeline a run goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// Parse, preview and convert (what the import dialog does before the
    /// user presses Import). Cheap: no project state.
    Convert,
    /// Also apply the converted sections to a fresh project through the same
    /// core command the app runs, then check the project.
    Apply,
}

/// Import options a run uses. The app's dialogs expose these as controls; a
/// run derives them from the input so every combination is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Knobs(pub u8);

impl Knobs {
    fn bit(self, n: u8) -> bool {
        self.0 & (1 << n) != 0
    }

    /// Knobs derived from the bytes of an input (stable for a given input).
    pub fn of(data: &[u8]) -> Self {
        Self(data.iter().fold(0u8, |acc, b| acc.rotate_left(1) ^ b))
    }
}

/// How far a run got.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub parsed: bool,
    pub converted: bool,
    pub applied: bool,
    pub sections: usize,
    pub shots: usize,
    /// Why the file was rejected (the parser's or the load's error), when it was.
    pub rejected: Option<String>,
}

/// An invariant the pipeline broke for an input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation(pub String);

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn violation(msg: impl Into<String>) -> Violation {
    Violation(msg.into())
}

/// Converted sections ready to apply.
struct Converted {
    sections: Vec<ImportSectionConfig>,
    closures: Vec<ImportClosure>,
    /// Which core command applies the sections.
    apply: ApplyWith,
}

/// The core import command a converted file is applied with.
enum ApplyWith {
    /// Every importer but Ariane.
    Sections,
    /// Ariane: the sections plus the file's entrance elevation (none when the
    /// file states the no-data value).
    Ariane(Option<f64>),
}

/// Run `files` (opened as `format`) through the importer pipeline.
pub fn run(
    format: Format,
    files: &FileSet,
    knobs: Knobs,
    depth: Depth,
) -> Result<Outcome, Violation> {
    with_scratch_dir(|dir| -> Result<Outcome, Violation> {
        let path = files
            .materialize(dir)
            .map_err(|e| violation(format!("harness: cannot stage files: {e:#}")))?;
        let (mut outcome, converted) = convert(format, &path, knobs)?;
        if let (Depth::Apply, Some(converted)) = (depth, converted) {
            match apply(converted, &dir.join("fuzz-project.stalx"))? {
                Ok(()) => outcome.applied = true,
                Err(refusal) => outcome.rejected = Some(refusal),
            }
        }
        Ok(outcome)
    })
    .map_err(|e| violation(format!("harness: scratch dir: {e:#}")))?
}

fn preview_serializes<T: serde::Serialize>(what: &str, value: &T) -> Result<(), Violation> {
    serde_json::to_string(value)
        .map(drop)
        .map_err(|e| violation(format!("{what} preview does not serialise for IPC: {e}")))
}

fn convert(
    format: Format,
    path: &Path,
    knobs: Knobs,
) -> Result<(Outcome, Option<Converted>), Violation> {
    let mut outcome = Outcome::default();
    let converted = match format {
        Format::Dmp => {
            // The DMP wizard's segmentation and conversion run in the frontend:
            // the backend's whole job is this parse.
            let dmp = match stalagmight_parsers::parsers::parse_dmp(path) {
                Ok(v) => v,
                Err(e) => {
                    outcome.rejected = Some(format!("{e:#}"));
                    return Ok((outcome, None));
                }
            };
            outcome.parsed = true;
            preview_serializes("DMP", &dmp)?;
            outcome.sections = dmp.sections.len();
            outcome.shots = dmp.sections.iter().map(|s| s.shots.len()).sum();
            return Ok((outcome, None));
        }
        Format::Tml | Format::Tmlu => {
            let file = match tml::parse_tml(path) {
                Ok(v) => v,
                Err(e) => {
                    outcome.rejected = Some(format!("{e:#}"));
                    return Ok((outcome, None));
                }
            };
            outcome.parsed = true;
            preview_serializes("TML", &file)?;
            let selected = knobs.bit(2).then(|| {
                (0..file.sections.len() as u32)
                    .step_by(2)
                    .collect::<HashSet<u32>>()
            });
            let opts = tml::TmlImportOptions {
                include_excluded: knobs.bit(0),
                comments_on_stations: knobs.bit(1),
                selected_sections: selected,
                ..tml::TmlImportOptions::default()
            };
            let elevation =
                stalagmight_models::ariane_elevation_or_absent(file.first_start_absolute_elevation);
            tml::convert_tml_with_options(&file, &opts)
                .ok()
                .map(|(sections, closures, _)| Converted {
                    sections,
                    closures,
                    apply: ApplyWith::Ariane(elevation),
                })
        }
        Format::Srv | Format::Wpj => {
            let parsed = match walls::parse_walls(path) {
                Ok(v) => v,
                Err(e) => {
                    outcome.rejected = Some(format!("{e:#}"));
                    return Ok((outcome, None));
                }
            };
            outcome.parsed = true;
            let _ = walls::preview_scalars(&parsed);
            preview_serializes("Walls", &walls::build_preview(&parsed))?;
            let opts = walls::WallsImportOptions {
                comments_on_stations: knobs.bit(1),
                include_splays: knobs.bit(0),
                selected_surveys: knobs.bit(2).then(|| [0u32, 2, 4].into_iter().collect()),
            };
            walls::convert_walls_to_import_configs(&parsed, &opts)
                .ok()
                .map(|(sections, closures, _)| Converted {
                    sections,
                    closures,
                    apply: ApplyWith::Sections,
                })
        }
        Format::Dat | Format::Mak => {
            let parsed = match compass::parse_compass(path) {
                Ok(v) => v,
                Err(e) => {
                    outcome.rejected = Some(format!("{e:#}"));
                    return Ok((outcome, None));
                }
            };
            outcome.parsed = true;
            let _ = compass::preview_scalars(&parsed);
            preview_serializes("Compass", &compass::build_preview(&parsed))?;
            let opts = compass::CompassImportOptions {
                include_excluded: knobs.bit(0),
                apply_corrections: knobs.bit(3),
                comments_on_stations: knobs.bit(1),
            };
            compass::convert_compass_to_import_configs(&parsed, &opts)
                .ok()
                .map(|(sections, closures, _)| Converted {
                    sections,
                    closures,
                    apply: ApplyWith::Sections,
                })
        }
        Format::Gpx => {
            let file = match gpx::parse_gpx(path) {
                Ok(v) => v,
                Err(e) => {
                    outcome.rejected = Some(format!("{e:#}"));
                    return Ok((outcome, None));
                }
            };
            outcome.parsed = true;
            preview_serializes("GPX", &file)?;
            gpx::convert_gpx_to_import_configs(
                &file,
                !knobs.bit(0),
                !knobs.bit(1),
                knobs.bit(2),
                &[],
                knobs.bit(3),
            )
            .ok()
            .map(|(sections, _)| Converted {
                sections,
                closures: Vec::new(),
                apply: ApplyWith::Sections,
            })
        }
    };
    if let Some(c) = &converted {
        outcome.converted = true;
        outcome.sections = c.sections.len();
        outcome.shots = c.sections.iter().map(|s| s.shots.len()).sum();
    }
    Ok((outcome, converted))
}

/// An [`EventSink`] that delivers nothing (the load and import paths refuse
/// to run without one).
struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

thread_local! {
    static STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

/// Run `f` against this thread's [`AppState`] (built once: building one opens
/// the tile cache and reads the config, far too slow per input). Callers start
/// from a fresh project, so nothing carries over between inputs.
pub(crate) fn with_state<T>(
    f: impl FnOnce(&AppState) -> Result<T, Violation>,
) -> Result<T, Violation> {
    STATE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let state =
                AppState::new().map_err(|e| violation(format!("harness: AppState::new: {e:#}")))?;
            state.set_event_sink(Arc::new(NullSink));
            *slot = Some(state);
        }
        match slot.as_ref() {
            Some(state) => f(state),
            None => Err(violation("harness: no AppState")),
        }
    })
}

/// Replace whatever `state` holds with a new, empty project whose file is
/// reserved at `path` but not written (the app's File > New with a deferred
/// first save).
pub(crate) fn fresh_project(state: &AppState, name: &str, path: &Path) -> Result<(), Violation> {
    state
        .do_create_project_full(name.to_string(), None, None, Some(path.to_path_buf()), true)
        .map(drop)
        .map_err(|e| violation(format!("harness: create project: {e:?}")))
}

/// `Ok(Err(reason))` when the app refused the import (a correct outcome).
fn apply(converted: Converted, project_path: &Path) -> Result<Result<(), String>, Violation> {
    with_state(|state| {
        fresh_project(state, "FuzzProject", project_path)?;
        let Converted {
            sections,
            closures,
            apply,
        } = converted;
        let result = match apply {
            ApplyWith::Ariane(elevation) => state.execute_tml_import_sections(
                sections,
                closures,
                None,
                Vec::new(),
                Vec::new(),
                VirtualShotMode::default(),
                elevation,
            ),
            ApplyWith::Sections => state.execute_import_sections(
                sections,
                closures,
                None,
                Vec::new(),
                Vec::new(),
                VirtualShotMode::default(),
            ),
        };
        if !result.success {
            return Ok(Err(result.error.unwrap_or_default()));
        }
        let project = state
            .get_project_state()
            .ok_or_else(|| violation("import succeeded but no project is open"))?;
        check_project(&project)?;
        Ok(Ok(()))
    })
}

/// Invariants of a project an import was accepted into.
pub fn check_project(project: &stalagmight_models::project::Project) -> Result<(), Violation> {
    for (si, section) in project.sections.iter().enumerate() {
        for (i, shot) in section.shots.iter().enumerate() {
            if !shot.all_finite() {
                return Err(violation(format!(
                    "accepted import left a non-finite number in section {si} shot {i}: {shot:?}"
                )));
            }
        }
    }
    for station in project.compute_station_coordinates() {
        let c = &station.coordinates;
        if !(c.x.is_finite() && c.y.is_finite() && c.z.is_finite()) {
            return Err(violation(format!(
                "accepted import computes a non-finite station position {:?} at {:?}",
                (c.x, c.y, c.z),
                station.station_ref
            )));
        }
    }
    Ok(())
}
