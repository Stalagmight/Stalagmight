//! The import funnel on the station graph (plan 10.5): what every importer
//! (Ariane, Compass, Walls, GPX, the DMP wizard, the New Line dialog snapped
//! onto a station) sends - parsed polylines with one placement each, loop
//! closures, overlap updates of existing shots - becomes one edit of the
//! survey, recorded as one undo entry by the caller.
//!
//! What a placement becomes:
//! - `Origin` / `Manual`: a new free piece, its `Placement` on the file's
//!   first station at the origin (plus the manual offset). A reversed line
//!   keeps that station in place (it is the line's LAST station after the
//!   reverse), exactly where "import, then Reverse Line" would leave it, so
//!   the least-squares gauge is the same station either way.
//! - `ZeroDatum`: a datum on the chosen station, attached via the new line;
//!   the anchor algorithm puts the piece on its GPS (the project's first
//!   datum becomes the frame, at the origin).
//! - `FromExisting` / `FromImporting` / `ManualSnap`: the chosen chain
//!   station of the new line IS the target station (a junction). Nothing is
//!   recorded as a merge and nothing moves: the new line hangs off the
//!   station it was placed on. The junction keeps its name; a comment the
//!   file gives it is joined to its own (the merge rule, plan 5.1); a
//!   different name the file gives it is kept as an alias of the junction on
//!   the new line, under a retired Station ID minted for it, as Merge
//!   Stations keeps the retired station's (I-import-export-4). The new line
//!   then knows the junction by that alias, so its own stations keep the
//!   file's names even where one is the junction's (invariant 8's alias
//!   exemption, as after a merge or a 0.6.0 conversion; 0.6.0 kept a name
//!   per slot). Only where the line has no such alias (the file gives the
//!   junction no other name, or no Station ID is left) does a station of the
//!   new line whose file name is the junction's take a letter suffix, and
//!   the receipt says so.
//!
//! One line that cannot be imported costs the user that line, never the file
//! (#814): a line the project refuses is left out, a line whose placement
//! cannot be made is imported as a free piece at the origin (a line joining
//! a refused one imports on its own), and the receipt names each in
//! `line_problems`. Only when every line is refused does the import fail.
//! The DMP wizard keeps the whole import or nothing ([`RefusedLine`]).
//!
//! Lines are built in one pass with the low-level construction API (the
//! topology and the uuid index are rebuilt once, not per line), the anchors
//! are settled once, and the closures are judged against the solve's own
//! across-one-shot skip set in one pass, so an import of thousands of lines
//! stays linear in the survey.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::core::{Command, ValidationWarning, Validator};
use crate::models::station_naming::{
    dedupe_line_name_from, fill_line_station_names, name_with_free_letter, seed_line_name_from,
    settle_line_station_names,
};
use crate::models::survey::changeset::{PreviewRow, RecordedPreview};
use crate::models::survey::{
    ClosureId, ClosureStrategy, EditContext, Line, Placement, Project, Shot, StationAlias,
    ZeroDatum, MAX_SHOT_UUID_CHARS,
};
use crate::models::{
    clamp_section_name, reminted_shot_uuid_avoiding, reverse_shots, sanitize_display_text,
    AzimuthReference, Coordinates, CreditEdit, CreditEditKind, ExistingShotUpdate, GpsCoordinates,
    ImportClosure, ImportLineConfig, ImportPlacement, LineId, Section, SectionId, ShotData,
    StationId, StationRef, SurveyCredit, FEATURE_ID_STRIDE, MAX_LINES_IN_ID_SPACE, MAX_LINE_ID,
    MAX_STATION_ID, SECTION_PALETTE,
};

use super::command_import::VirtualShotMode;

/// The longest text an import names in its receipt (a line name).
const MAX_RECEIPT_NAME_LEN: usize = crate::core::command::MAX_FILE_NAME_LEN;

/// Everything one import asks for (the arguments of `execute_import_lines`).
pub(super) struct ImportRequest<'a> {
    pub(super) lines: &'a [ImportLineConfig],
    pub(super) closures: &'a [ImportClosure],
    pub(super) file_hash: Option<&'a str>,
    pub(super) line_hashes: &'a [String],
    pub(super) updates: &'a [ExistingShotUpdate],
    pub(super) mode: VirtualShotMode,
    pub(super) surface_elevation: Option<f64>,
    pub(super) on_refused: RefusedLine,
}

/// What an import does when one line cannot be created or placed (#814).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RefusedLine {
    /// Leave it out (or unconnected), name it in the receipt, import the rest.
    SkipAndReport,
    /// Roll the whole import back, as every import did before #814: the DMP
    /// wizard maps the returned line ids onto its configs by position,
    /// records a content hash per selected line and sends overlap edits tied
    /// to particular lines, so a skipped or unconnected line would break all
    /// three.
    FailImport,
}

/// What [`LineBuilder::build`] does with a placement it cannot make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Unplaceable {
    /// Refuse the line (create-and-merge: the placement is the step).
    Refuse,
    /// Build the line as a free piece at the origin, as an `Origin` import
    /// places it, and report why ([`BuiltLine::unplaced`]).
    Float,
}

/// One step of an import as 0.6.0's batch recorded it, for the history
/// preview rows (worded on the project after the import).
#[derive(Debug, Clone)]
pub(super) enum ImportStep {
    /// A line was created.
    CreateLine { line: LineId },
    /// A datum was set on a slot of the new line.
    Datum {
        slot: StationRef,
        gps: GpsCoordinates,
    },
    /// The new line's slot `slot` was placed on the station at `target`.
    Join {
        slot: StationRef,
        target: StationRef,
    },
    /// The new line was moved by a manual offset.
    Move { line: LineId, offset: Coordinates },
}

/// What an import did, for the receipt, the result and the history entry.
#[derive(Debug, Default)]
pub(super) struct ImportOutcome {
    /// The created lines, in config order.
    pub(super) line_ids: Vec<LineId>,
    /// The created lines in import-index order (what the label names).
    pub(super) labelled_lines: Vec<LineId>,
    /// Create-time validation warnings of the imported shots.
    pub(super) warnings: Vec<ValidationWarning>,
    pub(super) shot_count: u32,
    pub(super) closures_created: u32,
    /// Closures skipped (an end not imported) or refused.
    pub(super) closures_dropped: u32,
    /// Overlap updates the doubly-anchored guard skipped.
    pub(super) doubly_anchored_shots: u32,
    /// The first line placed by a zero datum, sanitised for the receipt.
    pub(super) zero_datum_line: Option<String>,
    pub(super) zero_datum_count: u32,
    pub(super) surface_elevation_warning: Option<String>,
    pub(super) steps: Vec<ImportStep>,
    /// Existing (or same-batch) stations a new line was placed on.
    pub(super) junctions: Vec<StationId>,
    pub(super) closure_ids: Vec<ClosureId>,
    pub(super) section_ids: Vec<SectionId>,
    /// One receipt entry per line the import lost or could not connect
    /// (#814): the refused lines first, then the unconnected ones, each in
    /// file order.
    pub(super) line_problems: Vec<String>,
    /// One receipt entry per station that took a letter suffix because its
    /// file name is the name of the station its line joins, in file order.
    pub(super) renamed_stations: Vec<String>,
}

