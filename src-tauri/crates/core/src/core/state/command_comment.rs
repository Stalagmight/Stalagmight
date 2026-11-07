use super::AppState;
use crate::core::{Command, CommandResult, CommentHolder};
use crate::models::project::join_station_comment;
use crate::models::{Project, StationKey, StationRef};

impl AppState {
    /// [#576 W30-NF1] Station `sr`'s comment: the comments of every stored key
    /// that names it, joined in the save's order (`Project::one_row_per_station`
    /// with `join_station_comment`, as the station payload shows it). A legacy
    /// positional `Index` key can name a station beside its canonical key, so
    /// reading the canonical key alone missed a comment the station shows.
    pub(super) fn station_comment(project: &Project, sr: StationRef) -> Option<String> {
        let entries: Vec<(&StationKey, StationRef, String)> = project
            .station_comments
            .iter()
            .filter(|(key, _)| key.section_id == sr.section_id && project.key_is_ref(key, sr))
            .map(|(key, text)| (key, sr, text.clone()))
            .collect();
        project
            .one_row_per_station(entries, join_station_comment)
            .pop()
            .map(|(_, text)| text)
    }

    /// [#576 W30-NF1] Make `comment` station `sr`'s whole comment: every stored
    /// key that names the station is removed, then a non-empty comment is stored
    /// under its canonical key. Touching the canonical key alone left a comment
    /// under a legacy key in place - a clear did nothing, and a new comment was
    /// joined to the old one instead of replacing it.
    fn replace_station_comment(project: &mut Project, sr: StationRef, comment: Option<String>) {
        let naming: Vec<StationKey> = project
            .station_comments
            .keys()
            .filter(|key| key.section_id == sr.section_id && project.key_is_ref(key, sr))
            .cloned()
            .collect();
        for key in &naming {
            project.station_comments.remove(key);
        }
        if let Some(text) = comment.filter(|text| !text.is_empty()) {
            if let Some(key) = project.key_for_ref(sr) {
                project.station_comments.insert(key, text);
            }
        }
    }

    pub(super) fn set_station_comment(
        &self,
        station_ref: StationRef,
        comment: Option<String>,
    ) -> CommandResult {
        self.with_project_mut(|project| {
            // Validate station exists
            let section = match project.get_section(station_ref.section_id) {
                Some(s) => s,
                None => {
                    return CommandResult::failure(format!(
                        "Section {} not found",
                        station_ref.section_id
                    ));
                }
            };

            let max_station_index = section.shots.len() as u32;
            if station_ref.station_index > max_station_index {
                return CommandResult::failure(format!(
                    "Station index {} out of range (section has {} stations)",
                    station_ref.station_index,
                    max_station_index + 1
                ));
            }

            Self::replace_station_comment(project, station_ref, comment);

            project.touch();
            tracing::info!(
                ?station_ref,
                purpose = "set station comment",
                "Station comment updated"
            );
            CommandResult::success()
        })
    }

    /// Execute a move-comment operation as a single atomic undo/redo unit.
    /// Handles its own undo push (like `execute_batch` / `execute_delete_shot_and_split`).
    pub(super) fn execute_move_comment(
        &self,
        source: CommentHolder,
        target: CommentHolder,
        swap: bool,
        cmd: Command,
    ) -> CommandResult {
        // [Track B] (#191, D-B6) This is reached ONLY via execute()'s
        // `Command::MoveComment` arm (early `return self.execute_move_comment(..)`),
        // already under execute()'s command_lock AND past its read-only edit gate
        // (MoveComment is not a lifecycle pass-through). It takes no command_lock
        // of its own, so it inherits the guard rather than calling it directly. If
        // a future DIRECT caller is added, it MUST route through guard_mutation -
        // the chokepoint enumeration test fails if MoveComment leaks past execute.
        debug_assert!(
            self.edit_enabled(),
            "execute_move_comment reached with edit disabled - it must run only via execute()'s gate"
        );
        // Step 1: Take BEFORE snapshot
        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };

        // Step 2: Read source and target comments, then mutate
        {
            let mut proj_guard = self.project.lock();
            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };

            let source_comment = match Self::get_holder_comment(project, &source) {
                Ok(c) => c,
                Err(e) => return CommandResult::failure(e),
            };
            let target_comment = match Self::get_holder_comment(project, &target) {
                Ok(c) => c,
                Err(e) => return CommandResult::failure(e),
            };

