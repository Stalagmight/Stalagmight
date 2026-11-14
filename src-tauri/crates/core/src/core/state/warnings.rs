use anyhow::Result;
use std::collections::HashSet;

use crate::core::{Validator, WarningSeverity};
use crate::models::{
    DatumGap, Project, ProjectWarning, SectionId, ShotData, ShotId, DATUM_GPS_GAP,
};
use crate::persistence::Database;

use super::AppState;

/// Which section each live shot uuid is in now, for an ignored-warning row
/// (keyed by `(section_id, shot_uuid)`) whose stored section no longer holds
/// its shot. Split Section Here, delete-and-split and Combine Sections all move
/// shots to another section with their uuid, and the ignore has to follow the
/// shot instead of silently lapsing.
///
/// Only a uuid exactly one live shot holds has a home: shot uuids are unique
/// across the live project, and a repeat (a legacy file) stays where its row
/// says. Shared by the read side (`compute_project_*`) and the save carry-over
/// (`project_io`), so an ignore that follows its shot in the session is also
/// re-keyed by the next save.
pub(super) struct ShotHomes<'p> {
    homes: std::collections::HashMap<&'p str, Option<SectionId>>,
}

impl<'p> ShotHomes<'p> {
    pub(super) fn of(project: &'p Project) -> Self {
        let mut homes: std::collections::HashMap<&'p str, Option<SectionId>> =
            std::collections::HashMap::new();
        for section in &project.sections {
            for shot in &section.shots {
                if let Some(uuid) = shot.uuid.as_deref() {
                    homes
                        .entry(uuid)
                        .and_modify(|home| *home = None)
                        .or_insert(Some(section.id));
                }
            }
        }
        Self { homes }
    }

    /// The section `uuid` moved to, when `stored` (the row's section) no longer
    /// holds it and exactly one other section does.
    pub(super) fn moved_to(&self, stored: SectionId, uuid: &str) -> Option<SectionId> {
        match self.homes.get(uuid) {
            Some(Some(home)) if *home != stored => Some(*home),
            _ => None,
        }
    }
}

