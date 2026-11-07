import type { CommandError } from '../../../src/types/ipc/CommandError';
import type { MockCommandFamily, MockReply } from './types';

/**
 * The recent-projects commands of the Playwright IPC mock: the welcome screen's list, with
 * the renames and deletes a test makes, and the survey geometry the global view draws for
 * each recent file, with its error and delay hooks.
 *
 * Split out of `../tauri-ipc.ts` unchanged (#576 AFK-D3). Runs IN THE PAGE: `setupTauriMocks`
 * serializes this factory with `Function.prototype.toString` into its init script, so it
 * may use only `ctx` and browser globals - never an import (type-only imports are erased).
 */
export const recentProjectsCommands: MockCommandFamily = (ctx) => {
  // Geometry loading mock control
  // Paths listed here will fail geometry loading (throw error instead of returning data)
  const geometryErrorPaths = new Set<string>();
  // Paths listed here will delay geometry loading (resolve after delay ms)
  const geometryDelayPaths = new Map<string, number>();

  (window as unknown as Record<string, unknown>).__MOCK_SET_GEOMETRY_ERROR__ = (path: string) => {
    geometryErrorPaths.add(path);
  };
  (window as unknown as Record<string, unknown>).__MOCK_CLEAR_GEOMETRY_ERROR__ = (path: string) => {
    geometryErrorPaths.delete(path);
  };
  (window as unknown as Record<string, unknown>).__MOCK_SET_GEOMETRY_DELAY__ = (path: string, delayMs: number) => {
    geometryDelayPaths.set(path, delayMs);
  };
  (window as unknown as Record<string, unknown>).__MOCK_CLEAR_GEOMETRY_DELAY__ = (path: string) => {
    geometryDelayPaths.delete(path);
  };

  // Per-path project-name overrides applied by set_project_name_for_path,
  // so list_recent_projects reflects renames within a test; and the paths
  // delete_project removed, which drop out of it as the backend's do.
  const recentNameOverrides: Record<string, string> = {};
  const deletedProjectPaths = new Set<string>();

  return (cmd, args) => {
    switch (cmd) {
      case 'list_recent_projects':
        // `station_count: 0` / `is_remote: false` are what the UI read (`?? 0`, falsy)
        // while these rows omitted the two fields the backend always sends.
        return ([
          {
            name: 'Cave System Alpha',
            location: 'Yucatan, Mexico',
            file_path: '/projects/cave-alpha.stalx',
            modified_at: new Date().toISOString(),
            section_count: 5,
            total_length: 1250.5,
            station_count: 0,
            is_remote: false,
            gps_center: [-87.4631, 20.2137],
            gps_bounds: {
              north: 20.2200,
              south: 20.2074,
              east: -87.4568,
              west: -87.4694,
            },
          },
          {
            name: 'Underwater Cave Beta',
            location: 'Florida, USA',
            file_path: '/projects/cave-beta.stalx',
            modified_at: new Date(Date.now() - 86400000).toISOString(),
            section_count: 3,
            total_length: 820.0,
            station_count: 0,
            is_remote: false,
            gps_center: [-82.4584, 29.6516],
            gps_bounds: {
              north: 29.6550,
              south: 29.6482,
              east: -82.4521,
              west: -82.4647,
            },
          },
          {
            name: 'Deep Cave Gamma',
            location: 'Bahamas',
            file_path: '/projects/cave-gamma.stalx',
            modified_at: new Date(Date.now() - 172800000).toISOString(),
            section_count: 8,
            total_length: 2100.3,
            station_count: 0,
            is_remote: false,
            gps_center: [-77.3963, 25.0343],
            gps_bounds: null,
          },
          {
            name: 'Local Survey (No GPS)',
            location: 'Unknown',
            file_path: '/projects/local-survey.stalx',
            modified_at: new Date(Date.now() - 259200000).toISOString(),
            section_count: 2,
            total_length: 350.0,
            station_count: 0,
            is_remote: false,
            gps_center: null,
            gps_bounds: null,
          },
        ] satisfies MockReply<'list_recent_projects'>).filter((p) => !deletedProjectPaths.has(p.file_path)).map((p) =>
          recentNameOverrides[p.file_path]
            ? { ...p, name: recentNameOverrides[p.file_path]! }
            : p
        ) satisfies MockReply<'list_recent_projects'>;

      case 'remove_from_recent_projects':
        // Mock remove from recent - just return success
        console.log('[Mock IPC] remove_from_recent_projects called with:', args);
        return { warnings: [] } satisfies MockReply<'remove_from_recent_projects'>;

      case 'delete_project':
        // Mock destructive delete: like the backend, the file leaves the
        // recents list. Per-page state, so nothing leaks into another test.
        deletedProjectPaths.add(String(args?.path ?? ''));
        // `session_closed: false`: the mock never tears down the open session on a
        // delete (the reply before #576 C4 left the field out, which read as false).
        return { session_closed: false, warnings: [] } satisfies MockReply<'delete_project'>;

      case 'set_project_name_for_path': {
        const p = String(args?.path ?? '');
        const name = String(args?.name ?? '').trim();
        if (!name) return Promise.reject({ code: 'FAILED', message: 'Project name cannot be empty' } satisfies CommandError);
        recentNameOverrides[p] = name;
        return { warnings: [], remote_link: ctx.remoteLink() } satisfies MockReply<'set_project_name_for_path'>;
      }

      case 'get_survey_geometry_for_file': {
        const filePath = args?.path as string;
        console.log('[Mock IPC] get_survey_geometry_for_file called for:', filePath);

        // Check if this path should simulate an error
        if (geometryErrorPaths.has(filePath)) {
          throw new Error(`Failed to load geometry for ${filePath}`);
        }

        // Check if this path should be delayed
        const delay = geometryDelayPaths.get(filePath);
        // Return a SurveyGeometry-conformant payload matching src/types/survey.ts
        const mockGeometry: MockReply<'get_survey_geometry_for_file'> = {
          origin: [-82.46, 24.93] as [number, number],
          shots: [
            { from: [-82.46, 24.93] as [number, number], to: [-82.4599, 24.9301] as [number, number], kind: 'Normal', section_id: 1 },
            { from: [-82.4599, 24.9301] as [number, number], to: [-82.4598, 24.9302] as [number, number], kind: 'Normal', section_id: 1 },
          ],
        };

        if (delay && delay > 0) {
          return new Promise((resolve) => setTimeout(() => resolve(mockGeometry), delay)) satisfies Promise<MockReply<'get_survey_geometry_for_file'>>;
        }

        return mockGeometry satisfies MockReply<'get_survey_geometry_for_file'>;
      }

      default:
        return ctx.unhandled;
    }
  };
};
