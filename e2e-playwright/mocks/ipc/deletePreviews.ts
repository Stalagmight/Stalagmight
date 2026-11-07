import type { MockCommandFamily, MockReply } from './types';

/**
 * The shot-deletion previews of the Playwright IPC mock (`preview_delete_shot`,
 * `preview_delete_shot_range`): canned impact reports for the delete dialogs. The deletes
 * themselves stay in the core, which owns the project they change.
 *
 * Split out of `../tauri-ipc.ts` unchanged (#576 AFK-D3). Runs IN THE PAGE: `setupTauriMocks`
 * serializes this factory with `Function.prototype.toString` into its init script, so it
 * may use only `ctx` and browser globals - never an import (type-only imports are erased).
 */
export const deletePreviewCommands: MockCommandFamily = (ctx) => {
  return (cmd, args) => {
    switch (cmd) {
      case 'preview_delete_shot': {
        console.log('[Mock IPC] preview_delete_shot called with:', args);
        const sectionId = (args?.sectionId as number | undefined) ?? 1;
        return {
          removed_station: { section_id: sectionId, station_index: 0 },
          removed_merges: [
            {
              station_a: { section_id: sectionId, station_index: 0 },
              station_b: { section_id: sectionId, station_index: 1 },
            },
          ],
          removed_closures: [
            {
              closure_id: 1,
              station_a: { section_id: sectionId, station_index: 0 },
              station_b: { section_id: sectionId, station_index: 2 },
            },
          ],
          replacement_merges: [],
          replacement_closure_updates: [],
          closure_to_merge: [],
          merge_rewire_skipped: false,
          closure_rewire_skipped: true,
          shift_blocked: false,
          shift_blocked_reasons: [],
        } satisfies MockReply<'preview_delete_shot'>;
      }

      case 'preview_delete_shot_range': {
        console.log('[Mock IPC] preview_delete_shot_range called with:', args);
        const sectionId = (args?.sectionId as number | undefined) ?? 1;
        return {
          section_id: sectionId,
          start_pos: 1,
          end_pos: 2,
          kind: 'interior',
          removed_stations: [{ section_id: sectionId, station_index: 2 }],
          removed_merges: [
            {
              station_a: { section_id: sectionId, station_index: 1 },
              station_b: { section_id: sectionId, station_index: 2 },
            },
          ],
          removed_closures: [],
          replacement_merges: [],
          replacement_closure_updates: [],
          closure_to_merge: [],
          merge_rewire_skipped: false,
          closure_rewire_skipped: false,
          shift_blocked: false,
          shift_blocked_reasons: [],
          removed_datums: [],
          removed_comments: [],
        } satisfies MockReply<'preview_delete_shot_range'>;
      }

      default:
        return ctx.unhandled;
    }
  };
};
