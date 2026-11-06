/**
 * [#576 FZ-FZ-INT-4, FZ-FZ-INT-7] Every save and survey-edit IPC names the project
 * it was ISSUED against (`expectedProject`), which the backend checks under its
 * command lock and refuses with PROJECT_CHANGED when another project is open
 * (`core/state/expected_project.rs`).
 *
 * The argument is optional in the generated types, so the compiler cannot find a
 * store path that forgets it. This test drives every store path that sends one of
 * those commands - the list comes from the generated manifest, so a new command
 * without a row here fails too - and checks what the IPC carried.
 *
 * Store tier: the real store; the IPC is the shared mock. All data is synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';
import { makeProject, stationRef } from '../../test/factories/survey';
import manifest from '../../types/ipc/ipcCommands.json';
import { useProjectStore, type ProjectStore } from '../projectStore';

vi.mock('../../utils/ipc');
emptyBinaryRefresh();

const CREATED_AT = '2026-03-03T10:00:00Z';
const FILE = '/caves/CaveA.stalx';
const EXPECTED = { createdAt: CREATED_AT, filePath: FILE };

/** Every command whose IPC takes `expectedProject`, by the generated manifest. */
const TAKES_EXPECTED_PROJECT = manifest.commands
  .filter((c) => c.args.some((a) => a.key === 'expectedProject'))
  .map((c) => c.name)
  .sort();

const A = stationRef(1, 0);
const B = stationRef(1, 1);
const OFFSET = { x: 1, y: 2, z: 0 };

/** The store path that sends each command, called as the UI calls it. */
const PATHS: Record<string, (s: ProjectStore) => Promise<unknown>> = {
  create_loop_closure: (s) => s.createLoopClosure(A, B, 'Default' as never),
  create_section: (s) => s.createSection({ name: 'S', color: '#336699', shots: [] } as never),
  create_section_and_merge: (s) =>
    s.createSectionAndMerge({ name: 'S', color: '#336699', shots: [] } as never, 0, A, 'Default' as never),
  create_shot: (s) => s.createShot(1, { length: 1, azimuth_in: 0, azimuth_out: 180, depth_in: 0, depth_out: 0 } as never),
  delete_last_shot_and_section: (s) => s.deleteLastShotAndSection(1, 2),
  delete_section: (s) => s.deleteSection(1),
  delete_shot: (s) => s.deleteShot(1, 2),
  delete_shot_and_split: (s) => s.deleteShotAndSplit(1, 2, 'L', 'R'),
  delete_shot_range_shift: (s) => s.deleteShotRangeShift(1, 2, 3),
  delete_shot_range_split: (s) => s.deleteShotRangeSplit(1, 2, 3, 'L', 'R'),
  execute_batch: (s) => s.executeBatch([]),
  execute_compass_import: (s) => s.executeCompassImport('/in/a.mak'),
  execute_gpx_import: (s) => s.executeGpxImport('/in/a.gpx', null, true, true, true, [], false),
  execute_tml_import: (s) => s.executeTmlImport('/in/a.tml'),
  execute_walls_import: (s) => s.executeWallsImport('/in/a.wpj'),
  import_sections: (s) => s.importSections([]),
  merge_stations: (s) => s.mergeStations(A, B),
  move_comment: (s) => s.moveComment({ station: A } as never, { station: B } as never, false),
  move_section: (s) => s.moveSection(1, OFFSET),
  move_segment: (s) => s.moveSegment(1, OFFSET),
  move_station: (s) => s.moveStation(A, OFFSET),
  redo: (s) => s.redo(),
  redo_to: (s) => s.redoTo(0),
  remove_loop_closure: (s) => s.removeLoopClosure(1),
  remove_zero_datum: (s) => s.removeZeroDatum(A),
  repair_project_integrity: (s) => s.repairProjectIntegrity(),
  resolve_warning: (s) => s.resolveWarning(1, 2, 'W'),
  reverse_section: (s) => s.reverseSection(1, false),
  save_project: (s) => s.saveProject(),
  set_project_name_for_path: (s) => s.setProjectNameForPath(FILE, 'Renamed'),
  set_shot_kind: (s) => s.setShotKind(1, 2, 'Reference' as never),
  set_station_comment: (s) => s.updateStationComment(A, 'note'),
  set_surface_elevation: (s) => s.setSurfaceElevation(12),
  set_zero_datum: (s) => s.setZeroDatum(A, { latitude: 1, longitude: 2, elevation: 0 } as never, { fitToBounds: false }),
  split_shot_at_distance: (s) => s.splitShotAtDistance(1, 2, 0.5),
  undo: (s) => s.undo(),
  undo_to: (s) => s.undoTo(0),
  unmerge_station_pair: (s) => s.unmergeStationPair(A, B),
  unmerge_stations: (s) => s.unmergeStations(A),
  unresolve_warning: (s) => s.unresolveWarning(1, 2, 'W'),
  update_project_settings: (s) =>
    s.updateProjectSettings('Renamed', useProjectStore.getState().project!.validation as never, null),
  update_section: (s) => s.updateSection(1, { name: 'S2' } as never),
  update_shot: (s) => s.updateShot(1, 2, { length: 2 } as never),
};

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'save_project') return { saved_path: FILE, warnings: [] };
    if (command === 'repair_project_integrity') return { fixed: 0, remaining: 0, changed: false };
    if (command === 'check_project_integrity') return { messages: [], save_blocked: false };
    if (command === 'get_project_warnings' || command === 'list_recent_projects') return [];
    return {};
  });
  useProjectStore.setState({
    project: makeProject({ created_at: CREATED_AT, file_path: FILE }),
    isDirty: true,
    integrityIssues: [],
  });
});

describe('every save and edit IPC names the project it was issued against (#576 FZ-FZ-INT-4/7)', () => {
  it('has a store path for every command that takes expectedProject', () => {
    expect(Object.keys(PATHS).sort()).toEqual(TAKES_EXPECTED_PROJECT);
  });

  it.each(TAKES_EXPECTED_PROJECT)('%s carries expectedProject', async (command) => {
    const path = PATHS[command];
    expect(path, `no store path for ${command}`).toBeDefined();
    // A path's own aftermath may trip over the minimal answers; what was SENT is the point.
    await path!(useProjectStore.getState()).catch(() => undefined);
    const call = invokeIpcMock.mock.calls.find((c) => c[0] === command);
    expect(call, `${command} was not sent`).toBeDefined();
    expect((call![1] as Record<string, unknown> | undefined)?.expectedProject).toEqual(EXPECTED);
  });
});