/// Why one line could not be built. The funnel and the create-and-merge
/// recipe word it differently (0.6.0's texts).
#[derive(Debug)]
pub(super) enum LineError {
    /// The line itself is refused (today's create-line refusals), before
    /// anything is added.
    Create(String),
    /// Its placement is refused, before anything is added.
    Placement(String),
    /// A same-batch placement names a line not created yet.
    NotCreatedYet(u32),
    /// The model refused a station or the line after the line's stations were
    /// minted: the project is part-way, so the whole edit is refused (the
    /// pipeline rolls it back).
    Fatal(String),
}

/// Where a new line goes: what its placement becomes once checked.
#[derive(Debug, Clone, Copy)]
enum Plan {
    /// A free piece, its file's first station at the origin plus `offset`.
    Free { offset: Option<Coordinates> },
    /// A datum on the new line's station `station_index`.
    Datum {
        coordinates: GpsCoordinates,
        station_index: u32,
    },
    /// The new line's station is an existing (or same-batch) one.
    Join(Junction),
}

/// The placement's station on the new line, and what it is placed on.
#[derive(Debug, Clone, Copy)]
struct Junction {
    index: usize,
    station: StationId,
    target: StationRef,
}

/// Builds lines into a project, keeping the bookkeeping of one import: the
/// shot uuids in use (so a repeat is re-minted without rebuilding the
/// project's index per line), the next line id, the chains of the lines it
/// made (so a same-batch placement resolves without the topology), the line
/// names taken.
pub(super) struct LineBuilder {
    uuids: HashSet<String>,
    next_line_id: LineId,
    chains: HashMap<LineId, Vec<StationId>>,
    line_names: HashSet<String>,
    dedupe_from: HashMap<String, u64>,
    seed_from: u64,
    /// Existing slots placements name, resolved on the project as it was.
    existing: HashMap<StationRef, StationId>,
}

/// A line as built: its id, chain and what the epilogue needs.
pub(super) struct BuiltLine {
    pub(super) id: LineId,
    pub(super) warnings: Vec<ValidationWarning>,
    pub(super) junction: Option<(usize, StationId, StationRef)>,
    /// Stations of the line whose file name is the junction's and which took
    /// a letter suffix instead: `(file name, name now)`, in chain order.
    pub(super) renamed: Vec<(String, String)>,
    /// Why its placement could not be made, when [`Unplaceable::Float`]
    /// built it as a free piece instead (a [`LineError::Placement`] or
    /// [`LineError::NotCreatedYet`]).
    pub(super) unplaced: Option<LineError>,
}

impl LineBuilder {
    /// A builder over `project`, resolving the existing slots `targets` the
    /// placements name now (before any line is added).
    pub(super) fn new(project: &Project, targets: impl IntoIterator<Item = StationRef>) -> Self {
        let uuids = project
            .lines()
            .iter()
            .flat_map(|l| l.shots.iter().map(|s| s.uuid.clone()))
            .collect();
        let line_names = project
            .lines()
            .iter()
            .filter_map(|l| l.name.as_deref())
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        let topology = project.topology();
        let existing = targets
            .into_iter()
            .filter_map(|slot| topology.station_at(slot).map(|s| (slot, s)))
            .collect();
        Self {
            uuids,
            next_line_id: project.next_line_id(),
            chains: HashMap::new(),
            line_names,
            dedupe_from: HashMap::new(),
            seed_from: 1,
            existing,
        }
    }

    /// The station at slot `index` of a line this builder made.
    fn built_station(&self, line: LineId, index: u32) -> Option<StationId> {
        self.chains
            .get(&line)
            .and_then(|chain| chain.get(index as usize).copied())
    }

