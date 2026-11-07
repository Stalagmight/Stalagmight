/**
 * The `window` globals the Playwright specs read inside `page.evaluate`, typed
 * (#576 NF-W5LINT-2: 22 specs reached them through `(window as any)`, so a
 * renamed store action or a reshaped mock record went unnoticed until the test
 * ran). A spec writes `(window as E2EWindow).__E2E_TEST__`.
 *
 * Type-only: nothing here exists at runtime. A `page.evaluate` callback is
 * serialized into the page, so it may use these types but never import a value.
 *
 * Deliberately not a `declare global` augmentation of `Window`: several specs
 * describe their own slice of these globals as `Window & { __E2E_TEST__: ... }`,
 * and a global declaration would intersect with every one of them.
 */
import type { ConnectivityResult } from '../../src/types/ipc/ConnectivityResult';
import type { useGasPlanStore } from '../../src/store/gasPlanStore';
import type { useProjectStore } from '../../src/store/projectStore';
import type { useUIStore } from '../../src/store/uiStore';
import type { IpcOverrides } from './ipcOverrides';

/**
 * The stores `src/main.tsx` puts on `window.__E2E_TEST__` in the dev/test/E2E
 * build. Its own declaration there is optional (a production build has none);
 * here it is required, because every spec runs against the E2E build and reads
 * the stores once the app has rendered (`waitForAppReady`, or a spec's own wait
 * for the map canvas), which happens after main.tsx installed them.
 */
export interface E2ETestHooks {
  uiStore: typeof useUIStore;
  projectStore: typeof useProjectStore;
  gasPlanStore: typeof useGasPlanStore;
}

/**
 * The open project as the IPC mock (`mocks/tauri-ipc.ts`) holds it. Only the
 * fields a spec touches are named, and the shots stay `unknown[]`, so a spec
 * states the shot fields it relies on. `section_offsets` is `unknown` because the
 * mock holds it in two shapes (a `Record<sectionId, [dx, dy]>` or
 * `Array<[sectionId, [dx, dy]]>` pairs; see `sectionOffsetPairs` there).
 */
export interface MockCurrentProject {
  sections: Array<{ shots: unknown[] }>;
  section_offsets: unknown;
}

/** `window.__E2E_MOCKS__`, installed by `mocks/tauri-ipc.ts`; the project is null until one is open. */
export interface E2EMocks {
  getCurrentProject: () => MockCurrentProject | null;
  setCurrentProject: (project: MockCurrentProject | null) => void;
}

/**
 * `typeof window` plus the globals, so `window as E2EWindow` is a plain downcast
 * from the global's own type (no `unknown` hop).
 */
export type E2EWindow = typeof window & {
  __E2E_TEST__: E2ETestHooks;
  /** Optional: only a page set up with `setupTauriMocks` has it. */
  __E2E_MOCKS__?: E2EMocks;
  /**
   * Command -> canned reply (or `{ __throw: message }`), read by the mock's
   * dispatcher before its own handlers; see helpers/ipcOverrides.ts.
   */
  __MOCK_IPC_OVERRIDES__?: IpcOverrides;
  /**
   * What `check_merge_connectivity` returns instead of the mock's default, passed
   * through as given - so it is a whole `ConnectivityResult` (#576 NF-W7F-3: a reply
   * without `can_merge` / `reason` is not one the backend sends, and the merge tools
   * read both).
   */
  __MOCK_CONNECTIVITY_RESULT__?: ConnectivityResult;
  /**
   * What `plan_refs_for_anchors` answers, per anchored station (`'sectionId:stationIndex'`),
   * after a survey edit - a spec's stand-in for the backend's re-resolution (#576 E3);
   * any other anchor resolves to the station it was taken from (mocks/ipc/gasPlan.ts).
   */
  __MOCK_SET_ANCHOR_RESOLUTION__?: (
    resolution: Record<string, { section_id: number; station_index: number } | null>,
  ) => void;
  /** The mock's call logs: each entry is one call's `commands` (execute_batch) or `args`. */
  __MOCK_EXECUTE_BATCH_CALLS__?: unknown[];
  __MOCK_CREATE_SHOT_CALLS__?: unknown[];
  __MOCK_SPLIT_SHOT_CALLS__?: unknown[];
  __MOCK_DATA_TABLE_VIEW_CALLS__?: unknown[];
};
