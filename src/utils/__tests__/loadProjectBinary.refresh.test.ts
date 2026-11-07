/**
 * #576 A9-D2 - the refresh after an edit decodes the load's binary envelope.
 *
 * `refreshProjectState` used to pull the JSON `get_project_state` (~50 MB at
 * 75,000 shots), `JSON.parse` it whole and rebuild the struct-of-arrays with
 * `surveyDataFromLegacy`. It now asks `get_project_state_binary`, which answers
 * with the same msgpack + typed-array envelope the load decodes. These drive the
 * real decoder through `fetchProjectStateBinary`: what it returns, that an empty
 * answer means "no project", that a foreign wire version is refused, and that it
 * shares the decode with the load without overwriting the load's perf rows.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

import { projectStateEnvelope, type ProjectStateLike } from '../../test/projectStateEnvelope';
import { invokeIpcMock as invokeIpc } from '../../test/factories/ipc';

vi.mock('../ipc');
const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', async () => (await import('../../test/factories/tauri')).tauriCore(invoke));

const { fetchProjectStateBinary, loadProjectFullBinary } = await import('../loadProjectBinary');
const { getPerfTimings } = await import('../memoryProbe');

const GPS = { latitude: 20.5, longitude: -87.25, depth_reference: 0 };

/** Two sections, three shots of every kind, a datum, a merge, comments. */
function state(): ProjectStateLike {
  return {
    project: {
      name: 'Refresh Cave',
      file_path: '/caves/refresh.stalx',
      next_section_id: 3,
      sections: [
        {
          id: 1,
          name: 'Main',
          color: '#ff0000',
          direction: 'in',
          next_shot_id: 100_002,
          shots: [
            { id: 100_000, azimuth_in: 10, azimuth_out: 12, length: 5.5, depth_in: 1, depth_out: 2, left: 0.5, comment: 'sump', kind: 'Normal' },
            { id: 100_001, azimuth_in: 90, azimuth_out: 90, length: 3, depth_in: 2, depth_out: 2, kind: 'Jump', marker_index: 4 },
          ],
        },
        {
          id: 2,
          name: 'Side',
          color: '#00ff00',
          direction: 'out',
          next_shot_id: 200_001,
          azimuth_reference: { north: 'true', correction: 1.5, source: 'manual' },
          shots: [{ id: 200_000, azimuth_in: 180, azimuth_out: 180, length: 2, depth_in: 2, depth_out: 4, kind: 'Reference' }],
        },
      ],
      zero_datums: [{ station_ref: { section_id: 1, station_index: 0 }, gps: GPS }],
      section_offsets: { 2: [3, -4] },
    },
    stations: [
      { station_ref: { section_id: 1, station_index: 0 }, coordinates: { x: 0, y: 0, z: 1 }, is_zero_datum: true, gps_coordinates: GPS },
      { station_ref: { section_id: 1, station_index: 1 }, coordinates: { x: 5, y: 0.25, z: 2 }, comment: 'knot' },
      {
        station_ref: { section_id: 1, station_index: 2 },
        coordinates: { x: 5, y: 3, z: 2 },
        merged_with: [{ section_id: 2, station_index: 0 }],
      },
      { station_ref: { section_id: 2, station_index: 0 }, coordinates: { x: 5, y: 3, z: 2 }, merged_with: [{ section_id: 1, station_index: 2 }] },
      { station_ref: { section_id: 2, station_index: 1 }, coordinates: { x: 5, y: 1, z: 4 } },
    ],
    stats: {
      section_count: 2, shot_count: 3, total_length: 10.5, min_depth: 1, max_depth: 4,
      station_count: 5, loop_count: 0, penetration_distance: 8.5,
    },
    frozen_shots: [{ section_id: 1, shot_index: 1 }],
  };
}