    /// Build `config` as a new line (module docs), with `created` mapping the
    /// import indices of the lines built so far to their ids. Datums and
    /// placements are added; the caller settles the anchors once for the
    /// whole import (`ctx` collects the placements set here). A refusal
    /// other than [`LineError::Fatal`] comes before anything is added; a
    /// placement that cannot be made is refused or floated as `unplaceable`
    /// says.
    pub(super) fn build(
        &mut self,
        project: &mut Project,
        config: &ImportLineConfig,
        created: &HashMap<u32, LineId>,
        ctx: &mut EditContext,
        steps: &mut Vec<ImportStep>,
        unplaceable: Unplaceable,
    ) -> Result<BuiltLine, LineError> {
        let id = self.next_line_id;
        let mut shots = prepared_shots(config);
        check_new_line(id, config.name.as_deref(), &shots).map_err(LineError::Create)?;
        let warnings = validation_warnings(project, config.name.as_deref(), &shots);
        let n = shots.len();

        let (plan, unplaced) = match self.plan(project, config, id, n, created) {
            Ok(plan) => (plan, None),
            Err(e) if unplaceable == Unplaceable::Float => (Plan::Free { offset: None }, Some(e)),
            Err(e) => return Err(e),
        };
        let junction = match plan {
            Plan::Join(j) => Some(j),
            Plan::Free { .. } | Plan::Datum { .. } => None,
        };

        let StationNames {
            names,
            junction_alias,
            renamed,
        } = station_names(project, config, n, junction);
        // The alias's retired id is minted before the line's stations, so the
        // id `station_names` counted on is the one it gets.
        let junction_alias = match junction_alias {
            Some(name) => Some((
                project
                    .mint_station_id()
                    .map_err(|e| LineError::Fatal(e.to_string()))?,
                name,
            )),
            None => None,
        };
        let mut chain = Vec::with_capacity(n + 1);
        for (index, name) in names.into_iter().enumerate() {
            match junction {
                Some(j) if j.index == index => chain.push(j.station),
                _ => chain.push(
                    project
                        .mint_station(name)
                        .map_err(|e| LineError::Fatal(e.to_string()))?,
                ),
            }
        }

        let template = legacy_template(config);
        let mut seen = HashSet::new();
        let mut built = Vec::with_capacity(n);
        for (data, &to) in shots.drain(..).zip(chain.iter().skip(1)) {
            let mut data = data;
            data.uuid = self.claim_uuid(id, data.uuid.take(), &mut seen);
            if let Some(template) = &template {
                if data.credit().is_empty() {
                    data.credit = Some(Arc::clone(template));
                }
            }
            let shot = Shot::from_data(0, data, to);
            self.uuids.insert(shot.uuid.clone());
            built.push(shot);
        }
        let (legacy_surveyor, legacy_date) = if built.is_empty() {
            (config.surveyor.clone(), config.date)
        } else {
            (None, None)
        };
        let line = Line {
            id,
            name: Some(self.claim_line_name(config.name.as_deref())),
            color: config.color.clone(),
            direction: config.direction.unwrap_or_default(),
            quality: config.quality,
            azimuth_reference: config.azimuth_reference.map(AzimuthReference::sanitised),
            section_id: None,
            legacy_surveyor,
            legacy_date,
            start: chain.first().copied().unwrap_or_default(),
            shots: built,
            hand_moves: std::collections::BTreeMap::new(),
            next_shot_id: 0,
        };
        project
            .insert_line(line)
            .map_err(|e| LineError::Fatal(e.to_string()))?;
        self.next_line_id = id.saturating_add(1);

        apply_station_comments(project, config, &chain, junction);
        apply_external_ids(project, config, &chain);
        if let (Some(j), Some((alias, name))) = (junction, junction_alias) {
            keep_file_name_as_alias(project, j.station, alias, name, id);
        }
        steps.push(ImportStep::CreateLine { line: id });
        match plan {
            Plan::Free { offset } => {
                // The file's first station: the line's last after a reverse.
                let anchor = if config.reverse {
                    chain.last()
                } else {
                    chain.first()
                };
                if let Some(&station) = anchor {
                    let (x, y) = offset.map_or((0.0, 0.0), |o| (o.x, o.y));
                    project.placements.push(Placement { station, x, y });
                    ctx.placed.push(station);
                }
                if let Some(offset) = offset {
                    steps.push(ImportStep::Move { line: id, offset });
                }
            }
            Plan::Datum {
                coordinates,
                station_index,
            } => {
                if let Some(&station) = chain.get(station_index as usize) {
                    project.push_datum(ZeroDatum {
                        via: Some(id),
                        ..ZeroDatum::new(station, coordinates)
                    });
                }
                steps.push(ImportStep::Datum {
                    slot: StationRef::new(id, station_index),
                    gps: coordinates,
                });
            }
            Plan::Join(j) => {
                steps.push(ImportStep::Join {
                    slot: StationRef::new(id, j.index as u32),
                    target: j.target,
                });
            }
        }
        self.chains.insert(id, chain);
        Ok(BuiltLine {
            id,
            warnings,
            junction: junction.map(|j| (j.index, j.station, j.target)),
            renamed,
            unplaced,
        })
    }

    /// What `config`'s placement becomes for the new line `id` of `n` shots,
    /// or why it cannot be made. Reads the project only.
    fn plan(
        &self,
        project: &Project,
        config: &ImportLineConfig,
        id: LineId,
        n: usize,
        created: &HashMap<u32, LineId>,
    ) -> Result<Plan, LineError> {
        if let Some(junction) = self.junction_of(project, config, id, n, created)? {
            return Ok(Plan::Join(junction));
        }
        match &config.placement {
            ImportPlacement::ZeroDatum {
                coordinates,
                station_index,
            } => {
                if *station_index as usize > n {
                    return Err(LineError::Placement(unknown_slot(id, *station_index)));
                }
                GpsCoordinates::validate(
                    coordinates.latitude,
                    coordinates.longitude,
                    coordinates.depth_reference,
                )
                .map_err(LineError::Placement)?;
                Ok(Plan::Datum {
                    coordinates: *coordinates,
                    station_index: *station_index,
                })
            }
            ImportPlacement::Manual {
                offset: Some(offset),
                ..
            } => {
                if !(offset.x.is_finite() && offset.y.is_finite() && offset.z.is_finite()) {
                    return Err(LineError::Placement(
                        "Move offset must be finite (no NaN/inf)".to_string(),
                    ));
                }
                Ok(Plan::Free {
                    offset: Some(*offset),
                })
            }
            // A joining placement returned above (a junction or a refusal).
            ImportPlacement::Origin
            | ImportPlacement::Manual { offset: None, .. }
            | ImportPlacement::FromExisting { .. }
            | ImportPlacement::FromImporting { .. }
            | ImportPlacement::ManualSnap { .. } => Ok(Plan::Free { offset: None }),
        }
    }

    /// The chain position and station a joining placement puts the new line
    /// `id` (of `n` shots) on.
    fn junction_of(
        &self,
        project: &Project,
        config: &ImportLineConfig,
        id: LineId,
        n: usize,
        created: &HashMap<u32, LineId>,
    ) -> Result<Option<Junction>, LineError> {
        let (index, target, station) = match &config.placement {
            ImportPlacement::FromExisting {
                import_station,
                existing_station,
            } => (
                *import_station,
                *existing_station,
                self.existing.get(existing_station).copied(),
            ),
            ImportPlacement::ManualSnap {
                align_station_index,
                snapped_to_station,
            } => (
                (*align_station_index).min(n as u32),
                *snapped_to_station,
                self.existing.get(snapped_to_station).copied(),
            ),
            ImportPlacement::FromImporting {
                import_station,
                target_line_index,
                target_station_index,
            } => {
                let Some(&line) = created.get(target_line_index) else {
                    return Err(LineError::NotCreatedYet(*target_line_index));
                };
                let target = StationRef::new(line, *target_station_index);
                (
                    *import_station,
                    target,
                    self.built_station(line, *target_station_index),
                )
            }
            ImportPlacement::Origin
            | ImportPlacement::ZeroDatum { .. }
            | ImportPlacement::Manual { .. } => return Ok(None),
        };
        if index as usize > n {
            return Err(LineError::Placement(unknown_slot(id, index)));
        }
        let Some(station) = station.filter(|s| project.stations.contains_key(s)) else {
            return Err(LineError::Placement(unknown_slot(
                target.line_id,
                target.station_index,
            )));
        };
        Ok(Some(Junction {
            index: index as usize,
            station,
            target,
        }))
    }

