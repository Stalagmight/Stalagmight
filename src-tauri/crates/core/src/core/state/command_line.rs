use super::AppState;
use crate::core::{CommandResult, Validator};
use crate::models::{
    LineData, LineId, LineUpdates, FEATURE_ID_STRIDE, MAX_LINES_IN_ID_SPACE,
    MAX_LINE_ID,
};

impl AppState {
    pub(super) fn create_line(&self, data: LineData) -> CommandResult {
        let Ok(mut proj_guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };

        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };

        let max_shots = FEATURE_ID_STRIDE.saturating_sub(1) as usize;
        if data.shots.len() > max_shots {
            return CommandResult::failure(format!(
                "Line exceeds maximum shots per line (limit = {max_shots})"
            ));
        }

        // The OTHER half of the same encode (#555): a shot id is
        // `line_id * FEATURE_ID_STRIDE + position`, so the id about to be
        // minted must still leave a WHOLE line's worth of ids inside u32 -
        // the max-shots check above is what entitles it to them. Id 42_949
        // encodes only its first 67_296 shots before the line's unchecked
        // `next_shot_id += 1` leaves u32; id 42_950 is worse still, overflowing
        // `Line::new`'s bare `id * FEATURE_ID_STRIDE` before it holds
        // anything. Panic in debug, wrap in release, and a wrapped base aliases
        // distinct shots onto one id.
        if project.next_line_id > MAX_LINE_ID {
            return CommandResult::failure(format!(
                "Project has reached the maximum number of lines \
                 (limit = {MAX_LINES_IN_ID_SPACE}); shot ids encode as \
                 line_id * {FEATURE_ID_STRIDE} + position in 32 bits, so \
                 line ids past {MAX_LINE_ID} cannot hold shots"
            ));
        }

        // Reject non-finite geometry before it reaches the coordinate path (NaN/
        // inf slips past the zero/negative-length checks below). This is the
        // import/create-and-merge ingest path, alongside create_shot/update_shot.
        if data.shots.iter().any(|s| !s.all_finite()) {
            return CommandResult::failure(
                "Line has a shot with a non-finite (NaN/inf) value".to_string(),
            );
        }

        // Validate shots using the line_id that will be assigned
        let line_id_for_validation = project.next_line_id;
        let mut warnings = Vec::new();
        for (i, shot) in data.shots.iter().enumerate() {
            if shot.length == 0.0 {
                return CommandResult::failure(format!(
                    "Shot {}:{} has zero length",
                    line_id_for_validation,
                    i + 1
                ));
            }
            if shot.length < 0.0 {
                return CommandResult::failure(format!(
                    "Shot {}:{} has negative length: {:.1}m",
                    line_id_for_validation,
                    i + 1,
                    shot.length
                ));
            }
            // Per-project thresholds (#57) so create-time warnings match the panel.
            warnings.extend(Validator::validate_shot(
                shot,
                &project.validation,
                line_id_for_validation,
                i,
            ));
        }

        let line = project.add_line(data);
        let line_id = line.id;
        tracing::debug!(
            line_id = line_id,
            purpose = "add line to project",
            "Line created"
        );

        let result = CommandResult::success().with_line_id(line_id);
        if warnings.is_empty() {
            result
        } else {
            result.with_warnings(warnings)
        }
    }

    pub(super) fn delete_line(&self, line_id: LineId) -> CommandResult {
        self.with_project_mut(|project| {
            if project.get_line(line_id).is_some() {
                project.preserve_connections_for_line_deletion(line_id);
                project.cleanup_line_references(line_id);
                project.remove_line(line_id);
                tracing::debug!(
                    line_id,
                    purpose = "remove line and cleanup references",
                    "Line deleted (with cleanup)"
                );
                CommandResult::success()
            } else {
                CommandResult::failure(format!("Line {line_id} not found"))
            }
        })
    }

    /// Reverse a line end-to-end (issue #24).  The far end becomes station 0
    /// and every station keeps its world coordinate; see
    /// [`crate::models::Project::reverse_line`] for the transform.
    ///
    /// Frozen shots are deliberately NOT consulted here.  `update_shot` rejects
    /// azimuth/depth/pitch edits on a frozen shot because those move the station
    /// it is pinned to, but reverse re-associates each reading with the other end
    /// of the same physical line and leaves the line itself untouched, so the
    /// frozen invariant still holds.  Applying the frozen check would block
    /// reverse on any datum-anchored line for no benefit.
    pub(super) fn reverse_line(&self, line_id: LineId, swap_lr: bool) -> CommandResult {
        self.with_project_mut(|project| {
            if !project.reverse_line(line_id, swap_lr) {
                return CommandResult::failure(format!("Line {line_id} not found"));
            }
            tracing::debug!(
                line_id,
                swap_left_right = swap_lr,
                purpose = "reverse line end-to-end",
                "Line reversed"
            );

            // Swapping depths and reciprocating azimuths can newly satisfy or trip
            // the per-project discrepancy checks, so re-emit shot warnings the way
            // create_line/update_shot do rather than leaving the panel stale.
            let warnings: Vec<_> = project
                .get_line(line_id)
                .map(|line| {
                    line
                        .shots
                        .iter()
                        .enumerate()
                        .flat_map(|(i, shot)| {
                            Validator::validate_shot(
                                &shot.to_shot_data(),
                                &project.validation,
                                line_id,
                                i,
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();

            let result = CommandResult::success().with_line_id(line_id);
            if warnings.is_empty() {
                result
            } else {
                result.with_warnings(warnings)
            }
        })
    }

    pub(super) fn update_line(
        &self,
        line_id: LineId,
        updates: LineUpdates,
    ) -> CommandResult {
        self.with_project_mut(|project| {
            if project.update_line(line_id, &updates).is_some() {
                tracing::debug!(
                    line_id,
                    purpose = "update line metadata or shots",
                    "Line updated"
                );
                CommandResult::success()
            } else {
                CommandResult::failure(format!("Line {line_id} not found"))
            }
        })
    }
}