describe('fetchProjectStateBinary (#576 A9-D2)', () => {
  beforeEach(() => {
    invokeIpc.mockReset();
    invoke.mockReset();
  });

  it('asks get_project_state_binary and decodes project, stations, survey data, stats and frozen shots', async () => {
    invokeIpc.mockResolvedValue(projectStateEnvelope(state()));

    const result = await fetchProjectStateBinary();

    expect(invokeIpc).toHaveBeenCalledTimes(1);
    expect(invokeIpc.mock.calls[0]![0]).toBe('get_project_state_binary');
    expect(result).not.toBeNull();
    const { project, stations, surveyData, stats, frozen_shots } = result!;

    expect(project.name).toBe('Refresh Cave');
    expect(project.file_path).toBe('/caves/refresh.stalx');
    expect(project.section_offsets).toEqual({ 2: [3, -4] });
    expect(project.zero_datums).toEqual([{ station_ref: { section_id: 1, station_index: 0 }, gps: GPS }]);
    expect(project.sections.map((s) => s.id)).toEqual([1, 2]);
    // Lazy, exactly as after a load: nothing materialized until a consumer reads it.
    for (const section of project.sections) {
      expect(typeof Object.getOwnPropertyDescriptor(section, 'shots')!.get).toBe('function');
    }
    expect(project.sections[0]!.shots).toEqual([
      {
        id: 100_000, azimuth_in: 10, azimuth_out: 12, length: 5.5, depth_in: 1, depth_out: 2,
        pitch_in: null, pitch_out: null, left: 0.5, right: null, up: null, down: null,
        temperature: null, comment: 'sump', kind: 'Normal', marker_index: null,
      },
      {
        id: 100_001, azimuth_in: 90, azimuth_out: 90, length: 3, depth_in: 2, depth_out: 2,
        pitch_in: null, pitch_out: null, left: null, right: null, up: null, down: null,
        temperature: null, comment: null, kind: 'Jump', marker_index: 4,
      },
    ]);
    expect(project.sections[1]!.shots.map((s) => s.kind)).toEqual(['Reference']);
    expect(project.sections[1]!.azimuth_reference).toEqual({ north: 'true', correction: 1.5, source: 'manual' });

    expect(stations).toHaveLength(5);
    expect(stations[0]).toEqual({
      station_ref: { section_id: 1, station_index: 0 },
      coordinates: { x: 0, y: 0, z: 1 },
      is_zero_datum: true,
      gps_coordinates: GPS,
      merged_with: [],
    });
    expect(stations[1]!.comment).toBe('knot');
    expect(stations[2]!.merged_with).toEqual([{ section_id: 2, station_index: 0 }]);

    expect(surveyData.shotCount).toBe(3);
    expect(Array.from(surveyData.shotIds)).toEqual([100_000, 100_001, 200_000]);
    expect(Array.from(surveyData.shotSectionIds)).toEqual([1, 1, 2]);
    expect(Array.from(surveyData.shotLength)).toEqual([5.5, 3, 2]);
    expect(surveyData.shotComments.get(0)).toBe('sump');
    expect(surveyData.sections.map((s) => [s.id, s.shotOffset, s.shotCount])).toEqual([[1, 0, 2], [2, 2, 1]]);
    expect(surveyData.sections[1]!.azimuth_reference).toEqual({ north: 'true', correction: 1.5, source: 'manual' });
    expect(surveyData.stationCount).toBe(5);
    expect(Array.from(surveyData.stationCoords.slice(3, 6))).toEqual([5, 0.25, 2]);

    expect(stats).toEqual(state().stats);
    expect(frozen_shots).toEqual([{ section_id: 1, shot_index: 1 }]);
  });

  it('hands the refresh nothing of the load-only fields, whatever they say', async () => {
    invokeIpc.mockResolvedValue(
      projectStateEnvelope(state(), {
        loading_id: 7,
        is_remote: true,
        newer_schema_version: 99,
        speleo_link_unreadable: true,
        unreadable_project_settings: ['validation'],
        load_repairs: { shots: 3, section_offsets: 0, station_offsets: 0, gps_points: 0 },
        warnings: [{ code: 'W', message: 'from the envelope' }],
        display_settings: { showGrid: true },
      }),
    );

    const result = await fetchProjectStateBinary();

    expect(Object.keys(result!).sort()).toEqual(['frozen_shots', 'project', 'stations', 'stats', 'surveyData']);
  });

  it('an empty answer means no project is open, on every transport shape', async () => {
    for (const empty of [new ArrayBuffer(0), new Uint8Array(0), []]) {
      invokeIpc.mockResolvedValueOnce(empty);
      await expect(fetchProjectStateBinary()).resolves.toBeNull();
    }
  });

  it('decodes the Uint8Array and number-array transports like an ArrayBuffer', async () => {
    const bytes = new Uint8Array(projectStateEnvelope(state()));
    for (const raw of [bytes, Array.from(bytes)]) {
      invokeIpc.mockResolvedValueOnce(raw);
      const result = await fetchProjectStateBinary();
      expect(result!.project.name).toBe('Refresh Cave');
      expect(result!.surveyData.shotCount).toBe(3);
    }
  });

  it('refuses an envelope of another wire version instead of misdecoding it', async () => {
    invokeIpc.mockResolvedValue(projectStateEnvelope(state(), { version: 4 }));

    await expect(fetchProjectStateBinary()).rejects.toThrow(
      /get_project_state_binary version mismatch: expected 5, got 4/,
    );
  });

  it('refuses an answer that is not bytes', async () => {
    invokeIpc.mockResolvedValue({ project: null });

    await expect(fetchProjectStateBinary()).rejects.toThrow(/get_project_state_binary returned unexpected type/);
  });

  it('passes an IPC rejection through unchanged', async () => {
    invokeIpc.mockRejectedValue('Could not encode the project state: boom');

    await expect(fetchProjectStateBinary()).rejects.toBe('Could not encode the project state: boom');
  });

  it('shares the decode with the load, which keeps its own fields and perf rows', async () => {
    invokeIpc.mockResolvedValue(projectStateEnvelope(state()));
    await fetchProjectStateBinary();
    const afterRefresh = getPerfTimings();
    expect(Object.keys(afterRefresh)).toEqual(
      expect.arrayContaining(['refresh msgpack decode', 'refresh binary->JS materialize', 'refresh binary payload size (KB)']),
    );
    // The panel keeps the latest entry per label: a refresh after every edit
    // must not overwrite what the load measured.
    for (const loadRow of ['binary IPC roundtrip', 'msgpack decode', 'binary->JS materialize', 'binary payload size (KB)']) {
      expect(afterRefresh[loadRow]).toBeUndefined();
    }

    invoke.mockResolvedValue(projectStateEnvelope(state(), { loading_id: 42, is_remote: true, newer_schema_version: 13 }));
    const loaded = await loadProjectFullBinary('/caves/refresh.stalx');

    expect(invoke).toHaveBeenCalledWith('load_project_full_binary', { path: '/caves/refresh.stalx' });
    expect(loaded.loadingId).toBe(42);
    expect(loaded.isRemote).toBe(true);
    expect(loaded.newerSchemaVersion).toBe(13);
    expect(loaded.project.name).toBe('Refresh Cave');
    expect(loaded.surveyData.shotCount).toBe(3);
    expect(Object.keys(getPerfTimings())).toEqual(
      expect.arrayContaining(['binary IPC roundtrip', 'msgpack decode', 'binary->JS materialize', 'binary payload size (KB)']),
    );
  });
});