impl AppState {
    /// Compute project warnings from the given project.
    /// Used by both get_project_warnings command and load_project_full for validation phase.
    ///
    /// If `db` is provided, uses it for resolved warnings lookup (for pre-finalization loading).
    /// Otherwise falls back to self.database (normal operation after project is loaded).
    pub fn compute_project_warnings(
        &self,
        project: &Project,
        db: Option<&Database>,
    ) -> Vec<ProjectWarning> {
        // Load resolved (ignored) warnings. Suppression is keyed by the shot's
        // stable UUID (issue #57): an ignore stays bound to the same shot even
        // after inserts/deletes/reorders renumber `shot_id`. Pre-v8 rows that
        // carry no UUID fall back to the legacy (section_id, shot_id) position.
        //
        // [#517] Read side and save side use the SAME verbatim loader. The
        // strict one names every current column, so on a file whose schema was
        // never migrated (a forged `PRAGMA user_version` over pre-v8 tables) it
        // fails and `unwrap_or_default` shows every ignored warning as
        // unresolved - while the save quietly carries rows the user cannot see,
        // and a re-Ignore then fails against the missing `shot_uuid` column.
        let resolved = if let Some(db) = db {
            // Use provided database reference (pre-finalization)
            db.load_resolved_warnings_verbatim().unwrap_or_default()
        } else {
            // Fall back to self.database (post-finalization), plus the ignores an
            // "Add .stalx" carried in that no save has written yet (#674). Only
            // here: the `db` branch above reads a project that is not in the
            // slot yet, which the pending rows do not belong to.
            let mut rows = self.load_resolved_warnings().unwrap_or_default();
            rows.extend(self.pending_ignores());
            rows
        };
        // Key the lookup sets by borrowed slices into `resolved` so the
        // per-warning probe below allocates nothing. `resolved` outlives this
        // closure, so the &str borrows stay valid.
        // Key UUID-suppression by (section_id, uuid). Shot UUIDs are unique across
        // the live project, but a STORED row can name an id that two sections held
        // before the load renamed one copy: only its section says which shot it
        // meant (issue #57 / Codex). section_id is stable per shot (it doesn't
        // change when shot_id renumbers), so this keeps the position-independence
        // the UUID keying was added for.
        let mut resolved_by_uuid: std::collections::HashMap<(u32, &str), HashSet<&str>> =
            std::collections::HashMap::new();
        let mut resolved_by_position: HashSet<(u32, u32, &str)> = HashSet::new();
        // A row stored against an id the load renamed (`reminted_shot_uuid`)
        // still suppresses its shot until the next save re-keys it. Only when
        // this load renamed anything, and only for a row whose id no live shot
        // of its section holds, is the derivation paid for; owned keys, since
        // the derived id exists nowhere else to borrow from.
        let derive_aliases = self.load_renamed_shots();
        let live_ids: HashSet<(u32, &str)> = if derive_aliases {
            project
                .sections
                .iter()
                .flat_map(|s| {
                    s.shots
                        .iter()
                        .filter_map(move |shot| shot.uuid.as_deref().map(|u| (s.id, u)))
                })
                .collect()
        } else {
            HashSet::new()
        };
        let mut resolved_by_renamed: std::collections::HashMap<(u32, String), HashSet<&str>> =
            std::collections::HashMap::new();
        // A shot that a split or a combine moved into another section keeps its
        // uuid, so its ignore follows it there (see `ShotHomes`).
        let homes = ShotHomes::of(project);
        for r in &resolved {
            match &r.shot_uuid {
                Some(uuid) => {
                    resolved_by_uuid
                        .entry((r.section_id, uuid.as_str()))
                        .or_default()
                        .insert(r.warning_code.as_str());
                    let renamed =
                        derive_aliases && !live_ids.contains(&(r.section_id, uuid.as_str())) && {
                            let alias = crate::models::reminted_shot_uuid(r.section_id, uuid);
                            let held = live_ids.contains(&(r.section_id, alias.as_str()));
                            resolved_by_renamed
                                .entry((r.section_id, alias))
                                .or_default()
                                .insert(r.warning_code.as_str());
                            held
                        };
                    if !renamed {
                        if let Some(home) = homes.moved_to(r.section_id, uuid) {
                            resolved_by_uuid
                                .entry((home, uuid.as_str()))
                                .or_default()
                                .insert(r.warning_code.as_str());
                        }
                    }
                }
                None => {
                    resolved_by_position.insert((r.section_id, r.shot_id, r.warning_code.as_str()));
                }
            }
        }
        // A warning is suppressed if its shot's UUID was ignored, or (legacy
        // only) if its (section_id, shot_id) position was ignored. The resolve
        // command now REFUSES a no-UUID ignore (commands/validation.rs), and
        // load-time backfill upgrades legacy NULL-uuid rows to UUID keying when
        // the shot still maps to a live position (issue #57). The position
        // fallback is what lets a pre-v8 file's ignores survive the first load.
        // A residual edge remains (issue #57 finding #8): a legacy NULL-uuid row
        // whose position is dead at load - the ignored shot was already gone, so
        // the backfill cannot upgrade it (or, rarely, the backfill DB write
        // failed) - can later suppress an unrelated shot if an in-session insert
        // renumbers a different shot onto that stale id. It only affects pre-v8
        // NULL-uuid rows (every fresh ignore is UUID-keyed) and clears once the
        // position is upgraded. We keep the fallback because dropping it would
        // silently lose every legacy ignore for the still-live shots it protects.
        let is_resolved_for =
            |shot_uuid: &Option<String>, section_id: u32, shot_id: u32, code: &str| -> bool {
                if let Some(uuid) = shot_uuid {
                    if resolved_by_uuid
                        .get(&(section_id, uuid.as_str()))
                        .is_some_and(|codes| codes.contains(code))
                    {
                        return true;
                    }
                    if !resolved_by_renamed.is_empty()
                        && resolved_by_renamed
                            .get(&(section_id, uuid.clone()))
                            .is_some_and(|codes| codes.contains(code))
                    {
                        return true;
                    }
                }
                resolved_by_position.contains(&(section_id, shot_id, code))
            };

        // Per-project validation thresholds (#57): stored in the .stalx, seeded
        // from the global config at project creation, authoritative thereafter.
        let mut all_warnings = Vec::new();

        for section in &project.sections {
            for (shot_index, shot) in section.shots.iter().enumerate() {
                // Convert Shot to ShotData for validation
                let shot_data = ShotData {
                    // Validation reads geometry only; identity is irrelevant and
                    // this value is never used to build a shot.
                    uuid: None,
                    azimuth_in: shot.azimuth_in,
                    azimuth_out: shot.azimuth_out,
                    length: shot.length,
                    depth_in: shot.depth_in,
                    depth_out: shot.depth_out,
                    pitch_in: shot.pitch_in,
                    pitch_out: shot.pitch_out,
                    left: shot.left,
                    right: shot.right,
                    up: shot.up,
                    down: shot.down,
                    comment: shot.comment.clone(),
                    kind: shot.kind,
                    marker_index: shot.marker_index,
                    temperature: shot.temperature,
                    credit: None,
                };

                let warnings = Validator::validate_shot(
                    &shot_data,
                    &project.validation,
                    section.id,
                    shot_index,
                );
                for warning in warnings {
                    // Errors are never ignorable (issue #57): force is_resolved
                    // false so a stray resolved row (legacy/bulk) can't hide a
                    // blocking problem. Only advisory warnings honor a suppression.
                    let is_resolved = warning.severity != WarningSeverity::Error
                        && is_resolved_for(&shot.uuid, section.id, shot.id, &warning.code);
                    all_warnings.push(ProjectWarning {
                        section_id: section.id,
                        shot_id: shot.id,
                        shot_index,
                        code: warning.code,
                        message: warning.message,
                        severity: warning.severity,
                        is_resolved,
                        shot_uuid: shot.uuid.clone(),
                        datum_station_index: None,
                        datum_refs: None,
                    });
                }
            }
        }

        // Project-level: one advisory warning per GPS-fix zero datum whose GPS is
        // further from its surveyed position than 15 m plus 2% of the survey
        // distance to the nearest other datum (#741 decision 4). Attached to the
        // fix's adjacent shot (station 0 -> shot 0, station i -> shot i-1), so it
        // keys and ignores like any per-shot warning; `datum_station_index` names
        // the fix's station for "Go to station". The gap is the backend's
        // (`Project::gps_fix_gaps`), so every readout of it agrees.
        let mut gaps: Vec<_> = project
            .gps_fix_gaps()
            .into_iter()
            .filter(|(_, gap)| gap.exceeds_threshold)
            .collect();
        gaps.sort_by_key(|(sr, _)| (sr.section_id, sr.station_index));
        for (datum, gap) in gaps {
            let Some(section) = project.get_section(datum.section_id) else {
                continue;
            };
            let shot_index = datum.station_index.saturating_sub(1) as usize;
            let Some(shot) = section.shots.get(shot_index) else {
                continue;
            };
            let is_resolved = is_resolved_for(&shot.uuid, section.id, shot.id, DATUM_GPS_GAP);
            all_warnings.push(ProjectWarning {
                section_id: section.id,
                shot_id: shot.id,
                shot_index,
                message: datum_gps_gap_message(&gap),
                code: DATUM_GPS_GAP.to_string(),
                severity: crate::models::severity_for_code(DATUM_GPS_GAP),
                is_resolved,
                shot_uuid: shot.uuid.clone(),
                datum_station_index: Some(datum.station_index),
                datum_refs: None,
            });
        }

        // Sort by resolved (unresolved first), then severity (errors first), then by section and shot
        all_warnings.sort_by(|a, b| {
            let resolved_order = |r: bool| i32::from(r);
            let severity_order = |s: &WarningSeverity| match s {
                WarningSeverity::Error => 0,
                WarningSeverity::Warning => 1,
            };
            resolved_order(a.is_resolved)
                .cmp(&resolved_order(b.is_resolved))
                .then_with(|| severity_order(&a.severity).cmp(&severity_order(&b.severity)))
                .then_with(|| a.section_id.cmp(&b.section_id))
                .then_with(|| a.shot_index.cmp(&b.shot_index))
        });

        all_warnings
    }

