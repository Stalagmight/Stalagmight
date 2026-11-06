/**
 * What a save's answer may change once it lands.
 *
 * [#576 FZ-FZ-INT-4] A save's answer was applied to whichever project was open when
 * it landed: a Save As of one project re-pointed the project open by then at the
 * Save As file, and cleared its dirty flag. The answer now applies only if the
 * session and the project it was issued for are unchanged.
 *
 * Store tier: the real store and the real refresh decoder; the IPC is mocked and its
 * answers are released by hand, so the test decides the order. All data is synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { deferred, timerTurns, type Deferred } from '../../test/factories/async';
import { invokeIpcMock } from '../../test/factories/ipc';
import { makeProject } from '../../test/factories/survey';
import { noteProjectSessionChanged } from '../projectSession';
import { useProjectStore } from '../projectStore';

vi.mock('../../utils/ipc');

const CREATED_AT = '2026-08-01T10:00:00Z';
const OLD_FILE = '/caves/CaveA.stalx';
const NEW_FILE = '/caves/saved/CaveA-copy.stalx';

let save: Deferred<unknown>;
/** The state reads in flight, oldest first. */
let reads: Array<Deferred<ArrayBuffer>>;

beforeEach(() => {
  vi.clearAllMocks();
  save = deferred();
  reads = [];
  invokeIpcMock.mockImplementation((command: string) => {
    if (command === 'save_project') return save.promise;
    if (command === 'get_project_state_binary') {
      const read = deferred<ArrayBuffer>();
      reads.push(read);
      return read.promise;
    }
    if (command === 'check_project_integrity') return Promise.resolve({ messages: [], save_blocked: false });
    if (command === 'get_project_warnings') return Promise.resolve([]);
    return Promise.resolve(false);
  });
  useProjectStore.setState({
    project: makeProject({ created_at: CREATED_AT, file_path: OLD_FILE }),
    isDirty: true,
    integrityIssues: [],
  });
});

describe("a save's answer applies only to the project it was issued for (#576 FZ-FZ-INT-4)", () => {
  it('a Save As answering after the session changed re-points nothing and clears nothing', async () => {
    const saving = useProjectStore.getState().saveProject(NEW_FILE);
    await timerTurns(3);
    // Another project opened while the save was in flight.
    noteProjectSessionChanged();
    const other = makeProject({ created_at: '2026-08-02T10:00:00Z', file_path: '/caves/CaveB.stalx' });
    useProjectStore.setState({ project: other, isDirty: true });

    save.resolve({ saved_path: NEW_FILE, warnings: [] });
    await saving;
    await timerTurns(5);

    expect(useProjectStore.getState().project?.file_path).toBe('/caves/CaveB.stalx');
    expect(useProjectStore.getState().isDirty).toBe(true);
  });
});