    /// The uuid a shot of the new line `line` keeps: its own unless empty,
    /// too long, repeated earlier in the line (the first occurrence keeps it;
    /// a fresh one is minted) or held by another line (re-minted as 0.6.0's
    /// `reassign_repeated_shot_uuids` did: the project keeps its ids, the
    /// newcomer yields).
    fn claim_uuid(
        &self,
        line: LineId,
        uuid: Option<String>,
        seen: &mut HashSet<String>,
    ) -> Option<String> {
        let uuid = uuid.filter(|u| !u.is_empty() && u.chars().count() <= MAX_SHOT_UUID_CHARS)?;
        if !seen.insert(uuid.clone()) {
            return None;
        }
        if !self.uuids.contains(&uuid) {
            return Some(uuid);
        }
        let fresh = reminted_shot_uuid_avoiding(line, &uuid, |c| self.uuids.contains(c));
        tracing::debug!(
            line_id = line,
            purpose = "project-unique shot uuids",
            "an imported shot uuid was already in the project; re-minted"
        );
        Some(fresh)
    }

    /// The name the new line takes (0.6.0's normalisation): trimmed, `Line n`
    /// when it has none, `name (k)` when another line holds it.
    fn claim_line_name(&mut self, name: Option<&str>) -> String {
        let taken = &self.line_names;
        let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(name) if !taken.contains(name) => name.to_string(),
            Some(name) => {
                let from = self.dedupe_from.entry(name.to_string()).or_insert(2);
                dedupe_line_name_from(name, from, &|n| taken.contains(n))
            }
            None => seed_line_name_from(&mut self.seed_from, &|n| taken.contains(n)),
        };
        self.line_names.insert(name.clone());
        name
    }
}

/// The config's shots as the line holds them: reversed when the config says
/// so (left and right are never swapped on import), and with the dialog's
/// credit overrides applied (shots sharing one credit share the result).
fn prepared_shots(config: &ImportLineConfig) -> Vec<ShotData> {
    let mut shots = config.shots.clone();
    if config.reverse {
        reverse_shots(&mut shots, false);
    }
    let overrides = CreditEdit {
        set_surveyors: config.surveyors.clone(),
        set_explorers: config.explorers.clone(),
        set_date: config.override_date.map(Some),
        ..Default::default()
    };
    if overrides.kind() != CreditEditKind::Nothing {
        let mut applied: HashMap<*const SurveyCredit, Option<Arc<SurveyCredit>>> = HashMap::new();
        for shot in &mut shots {
            let key = shot.credit.as_ref().map_or(std::ptr::null(), Arc::as_ptr);
            let next = applied
                .entry(key)
                .or_insert_with(|| {
                    let credit = overrides.apply(shot.credit());
                    (!credit.is_empty()).then(|| Arc::new(credit))
                })
                .clone();
            shot.credit = next;
        }
    }
    shots
}

/// The legacy line-level surveyor and date as the credit of every shot that
/// has none (0.6.0's `Line::new` template).
fn legacy_template(config: &ImportLineConfig) -> Option<Arc<SurveyCredit>> {
    SurveyCredit::from_legacy(config.surveyor.as_deref(), config.date)
        .into_storable()
        .stored()
        .map(Arc::new)
}

/// Today's create-line refusals for line `id` with `shots`.
fn check_new_line(id: LineId, name: Option<&str>, shots: &[ShotData]) -> Result<(), String> {
    let max = FEATURE_ID_STRIDE.saturating_sub(1) as usize;
    if shots.len() > max {
        return Err(format!(
            "Line exceeds maximum shots per line (limit = {max})"
        ));
    }
    if id > MAX_LINE_ID {
        return Err(format!(
            "Project has reached the maximum number of lines (limit = \
             {MAX_LINES_IN_ID_SPACE}); shot ids encode as line_id * {FEATURE_ID_STRIDE} + \
             position in 32 bits, so line ids past {MAX_LINE_ID} cannot hold shots"
        ));
    }
    if shots.iter().any(|s| !s.all_finite()) {
        return Err("Line has a shot with a non-finite (NaN/inf) value".to_string());
    }
    // A distance past the model's bound is the line's refusal, in words, as
    // at the create-line door: left to the import's final validation it
    // refused the whole file as "would corrupt the survey structure".
    if let Some(refusal) = shots.iter().find_map(ShotData::measurement_refusal) {
        return Err(refusal);
    }
    let line_name = shot_label_name(name);
    for (i, shot) in shots.iter().enumerate() {
        if shot.length == 0.0 {
            return Err(format!(
                "Shot {line_name}: {i} \u{2192} {} has zero length",
                i + 1
            ));
        }
        if shot.length < 0.0 {
            return Err(format!(
                "Shot {line_name}: {i} \u{2192} {} has negative length: {:.1}m",
                i + 1,
                shot.length
            ));
        }
    }
    Ok(())
}

/// A new line's name in its shot labels (`New line` when it has none).
fn shot_label_name(name: Option<&str>) -> String {
    name.map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("New line")
        .to_string()
}

/// The create-time validation warnings of `shots` against the project's
/// thresholds, named by position (`Main: 4 -> 5`), as 0.6.0's create line.
fn validation_warnings(
    project: &Project,
    name: Option<&str>,
    shots: &[ShotData],
) -> Vec<ValidationWarning> {
    let line_name = shot_label_name(name);
    shots
        .iter()
        .enumerate()
        .flat_map(|(i, shot)| {
            Validator::validate_shot(shot, &project.settings.validation, || {
                format!("{line_name}: {i} \u{2192} {}", i + 1)
            })
        })
        .collect()
}

/// The refusal for a slot that does not exist (the model's `UnknownSlot`).
fn unknown_slot(line: LineId, index: u32) -> String {
    format!("Station {index} of line {line} does not exist")
}

/// The chain position a per-station entry indexed in the FILE's order lands
/// on (mirrored for a reversed import), `None` past the last station.
fn mirrored(config: &ImportLineConfig, index: u32, n: usize) -> Option<usize> {
    let index = index as usize;
    if config.reverse {
        n.checked_sub(index)
    } else {
        (index <= n).then_some(index)
    }
}

/// The names [`station_names`] gives a new line's stations, and what the
/// file named differently at a junction.
struct StationNames {
    /// In chain order.
    names: Vec<String>,
    /// The file's own name for the junction station, when it is not the
    /// junction's and a Station ID is left for it: kept as an alias on the
    /// new line ([`keep_file_name_as_alias`]), so the line's own stations
    /// were named without the junction's name in the way.
    junction_alias: Option<String>,
    /// Stations whose file name is the junction's, `(file name, name now)`.
    renamed: Vec<(String, String)>,
}