    /// Pin the position-keyed ignores of JUST-reversed sections to the physical shot
    /// they were taken on.
    ///
    /// A resolved-warning row with a NULL `shot_uuid` is keyed by `(section_id, shot_id)`,
    /// and the reverse deliberately keeps shot IDs attached to POSITIONS rather than to
    /// records - so after it, `shot_id` names a different physical shot. Left alone, the
    /// save-time carry-over in `project_io` upgrades such a row via
    /// `live_by_id.get(&(section_id, shot_id))` and pins it, by UUID, to that OTHER shot,
    /// permanently suppressing an unrelated warning.
    ///
    /// NULL-uuid rows are still minted today: `commands::validation` deliberately stores
    /// `None` when the shot's UUID is not yet on disk (a shot created this session, or a
    /// pre-v6 file), so this is not only a legacy-data concern.
    ///
    /// The mirror: `shots[p].id` is unchanged by the reverse (ids stay with positions),
    /// while the record that HELD that id moved to position `N - 1 - p`. So the row keyed
    /// by `shots[p].id` belongs to `shots[N - 1 - p]`'s UUID.
    ///
    /// The map is rebuilt from the CURRENT project on every call, so it is not "its own
    /// inverse" in any useful sense: what makes undo/redo of a reverse safe to re-run is
    /// the INTERVENING re-reverse, which is what changes the map. Run it twice against ONE
    /// project state and the second pass re-pins the row a second time, onto a shot that
    /// was never reversed onto it. `section_ids` must therefore name each section at most
    /// once - `Command::reversed_section_ids` parity-reduces its `Batch` arm to guarantee it.
    ///
    /// Best-effort: no database (an unsaved project) means no rows exist to fix.
    pub(super) fn pin_position_keyed_warnings_after_reverse(&self, section_ids: &[SectionId]) {
        let live: std::collections::HashMap<(u32, u32), String> = {
            let guard = match self.project.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let Some(project) = guard.as_ref() else {
                return;
            };
            section_ids
                .iter()
                .filter_map(|sid| project.get_section(*sid))
                .flat_map(|section| {
                    let last = section.shots.len().saturating_sub(1);
                    section.shots.iter().enumerate().filter_map(move |(p, s)| {
                        let mirrored = section.shots.get(last - p)?;
                        Some(((section.id, s.id), mirrored.uuid.clone()?))
                    })
                })
                .collect()
        };
        if live.is_empty() {
            return;
        }
        let db_guard = match self.database.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(db) = db_guard.as_ref() else {
            return;
        };
        // Only pin onto a UUID the FILE already holds. A pre-v6 project, or any shot whose UUID
        // was minted in memory and not yet saved, would otherwise get that transient value
        // written into the persisted warning row - and if the reverse is discarded, or the app
        // exits before the next save, the following load mints a DIFFERENT UUID and the ignore
        // is orphaned onto a shot that no longer exists (issue #57 / Codex). Those shots simply
        // keep their position-keyed rows, which the save carry-over re-keys once the UUIDs are
        // durable. A failed lookup is treated as "not persisted": skipping a pin costs an ignore
        // migrating to the mirrored shot, writing a bad one costs it outright.
        let live: std::collections::HashMap<(u32, u32), String> = live
            .into_iter()
            .filter(|((section_id, _), uuid)| {
                db.shot_uuid_is_persisted(*section_id, uuid)
                    .unwrap_or(false)
            })
            .collect();
        if live.is_empty() {
            return;
        }
        match db.backfill_resolved_warning_uuids(&live) {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                pinned = n,
                "Pinned position-keyed warning ignores to their shot's UUID after a section reverse"
            ),
            Err(e) => tracing::warn!(
                reason = %e,
                purpose = "pin ignores across a section reverse",
                "Could not pin position-keyed warning ignores; one may migrate to the mirrored shot"
            ),
        }
    }

    /// Pin the position-keyed ignores of `section_ids`' shots to the shots'
    /// uuids BEFORE a structural edit moves those shots to another section or
    /// index (Split Section Here, Combine Sections, and the reverse inside a
    /// combine). A NULL-uuid row is keyed by `(section_id, shot_id)`, and the
    /// move changes both, so the ignore would lapse - or, once a later
    /// renumber lands another shot on that id, suppress the wrong warning. A
    /// uuid-keyed row then follows its shot through [`ShotHomes`], in the
    /// session and through the save, and an undo before the save puts the shot
    /// back where the row's section says.
    ///
    /// Pinned onto the shot's LIVE uuid, whether or not the file holds it yet:
    /// a shot added since the last save (the common case, since the resolve
    /// command keys such a shot's ignore by position), or one an earlier
    /// unsaved split or combine already moved. In a file that stores shot
    /// uuids, a uuid minted in this session is the one the next save writes;
    /// if the session is discarded instead, the shot never reaches the file and
    /// a row keyed by its uuid matches nothing, where a position-keyed one
    /// could suppress whatever shot holds that position on disk. The one
    /// exception is a pre-v6 file not saved since (its shots have no uuid on
    /// disk, so every load mints new ones): there only a uuid the file holds
    /// is pinned, as [`Self::pin_position_keyed_warnings_after_reverse`] does,
    /// and the rest keep their position-keyed rows (the residual it documents).
    pub(super) fn pin_position_keyed_ignores_before_move(&self, section_ids: &[SectionId]) {
        let live: std::collections::HashMap<(u32, u32), String> = {
            let guard = match self.project.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let Some(project) = guard.as_ref() else {
                return;
            };
            section_ids
                .iter()
                .filter_map(|sid| project.get_section(*sid))
                .flat_map(|section| {
                    section
                        .shots
                        .iter()
                        .filter_map(move |shot| Some(((section.id, shot.id), shot.uuid.clone()?)))
                })
                .collect()
        };
        if live.is_empty() {
            return;
        }
        let db_guard = match self.database.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(db) = db_guard.as_ref() else {
            return;
        };
        // A failed lookup is read as "the file has uuid-less shots": skipping a
        // pin costs an ignore, writing a transient uuid costs it on a discard.
        let file_stores_uuids = !db.holds_shots_without_uuid().unwrap_or(true);
        let live: std::collections::HashMap<(u32, u32), String> = live
            .into_iter()
            .filter(|((section_id, _), uuid)| {
                file_stores_uuids
                    || db
                        .shot_uuid_is_persisted(*section_id, uuid)
                        .unwrap_or(false)
            })
            .collect();
        if live.is_empty() {
            return;
        }
        match db.backfill_resolved_warning_uuids(&live) {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                pinned = n,
                "Pinned position-keyed warning ignores to their shot's UUID before a section \
                 split or combine"
            ),
            Err(e) => tracing::warn!(
                reason = %e,
                purpose = "pin ignores across a section split or combine",
                "Could not pin position-keyed warning ignores; one may lapse with its moved shot"
            ),
        }
    }

    /// Load resolved warnings from database.
    ///
    /// [#517] Verbatim, like the save carry-over: a shape this build's SELECT
    /// cannot name is a file to read leniently, not a file with no ignores. The
    /// return type is identical and the read is strictly more permissive, so
    /// every caller sees the same rows or more.
    pub fn load_resolved_warnings(&self) -> Result<Vec<crate::persistence::ResolvedWarning>> {
        let db_guard = self
            .database
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock database"))?;
        let db = db_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database open"))?;
        db.load_resolved_warnings_verbatim()
    }

    /// Save a resolved warning to database
    pub fn save_resolved_warning(
        &self,
        warning: &crate::persistence::ResolvedWarning,
    ) -> Result<()> {
        let db_guard = self
            .database
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock database"))?;
        let db = db_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database open"))?;
        db.save_resolved_warning(warning)
    }

    /// Whether the open project's load renamed shot UUIDs (see
    /// `AppState::load_renamed_shots`).
    fn load_renamed_shots(&self) -> bool {
        self.load_renamed_shots
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether a shot UUID is already persisted on disk (see
    /// `Database::shot_uuid_is_persisted`). Returns false when no database is open
    /// or the lookup fails - the safe default, which keeps a new ignore
    /// position-keyed rather than binding it to an unpersisted UUID (issue #57).
    pub fn shot_uuid_is_persisted(&self, section_id: u32, uuid: &str) -> bool {
        let Ok(db_guard) = self.database.lock() else {
            return false;
        };
        let Some(db) = db_guard.as_ref() else {
            return false;
        };
        if db.shot_uuid_is_persisted(section_id, uuid).unwrap_or(false) {
            return true;
        }
        // A shot the load renamed (a cross-section repeat in a file saved before
        // shot UUIDs were project-wide unique) holds an id the file does not, but
        // every open derives the SAME id from the one on disk, so an ignore keyed
        // by it survives a close without saving just as well. Only a load that
        // renamed anything can have produced one.
        if !self.load_renamed_shots() {
            return false;
        }
        db.section_shot_uuids(section_id)
            .unwrap_or_default()
            .iter()
            .any(|stored| crate::models::reminted_shot_uuid(section_id, stored) == uuid)
    }

    /// Delete a resolved warning from database (un-ignore). `shot_uuid` keys the
    /// deletion by stable identity when present; the legacy position is also
    /// cleared so a pre-v8 row is removed regardless.
    pub fn delete_resolved_warning(
        &self,
        section_id: u32,
        shot_id: u32,
        warning_code: &str,
        shot_uuid: Option<&str>,
    ) -> Result<()> {
        // An ignore an "Add .stalx" carried in and no save has written yet
        // (#674) lives only in memory; un-ignoring it is forgetting it there.
        let forgot_pending =
            self.forget_pending_ignore(section_id, shot_id, warning_code, shot_uuid);
        let db_guard = self
            .database
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock database"))?;
        let Some(db) = db_guard.as_ref() else {
            if forgot_pending {
                return Ok(());
            }
            return Err(anyhow::anyhow!("No database open"));
        };
        // A row stored against the id this shot had before the load renamed it
        // (`stored_shot_uuid_matches`) suppresses it too, so un-ignoring clears
        // that row as well. Only a shot whose live id the file does not hold
        // can have been renamed, so the scan is skipped for every other one.
        let maybe_renamed = shot_uuid.filter(|live| {
            self.load_renamed_shots()
                && !db.shot_uuid_is_persisted(section_id, live).unwrap_or(false)
        });
        if let Some(live) = maybe_renamed {
            for row in db.load_resolved_warnings_verbatim().unwrap_or_default() {
                let Some(stored) = row.shot_uuid.as_deref() else {
                    continue;
                };
                if row.section_id == section_id
                    && row.warning_code == warning_code
                    && stored != live
                    && crate::models::stored_shot_uuid_matches(section_id, stored, live)
                {
                    db.delete_resolved_warning(
                        section_id,
                        row.shot_id,
                        warning_code,
                        Some(stored),
                    )?;
                }
            }
        }
        db.delete_resolved_warning(section_id, shot_id, warning_code, shot_uuid)
    }

    /// User-facing **resolve** (the `resolve_warning` command's persistence step):
    /// the gated + dirtying counterpart of [`Self::save_resolved_warning`]. A resolved
    /// warning PROLIFERATES to the server (`resolved_warnings` is NOT stripped from the
    /// push snapshot - #197), so it is a real remote-dirtying edit, not local-only view
    /// state: a read-only remote project must reject it (`guard_mutation`), and a
    /// successful resolve in an edit session must mark the project dirty so `Done`
    /// pushes it - otherwise it is silently dropped on the next pull. The low-level
    /// [`Self::save_resolved_warning`] accessor stays ungated for the load-time backfill
    /// and tests.
    pub fn save_resolved_warning_gated(
        &self,
        warning: &crate::persistence::ResolvedWarning,
    ) -> Result<(), String> {
        self.guard_mutation("Resolve warning")
            .map_err(|f| f.error.unwrap_or_default())?;
        self.save_resolved_warning(warning)
            .map_err(|e| e.to_string())?;
        // Dirty-bit flip (#191/#197): set-only; no-op for local-only (no speleo_link).
        self.mark_remote_forward_edit();
        Ok(())
    }

    /// User-facing **unresolve** (the `unresolve_warning` command's persistence step):
    /// the gated + dirtying counterpart of [`Self::delete_resolved_warning`], for the
    /// same reason as [`Self::save_resolved_warning_gated`].
    pub fn delete_resolved_warning_gated(
        &self,
        section_id: u32,
        shot_id: u32,
        warning_code: &str,
        shot_uuid: Option<&str>,
    ) -> Result<(), String> {
        self.guard_mutation("Unresolve warning")
            .map_err(|f| f.error.unwrap_or_default())?;
        self.delete_resolved_warning(section_id, shot_id, warning_code, shot_uuid)
            .map_err(|e| e.to_string())?;
        self.mark_remote_forward_edit();
        Ok(())
    }

    /// Look up a shot's stable UUID by its `(section_id, shot_id)` identity in
    /// the open project. Used by the resolve/unresolve commands to key an
    /// "Ignore" by UUID (issue #57). After `do_load_project_full` every shot
    /// has a UUID, so this is `Some` for any shot the user can act on; returns
    /// `None` only if no project is open or the shot is gone.
    pub fn shot_uuid_for(&self, section_id: SectionId, shot_id: ShotId) -> Option<String> {
        let guard = self.project.lock().ok()?;
        let project = guard.as_ref()?;
        let section = project.sections.iter().find(|s| s.id == section_id)?;
        let shot = section.shots.iter().find(|s| s.id == shot_id)?;
        shot.uuid.clone()
    }
}

/// The `datum_gps_gap` warning text: the gap in metres, which way the GPS lies
/// from the surveyed station, and the threshold it exceeds.
fn datum_gps_gap_message(gap: &DatumGap) -> String {
    format!(
        "The GPS position of this zero datum is {:.1} m from where the survey puts it \
         (bearing {:.0} degrees), more than the {:.1} m expected. Check the GPS reading \
         and the survey between the datums.",
        gap.gap_m, gap.bearing_deg, gap.threshold_m
    )
}
