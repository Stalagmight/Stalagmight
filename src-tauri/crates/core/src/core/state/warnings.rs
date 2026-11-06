use anyhow::Result;
use std::collections::HashSet;

use crate::core::{Validator, WarningSeverity};
use crate::models::{
    Project, ProjectWarning, SectionId, ShotData, ShotId, MULTIPLE_DATUMS_PER_MERGE_COMPONENT,
};
use crate::persistence::Database;

use super::AppState;

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
            // Fall back to self.database (post-finalization)
            self.load_resolved_warnings().unwrap_or_default()
        };
        // Key the lookup sets by borrowed slices into `resolved` so the
        // per-warning probe below allocates nothing. `resolved` outlives this
        // closure, so the &str borrows stay valid.
        // Key UUID-suppression by (section_id, uuid): `ensure_unique_shot_uuids`
        // only de-dups WITHIN a section, so two sections can legitimately carry the
        // same shot UUID. Keying by UUID alone would let an ignore on one cross-
        // suppress the same code on the other section's shot (issue #57 / Codex).
        // section_id is stable per shot (it doesn't change when shot_id renumbers),
        // so this keeps the position-independence the UUID keying was added for.
        let mut resolved_by_uuid: std::collections::HashMap<(u32, &str), HashSet<&str>> =
            std::collections::HashMap::new();
        let mut resolved_by_position: HashSet<(u32, u32, &str)> = HashSet::new();
        for r in &resolved {
            match &r.shot_uuid {
                Some(uuid) => {
                    resolved_by_uuid
                        .entry((r.section_id, uuid.as_str()))
                        .or_default()
                        .insert(r.warning_code.as_str());
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

        // Project-level: two or more zero datums in one merge-connected component
        // over-constrain the rigid group, so the offset cascade tears a merge edge
        // apart. Emit ONE error per component (not one per datum, which collided on
        // the (section, shot, code) key - issue #57, finding #4), attached to the
        // first datum's adjacent shot, carrying the FULL datum list so the Resolve
        // view can offer "keep this one, remove the others". (Per-shot
        // ProjectWarning shape: anchor station 0 -> shot 0; station i -> shot i-1.)
        for component in project.over_anchored_merge_components() {
            // Attach the warning to the first datum that maps to a real shot. The
            // first datum may sit at station 0 of an empty section (no shot there);
            // skipping on it alone would silently drop the whole component's error
            // even when another datum in the component IS attachable (Codex). Fall
            // through the component to the first datum with a shot.
            let representative = component.iter().find_map(|d| {
                let section = project.get_section(d.section_id)?;
                let shot_index = if d.station_index == 0 {
                    0
                } else {
                    (d.station_index - 1) as usize
                };
                let shot = section.shots.get(shot_index)?;
                Some((d, section, shot_index, shot))
            });
            let Some((first, section, shot_index, shot)) = representative else {
                continue;
            };
            let anchors = component
                .iter()
                .map(|r| format!("{}:{}", r.section_id, r.station_index))
                .collect::<Vec<_>>()
                .join(", ");
            all_warnings.push(ProjectWarning {
                section_id: section.id,
                shot_id: shot.id,
                shot_index,
                message: format!(
                    "This merge group is anchored by {} zero datums ({anchors}). Two or more GPS anchors over-constrain one rigid group; keep one and remove the others.",
                    component.len()
                ),
                code: MULTIPLE_DATUMS_PER_MERGE_COMPONENT.to_string(),
                // An error (severity_for_code), so it is never ignorable and the
                // generic error force-false above also covers it: it clears only
                // when a datum is actually removed (then this warning is no longer
                // generated at all), never via a stray resolved row.
                severity: crate::models::severity_for_code(MULTIPLE_DATUMS_PER_MERGE_COMPONENT),
                is_resolved: false,
                shot_uuid: shot.uuid.clone(),
                datum_station_index: Some(first.station_index),
                datum_refs: Some(component.clone()),
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
    /// by `shots[p].id` belongs to `shots[N - 1 - p]`, whose id is now
    /// `shots[N - 1 - p].id`.
    ///
    /// [#576 FZ-PERSIST-2] Every such row is first MOVED to that id, so it follows its
    /// shot whether or not the shot's UUID is on disk; then the ones whose shot's UUID IS
    /// on disk are pinned to it. Pinning alone left the rows of a shot not saved yet
    /// where they were: one reverse moved its ignore to the mirrored shot, and reversing
    /// back (or Undo) then pinned it there for good - by the UUID of the mirrored shot,
    /// which is on disk. If the move cannot be made (a target slot holds another ignore
    /// with the same code), the rows stay and only the mirror pin is applied, as before.
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
        type Slots = std::collections::HashMap<(u32, u32), String>;
        // `moves`: (section, id the row holds, id its shot has now). `mirrored`: the
        // shot each row belongs to, by the id it holds. `current`: the shot at each id.
        let (moves, mirrored, current) = {
            let guard = self.project.lock();
            let Some(project) = guard.as_ref() else {
                return;
            };
            let mut moves = Vec::new();
            let mut mirrored = Slots::new();
            let mut current = Slots::new();
            for section in section_ids
                .iter()
                .filter_map(|sid| project.get_section(*sid))
            {
                let last = section.shots.len().saturating_sub(1);
                for (p, shot) in section.shots.iter().enumerate() {
                    let Some(owner) = section.shots.get(last - p) else {
                        continue;
                    };
                    if shot.id != owner.id {
                        moves.push((section.id, shot.id, owner.id));
                    }
                    if let Some(uuid) = &owner.uuid {
                        mirrored.insert((section.id, shot.id), uuid.clone());
                    }
                    if let Some(uuid) = &shot.uuid {
                        current.insert((section.id, shot.id), uuid.clone());
                    }
                }
            }
            (moves, mirrored, current)
        };
        let db_guard = self.database.lock();
        let Some(db) = db_guard.as_ref() else {
            return;
        };
        // Moved: each row now sits at its own shot's id, so it is pinned by the shot AT
        // that id. Not moved: by the mirror, as the rows still hold the old ids.
        let pins = if moves.is_empty() {
            current
        } else {
            match db.rekey_position_resolved_warnings(&moves) {
                Ok(_) => current,
                Err(e) => {
                    tracing::warn!(
                        reason = %e,
                        purpose = "keep ignores on their shots across a section reverse",
                        "Could not move position-keyed warning ignores with their shots; \
                         one on a shot not saved yet may migrate to the mirrored shot"
                    );
                    mirrored
                }
            }
        };
        // Only pin onto a UUID the FILE already holds. A pre-v6 project, or any shot whose UUID
        // was minted in memory and not yet saved, would otherwise get that transient value
        // written into the persisted warning row - and if the reverse is discarded, or the app
        // exits before the next save, the following load mints a DIFFERENT UUID and the ignore
        // is orphaned onto a shot that no longer exists (issue #57 / Codex). Those shots simply
        // keep their position-keyed rows, which the save carry-over re-keys once the UUIDs are
        // durable. A failed lookup is treated as "not persisted": skipping a pin costs nothing
        // now that the row has moved with its shot; writing a bad one costs it outright.
        let pins: Slots = pins
            .into_iter()
            .filter(|((section_id, _), uuid)| {
                db.shot_uuid_is_persisted(*section_id, uuid)
                    .unwrap_or(false)
            })
            .collect();
        if pins.is_empty() {
            return;
        }
        match db.backfill_resolved_warning_uuids(&pins) {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                pinned = n,
                "Pinned position-keyed warning ignores to their shot's UUID after a section reverse"
            ),
            Err(e) => tracing::warn!(
                reason = %e,
                purpose = "pin ignores across a section reverse",
                "Could not pin position-keyed warning ignores; they stay keyed by position"
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
        let db_guard = self.database.lock();
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
        let db_guard = self.database.lock();
        let db = db_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database open"))?;
        db.save_resolved_warning(warning)
    }

    /// Whether a shot UUID is already persisted on disk (see
    /// `Database::shot_uuid_is_persisted`). Returns false when no database is open
    /// or the lookup fails - the safe default, which keeps a new ignore
    /// position-keyed rather than binding it to an unpersisted UUID (issue #57).
    pub fn shot_uuid_is_persisted(&self, section_id: u32, uuid: &str) -> bool {
        let db_guard = self.database.lock();
        let Some(db) = db_guard.as_ref() else {
            return false;
        };
        db.shot_uuid_is_persisted(section_id, uuid).unwrap_or(false)
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
        let db_guard = self.database.lock();
        let db = db_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database open"))?;
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
    ///
    /// [#576 RV5-1] Between commands, under `command_lock`: a save carries the
    /// ignores from the open file into a fresh temp file and renames it over the
    /// open one, so an ignore written through the open handle in between was
    /// lost with `Ok` returned. Never call it while holding `command_lock`.
    ///
    /// [#576 C4] A refusal is the gate's own [`crate::core::CommandError`], so its
    /// code reaches the IPC; a persistence failure is unclassified (`FAILED`).
    pub fn save_resolved_warning_gated(
        &self,
        warning: &crate::persistence::ResolvedWarning,
    ) -> Result<(), crate::core::CommandError> {
        let _between_commands = self.hold_commands();
        self.guard_mutation("Resolve warning")?;
        self.save_resolved_warning(warning)
            .map_err(|e| crate::core::CommandError::failed(e.to_string()))?;
        // Dirty-bit flip (#191/#197): set-only; no-op for local-only (no speleo_link).
        self.mark_remote_forward_edit();
        Ok(())
    }

    /// User-facing **unresolve** (the `unresolve_warning` command's persistence step):
    /// the gated + dirtying counterpart of [`Self::delete_resolved_warning`], for the
    /// same reason as [`Self::save_resolved_warning_gated`], and under
    /// `command_lock` like it (#576 RV5-1).
    pub fn delete_resolved_warning_gated(
        &self,
        section_id: u32,
        shot_id: u32,
        warning_code: &str,
        shot_uuid: Option<&str>,
    ) -> Result<(), crate::core::CommandError> {
        let _between_commands = self.hold_commands();
        self.guard_mutation("Unresolve warning")?;
        self.delete_resolved_warning(section_id, shot_id, warning_code, shot_uuid)
            .map_err(|e| crate::core::CommandError::failed(e.to_string()))?;
        self.mark_remote_forward_edit();
        Ok(())
    }

    /// Look up a shot's stable UUID by its `(section_id, shot_id)` identity in
    /// the open project. Used by the resolve/unresolve commands to key an
    /// "Ignore" by UUID (issue #57). After `do_load_project_full` every shot
    /// has a UUID, so this is `Some` for any shot the user can act on; returns
    /// `None` only if no project is open or the shot is gone.
    ///
    /// [#576 NF-W6A-3] Read from the SETTLED project: the commands run on the
    /// pool, and a structural edit mutates the project before it settles it, so a
    /// plain lookup could resolve the id against a project no user ever sees.
    /// Called from the two commands only, never inside a command span
    /// (`settled_snapshot` is not reentrant).
    pub fn shot_uuid_for(&self, section_id: SectionId, shot_id: ShotId) -> Option<String> {
        let project = self.settled_snapshot()?;
        let section = project.sections.iter().find(|s| s.id == section_id)?;
        let shot = section.shots.iter().find(|s| s.id == shot_id)?;
        shot.uuid.clone()
    }
}