/// The names of the new line's stations, in chain order: the file's names
/// (mirrored), the rest by the creation rules around them; with none, the
/// seed rule (each station's index). A junction keeps its own name and a new
/// station that would repeat it yields (letter suffix).
fn station_names(
    project: &Project,
    config: &ImportLineConfig,
    n: usize,
    junction: Option<Junction>,
) -> StationNames {
    let mut names: Vec<Option<String>> = vec![None; n + 1];
    // Whether the file names a station of its own (the junction's name is
    // the junction's, whatever the file calls it).
    let mut named = false;
    for (index, name) in &config.station_names {
        let Some(position) = mirrored(config, *index, n) else {
            continue;
        };
        if let Some(slot) = names.get_mut(position) {
            *slot = Some(name.clone());
            named |= junction.is_none_or(|j| j.index != position);
        }
    }
    // The file's own names, trimmed, before the junction's replaces its slot.
    let file_names: Vec<Option<String>> = names
        .iter()
        .map(|name| {
            name.as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        })
        .collect();
    let fixed = junction.and_then(|j| {
        project
            .stations
            .get(&j.station)
            .map(|s| (j.index, s.name.clone()))
    });
    // The file's own different name for the junction, kept as an alias on
    // this line when a Station ID is left for it (minted first, before the
    // line's stations): the line then knows the junction by it, and the
    // junction's own name need not be unique on the line (invariant 8).
    let junction_alias = fixed
        .as_ref()
        .and_then(|(index, name)| {
            file_names
                .get(*index)
                .cloned()
                .flatten()
                .filter(|file| file != name)
        })
        .filter(|_| project.counters.next_station_id < MAX_STATION_ID);
    if let Some((index, name)) = &fixed {
        if let Some(slot) = names.get_mut(*index) {
            *slot = Some(name.clone());
        }
    }
    if named {
        fill_line_station_names(&mut names);
    }
    settle_line_station_names(&mut names);
    let mut names: Vec<String> = names.into_iter().map(Option::unwrap_or_default).collect();
    let mut renamed = Vec::new();
    if let Some((index, name)) = fixed {
        if junction_alias.is_some() {
            // The line knows the junction by the file's name, so the first
            // station the file itself names as the junction is named keeps
            // that name (the seeds and the settle above still avoided it).
            let first = file_names
                .iter()
                .enumerate()
                .position(|(i, file)| i != index && file.as_deref() == Some(name.as_str()));
            if let Some(slot) = first.and_then(|i| names.get_mut(i)) {
                slot.clone_from(&name);
            }
        } else {
            let mut taken: HashSet<String> = names.iter().cloned().collect();
            taken.insert(name.clone());
            for (i, slot) in names.iter_mut().enumerate() {
                if i != index && *slot == name {
                    let fresh = name_with_free_letter(&name, &|c| taken.contains(c));
                    taken.insert(fresh.clone());
                    *slot = fresh;
                }
            }
        }
        for (i, (file, now)) in file_names.iter().zip(&names).enumerate() {
            if let Some(file) = file.as_ref().filter(|file| **file == name) {
                if i != index && now != file {
                    renamed.push((file.clone(), now.clone()));
                }
            }
        }
        if let Some(slot) = names.get_mut(index) {
            *slot = name;
        }
    }
    StationNames {
        names,
        junction_alias,
        renamed,
    }
}

/// Keep `name`, the file's own name for the existing station `station` the
/// new line `line` joins, as an alias of that station on that line under the
/// retired Station ID `id` minted for it: what Merge Stations keeps of the
/// station it retires (the moving line knows the junction by it), and what
/// the migration keeps of a 0.6.0 merge member. Detaching the line gives it
/// this id and name back.
fn keep_file_name_as_alias(
    project: &mut Project,
    station: StationId,
    id: StationId,
    name: String,
    line: LineId,
) {
    if let Some(station) = project.stations.get_mut(&station) {
        station.aliases.push(StationAlias {
            id,
            name: Some(name),
            line: Some(line),
        });
    }
}

/// The file's station comments on the new line's stations (mirrored); on the
/// junction, joined to the station's own comment (plan 5.1's merge rule).
fn apply_station_comments(
    project: &mut Project,
    config: &ImportLineConfig,
    chain: &[StationId],
    junction: Option<Junction>,
) {
    let n = chain.len().saturating_sub(1);
    for (index, text) in &config.station_comments {
        if text.trim().is_empty() {
            continue;
        }
        let Some(position) = mirrored(config, *index, n) else {
            continue;
        };
        let Some(station) = chain
            .get(position)
            .and_then(|s| project.stations.get_mut(s))
        else {
            continue;
        };
        let on_junction = junction.is_some_and(|j| j.index == position);
        station.comment = match station.comment.take() {
            Some(own) if on_junction && !own.is_empty() && own != *text => {
                Some(format!("{own}\n\n{text}"))
            }
            Some(own) if on_junction && !own.is_empty() => Some(own),
            _ => Some(text.clone()),
        };
    }
}

/// The file's station identities (a Compass or Walls name, an Ariane ID) on
/// the new line's stations (mirrored), as their `external` attribute (plan
/// 10.5). A station that holds one already keeps it: the junction this line
/// joins, or a station an earlier line of the batch named (plan E7's merge
/// rule).
fn apply_external_ids(project: &mut Project, config: &ImportLineConfig, chain: &[StationId]) {
    let n = chain.len().saturating_sub(1);
    for (index, id) in &config.station_external {
        if id.trim().is_empty() {
            continue;
        }
        let Some(position) = mirrored(config, *index, n) else {
            continue;
        };
        let Some(station) = chain
            .get(position)
            .and_then(|s| project.stations.get_mut(s))
        else {
            continue;
        };
        if station.external.is_none() {
            station.external = Some(id.clone());
        }
    }
}

/// The line-id budget (#555) for a batch of `count` lines, checked before
/// any mutation: a file that cannot fit is refused whole.
pub(super) fn check_line_budget(project: &Project, count: usize) -> Result<(), String> {
    let spent = project.next_line_id() as usize;
    if count.saturating_add(spent) > MAX_LINES_IN_ID_SPACE {
        return Err(format!(
            "Import refused: this file would add {count} lines and the project has \
             already used {spent} line ids, which is past the limit of \
             {MAX_LINES_IN_ID_SPACE} lines. Shot ids encode as \
             line_id * {FEATURE_ID_STRIDE} + position in 32 bits, so line \
             ids past {MAX_LINE_ID} cannot hold shots. Nothing was imported."
        ));
    }
    Ok(())
}

/// The existing slots the placements of `lines` name.
pub(super) fn placement_targets(lines: &[ImportLineConfig]) -> Vec<StationRef> {
    lines
        .iter()
        .filter_map(|config| match &config.placement {
            ImportPlacement::FromExisting {
                existing_station, ..
            } => Some(*existing_station),
            ImportPlacement::ManualSnap {
                snapped_to_station, ..
            } => Some(*snapped_to_station),
            _ => None,
        })
        .collect()
}

