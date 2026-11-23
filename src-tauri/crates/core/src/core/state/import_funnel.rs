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
//!   station it was placed on, so the target is **kept** (RS 4.1) and the new
//!   line shows the target's shown identity (its kept station; for a loop end
//!   a closure archives, the kept end's). For `FromImporting` the target is a
//!   station of a line this import made earlier, and that line's station is
//!   the one kept.
//! - The file's own station there is **archived** when the file gives it a
//!   name, a comment or an external id that differs from the target
//!   station's own: a record of the junction `{a fresh Station ID, the
//!   file's name (the target's when the file gives none), its comment, its
//!   external id, lines: [the new line]}`, so Detach Line from Station gives
//!   the line the file's station back. Comments are never joined: the target
//!   keeps its own comment and external id. The record's id is minted after
//!   the line's stations; when no Station ID is left for it the record is
//!   dropped (never the line), and the receipt says what was not kept.
//! - The new line's own stations keep the file's names (settled around the
//!   file's own name for the junction, so the file's numbering stays); one
//!   whose name is the junction's shown name takes a letter suffix (rule L:
//!   it is shown on this line only, the junction on two or more), and the
//!   receipt says so.
//! - The file's loop closures archive an end (RS 9), decided in list order
//!   once the lines are classified: rule K's end is kept (node candidates:
//!   the end whose shown identity is shown on the longest line) when the
//!   closure may keep it (invariants C1, C2), else the other end when it
//!   may, else the closure archives nothing. The names a kept end then
//!   repeats on a line are lettered by rule L.
//! - The receipt names every file station archived at a join and every
//!   closure archive (notes, `ImportReceipt::info`), and as warnings every
//!   file station not kept, every station the project held that a closure
//!   now shows as another, and every letter.
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
    ArchivedStation, ClosureEnd, ClosureId, ClosureStrategy, EditContext, Line, Placement, Project,
    Shot, ZeroDatum, MAX_SHOT_UUID_CHARS,
};
use crate::models::{
    clamp_section_name, reminted_shot_uuid_avoiding, reverse_shots, sanitize_display_text,
    AzimuthReference, Coordinates, CreditEdit, CreditEditKind, ExistingShotUpdate, GpsCoordinates,
    ImportClosure, ImportLineConfig, ImportPlacement, LineId, Section, SectionId, ShotData,
    StationId, StationRef, SurveyCredit, FEATURE_ID_STRIDE, MAX_LINES_IN_ID_SPACE, MAX_LINE_ID,
    SECTION_PALETTE,
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
    /// Receipt notes (`ImportReceipt::info`) on identities (RS 4.1, 9): each
    /// file station archived at a join, then each closure's archive (or
    /// that it keeps both names), in file order.
    pub(super) identity_notes: Vec<String>,
    /// Receipt warnings on identities: a file station a join could not keep,
    /// a project station a closure now shows as another, and each rule-L
    /// letter the closures made.
    pub(super) identity_warnings: Vec<String>,
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
    /// What became of the file's own station at the junction, when the file
    /// gives it a name, comment or external id the kept station does not
    /// have (RS 4.1).
    pub(super) file_station: Option<FileStation>,
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
            junction_file_name,
            renamed,
        } = station_names(project, config, n, junction);
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
        apply_external_ids(project, config, &chain, junction);
        // After the line's stations: a record the Station IDs cannot hold is
        // dropped (and reported), never the line.
        let file_station = junction.and_then(|j| {
            let file = FileIdentity {
                name: junction_file_name,
                comment: file_comment_at(config, n, j.index),
                external: file_external_at(config, n, j.index),
            };
            archive_file_station(project, j.station, file, id)
        });
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
            file_station,
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

/// The names [`station_names`] gives a new line's stations, and the file's
/// own name for the station it joins.
struct StationNames {
    /// In chain order; the junction's slot holds the name the line shows it
    /// by (the target's shown identity, RS 4.1).
    names: Vec<String>,
    /// The file's own name for the junction station, as settled with the
    /// line's other names (`None` when the file gives it none, or there is
    /// no junction): the name of the file's station a record keeps
    /// ([`archive_file_station`]).
    junction_file_name: Option<String>,
    /// Stations whose file name is the junction's shown name, `(file name,
    /// name now)`.
    renamed: Vec<(String, String)>,
}

