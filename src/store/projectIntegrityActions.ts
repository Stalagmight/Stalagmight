import type { StoreApi } from 'zustand';
import i18n from '../i18n';
import { invokeIpc } from '../utils/ipc';
import { errorCode } from '../utils/commandError';
import { isEditGateCode, READ_ONLY_EDIT_NOTICE } from '../utils/editErrors';
import { TOAST_KEYS } from '../utils/toastKeys';
import { useUIStore } from './uiStore';
import { refuseMutationDuringCarry } from './moveCarry';
import type { IntegrityRepairResult } from '../types/commands';
import type { ProjectStore } from './projectStore';

/**
 * The validation-warning and structural-integrity actions.
 *
 * Split out of `projectStore.ts` rather than rebaselined, for the reason
 * `projectConfigActions.ts` gives and #157 requires: that file is already past the
 * 1500-LOC ratchet, so it may shrink but not grow, and a file at that size is to be split
 * at an API boundary instead of having its baseline raised. Same `(set, get)` factory
 * shape as its siblings, spread into `create<ProjectStore>`.
 *
 * Only the actions live here. The state they write (`warnings`, `integrityIssues`,
 * `integritySaveBlocked`, `integrityDismissed`, `integrityDialogOpen`) stays declared on
 * the store, which resets it inline on load, create and close.
 */
export interface ProjectIntegrityActions {
  /** Re-check the open project's structural integrity, updating integrityIssues. */
  checkProjectIntegrity: () => Promise<string[]>;
  /** Repair structural integrity; returns {fixed, remaining} and refreshes
   * integrityIssues from the true post-repair state (remaining>0 = detect-only
   * issues that need manual attention). */
  repairProjectIntegrity: () => Promise<IntegrityRepairResult>;
  refreshWarnings: () => Promise<void>;
  /** Resolves false when the resolve did not persist; it has said why in a toast already. */
  resolveWarning: (sectionId: number, shotId: number, warningCode: string, comment?: string) => Promise<boolean>;
  unresolveWarning: (sectionId: number, shotId: number, warningCode: string) => Promise<void>;
}

/**
 * Show the friendly read-only notice, under the key executeCommand uses, when `e` is a
 * backend edit-gate refusal (`isEditGateCode`); true when it was one. For the actions
 * here that call the IPC directly instead of going through executeCommand: their
 * refusal arrives as the rejection's CommandError.
 */
function showReadOnlyRefusal(e: unknown): boolean {
  const code = errorCode(e);
  if (!isEditGateCode(code)) return false;
  useUIStore.getState().showToast(READ_ONLY_EDIT_NOTICE(code), TOAST_KEYS.EDIT_DISABLED, 'warning');
  return true;
}

// Monotonic sequence so overlapping fire-and-forget integrity checks are
// last-issued-wins: an earlier check that resolves after a newer one was started
// must not clobber the newer result (e.g. a manual save's check racing a Repair's
// check on the same project, where the project-identity guard can't tell them apart).
let integrityCheckSeq = 0;

