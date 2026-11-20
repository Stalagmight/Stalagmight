//! Warnings and their ignores on the station-graph model.
//!
//! Ignored warnings are survey content now (plan 6.5): the file's
//! `ignored_warnings` member, held in memory (`FileState`), keyed by the
//! shot's persistent uuid. An ignore marks the project dirty and rides the
//! next save (declared change: "Don't save" discards it; 0.6.0 wrote it into
//! the file at once). The IPC keeps its `ResolvedWarning` shape, converted
//! here through the open project's shots.
//!
//! C0 ports the ignore store. The warnings computation itself
//! (`compute_project_warnings`: validation, GPS gaps, across-one-shot
//! closures, the datum-gap ignore pinning) is ported by the warnings work
//! package; until then it reports no warnings.

use anyhow::Result;

use super::AppState;
use crate::models::survey::Project;
use crate::models::{LineId, ProjectWarning, ShotId};
use crate::persistence::stalx_v2::IgnoredWarning;
use crate::persistence::ResolvedWarning;

/// `(line, session shot id)` of the shot with `uuid` in `project`.
fn locate(project: &Project, uuid: &str) -> Option<(LineId, ShotId)> {
    let at = project.shot_by_uuid(uuid)?;
    let line = project.line(at.line)?;
    let shot = line.shots.get(at.index)?;
    Some((line.id, shot.id))
}

impl AppState {
    /// The open project's warnings. Not ported in C0: none.
    pub fn compute_project_warnings(&self, _project: &Project) -> Vec<ProjectWarning> {
        crate::core::stub::not_ported("warnings::compute_project_warnings")
    }

    /// Every ignore of the open project, in the IPC's shape (an ignore whose
    /// shot is gone reads line and shot 0).
    pub fn load_resolved_warnings(&self) -> Result<Vec<ResolvedWarning>> {
        let ignores = self.with_file(|file| file.ignored_warnings().to_vec());
        let guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let project = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No project open"))?;
        Ok(ignores
            .into_iter()
            .map(|i| {
                let (line_id, shot_id) = locate(project, &i.shot).unwrap_or((0, 0));
                ResolvedWarning {
                    line_id,
                    shot_id,
                    warning_code: i.code,
                    resolved_at: i.at,
                    resolved_comment: i.comment,
                    shot_uuid: Some(i.shot),
                }
            })
            .collect())
    }

    /// Record an ignore (replacing one for the same shot and code). Marks
    /// the project dirty; the next save writes it.
    pub fn save_resolved_warning(&self, warning: &ResolvedWarning) -> Result<()> {
        let uuid = match warning.shot_uuid.clone() {
            Some(uuid) => uuid,
            None => self
                .shot_uuid_for(warning.line_id, warning.shot_id)
                .ok_or_else(|| anyhow::anyhow!("the shot no longer exists"))?,
        };
        self.with_file(|file| {
            let list = &mut file.extras.ignored_warnings;
            list.retain(|i| !(i.shot == uuid && i.code == warning.warning_code));
            list.push(IgnoredWarning {
                shot: uuid,
                code: warning.warning_code.clone(),
                at: warning.resolved_at,
                comment: warning.resolved_comment.clone(),
            });
            list.sort();
            file.dirty = true;
        });
        Ok(())
    }

    /// Every shot uuid is durable with the survey now (ignores ride the same
    /// save): true when the shot exists in the open project.
    pub fn shot_uuid_is_persisted(&self, _line_id: u32, uuid: &str) -> bool {
        self.project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|p| p.shot_by_uuid(uuid).is_some())
    }

    /// Drop an ignore. Marks the project dirty when one was dropped.
    pub fn delete_resolved_warning(
        &self,
        line_id: u32,
        shot_id: u32,
        warning_code: &str,
        shot_uuid: Option<&str>,
    ) -> Result<()> {
        let uuid = match shot_uuid {
            Some(uuid) => Some(uuid.to_string()),
            None => self.shot_uuid_for(line_id, shot_id),
        };
        let Some(uuid) = uuid else {
            return Ok(());
        };
        self.with_file(|file| {
            let list = &mut file.extras.ignored_warnings;
            let before = list.len();
            list.retain(|i| !(i.shot == uuid && i.code == warning_code));
            if list.len() != before {
                file.dirty = true;
            }
        });
        Ok(())
    }

    /// [`Self::save_resolved_warning`] behind the read-only gate (#197: an
    /// ignore is pushed, so it is a remote-dirtying edit).
    pub fn save_resolved_warning_gated(&self, warning: &ResolvedWarning) -> Result<(), String> {
        self.guard_mutation("Resolve warning")
            .map_err(|f| f.error.unwrap_or_default())?;
        let warning = ResolvedWarning {
            warning_code: self.ignore_code_for(
                warning.line_id,
                warning.shot_id,
                &warning.warning_code,
            ),
            ..warning.clone()
        };
        self.save_resolved_warning(&warning)
            .map_err(|e| e.to_string())?;
        self.mark_remote_forward_edit();
        Ok(())
    }

    /// [`Self::delete_resolved_warning`] behind the read-only gate.
    pub fn delete_resolved_warning_gated(
        &self,
        line_id: u32,
        shot_id: u32,
        warning_code: &str,
        shot_uuid: Option<&str>,
    ) -> Result<(), String> {
        self.guard_mutation("Unresolve warning")
            .map_err(|f| f.error.unwrap_or_default())?;
        let warning_code = self.ignore_code_for(line_id, shot_id, warning_code);
        self.delete_resolved_warning(line_id, shot_id, &warning_code, shot_uuid)
            .map_err(|e| e.to_string())?;
        self.mark_remote_forward_edit();
        Ok(())
    }

    /// The code an ignore of `warning_code` on this shot is stored under. The
    /// datum GPS-gap codes are pinned to the datum's GPS in 0.6.0
    /// (`datum_gap_ignore_code`); that pinning is ported with the warnings
    /// computation. Until then: the code as given.
    pub fn ignore_code_for(
        &self,
        _line_id: LineId,
        _shot_id: ShotId,
        warning_code: &str,
    ) -> String {
        warning_code.to_string()
    }

    /// The persistent uuid of a shot of the open project.
    pub fn shot_uuid_for(&self, line_id: LineId, shot_id: ShotId) -> Option<String> {
        let guard = self.project.lock().ok()?;
        let project = guard.as_ref()?;
        let line = project.line(line_id)?;
        line.shots
            .iter()
            .find(|s| s.id == shot_id)
            .map(|s| s.uuid.clone())
    }
}