            // Source must have a comment
            let Some(comment_text) = source_comment else {
                return CommandResult::failure("Source has no comment to move".to_string());
            };

            // Set target = source's comment
            if let Err(e) = Self::set_holder_comment(project, &target, Some(comment_text)) {
                return CommandResult::failure(e);
            }

            // Set source = target's comment (swap) or None (move)
            let new_source_comment = if swap { target_comment } else { None };
            if let Err(e) = Self::set_holder_comment(project, &source, new_source_comment) {
                return CommandResult::failure(e);
            }

            project.touch();
        }

        // Steps 3-4: [#576 A4] Through the shared forward epilogue - a comment is
        // metadata, so no derived-state step - which takes the AFTER snapshot,
        // pushes the single undo entry and flips the dirty bit, in that order.
        if let Err(failure) = self.record_snapshot_edit(
            None,
            super::command_epilogue::EpilogueSteps::NONE,
            cmd,
            before_snapshot,
        ) {
            return failure;
        }

        tracing::info!(
            swap = swap,
            purpose = "move comment atomically",
            "Move comment completed"
        );
        CommandResult::success()
    }

    /// Whether this is a `MoveComment` whose source and target are the SAME holder, and that
    /// holder exists and carries a comment - i.e. provably a no-op (A16, #576).
    ///
    /// Moving a comment onto the holder it is already on changes nothing, and neither does
    /// swapping it with itself. Run through `execute_move_comment`, the move wrote the text onto
    /// the target and then cleared the source - the same holder - deleting the comment, and the
    /// swap pushed an undo entry (dropping the redo branch) and stamped the remote dirty bit.
    /// The map's swap dialog does not compare the two holders, so the backend is the guard.
    ///
    /// A missing holder or one without a comment is NOT a no-op: it falls through so the
    /// command fails exactly as before.
    pub(super) fn is_provable_move_comment_noop(&self, cmd: &Command) -> bool {
        let Command::MoveComment { source, target, .. } = cmd else {
            return false;
        };
        if source != target {
            return false;
        }
        let guard = self.project.lock();
        let Some(project) = guard.as_ref() else {
            return false;
        };
        matches!(Self::get_holder_comment(project, source), Ok(Some(_)))
    }

    /// Read the comment from a CommentHolder (station or shot)
    fn get_holder_comment(
        project: &Project,
        holder: &CommentHolder,
    ) -> Result<Option<String>, String> {
        match holder {
            CommentHolder::Station { station_ref } => {
                // Validate station exists
                let section = project
                    .get_section(station_ref.section_id)
                    .ok_or_else(|| format!("Section {} not found", station_ref.section_id))?;
                let max_station_index = section.shots.len() as u32;
                if station_ref.station_index > max_station_index {
                    return Err(format!(
                        "Station index {} out of range",
                        station_ref.station_index
                    ));
                }
                Ok(Self::station_comment(project, *station_ref))
            }
            CommentHolder::Shot {
                section_id,
                shot_id,
            } => {
                let section = project
                    .get_section(*section_id)
                    .ok_or_else(|| format!("Section {section_id} not found"))?;
                let shot = section
                    .get_shot(*shot_id)
                    .ok_or_else(|| format!("Shot {shot_id} not found in section {section_id}"))?;
                Ok(shot.comment.clone())
            }
        }
    }

    /// Set the comment on a CommentHolder (station or shot)
    fn set_holder_comment(
        project: &mut Project,
        holder: &CommentHolder,
        comment: Option<String>,
    ) -> Result<(), String> {
        match holder {
            CommentHolder::Station { station_ref } => {
                Self::replace_station_comment(project, *station_ref, comment);
                Ok(())
            }
            CommentHolder::Shot {
                section_id,
                shot_id,
            } => {
                let section = project
                    .get_section_mut(*section_id)
                    .ok_or_else(|| format!("Section {section_id} not found"))?;
                let shot = section
                    .shots
                    .iter_mut()
                    .find(|s| s.id == *shot_id)
                    .ok_or_else(|| format!("Shot {shot_id} not found in section {section_id}"))?;
                match comment {
                    Some(text) if !text.is_empty() => {
                        shot.comment = Some(text);
                    }
                    _ => {
                        shot.comment = None;
                    }
                }
                Ok(())
            }
        }
    }
}
