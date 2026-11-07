import type { MockCommandFamily, MockReply } from './types';

/**
 * The Walls import pair of the Playwright IPC mock: `parse_walls` serves a canned one-survey
 * project, and `execute_walls_import` records what the dialog sent and answers a
 * one-section receipt without landing anything in the core's project.
 *
 * Split out of `../tauri-ipc.ts` unchanged (#576 AFK-D3). Runs IN THE PAGE: `setupTauriMocks`
 * serializes this factory with `Function.prototype.toString` into its init script, so it
 * may use only `ctx` and browser globals - never an import (type-only imports are erased).
 */
export const wallsImportCommands: MockCommandFamily = (ctx) => {
  // Records execute_walls_import args (esp. selectedSurveys) so the Walls
  // import dialog spec can assert the select-all→null / explicit-index path.
  let wallsImportCalls: Array<Record<string, unknown>> = [];

  (window as unknown as Record<string, unknown>).__MOCK_GET_WALLS_IMPORT_CALLS__ = () => [...wallsImportCalls];
  (window as unknown as Record<string, unknown>).__MOCK_RESET_WALLS_IMPORT_CALLS__ = () => {
    wallsImportCalls = [];
  };

  return (cmd, args) => {
    switch (cmd) {
      case 'execute_walls_import': {
        // Record the import args (selectedSurveys, includeSplays, …) for
        // assertions, then return a one-section success so the dialog runs
        // its "imported" exit path (not the "nothing imported" branch).
        wallsImportCalls.push({ ...(args ?? {}) });
        const wallsPath = (args?.path as string | undefined) ?? '';
        const wallsFileName = wallsPath.split('/').pop() ?? wallsPath;
        return {
          warnings: [],
          remote_link: ctx.remoteLink(),
          section_ids: [1],
          import_receipt: {
            source_format: 'Walls',
            file_name: wallsFileName,
            sections_found: 1,
            sections_imported: 1,
            shot_count: 0,
            closures_requested: 0,
            closures_created: 0,
            unit_conversion: null,
            warnings: [],
            info: [],
            zero_datum_created: null,
          },
        } satisfies MockReply<'execute_walls_import'>;
      }

      // A one-survey Walls project, canned: what the parser reads out of a real file
      // is pinned by the parsers crate's Walls tests.
      case 'parse_walls':
        return {
          project_name: 'CaveA',
          book_count: 1,
          survey_count: 1,
          resolved_survey_count: 1,
          shot_count: 3,
          splay_count: 0,
          has_georeference: false,
          datum: null,
          utm_zone: null,
          fixed_station_count: 0,
          missing_file_count: 0,
          skipped_attachment_count: 0,
          any_feet_units: false,
          surveys: [
            {
              name: 'A1',
              title: 'Survey A1',
              date: '2024-03-01',
              segment: '',
              shot_count: 3,
              splay_count: 0,
              fixed_station_count: 0,
              resolved: true,
            },
          ],
          warnings: [],
        } satisfies MockReply<'parse_walls'>;

      default:
        return ctx.unhandled;
    }
  };
};
