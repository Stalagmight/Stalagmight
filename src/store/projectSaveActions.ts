import type { StoreApi } from 'zustand';
import type { ProjectStore } from './projectStore';
import type { CommandResult } from '../types/commands';
import { invokeIpc } from '../utils/ipc';
import { useUIStore } from './uiStore';
import i18n from '../i18n';
import { TOAST_KEYS } from '../utils/toastKeys';
import { autosaveFailureMessage } from '../utils/saveFeedback';
import { classifySaveError, isPermissionError } from '../utils/saveErrors';
import { noticeOlderVersionsDropFixes } from '../utils/gpsFixSaveNotice';

/**
 * Local-save completion belongs to the project and edits that issued it.
 *
 * `command` is `save_project`, or `save_project_overwriting` for the save
 * prompt's Overwrite answer (the external-modification guard refused the save,
 * `EXTERNALLY_MODIFIED`); both finish the same way.
 */
export function createSaveProjectAction(
  set: StoreApi<ProjectStore>['setState'],
  get: () => ProjectStore,
  command: 'save_project' | 'save_project_overwriting' = 'save_project',
): ProjectStore['saveProject'] {
  return async (path?: string, isAutosave?: boolean) => {
    // The `.stalx` v2 save never heals: it validates and refuses an invalid
    // survey instead, leaving the project as it was. The refresh below for a
    // project the integrity check flagged is kept as a cheap safety net (it
    // costs a get_project_state only on such a project, never on a clean save).
    const issued = get();
    let ownerPath = issued.project?.file_path;
    const current = () => get().project?.file_path === ownerPath &&
      get().project?.created_at === issued.project?.created_at &&
      get().loading.clientOpId === issued.loading.clientOpId;
    const hadIntegrityIssues = issued.integrityIssues.length > 0;
    const result = await invokeIpc<CommandResult>(
      command,
      command === 'save_project' ? { path, isAutosave: isAutosave === true } : { path },
      {
        purpose: command === 'save_project' ? 'persist project changes' : 'overwrite a project file another program changed',
        fields: { path, isAutosave },
      }
    );
    if (!current()) return result;
    if (result.success) {
      // A detached copy was written; the OPEN project was not saved. Do not
      // re-point file_path and do not clear isDirty (see CommandResult).
      if (result.unlinked_copy_path) {
        useUIStore
          .getState()
          .showToast(
            i18n.t('speleodb.managedFile.copySaved', { path: result.unlinked_copy_path }),
            TOAST_KEYS.SPELEODB_UNLINKED_COPY,
            'warning'
          );
        // The heals above ran on the LIVE project before the pipeline decided to
        // write a copy, so the frontend's mirror is stale here too - and the
        // healed state reached only the copy, never the managed file, which
        // leaves the session dirty even if it entered the save clean.
        if (hadIntegrityIssues) {
          await get().refreshProjectState();
          if (current()) set({ isDirty: true });
        }
        return result;
      }
      // Update file_path if the backend returned it (first save / save-as)
      if (result.saved_path) {
        const project = get().project;
        // Only mint a new project object when the path ACTUALLY changed (first
        // save / Save As). A successful save always echoes `saved_path` (Rust
        // `CommandResult::success().with_path(...)`), so an autosave to the same
        // path would otherwise re-spread the project on every debounced write.
        // That changes the project's identity, and MapCanvas's republish
        // useLayoutEffect keys on it - a no-op save would trigger a full survey
        // rebuild + republish, briefly blanking all geometry (#mem).
        if (project && project.file_path !== result.saved_path) {
          ownerPath = result.saved_path;
          set({ project: { ...project, file_path: result.saved_path } });
        }
      }
      // Lightweight: just refresh undo/redo availability
      await get().refreshUndoState();
      if (!current()) return result;
      // The first successful save opens the on-disk database for a deferred project
      // - UNLESS the backend saved the file but couldn't reopen the DB (it clears
      // the handle and flags DATABASE_NOT_REOPENED). In that case keep
      // databaseOpen=false so DB-backed actions stay disabled (issue #57 / Codex).
      const dbNotReopened = result.warnings?.some((w) => w.code === 'DATABASE_NOT_REOPENED') ?? false;
      set({
        isDirty: get().projectTreeGeneration !== issued.projectTreeGeneration,
        databaseOpen: !dbNotReopened,
        autosaveFailure: null,
        autosaveFailureKind: null,
      });
      // The file holds GPS fixes an older version drops on its save (#741).
      noticeOlderVersionsDropFixes(result, issued.project?.created_at);
      // If the save healed the project in memory (re-minted duplicate UUIDs,
      // dropped orphan line offsets, sanitized non-finite geometry), the
      // frontend's shots/stations are now stale vs the saved file - refresh so the
      // rendered state matches. Gated on hadIntegrityIssues so a clean save/autosave
      // skips the extra get_project_state fetch and re-render.
      if (hadIntegrityIssues) {
        await get().refreshProjectState();
      }
      // Don't let geometric corruption persist silently: re-check after save so
      // drifted merges surface in the integrity banner this session, not only on
      // the next load. Skip on a clean autosave to avoid churn - BUT still
      // re-check when the project entered the save flagged (hadIntegrityIssues),
      // because the save path auto-heals some issues (duplicate UUIDs / orphan
      // offsets) on autosave too; without this the banner would stay stale after
      // an autosave silently cleared them.
      if (current() && (!isAutosave || hadIntegrityIssues)) {
        void get().checkProjectIntegrity();
      }
    } else {
      const permission = isPermissionError(result.error);
      // An autosave failure is an ongoing condition, not a modal moment: feed
      // the banner (docs/notifications.md). `autosaveFailureMessage` keeps the
      // raw "FS_*:" markers out of it - the banner renders this string.
      // Gated on the project still being dirty for the same reason useAutoSave
      // gates its red status: a manual Ctrl+S that landed while this autosave was
      // in flight already put the changes on disk, and a banner saying the latest
      // changes are not on disk on a clean project is a claim the user cannot
      // clear by saving (#429).
      if (isAutosave && get().isDirty) {
        set({
          autosaveFailure: autosaveFailureMessage(result.error),
          autosaveFailureKind: classifySaveError(result.error),
        });
      }
      // A filesystem permission denial (e.g. macOS forgot folder access after
      // an update) needs actionable guidance, not a raw error. Skip on autosave
      // so we don't reopen the dialog on every debounced retry.
      if (permission && !isAutosave) {
        useUIStore.getState().openDialog('save-permission', {
          detail: result.error,
          context: 'save',
        });
      }
      // A save is also blocked when the project has unresolvable cross-references;
      // re-check so they surface in the integrity banner (the repair entry point)
      // and the user can Repair and re-save. Awaited (not fire-and-forget) so
      // `integritySaveBlocked` is up to date by the time saveProject resolves -
      // the auto-save hook reads it to decide whether to pause, and
      // surfaceSaveFailure reads it to route a blocked save to Review & Repair.
      // (On a permission error this is a harmless no-op: the project is
      // structurally fine, so integritySaveBlocked stays false.)
      await get().checkProjectIntegrity();
    }
    return result;
  };
}