export function createProjectIntegrityActions(
  set: StoreApi<ProjectStore>['setState'],
  get: StoreApi<ProjectStore>['getState']
): ProjectIntegrityActions {
  return {
    refreshWarnings: async () => {
      try {
        const warnings = await invokeIpc('get_project_warnings', undefined, {
          purpose: 'refresh validation warnings',
        });
        set({ warnings });
      } catch (e) {
        console.error('Failed to refresh warnings:', e);
        set({ warnings: [] });
      }
    },

    resolveWarning: async (sectionId: number, shotId: number, warningCode: string, comment?: string): Promise<boolean> => {
      // GHOST-2: a survey mutation (resolved warnings are pushed content), see `executeCommand`.
      if (refuseMutationDuringCarry()) return false;
      try {
        await invokeIpc(
          'resolve_warning',
          { sectionId, shotId, warningCode, comment },
          { purpose: 'acknowledge validation warning', fields: { sectionId, shotId, warningCode } }
        );
        // The SpeleoDB link bits came back on the answer; `invokeIpc` applied them (#576 D22).
        await get().refreshWarnings();
        return true;
      } catch (e) {
        // Surface failure to the caller (e.g. the shot no longer exists) so the
        // resolve flow doesn't report a suppression that never persisted (#57) -
        // and say why here, once, for every caller (#576 NF-W2RO-2): a read-only
        // refusal gets the same friendly notice as every other edit, anything else
        // the resolve flow's own message.
        console.error('Failed to resolve warning:', e);
        if (!showReadOnlyRefusal(e)) {
          useUIStore.getState().showToast(i18n.t('detailPanel.warnings.resolveFailed'), TOAST_KEYS.WARNING_RESOLVE, 'error');
        }
        return false;
      }
    },

    unresolveWarning: async (sectionId: number, shotId: number, warningCode: string) => {
      if (refuseMutationDuringCarry()) return; // GHOST-2, see `resolveWarning`
      try {
        await invokeIpc(
          'unresolve_warning',
          { sectionId, shotId, warningCode },
          { purpose: 'reopen validation warning', fields: { sectionId, shotId, warningCode } }
        );
        await get().refreshWarnings(); // link bits: applied by `invokeIpc`, see `resolveWarning`
      } catch (e) {
        console.error('Failed to unresolve warning:', e);
        showReadOnlyRefusal(e); // #576 NF-W2RO-2, see `resolveWarning`
      }
    },

    checkProjectIntegrity: async () => {
      // The check is fire-and-forget (the post-load App effect, the after-every-
      // command wrapper, a failed save) and the backend scans whatever project is
      // currently loaded with no path argument. Correlate the result with the
      // project that was active when it STARTED: if the user switches or closes
      // projects while the check is in flight, a late-resolving result must not
      // overwrite the new project's freshly-reset banner/save-block state.
      // clientOpId bumps on every load (catches a switch); file_path catches a
      // close (project → null).
      const pathAtStart = get().project?.file_path ?? null;
      const opAtStart = get().loading.clientOpId;
      const mySeq = ++integrityCheckSeq;
      try {
        const result = await invokeIpc('check_project_integrity', undefined, {
          purpose: 'check project structural integrity after load',
        });
        const cur = get();
        if (
          mySeq !== integrityCheckSeq ||
          cur.project === null ||
          (cur.project.file_path ?? null) !== pathAtStart ||
          cur.loading.clientOpId !== opAtStart
        ) {
          // Drop this result if it has been superseded. Two ways:
          // 1. A NEWER check was issued after this one started (last-issued-wins) -
          //    so an earlier check can't clobber a fresher result even on the SAME
          //    project (e.g. a save's fire-and-forget check racing a Repair's check,
          //    which the project-identity guard below can't tell apart).
          // 2. The project switched/closed mid-check. The explicit project===null
          //    check is needed for the UNSAVED case: file_path stays null and
          //    closeProject doesn't bump clientOpId, so without it a late result would
          //    repopulate the banner with no project loaded (leaking onto the global view).
          return [];
        }
        const issues = result.messages;
        set((state) => {
          // Keep a dismissal sticky across re-checks (e.g. after every save) as long
          // as the issue SET is unchanged; reset it when the set genuinely changes so
          // newly-appeared issues re-show the banner. Compare order-independently:
          // the backend's message order isn't stable (orphan offset/comment messages
          // come from HashMap iteration), so a positional compare could spuriously
          // reset a dismissal - and thus resurface a banner - on a mere reorder.
          const prev = [...state.integrityIssues].sort();
          const next = [...issues].sort();
          const unchanged =
            prev.length === next.length && prev.every((v, i) => v === next[i]);
          return {
            integrityIssues: issues,
            integritySaveBlocked: result.save_blocked,
            integrityDismissed: unchanged ? state.integrityDismissed : false,
            // Keep the Review & Repair dialog flag consistent with what's actually
            // rendered (the dialog gate is integrityDialogOpen && issues.length > 0).
            // When the last issue clears - via Repair or a command that resolved it -
            // close the dialog explicitly, so the flag can't linger true-but-hidden
            // and pop the dialog back open if a later issue reappears.
            integrityDialogOpen: issues.length === 0 ? false : state.integrityDialogOpen,
          };
        });
        return issues;
      } catch (e) {
        console.error('Failed to check project integrity:', e);
        return [];
      }
    },

    repairProjectIntegrity: async () => {
      // GHOST-2: Repair mutates the survey. A refusal is thrown (the result type has no
      // failure shape, and a zero result would read as a clean repair); the caller
      // recognises it with `isMoveCarryRefusal` and shows nothing more.
      const refused = refuseMutationDuringCarry();
      if (refused) throw refused;
      try {
        const result = await invokeIpc('repair_project_integrity', undefined, {
          purpose: 'repair project structural integrity',
        });
        // Repair mutates the project; refresh derived state so the UI reflects it.
        await get().refreshProjectState();
        // Repair records a snapshot-pair undo entry, so refresh undo/redo
        // availability - the Undo button must enable so the user can revert it.
        await get().refreshUndoState();
        // Re-check rather than assuming clean: some issues are detect-only and not
        // auto-repairable (a drifted aligned loop closure - recompute/remove it;
        // non-finite shot geometry - fix the data), so reflect the TRUE post-repair
        // state instead of clearing the banner on issues that still exist.
        await get().checkProjectIntegrity();
        // A repair that actually changed the project leaves unsaved edits: mark the
        // store dirty so auto-save runs and the close prompt fires. Otherwise a
        // project that was clean (isDirty false) when the user clicked Repair still
        // looks "saved", and the fix is silently lost on close/reopen.
        //
        // Key on `result.changed`, NOT `result.fixed > 0`: the repair always
        // re-solves offsets, which heals sub-tolerance drift that never counted as a
        // violation, so a genuine fix can report fixed === 0. `changed` is the
        // backend's honest "the snapshot was mutated" signal (the same condition
        // under which it records the undo entry), so it dirties on those silent
        // fixes too while still leaving a true no-op repair clean.
        // The SpeleoDB link bits (#204/fix 8, #303: un-pushed and no longer merely
        // unwound after a repair that changed something) rode back on the answer and
        // `invokeIpc` applied them (#576 D22).
        if (result.changed) set({ isDirty: true });
        return result;
      } catch (e) {
        // #576 D39(a): the backend refuses Repair when editing is blocked (a read-only
        // SpeleoDB project, a file a newer version saved, the read-only Plan/Export
        // workspace) with the same edit-gate code every other edit gets.
        // That is a read-only notice, not a failure: show the same toast, under the
        // same key, as executeCommand does, and re-throw it so the caller neither
        // reports a repair nor shows its own "failed" toast (`isEditGateCode`).
        if (showReadOnlyRefusal(e)) throw e;
        // Do NOT fabricate a successful no-op { fixed: 0, remaining: 0 }: a thrown
        // repair (an IPC rejection during window/app teardown, a serialization
        // failure, or a backend panic) must be distinguishable from a genuine clean
        // repair. Otherwise the caller shows a fake "Repaired 0 issues" success toast
        // and swallows the error. Re-throw so the caller surfaces the failure and
        // keeps the Repair entry point open. (integrityIssues / integritySaveBlocked
        // are untouched here, so the banner and save-block stay correct.)
        console.error('Failed to repair project integrity:', e);
        throw e;
      }
    },
  };
}
