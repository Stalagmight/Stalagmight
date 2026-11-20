import type { StoreApi } from 'zustand';
import { invokeIpc } from '../utils/ipc';
import type { ProjectStore } from './projectStore';

/**
 * What the open project's FILE says about it, beside the survey (`.stalx` v2,
 * plan 6.4 and 7.2.10): whether it was made with a newer Stalagmight (so it is
 * open read-only), and the one-time notice a legacy file's conversion left.
 *
 * Split out of `projectStore.ts` for the reason `projectHistoryBannerActions.ts`
 * gives (that file is held by the TS file-size ratchet). Same `(set, get)` shape,
 * spread into `create<ProjectStore>`. The STATE is the store's: it resets it on
 * load, create and close by spreading {@link FILE_STATUS_CLEARED}.
 */
export interface ProjectFileStatusState {
  /** Why the open file is read-only ("made with a newer Stalagmight": a feature
   *  or survey field this build does not know), or null when the file allows
   *  edits. The backend refuses every edit, undo and save of such a file with
   *  `READ_ONLY_FILE: ...`; the frontend gates its editing controls on it
   *  (`useEditingBlocked`) and shows a banner. */
  fileReadOnlyReason: string | null;
  /** The notice a legacy (SQLite) open leaves when the conversion moved
   *  stations ("Converted from the old file format; ..."), shown as a banner
   *  until dismissed. The backend hands it over once. */
  conversionNotice: string | null;
}

export interface ProjectFileStatusActions {
  /** Ask the backend for the open file's status: the read-only reason (a peek)
   *  and the conversion notice (taken once). */
  checkFileStatus: () => Promise<void>;
  /** Hide the conversion notice for the open project. */
  dismissConversionNotice: () => void;
}

/** The cleared state, as ONE spreadable value (see `HISTORY_BANNER_CLEARED`). */
export const FILE_STATUS_CLEARED = {
  fileReadOnlyReason: null,
  conversionNotice: null,
} as const;

/** Last-issued-wins, as for the other post-load checks. */
let fileStatusCheckSeq = 0;

export function createProjectFileStatusActions(
  set: StoreApi<ProjectStore>['setState'],
  get: StoreApi<ProjectStore>['getState'],
): ProjectFileStatusActions {
  return {
    checkFileStatus: async () => {
      // Correlated with the project open when the check STARTED, exactly as
      // checkHistoryDiscarded does: a reply that lands after a switch or a
      // close must not paint the previous file's status over the new one.
      const pathAtStart = get().project?.file_path ?? null;
      const opAtStart = get().loading.clientOpId;
      const mySeq = ++fileStatusCheckSeq;
      const stale = () => {
        const cur = get();
        return (
          mySeq !== fileStatusCheckSeq ||
          cur.project === null ||
          (cur.project.file_path ?? null) !== pathAtStart ||
          cur.loading.clientOpId !== opAtStart
        );
      };
      try {
        const readOnly = await invokeIpc<string | null>('project_read_only_reason', undefined, {
          purpose: 'learn whether the open file was made with a newer Stalagmight',
        });
        if (!stale()) set({ fileReadOnlyReason: readOnly ?? null });
      } catch (e) {
        console.error('Failed to read the project read-only reason:', e);
      }
      try {
        // A take: the backend answers it once per open. A second check (React
        // StrictMode runs the effect twice) gets null and must not clear it.
        const notice = await invokeIpc<string | null>('take_conversion_notice', undefined, {
          purpose: 'show the old-format conversion notice once',
        });
        if (notice && !stale()) set({ conversionNotice: notice });
      } catch (e) {
        console.error('Failed to read the conversion notice:', e);
      }
    },

    dismissConversionNotice: () => set({ conversionNotice: null }),
  };
}