/// The names of the new line's stations, in chain order: the file's names
/// (mirrored), the rest by the creation rules around them; with none, the
/// seed rule (each station's index). The names are settled around the file's
/// own name for the junction (so the file's numbering stays); the line then
/// shows the junction by the target's shown identity (RS 4.1: the target is
/// kept), and a new station that would repeat that name takes a letter
/// suffix (rule L: it is shown on this line only, the junction on two or
/// more lines).
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
    // The name the line shows the junction by: the target's shown identity
    // (its own, or a closure's kept end's, RS 5.1).
    let shown = junction.and_then(|j| {
        project
            .shown_name(j.station)
            .map(|name| (j.index, name.to_string()))
    });
    let file_junction = shown
        .as_ref()
        .and_then(|(index, _)| file_names.get(*index).cloned().flatten());
    if let Some((index, name)) = &shown {
        if let Some(slot) = names.get_mut(*index) {
            *slot = Some(file_junction.clone().unwrap_or_else(|| name.clone()));
        }
    }
    if named {
        fill_line_station_names(&mut names);
    }
    settle_line_station_names(&mut names);
    let mut names: Vec<String> = names.into_iter().map(Option::unwrap_or_default).collect();
    let mut renamed = Vec::new();
    let mut junction_file_name = None;
    if let Some((index, name)) = shown {
        if file_junction.is_some() {
            // The file's name for the junction, as the settle left it.
            junction_file_name = names.get(index).cloned();
        }
        // Rule L: a new station named like the junction takes the letter.
        // The junction's own slot is replaced below, so its settled name
        // takes nothing.
        let mut taken: HashSet<String> = names
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != index)
            .map(|(_, n)| n.clone())
            .collect();
        taken.insert(name.clone());
        for (i, slot) in names.iter_mut().enumerate() {
            if i != index && *slot == name {
                let fresh = name_with_free_letter(&name, &|c| taken.contains(c));
                taken.insert(fresh.clone());
                *slot = fresh;
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
        junction_file_name,
        renamed,
    }
}

/// The identity the file gives its own station at a junction.
#[derive(Debug, Clone, Default)]
struct FileIdentity {
    name: Option<String>,
    comment: Option<String>,
    external: Option<String>,
}

/// What became of the file's own station at a junction (RS 4.1), for the
/// receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FileStation {
    /// Archived in the junction: a record naming the new line.
    Archived {
        /// The record's Station ID and name.
        id: StationId,
        name: String,
        /// Whether the file gave the station a name of its own (else the
        /// record carries the kept station's).
        named: bool,
        /// Whether the record carries the file's comment.
        comment: bool,
    },
    /// Not kept: no Station ID was left for its record.
    Dropped { name: Option<String>, comment: bool },
}

/// The comment the file gives the station at chain position `position` of
/// the new line of `n` shots: the last non-blank one, as
/// [`apply_station_comments`] lets the last one win elsewhere.
fn file_comment_at(config: &ImportLineConfig, n: usize, position: usize) -> Option<String> {
    config
        .station_comments
        .iter()
        .filter(|(index, text)| {
            !text.trim().is_empty() && mirrored(config, *index, n) == Some(position)
        })
        .map(|(_, text)| text.clone())
        .next_back()
}

/// The external id the file gives the station at chain position `position`:
/// the first non-blank one, as [`apply_external_ids`] keeps the first.
fn file_external_at(config: &ImportLineConfig, n: usize, position: usize) -> Option<String> {
    config
        .station_external
        .iter()
        .find(|(index, id)| !id.trim().is_empty() && mirrored(config, *index, n) == Some(position))
        .map(|(_, id)| id.clone())
}

/// RS 4.1's import join: the target `station` is kept (its identity, comment
/// and external id unchanged, comments never joined); the file's own station
/// there is archived as a record of the junction naming the new line `line`
/// when the file gives it a name, a comment or an external id that differs
/// from the target's own, so Detach Line from Station gives the line the
/// file's station back. The record takes a fresh Station ID; when none is
/// left it is dropped and the receipt says what was lost. `None` when the
/// file's station is the target's.
fn archive_file_station(
    project: &mut Project,
    station: StationId,
    file: FileIdentity,
    line: LineId,
) -> Option<FileStation> {
    let target = project.stations.get(&station)?;
    let differs = file.name.as_ref().is_some_and(|n| *n != target.name)
        || file
            .comment
            .as_ref()
            .is_some_and(|c| target.comment.as_ref() != Some(c))
        || file
            .external
            .as_ref()
            .is_some_and(|e| target.external.as_ref() != Some(e));
    if !differs {
        return None;
    }
    let named = file.name.is_some();
    let name = file.name.unwrap_or_else(|| target.name.clone());
    let comment = file.comment.is_some();
    let Ok(id) = project.mint_station_id() else {
        tracing::warn!(
            station,
            line_id = line,
            purpose = "keep the file's station at an import join (RS 4.1)",
            "No Station ID left for the file's own station at a junction; not kept"
        );
        return Some(FileStation::Dropped {
            name: named.then_some(name),
            comment,
        });
    };
    let target = project.stations.get_mut(&station)?;
    target.archived.push(ArchivedStation {
        id,
        name: name.clone(),
        comment: file.comment,
        external: file.external,
        lines: vec![line],
    });
    Some(FileStation::Archived {
        id,
        name,
        named,
        comment,
    })
}

