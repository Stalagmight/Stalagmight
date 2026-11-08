import type { MockCommandFamily, MockReply } from './types';

/**
 * The fields of the core's open project (`currentProject` in `../tauri-ipc.ts`) the raw
 * tables read beyond the `sections` the context types.
 */
type RawTableProject = {
  name: string;
  location: string | null;
  created_at: string;
  modified_at: string;
  sections: unknown[];
};

/**
 * The raw sqlite browser of the Playwright IPC mock (the data table's fullscreen view): the
 * table names and each table's columns and rows, built from the core's open project
 * (`ctx.currentProject()`).
 *
 * Split out of `../tauri-ipc.ts` unchanged (#576). Runs IN THE PAGE: `setupTauriMocks`
 * serializes this factory with `Function.prototype.toString` into its init script, so it
 * may use only `ctx` and browser globals - never an import (type-only imports are erased).
 */
export const rawTableCommands: MockCommandFamily = (ctx) => {
  return (cmd, args) => {
    switch (cmd) {
      // Raw sqlite browsing (data table fullscreen)
      case 'get_raw_table_names': {
        console.log('[Mock IPC] get_raw_table_names called');
        if (!ctx.currentProject()) {
          throw new Error('No project database; save the project to browse raw data');
        }
        // Alphabetical, mirroring list_raw_table_names' ORDER BY name.
        return ['project', 'sections', 'shots', 'undo_stack'] satisfies MockReply<'get_raw_table_names'>;
      }

      case 'get_raw_table': {
        console.log('[Mock IPC] get_raw_table called with:', args);
        if (!ctx.currentProject()) {
          throw new Error('No project database; save the project to browse raw data');
        }
        const project = ctx.currentProject() as RawTableProject;
        const table = (args?.table as string) ?? '';
        const limit = Math.min((args?.limit as number) ?? 1000, 10_000);
        const offset = (args?.offset as number) ?? 0;
        // Values are stringified "server-side" like Database::read_raw_table:
        // NULL → "NULL", blobs → a size placeholder like "<blob 4.2 KB>".
        const str = (value: unknown): string =>
          value === null || value === undefined ? 'NULL' : String(value);
        const rawSections = project.sections as Array<{
          id: number;
          name?: string | null;
          surveyor?: string | null;
          date?: string | null;
          color?: string | null;
          direction?: string;
          shots?: Array<{ id: number } & Record<string, unknown>>;
        }>;
        let columns: string[];
        let allRows: string[][];
        switch (table) {
          case 'project':
            columns = ['id', 'name', 'location', 'created_at', 'modified_at', 'schema_version', 'next_closure_id'];
            allRows = [[
              '1',
              project.name,
              str(project.location),
              project.created_at,
              project.modified_at,
              '7',
              '1',
            ]];
            break;
          case 'sections':
            columns = ['id', 'name', 'surveyor', 'date', 'color', 'direction', 'quality', 'created_at'];
            allRows = rawSections.map((section) => [
              String(section.id),
              str(section.name),
              str(section.surveyor),
              str(section.date),
              str(section.color),
              section.direction ?? 'in',
              '1',
              project.created_at,
            ]);
            break;
          case 'shots':
            columns = [
              'id', 'section_id', 'sequence_order', 'azimuth_in', 'azimuth_out', 'length',
              'depth_in', 'depth_out', 'pitch_in', 'pitch_out', 'left_distance', 'right_distance',
              'up_distance', 'down_distance', 'comment', 'kind', 'marker_index', 'temperature', 'uuid',
            ];
            allRows = rawSections.flatMap((section) =>
              (section.shots ?? []).map((shot, index) => [
                String(shot.id),
                String(section.id),
                String(index),
                str(shot.azimuth_in),
                str(shot.azimuth_out),
                str(shot.length),
                str(shot.depth_in),
                str(shot.depth_out),
                str(shot.pitch_in),
                str(shot.pitch_out),
                str(shot.left),
                str(shot.right),
                str(shot.up),
                str(shot.down),
                str(shot.comment),
                shot.kind === 'Jump' ? '1' : shot.kind === 'Reference' ? '2' : '0',
                str(shot.marker_index),
                str(shot.temperature),
                'NULL',
              ])
            );
            break;
          case 'undo_stack':
            // BLOB columns render as size placeholders, like the real backend.
            columns = ['id', 'sequence', 'command_json', 'undo_data_json', 'executed_at'];
            allRows = [['1', '1', '<blob 1.2 KB>', '<blob 4.2 KB>', project.modified_at]];
            break;
          default:
            throw new Error(`Unknown table: ${table}`);
        }
        return {
          columns,
          rows: allRows.slice(offset, offset + limit),
          total_rows: allRows.length,
        } satisfies MockReply<'get_raw_table'>;
      }

      default:
        return ctx.unhandled;
    }
  };
};