/// Run the import (module docs) on `project`. Any `Err` leaves the project
/// part-way: the caller restores it (the pipeline's rollback).
pub(super) fn import_lines(
    project: &mut Project,
    request: &ImportRequest<'_>,
) -> Result<ImportOutcome, String> {
    check_line_budget(project, request.lines.len())?;
    let mut outcome = ImportOutcome {
        doubly_anchored_shots: super::import_overlap::apply_overlap_updates(
            project,
            request.updates,
        ),
        ..ImportOutcome::default()
    };

    let before = project.anchor_snapshot();
    let mut builder = LineBuilder::new(project, placement_targets(request.lines));
    let mut ctx = EditContext::none();
    let mut created: HashMap<u32, LineId> = HashMap::new();
    // What one bad line cost (#814): lines the project refused and left
    // out, and lines imported unconnected because their placement could not
    // be made.
    let mut skipped: Vec<String> = Vec::new();
    let mut unconnected: Vec<String> = Vec::new();
    for (idx, config) in request.lines.iter().enumerate() {
        // Sanitised where the file-controlled name is born (0.6.0's F12c):
        // the DMP route never calls `decorate_receipt`.
        let line_name = receipt_line_name(config, idx);
        let built = match builder.build(
            project,
            config,
            &created,
            &mut ctx,
            &mut outcome.steps,
            Unplaceable::Float,
        ) {
            Ok(built) => built,
            Err(LineError::Create(why)) => {
                // Refused before anything was added: nothing to undo.
                tracing::debug!(
                    line = idx,
                    reason = %receipt_reason(&why),
                    purpose = "import every line that can be imported (#814)",
                    "Line refused; skipping it and importing the rest"
                );
                let entry = format!(
                    "Line \"{line_name}\" was not imported: {}",
                    receipt_reason(&why)
                );
                if request.on_refused == RefusedLine::FailImport {
                    return Err(whole_import_undone(&entry));
                }
                skipped.push(entry);
                continue;
            }
            Err(error) => {
                let name = config.name.as_deref().unwrap_or("unnamed");
                return Err(funnel_error(name, &config.placement, error));
            }
        };
        created.insert(config.line_index, built.id);
        outcome.line_ids.push(built.id);
        outcome.shot_count = outcome.shot_count.saturating_add(config.shots.len() as u32);
        outcome.warnings.extend(built.warnings);
        if let Some((_, station, _)) = built.junction {
            outcome.junctions.push(station);
            let joined = project.stations.get(&station).map(|s| s.name.as_str());
            for (file, now) in &built.renamed {
                outcome.renamed_stations.push(format!(
                    "Line \"{line_name}\" joins station \"{}\", so its own station \"{}\" \
                     was renamed \"{}\"",
                    receipt_reason(joined.unwrap_or(file)),
                    receipt_reason(file),
                    receipt_reason(now),
                ));
            }
        }
        if let Some(error) = built.unplaced {
            let entry = unplaced_problem(&line_name, &config.placement, error);
            if request.on_refused == RefusedLine::FailImport {
                return Err(whole_import_undone(&entry));
            }
            unconnected.push(entry);
        } else if matches!(config.placement, ImportPlacement::ZeroDatum { .. }) {
            outcome.zero_datum_count += 1;
            if outcome.zero_datum_line.is_none() {
                outcome.zero_datum_line = Some(line_name);
            }
        }
    }
    // One log line per import, not per refused line: a hostile file
    // controls how many lines it refuses.
    if !skipped.is_empty() || !unconnected.is_empty() {
        tracing::warn!(
            refused = skipped.len(),
            unconnected = unconnected.len(),
            purpose = "import every line that can be imported (#814)",
            "Some lines were left out or imported unconnected"
        );
    }
    // Every line refused: there is no partial import to keep, so this is the
    // one per-line outcome that still fails the import, naming why. The
    // pipeline's rollback also undoes the overlap updates applied above.
    if outcome.line_ids.is_empty() && !request.lines.is_empty() {
        return Err(nothing_imported_message(&skipped));
    }
    project.settle_anchors(&before, &ctx);

    let mut labelled: Vec<(u32, LineId)> = created.iter().map(|(&i, &id)| (i, id)).collect();
    labelled.sort_unstable();
    outcome.labelled_lines = labelled.into_iter().map(|(_, id)| id).collect();

    outcome.section_ids = assign_sections_by_name(project, request.lines, &created)?;
    import_closures(project, request.closures, &builder, &created, &mut outcome);
    // The file hash is kept on a partial import: part of the file did land,
    // so re-importing it unchanged should be flagged (a file fixed to bring
    // the lost line in hashes differently). The line hashes are one per
    // SELECTED line, not per landed one, so a partial import records none:
    // a refused line marked "already imported" would be deselected on the
    // next import of the same file.
    let line_hashes = if skipped.is_empty() {
        request.line_hashes
    } else {
        &[]
    };
    track_hashes(project, request.file_hash, line_hashes);
    skipped.extend(unconnected);
    outcome.line_problems = skipped;
    let scope: BTreeSet<LineId> = outcome.line_ids.iter().copied().collect();
    let changed = super::import_classify::classify_virtual_shots(project, request.mode, &scope);
    if changed > 0 {
        tracing::info!(
            changed,
            mode = ?request.mode,
            purpose = "assign Jump/Reference kinds at import finalize (#27)",
            "Classified imported virtual shots"
        );
    }
    outcome.surface_elevation_warning = merge_surface_elevation(project, request.surface_elevation);
    // Block corruption at the source with 0.6.0's words (the pipeline
    // validates again and would refuse in its own).
    let violations = project.validate();
    if let Some(first) = violations.first() {
        tracing::error!(
            count = violations.len(),
            first = %first,
            purpose = "block corruption at the source",
            "the import would leave the survey invalid; rolled back, nothing imported"
        );
        return Err(
            "Blocked: the imported data would corrupt the survey structure \
                    (dangling references or invalid geometry). Nothing was imported."
                .to_string(),
        );
    }
    Ok(outcome)
}

/// How a line the file names reads in an import receipt (#814): its name
/// from the file, sanitised like every file-controlled receipt string, or its
/// position when it has none.
fn receipt_line_name(config: &ImportLineConfig, idx: usize) -> String {
    sanitize_display_text(
        &config
            .name
            .clone()
            .unwrap_or_else(|| format!("Line {}", idx + 1)),
        MAX_RECEIPT_NAME_LEN,
    )
}