/// The file's station comments on the new line's own stations (mirrored).
/// The junction keeps its own comment: the file's comment for it is its
/// archived station's ([`archive_file_station`], RS 4.1: comments are never
/// joined).
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
        if junction.is_some_and(|j| j.index == position) {
            continue;
        }
        if let Some(station) = chain
            .get(position)
            .and_then(|s| project.stations.get_mut(s))
        {
            station.comment = Some(text.clone());
        }
    }
}

/// The file's station identities (a Compass or Walls name, an Ariane ID) on
/// the new line's own stations (mirrored), as their `external` attribute
/// (plan 10.5); a repeated index keeps the first. The junction keeps its own
/// (the file's is its archived station's, RS 4.1).
fn apply_external_ids(
    project: &mut Project,
    config: &ImportLineConfig,
    chain: &[StationId],
    junction: Option<Junction>,
) {
    let n = chain.len().saturating_sub(1);
    for (index, id) in &config.station_external {
        if id.trim().is_empty() {
            continue;
        }
        let Some(position) = mirrored(config, *index, n) else {
            continue;
        };
        if junction.is_some_and(|j| j.index == position) {
            continue;
        }
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
    // Rule L's "new on the line" is judged against the project before the
    // import; only the closures' archives can repeat a name across lines
    // (the joins letter their own lines), so only a file with closures
    // pays for the copy.
    let identities_before = (!request.closures.is_empty()).then(|| project.clone());
    let first_new_station = project.counters.next_station_id;
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
        if let Some((_, station, target)) = built.junction {
            outcome.junctions.push(station);
            let joined = project.shown_name(station);
            for (file, now) in &built.renamed {
                outcome.renamed_stations.push(format!(
                    "Line \"{line_name}\" joins station \"{}\", so its own station \"{}\" \
                     was renamed \"{}\"",
                    receipt_reason(joined.unwrap_or(file)),
                    receipt_reason(file),
                    receipt_reason(now),
                ));
            }
            if let Some(file_station) = &built.file_station {
                let joined = slot_label(project, target.line_id, station);
                match file_station_entry(&line_name, &joined, file_station) {
                    Entry::Note(note) => outcome.identity_notes.push(note),
                    Entry::Warning(warning) => outcome.identity_warnings.push(warning),
                }
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
    // After the classification: rule K reads the lines' measured lengths,
    // which count Jumps and not References.
    if let Some(before) = &identities_before {
        archive_import_closures(project, before, first_new_station, &mut outcome);
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

/// A receipt entry on identities: a note (`ImportReceipt::info`) or a
/// warning.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    Note(String),
    Warning(String),
}

/// `Line / Station` as a receipt names a station: the line's name and
/// `station`, both sanitised like every file-controlled receipt string.
fn station_label(project: &Project, line: Option<LineId>, station: &str) -> String {
    let line = line
        .and_then(|id| project.line(id))
        .and_then(|l| l.name.as_deref())
        .unwrap_or("?");
    format!("{} / {}", receipt_reason(line), receipt_reason(station))
}

/// The slot of `line` at `node` as a receipt names it: the line, and the
/// name the slot shows (its node's shown identity).
fn slot_label(project: &Project, line: LineId, node: StationId) -> String {
    station_label(
        project,
        Some(line),
        project.shown_name(node).unwrap_or_default(),
    )
}

/// The receipt entry for what became of the file's own station where line
/// `line_name` joins the station `joined` (RS 4.1): archived is a note, not
/// kept a warning.
fn file_station_entry(line_name: &str, joined: &str, file: &FileStation) -> Entry {
    let lead = format!("Line \"{line_name}\" joins station \"{joined}\" and shows it there");
    match file {
        FileStation::Archived {
            name,
            named,
            comment,
            ..
        } => {
            let station = if *named {
                format!("the file's own station \"{}\" there", receipt_reason(name))
            } else {
                "the file's own station there".to_string()
            };
            let with = if *comment { ", with its comment," } else { "" };
            Entry::Note(format!(
                "{lead}; {station}{with} is archived: Detach Line from Station brings it back"
            ))
        }
        FileStation::Dropped { name, comment } => {
            let station = match name {
                Some(name) => format!("the file's own station \"{}\" there", receipt_reason(name)),
                None => "the file's own station there".to_string(),
            };
            let with = if *comment { ", with its comment," } else { "" };
            Entry::Warning(format!(
                "{lead}; {station}{with} was not kept: no Station ID is left for it"
            ))
        }
    }
}

/// RS 9: each loop closure this import made archives an end, decided in list
/// order (each seeing the archives before it) once the lines are classified:
/// rule K's end is kept when a closure may keep it, else the other end when
/// that one may be kept, else the closure archives nothing
/// ([`import_closure_keep`]). The names a kept end then repeats on a line
/// are settled by rule L against `before` (the project before the import).
/// The receipt names every archive (a note; a warning when the hidden end
/// is a station the project held before the import, `first_new_station`
/// being the first Station ID the import minted), every closure that keeps
/// both names, and every letter.
fn archive_import_closures(
    project: &mut Project,
    before: &Project,
    first_new_station: StationId,
    outcome: &mut ImportOutcome,
) {
    if outcome.closure_ids.is_empty() {
        return;
    }
    let made: HashSet<ClosureId> = outcome.closure_ids.iter().copied().collect();
    for position in 0..project.closures.len() {
        let closure = project.closures[position];
        if !made.contains(&closure.id) {
            continue;
        }
        let archived = import_closure_keep(project, closure.a, closure.b).map(|keep| {
            if keep == closure.a {
                ClosureEnd::B
            } else {
                ClosureEnd::A
            }
        });
        project.closures[position].archived = archived;
    }
    // The keeps above already hold C1 and C2; the normalisation every
    // upgrade and merge runs is the safety net.
    let unarchived = project.normalise_closure_archives();
    if !unarchived.is_empty() {
        tracing::warn!(
            count = unarchived.len(),
            purpose = "archive one end of each imported loop closure (RS 9)",
            "The closure normalisation dropped imported archives"
        );
    }
    let lettered = project.letter_collisions(before);

    for closure in project.closures.iter().filter(|c| made.contains(&c.id)) {
        // An end is named by its identity's home line (the line the user
        // knows it by), else the line the file attached the closure on.
        let line_of = |identity: StationId, end: StationId| {
            let via = if end == closure.a {
                closure.a_via
            } else {
                closure.b_via
            };
            project.home_line(identity).or(via)
        };
        let Some((archived, kept)) = closure.archive() else {
            // Each end as the user sees it (an end another closure archives
            // shows its kept end).
            let end = |node: StationId| {
                let shown = project.shown(node);
                let name = project.shown_name(node).unwrap_or_default();
                station_label(project, line_of(shown, node), name)
            };
            outcome.identity_notes.push(format!(
                "Loop closure between \"{}\" and \"{}\" shows both names: neither end can be \
                 shown as the other",
                end(closure.a),
                end(closure.b)
            ));
            continue;
        };
        let own = project.station(archived).map_or("", |s| s.name.as_str());
        let hidden = station_label(project, line_of(archived, archived), own);
        let shown = station_label(
            project,
            line_of(project.shown(kept), kept),
            project.shown_name(kept).unwrap_or_default(),
        );
        if archived < first_new_station {
            outcome.identity_warnings.push(format!(
                "Loop closure: station \"{hidden}\", already in the project, is now shown as \
                 \"{shown}\": Remove Closure brings its name back"
            ));
        } else {
            outcome.identity_notes.push(format!(
                "Loop closure: station \"{hidden}\" is shown as \"{shown}\": Remove Closure \
                 brings its own name back"
            ));
        }
    }
    for letter in lettered {
        let station = station_label(project, project.home_line(letter.station), &letter.from);
        let line = project
            .line(letter.line)
            .and_then(|l| l.name.as_deref())
            .unwrap_or("?");
        outcome.identity_warnings.push(format!(
            "Station \"{station}\" was renamed \"{}\": after a loop closure, line \"{}\" shows \
             another station named \"{}\"",
            receipt_reason(&letter.to),
            receipt_reason(line),
            receipt_reason(&letter.from),
        ));
    }
}

/// The end a loop closure an import made between `a` and `b` keeps (RS 9):
/// rule K's (node candidates, RS 2: the end whose shown identity is shown on
/// the longest line) when the closure may keep it, else the other end when
/// it may, else `None` (the closure archives nothing).
fn import_closure_keep(project: &Project, a: StationId, b: StationId) -> Option<StationId> {
    let first = project.rule_k_nodes(&[a, b])?;
    let other = if first == a { b } else { a };
    [first, other]
        .into_iter()
        .find(|&keep| closure_keep_allowed(project, a, b, keep))
}

/// Whether a closure between `a` and `b` may keep `keep` (RS 5.2): `keep` is
/// one of the two, the end it archives is archived by no other closure (C1),
/// and `keep` does not already show that end (C2). RS-M2 adds this rule to
/// the model as `Project::closure_keep_allowed`; the integrator calls that
/// one instead.
fn closure_keep_allowed(project: &Project, a: StationId, b: StationId, keep: StationId) -> bool {
    let archived = if keep == a {
        b
    } else if keep == b {
        a
    } else {
        return false;
    };
    a != b && !project.is_closure_archived(archived) && project.shown(keep) != archived
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
                        keep: None,
                        renames: None,
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
