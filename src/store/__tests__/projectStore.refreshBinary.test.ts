/**
 * #576 A9-D2 - `refreshProjectState` refreshes through the binary envelope.
 *
 * After every edit the store used to pull the JSON `get_project_state` and rebuild
 * `surveyData` with `surveyDataFromLegacy`. It now asks `get_project_state_binary`
 * and takes project, stations, surveyData, stats and frozen shots from the same
 * decoder the load uses. Store tier: the real store and the real decoder, with the
 * IPC mocked and the envelope built here from synthetic data.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { projectStateEnvelope, type ProjectStateLike } from '../../test/projectStateEnvelope';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

/** What the warnings refresh answers, so a test can see it ran. */
const REFRESHED_WARNINGS = [{ code: 'from-the-warnings-refresh' }];

function state(name = 'After Edit'): ProjectStateLike {
  return {
    project: {
      name,
      file_path: '/caves/after.stalx',
      sections: [
        {
          id: 1,
          name: 'Main',
          color: '#ff0000',
          azimuth_reference: { north: 'grid', correction: -0.5, source: 'manual' },
          shots: [
            { id: 100_000, azimuth_in: 10, azimuth_out: 10, length: 4, depth_in: 0, depth_out: 1, kind: 'Normal' },
            { id: 100_001, azimuth_in: 20, azimuth_out: 20, length: 6, depth_in: 1, depth_out: 3, kind: 'Normal' },
          ],
        },
      ],
    },
    stations: [0, 1, 2].map((i) => ({
      station_ref: { section_id: 1, station_index: i },
      coordinates: { x: i * 5, y: 0, z: i },
    })),
    stats: {
      section_count: 1, shot_count: 2, total_length: 10, min_depth: 0, max_depth: 2,
      station_count: 3, loop_count: 0, penetration_distance: 10,
    },
    frozen_shots: [{ section_id: 1, shot_index: 0 }],
  };
}

/** Serve the refresh: the binary state, the undo flags, the warnings. */
function serve(binary: ArrayBuffer): string[] {
  const calls: string[] = [];
  invokeIpcMock.mockImplementation(async (command: string) => {
    calls.push(command);
    switch (command) {
      case 'get_project_state_binary':
        return binary;
      case 'can_undo':
        return true;
      case 'can_redo':
        return false;
      default:
        return REFRESHED_WARNINGS;
    }
  });
  return calls;
}

const BEFORE = {
  name: 'Before Edit',
  location: null,
  created_at: '2026-01-01T00:00:00Z',
  modified_at: '2026-01-01T00:00:00Z',
  sections: [],
  station_merges: [],
  zero_datums: [],
  loop_closures: [],
  file_path: '/caves/after.stalx',
  next_section_id: 1,
  section_offsets: {},
  imported_file_hashes: [],
  imported_section_hashes: [],
  next_closure_id: 1,
};
const STALE_ADJUSTED = { stale: true };

beforeEach(() => {
  vi.clearAllMocks();
  useProjectStore.setState({
    project: BEFORE as never,
    stations: [],
    surveyData: null,
    stationsAdjusted: [] as never,
    surveyDataAdjusted: STALE_ADJUSTED as never,
    stats: null,
    frozenShots: [],
    warnings: [],
    canUndo: false,
    canRedo: false,
    newerSchemaVersion: null,
    unreadableProjectSettings: [],
    speleoLinkUnreadable: false,
  });
});

describe('refreshProjectState through get_project_state_binary (#576 A9-D2)', () => {
  it('takes project, stations, survey data, stats and frozen shots from the binary decode', async () => {
    const calls = serve(projectStateEnvelope(state()));
    const generationBefore = useProjectStore.getState().projectTreeGeneration;

    await useProjectStore.getState().refreshProjectState();

    const s = useProjectStore.getState();
    expect(calls).toContain('get_project_state_binary');
    expect(calls).not.toContain('get_project_state');
    expect(s.project!.name).toBe('After Edit');
    expect(s.projectTreeGeneration).toBeGreaterThan(generationBefore);
    // The decoder's lazy sections, as after a load.
    expect(typeof Object.getOwnPropertyDescriptor(s.project!.sections[0]!, 'shots')!.get).toBe('function');
    expect(s.project!.sections[0]!.shots.map((shot) => shot.length)).toEqual([4, 6]);
    expect(s.stations).toHaveLength(3);
    expect(s.stations[2]!.coordinates).toEqual({ x: 10, y: 0, z: 2 });
    expect(s.surveyData!.shotCount).toBe(2);
    expect(Array.from(s.surveyData!.shotLength)).toEqual([4, 6]);
    expect(s.surveyData!.stationCount).toBe(3);
    // The decoder's survey data, not a rebuild from the legacy shapes:
    // `surveyDataFromLegacy` never carried a section's north reference.
    expect(s.surveyData!.sections[0]!.azimuth_reference).toEqual({ north: 'grid', correction: -0.5, source: 'manual' });
    expect(s.stationsAdjusted).toBeNull();
    expect(s.surveyDataAdjusted).toBeNull();
    expect(s.stats).toEqual(state().stats);
    expect(s.frozenShots).toEqual([{ section_id: 1, shot_index: 0 }]);
    // The undo flags and warnings still refresh after it.
    expect(s.canUndo).toBe(true);
    expect(s.warnings).toEqual(REFRESHED_WARNINGS);
  });

  it('ignores the load-only fields of the envelope', async () => {
    serve(
      projectStateEnvelope(state(), {
        is_remote: true,
        newer_schema_version: 99,
        speleo_link_unreadable: true,
        unreadable_project_settings: ['validation'],
        warnings: [{ code: 'from-the-envelope' }],
      }),
    );

    await useProjectStore.getState().refreshProjectState();

    const s = useProjectStore.getState();
    expect(s.project!.name).toBe('After Edit');
    expect(s.newerSchemaVersion).toBeNull();
    expect(s.unreadableProjectSettings).toEqual([]);
    expect(s.speleoLinkUnreadable).toBe(false);
    expect(s.warnings).toEqual(REFRESHED_WARNINGS);
  });

  it('leaves the store as it is on an empty answer (no project open), and still refreshes undo and warnings', async () => {
    const calls = serve(new ArrayBuffer(0));

    await useProjectStore.getState().refreshProjectState();

    const s = useProjectStore.getState();
    expect(s.project).toBe(BEFORE);
    expect(s.surveyDataAdjusted).toBe(STALE_ADJUSTED);
    expect(calls).toEqual(expect.arrayContaining(['get_project_state_binary', 'can_undo', 'can_redo']));
    expect(s.canUndo).toBe(true);
    expect(s.warnings).toEqual(REFRESHED_WARNINGS);
  });

  it('logs a refused envelope and changes nothing, as any refresh failure does', async () => {
    const calls = serve(projectStateEnvelope(state(), { version: 4 }));
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    await useProjectStore.getState().refreshProjectState();

    const s = useProjectStore.getState();
    expect(errorSpy).toHaveBeenCalledWith('Failed to refresh project state:', expect.any(Error));
    expect(String(errorSpy.mock.calls[0]![1])).toMatch(/version mismatch: expected 5, got 4/);
    expect(s.project).toBe(BEFORE);
    expect(calls).toEqual(['get_project_state_binary']);
    errorSpy.mockRestore();
  });
});
