use super::AppState;
use crate::core::{CommandResult, Validator};
use crate::models::{
    SectionData, SectionId, SectionUpdates, FEATURE_ID_STRIDE, MAX_SECTIONS_IN_ID_SPACE,
    MAX_SECTION_ID,
};

impl AppState {
    pub(super) fn create_section(&self, data: SectionData) -> CommandResult {
        let mut proj_guard = self.project.lock();

        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };

        let max_shots = FEATURE_ID_STRIDE.saturating_sub(1) as usize;
        if data.shots.len() > max_shots {
            return CommandResult::failure(format!(
                "Section exceeds maximum shots per section (limit = {max_shots})"
            ));
        }

        // The OTHER half of the same encode (#555): a shot id is
        // `section_id * FEATURE_ID_STRIDE + position`, so the id about to be
        // minted must still leave a WHOLE section's worth of ids inside u32 -
        // the max-shots check above is what entitles it to them. Id 42_949
        // encodes only its first 67_296 shots before the section's unchecked
        // `next_shot_id += 1` leaves u32; id 42_950 is worse still, overflowing
        // `Section::new`'s bare `id * FEATURE_ID_STRIDE` before it holds
        // anything. Panic in debug, wrap in release, and a wrapped base aliases
        // distinct shots onto one id.
        if project.next_section_id > MAX_SECTION_ID {
            return CommandResult::failure(format!(
                "Project has reached the maximum number of sections \
                 (limit = {MAX_SECTIONS_IN_ID_SPACE}); shot ids encode as \
                 section_id * {FEATURE_ID_STRIDE} + position in 32 bits, so \
                 section ids past {MAX_SECTION_ID} cannot hold shots"
            ));
        }

        // Reject non-finite geometry before it reaches the coordinate path (NaN/
        // inf slips past the zero/negative-length checks below). This is the
        // import/create-and-merge ingest path, alongside create_shot/update_shot.
        if data.shots.iter().any(|s| !s.all_finite()) {
            return CommandResult::failure(
                "Section has a shot with a non-finite (NaN/inf) value".to_string(),
            );
        }

        // Validate shots using the section_id that will be assigned
        let section_id_for_validation = project.next_section_id;
        let mut warnings = Vec::new();
        for (i, shot) in data.shots.iter().enumerate() {
            if shot.length == 0.0 {
                return CommandResult::failure(format!(
                    "Shot {}:{} has zero length",
                    section_id_for_validation,
                    i + 1
                ));
            }
            if shot.length < 0.0 {
                return CommandResult::failure(format!(
                    "Shot {}:{} has negative length: {:.1}m",
                    section_id_for_validation,
                    i + 1,
                    shot.length
                ));
            }
            // Per-project thresholds (#57) so create-time warnings match the panel.
            warnings.extend(Validator::validate_shot(
                shot,
                &project.validation,
                section_id_for_validation,
                i,
            ));
        }

        let section = project.add_section(data);
        let section_id = section.id;
        tracing::debug!(
            section_id = section_id,
            purpose = "add section to project",
            "Section created"
        );

        let result = CommandResult::success().with_section_id(section_id);
        if warnings.is_empty() {
            result
        } else {
            result.with_warnings(warnings)
        }
    }

    pub(super) fn delete_section(&self, section_id: SectionId) -> CommandResult {
        self.with_project_mut(|project| {
            if project.get_section(section_id).is_some() {
                project.preserve_connections_for_section_deletion(section_id);
                project.cleanup_section_references(section_id);
                project.remove_section(section_id);
                tracing::debug!(
                    section_id,
                    purpose = "remove section and cleanup references",
                    "Section deleted (with cleanup)"
                );
                CommandResult::success()
            } else {
                CommandResult::failure(format!("Section {section_id} not found"))
            }
        })
    }

    /// Reverse a section end-to-end (issue #24).  The far end becomes station 0
    /// and every station keeps its world coordinate; see
    /// [`crate::models::Project::reverse_section`] for the transform.
    ///
    /// Frozen shots are deliberately NOT consulted here.  `update_shot` rejects
    /// azimuth/depth/pitch edits on a frozen shot because those move the station
    /// it is pinned to, but reverse re-associates each reading with the other end
    /// of the same physical line and leaves the line itself untouched, so the
    /// frozen invariant still holds.  Applying the frozen check would block
    /// reverse on any datum-anchored section for no benefit.
    pub(super) fn reverse_section(&self, section_id: SectionId, swap_lr: bool) -> CommandResult {
        self.with_project_mut(|project| {
            if !project.reverse_section(section_id, swap_lr) {
                return CommandResult::failure(format!("Section {section_id} not found"));
            }
            tracing::debug!(
                section_id,
                swap_left_right = swap_lr,
                purpose = "reverse section end-to-end",
                "Section reversed"
            );

            // Swapping depths and reciprocating azimuths can newly satisfy or trip
            // the per-project discrepancy checks, so re-emit shot warnings the way
            // create_section/update_shot do rather than leaving the panel stale.
            let warnings: Vec<_> = project
                .get_section(section_id)
                .map(|section| {
                    section
                        .shots
                        .iter()
                        .enumerate()
                        .flat_map(|(i, shot)| {
                            Validator::validate_shot(
                                &shot.to_shot_data(),
                                &project.validation,
                                section_id,
                                i,
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();

            let result = CommandResult::success().with_section_id(section_id);
            if warnings.is_empty() {
                result
            } else {
                result.with_warnings(warnings)
            }
        })
    }

    pub(super) fn update_section(
        &self,
        section_id: SectionId,
        updates: SectionUpdates,
    ) -> CommandResult {
        self.with_project_mut(|project| {
            if project.update_section(section_id, &updates).is_some() {
                tracing::debug!(
                    section_id,
                    purpose = "update section metadata or shots",
                    "Section updated"
                );
                CommandResult::success()
            } else {
                CommandResult::failure(format!("Section {section_id} not found"))
            }
        })
    }
}
