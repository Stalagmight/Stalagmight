import type { MockCommandFamily, MockReply } from './types';

/**
 * The placement-suggestion commands of the Playwright IPC mock: where an imported or an
 * orphaned section might attach (spec-seeded through the hooks below), how many sections an
 * import left unanchored, and the segment geometry a DMP wizard suggestion is drawn against.
 *
 * Split out of `../tauri-ipc.ts` unchanged (#576 AFK-D3). Runs IN THE PAGE: `setupTauriMocks`
 * serializes this factory with `Function.prototype.toString` into its init script, so it
 * may use only `ctx` and browser globals - never an import (type-only imports are erased).
 */
export const placementCommands: MockCommandFamily = (ctx) => {
  let mockOrphanPlacementSuggestions: MockReply<'suggest_orphan_placements'> = [];
  let mockImportPlacementSuggestions: MockReply<'suggest_all_import_placements'> = [];

  (window as unknown as Record<string, unknown>).__MOCK_SET_ORPHAN_SUGGESTIONS__ = (suggestions: unknown[]) => {
    // shape not checked: spec-seeded suggestions, passed through as given
    mockOrphanPlacementSuggestions = (Array.isArray(suggestions) ? [...suggestions] : []) as MockReply<'suggest_orphan_placements'>;
  };
  (window as unknown as Record<string, unknown>).__MOCK_RESET_ORPHAN_SUGGESTIONS__ = () => {
    mockOrphanPlacementSuggestions = [];
  };
  (window as unknown as Record<string, unknown>).__MOCK_SET_IMPORT_PLACEMENT_SUGGESTIONS__ = (suggestions: unknown[]) => {
    // shape not checked: spec-seeded suggestions, passed through as given
    mockImportPlacementSuggestions = (Array.isArray(suggestions) ? [...suggestions] : []) as MockReply<'suggest_all_import_placements'>;
  };
  (window as unknown as Record<string, unknown>).__MOCK_RESET_IMPORT_PLACEMENT_SUGGESTIONS__ = () => {
    mockImportPlacementSuggestions = [];
  };

  return (cmd, args) => {
    switch (cmd) {
      case 'suggest_import_placement':
        return [] satisfies MockReply<'suggest_import_placement'>;

      case 'suggest_all_import_placements':
        return [...mockImportPlacementSuggestions] satisfies MockReply<'suggest_all_import_placements'>;

      case 'suggest_orphan_placements':
        return [...mockOrphanPlacementSuggestions] satisfies MockReply<'suggest_orphan_placements'>;

      case 'count_unanchored_sections':
        return 0 satisfies MockReply<'count_unanchored_sections'>;

      case 'get_segment_geometry':
        return {
          sections: [
            {
              section_id: (args?.sectionId as number | undefined) ?? 1,
              color: '#4a90d9',
              shot_lines: [[[0, 0], [10, 0]]],
            },
          ],
        } satisfies MockReply<'get_segment_geometry'>;

      default:
        return ctx.unhandled;
    }
  };
};
