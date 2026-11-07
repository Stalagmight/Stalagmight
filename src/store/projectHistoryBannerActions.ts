import type { StoreApi } from 'zustand';
import { invokeIpc } from '../utils/ipc';
import type { ProjectStore } from './projectStore';

/**
 * [#473] The discarded-history banner's two actions.
 *
 * Split out of `projectStore.ts` rather than rebaselined, for the reason
 * `projectConfigActions.ts` gives and #157 requires: that file is already past the
 * 1500-LOC ratchet, so it may shrink but not grow, and a file at that size is to be split
 * at an API boundary instead of having its baseline raised. Same `(set, get)` factory
 * shape as its sibling, spread into `create<ProjectStore>`.
 *
 * The slice's STATE is declared here too, but separately: the store owns those fields
 * (it resets them inline on load, create and close), so the factory returns actions only.
 */
export interface ProjectHistoryBannerState {
  /** [#473] True when the open project's persisted undo/redo history could not be read
   * and was replaced by an empty stack. See `AppState::history_discarded` for why ONE
   * flag serves both stacks. */
  historyDiscarded: boolean;
  /** True once the user dismissed the discarded-history banner for the OPEN project.
   * Frontend-only, like `integrityDismissed`: the backend flag describes an ongoing
   * condition and keeps answering true, so the dismissal has to live here. */
  historyDiscardedDismissed: boolean;
}

export interface ProjectHistoryBannerActions {
  /** [#473] Ask the backend whether the open project lost its history on load. */
  checkHistoryDiscarded: () => Promise<void>;
  /** Hide the discarded-history banner for the open project. */
  dismissHistoryDiscarded: () => void;
}

/**
 * Monotonic sequence so overlapping fire-and-forget checks are last-issued-wins, exactly
 * as `integrityCheckSeq` does for the integrity banner: an earlier check that resolves
 * after a newer one was started must not clobber the newer result.
 */
let historyDiscardedCheckSeq = 0;

/**
 * The banner's cleared state, as ONE spreadable value.
 *
 * Five places reset it - initial state, close, load, and two creates - and a review of
 * this feature flagged exactly that as the risk: five sites that must each remember, so a
 * sixth added later is silently unguarded. Spreading one constant is what makes the sixth
 * correct by construction.
 */
export const HISTORY_BANNER_CLEARED = {
  historyDiscarded: false,
  historyDiscardedDismissed: false,
} as const;

export function createProjectHistoryBannerActions(
  set: StoreApi<ProjectStore>['setState'],
  get: StoreApi<ProjectStore>['getState'],
): ProjectHistoryBannerActions {
  return {
    // [#473] Peek, don't take: the backend flag describes an ongoing condition and
    // keeps answering the same thing for as long as the project is open, so this is
    // safe to call more than once (React StrictMode runs the effect twice in dev).
    // A failure here is left as "no banner" deliberately - the condition it reports
    // is itself a degraded read, and a second failure must not escalate into a
    // broken project view.
    checkHistoryDiscarded: async () => {
      // Correlate the reply with the project that was active when the check STARTED,
      // exactly as checkProjectIntegrity does and for the same reason. The backend flag is
      // a live read of whatever project is in the slot, which makes the VALUE honest at
      // service time but says nothing about DELIVERY: project A discarded -> check in
      // flight -> user switches to B -> loadProject resets this to false -> A's reply lands
      // `true` and paints a false data-loss banner over B. clientOpId bumps on every load
      // (catches a switch); the explicit project===null catches a close, which leaves
      // file_path null and does not bump clientOpId.
      const pathAtStart = get().project?.file_path ?? null;
      const opAtStart = get().loading.clientOpId;
      const mySeq = ++historyDiscardedCheckSeq;
      try {
        const discarded = await invokeIpc('history_discarded', undefined, {
          purpose: 'report a history load that was discarded',
        });
        const cur = get();
        if (
          mySeq !== historyDiscardedCheckSeq ||
          cur.project === null ||
          (cur.project.file_path ?? null) !== pathAtStart ||
          cur.loading.clientOpId !== opAtStart
        ) {
          return;
        }
        set((state) => ({
          historyDiscarded: discarded,
          // Clear a stale dismissal when the condition clears, so a LATER project
          // that does lose its history still gets a banner.
          historyDiscardedDismissed: discarded ? state.historyDiscardedDismissed : false,
        }));
      } catch (e) {
        console.error('Failed to read the discarded-history flag:', e);
      }
    },

    dismissHistoryDiscarded: () => set({ historyDiscardedDismissed: true }),
  };
}