/// Why a line was refused, as a receipt entry carries it. The reason names
/// the line and its stations from the file's own text (`Shot Main: 1 -> 2 has
/// zero length`), so it gets the same filter and bound as the line's name: no
/// bidi override or control character reaches the receipt or the failure
/// message built from it.
fn receipt_reason(reason: &str) -> String {
    sanitize_display_text(reason, MAX_RECEIPT_NAME_LEN)
}

/// The receipt entry for a line that imported but whose `placement` could
/// not be made (`error`, from [`BuiltLine::unplaced`]).
fn unplaced_problem(line_name: &str, placement: &ImportPlacement, error: LineError) -> String {
    let what = match placement {
        ImportPlacement::ZeroDatum { .. } => "its zero datum could not be set",
        ImportPlacement::FromExisting { .. } => {
            "its connection to the existing survey could not be made"
        }
        ImportPlacement::FromImporting { .. } => {
            "its connection to the line it joins could not be made"
        }
        ImportPlacement::Manual { .. } => "it could not be moved to where it was placed",
        ImportPlacement::ManualSnap { .. } => "it could not be snapped to its station",
        ImportPlacement::Origin => "it could not be placed",
    };
    match error {
        // The line it joins was refused above, or comes later in the list.
        LineError::NotCreatedYet(_) => format!(
            "Line \"{line_name}\" was imported on its own: the line it joins was not imported"
        ),
        LineError::Placement(why) | LineError::Create(why) | LineError::Fatal(why)
            if !why.is_empty() =>
        {
            format!(
                "Line \"{line_name}\" was imported, but {what}: {}",
                receipt_reason(&why)
            )
        }
        LineError::Placement(_) | LineError::Create(_) | LineError::Fatal(_) => {
            format!("Line \"{line_name}\" was imported, but {what}")
        }
    }
}

/// The failure of an all-or-nothing import ([`RefusedLine::FailImport`]):
/// the first line problem, after saying that nothing was kept.
fn whole_import_undone(problem: &str) -> String {
    format!(
        "The import was undone and nothing was kept, because one line could not be \
         imported whole. {problem}"
    )
}

/// The failure message when no line of the file could be imported, naming
/// the first few reasons (#814).
fn nothing_imported_message(skipped: &[String]) -> String {
    const SHOWN: usize = 5;
    let mut message = "Nothing was imported: no line in the file could be imported.".to_string();
    for line in skipped.iter().take(SHOWN) {
        message.push(' ');
        message.push_str(line);
        message.push('.');
    }
    if skipped.len() > SHOWN {
        let more = skipped.len() - SHOWN;
        message = format!("{message} ({more} more lines were not imported.)");
    }
    message
}

/// 0.6.0's wording of a line the funnel could not build.
fn funnel_error(name: &str, placement: &ImportPlacement, error: LineError) -> String {
    match error {
        LineError::Create(why) | LineError::Fatal(why) => {
            format!("Failed to create line \"{name}\": {why}")
        }
        LineError::NotCreatedYet(index) => format!(
            "Line \"{name}\" references import line index {index} which has not been created yet"
        ),
        LineError::Placement(why) => match placement {
            ImportPlacement::ZeroDatum { .. } => {
                format!("Failed to set zero datum for line \"{name}\": {why}")
            }
            ImportPlacement::FromImporting { .. } => {
                format!("Failed to merge line \"{name}\" with imported line: {why}")
            }
            ImportPlacement::ManualSnap { .. } => {
                format!("Failed to snap line \"{name}\": {why}")
            }
            ImportPlacement::Manual { .. } => format!("Failed to move line \"{name}\": {why}"),
            ImportPlacement::FromExisting { .. } | ImportPlacement::Origin => {
                format!("Failed to merge line \"{name}\": {why}")
            }
        },
    }
}

/// Auto-grouping (#327): a config naming a Section joins the project's
/// Section of that name (cleaned as every automatic path cleans it), created
/// when there is none, with the next palette colour. Returns the Sections
/// created.
fn assign_sections_by_name(
    project: &mut Project,
    lines: &[ImportLineConfig],
    created: &HashMap<u32, LineId>,
) -> Result<Vec<SectionId>, String> {
    let mut by_name: HashMap<String, SectionId> = project
        .sections
        .iter()
        .map(|s| (s.name.clone(), s.id))
        .collect();
    let mut made = Vec::new();
    let mut assigned: HashMap<LineId, SectionId> = HashMap::new();
    for config in lines {
        let Some(name) = config.cave_section.as_deref().map(clamp_section_name) else {
            continue;
        };
        let Some(&line) = created.get(&config.line_index) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let section = if let Some(&id) = by_name.get(&name) {
            id
        } else {
            let id = project.mint_section_id().map_err(|e| e.to_string())?;
            let color = SECTION_PALETTE[project.sections.len() % SECTION_PALETTE.len()];
            project.sections.push(Section {
                id,
                name: name.clone(),
                color: color.to_string(),
            });
            by_name.insert(name, id);
            made.push(id);
            id
        };
        assigned.insert(line, section);
    }
    if !assigned.is_empty() {
        for line in project.lines_mut() {
            if let Some(&section) = assigned.get(&line.id) {
                line.section_id = Some(section);
            }
        }
    }
    Ok(made)
}

/// The import's loop closures (non-fatal, counted on the receipt): each pair
/// resolved on the lines this import made, refused as a closure is (one
/// station; not connected unless both pieces hold a datum; a second closure
/// for the pair), then judged against the solve's across-one-shot rule in
/// list order, each seeing the ones before it (#737). Strategy `Batch`.
fn import_closures(
    project: &mut Project,
    closures: &[ImportClosure],
    builder: &LineBuilder,
    created: &HashMap<u32, LineId>,
    outcome: &mut ImportOutcome,
) {
    if closures.is_empty() {
        return;
    }
    let topology = project.topology_arc();
    let placement = project.placement_datums();
    let placed = |s: StationId| {
        topology
            .component_of(s)
            .and_then(|c| placement.get(c).copied().flatten())
            .is_some()
    };
    let mut pairs: HashSet<(StationId, StationId)> = project
        .closures
        .iter()
        .map(|c| (c.a.min(c.b), c.a.max(c.b)))
        .collect();
    let mut new_ids = Vec::new();
    for closure in closures {
        let end = |line_index: u32, station_index: u32| {
            let line = *created.get(&line_index)?;
            Some((line, builder.built_station(line, station_index)))
        };
        let (Some((from_line, a)), Some((to_line, b))) = (
            end(closure.from_line_index, closure.from_station_index),
            end(closure.to_line_index, closure.to_station_index),
        ) else {
            tracing::warn!(
                from_line_index = closure.from_line_index,
                to_line_index = closure.to_line_index,
                purpose = "import loop closures",
                "Closure line not found in created lines, skipping"
            );
            outcome.closures_dropped += 1;
            continue;
        };
        let refusal = match (a, b) {
            (Some(a), Some(b)) if a == b => Some("its two stations are one station"),
            (Some(a), Some(b)) if !(topology.connected(a, b) || (placed(a) && placed(b))) => {
                Some("its stations are not connected")
            }
            (Some(a), Some(b)) if pairs.contains(&(a.min(b), a.max(b))) => {
                Some("a closure already joins these stations")
            }
            (Some(_), Some(_)) => None,
            _ => Some("a station does not exist"),
        };
        if let Some(why) = refusal {
            tracing::warn!(
                from_line_id = from_line,
                to_line_id = to_line,
                why,
                purpose = "import loop closures",
                "Failed to create loop closure from import, skipping"
            );
            outcome.closures_dropped += 1;
            continue;
        }
        let (Some(a), Some(b)) = (a, b) else {
            continue;
        };
        pairs.insert((a.min(b), a.max(b)));
        new_ids.push(project.push_closure(
            a,
            Some(from_line),
            b,
            Some(to_line),
            ClosureStrategy::Batch,
        ));
    }
    if new_ids.is_empty() {
        return;
    }
    let across: HashSet<ClosureId> = project
        .closures_across_one_shot()
        .into_iter()
        .map(|c| c.closure_id)
        .filter(|id| new_ids.contains(id))
        .collect();
    if !across.is_empty() {
        tracing::warn!(
            count = across.len(),
            purpose = "import loop closures",
            "Imported closures would join the two ends of one shot (#737); skipped"
        );
        project.closures.retain(|c| !across.contains(&c.id));
        outcome.closures_dropped += across.len() as u32;
    }
    for id in new_ids.into_iter().filter(|id| !across.contains(id)) {
        tracing::debug!(
            closure_id = id,
            purpose = "import loop closures",
            "Created loop closure from import"
        );
        outcome.closures_created += 1;
        outcome.closure_ids.push(id);
    }
}

/// Record the file hash and the line content hashes (duplicates skipped), so
/// a re-import is recognised and one undo takes them back with the lines.
pub(super) fn track_hashes(project: &mut Project, file_hash: Option<&str>, line_hashes: &[String]) {
    if let Some(hash) = file_hash {
        if !project.imported_file_hashes.iter().any(|h| h == hash) {
            project.imported_file_hashes.push(hash.to_string());
        }
    }
    if line_hashes.is_empty() {
        return;
    }
    let mut held: HashSet<String> = project.imported_line_hashes.iter().cloned().collect();
    for hash in line_hashes {
        if held.insert(hash.clone()) {
            project.imported_line_hashes.push(hash.clone());
        }
    }
}

/// An Ariane file's entrance elevation (#431), merged without clobbering: an
/// unset project takes it; a different existing value is kept and the
/// mismatch is a receipt warning naming both (0.005 m: the message's own
/// precision, so a unit round trip never warns).
fn merge_surface_elevation(project: &mut Project, incoming: Option<f64>) -> Option<String> {
    let incoming = incoming.filter(|v| v.is_finite())?;
    match project.settings.surface_elevation {
        None => {
            project.settings.surface_elevation = Some(incoming);
            None
        }
        Some(existing) if (existing - incoming).abs() > 0.005 => Some(format!(
            "Imported file's entrance elevation ({incoming:.2} m) differs from \
             the project's existing value ({existing:.2} m); kept {existing:.2} m"
        )),
        Some(_) => None,
    }
}

/// How many sub-command rows 0.6.0's batch preview listed before counting
/// the rest.
const BATCH_PREVIEW_ROWS: usize = 8;

/// The longest label or preview text a history entry is given (an entry
/// refuses texts past `EntryLimits::max_text_chars`; file-controlled line
/// names could reach it).
pub(super) const MAX_HISTORY_TEXT: usize = 512;

/// 0.6.0's batch preview of an import, worded on the project after it: the
/// first [`BATCH_PREVIEW_ROWS`] placeholder sub-commands (`Create line`, `Set
/// zero datum at`, `Merge stations`, `Move line`) with their details, then a
/// count of the rest.
pub(super) fn import_preview(steps: &[ImportStep], project: &Project) -> RecordedPreview {
    let names = super::labels::NameBook::of(project);
    let station = |slot: StationRef| {
        names
            .station(slot)
            .unwrap_or_else(|| "a station".to_string())
    };
    let rows = steps
        .iter()
        .take(BATCH_PREVIEW_ROWS)
        .map(|step| {
            let (cmd, detail) = match step {
                ImportStep::CreateLine { line } => {
                    let name = project.line(*line).and_then(|l| l.name.clone());
                    let detail = name
                        .as_ref()
                        .map(|n| format!("Name: {n}"))
                        .unwrap_or_default();
                    (
                        Command::CreateLine {
                            name,
                            surveyor: None,
                            date: None,
                            color: String::new(),
                            direction: None,
                            shots: Vec::new(),
                            cave_section_id: None,
                        },
                        detail,
                    )
                }
                ImportStep::Datum { slot, gps } => (
                    Command::SetZeroDatum {
                        station_ref: *slot,
                        coordinates: *gps,
                        accuracy_m: None,
                        entrance: None,
                    },
                    format!("Lat: {:.6}, Lon: {:.6}", gps.latitude, gps.longitude),
                ),
                ImportStep::Join { slot, target } => (
                    Command::MergeStations {
                        station_a: *slot,
                        station_b: *target,
                    },
                    format!("{} \u{2194} {}", station(*slot), station(*target)),
                ),
                ImportStep::Move { line, offset } => {
                    let mut parts = Vec::new();
                    if let Some(name) = names.line(*line) {
                        parts.push(name);
                    }
                    parts.push(offset.display_offset());
                    (
                        Command::MoveLine {
                            line_id: *line,
                            offset: *offset,
                        },
                        parts.join(", "),
                    )
                }
            };
            PreviewRow {
                field: history_text(&super::labels::command_label(project, &cmd)),
                old: String::new(),
                new: history_text(&detail),
            }
        })
        .collect();
    RecordedPreview {
        rows,
        more: steps.len().saturating_sub(BATCH_PREVIEW_ROWS) as u64,
    }
}

/// `text` fit for a history entry (control characters dropped, clamped).
pub(super) fn history_text(text: &str) -> String {
    sanitize_display_text(text, MAX_HISTORY_TEXT)
}
