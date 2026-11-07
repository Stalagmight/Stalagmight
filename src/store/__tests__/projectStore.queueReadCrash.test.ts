/**
 * [#576 RV5-6] A read that crashed in the backend does not wedge the edit queue.
 *
 * Edits run one at a time on the mutation queue (`runSerialized`), and each one's
 * aftermath - the refresh's `can_undo` / `can_redo` and warnings reads, the integrity
 * re-check - is awaited inside its queue entry, as is every workspace-mode send. Those
 * commands used to have no failure shape: a body that panicked on the backend's pool
 * was re-raised and the call never answered, so the entry never settled and every
 * later edit waited behind it, silently. They now answer the crash as an
 * `INTERNAL_ERROR` rejection; this pins that the frontend takes that rejection in its
 * stride - the edit that ran still settles as the success it was, and the next edit
 * still reaches the backend.
 *
 * The real `invokeIpc` and store run; only Tauri's `invoke` is mocked.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { sendWorkspaceMode } from '../workspaceModeSync';
import { projectStateEnvelope } from '../../test/projectStateEnvelope';
import type { CommandError } from '../../types/ipc/CommandError';

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', async () => (await import('../../test/factories/tauri')).tauriCore(invokeMock));

/** What the pool answers for a body that panicked (`pool::run`'s `PanicReply`). */
const CRASH: CommandError = {
  code: 'INTERNAL_ERROR',
  message: 'The operation failed unexpectedly (task 7 panicked)',
};

const PROJECT = {
  name: 'CaveA',
  location: null,
  created_at: '2026-01-01T00:00:00Z',
  modified_at: '2026-01-01T00:00:01Z',
  sections: [],
  station_merges: [],
  zero_datums: [],
  loop_closures: [],
  file_path: '/tmp/cavea.stalx',
  next_section_id: 1,
  section_offsets: {},
  imported_file_hashes: [],
  imported_section_hashes: [],
};

/**
 * A healthy backend, except that `crashes(command)` decides which calls crash: those
 * reject with {@link CRASH}. The edit itself and the project refresh always answer.
 */
function backend(crashes: (command: string) => boolean) {
  invokeMock.mockImplementation(async (command: string) => {
    if (command === 'move_section') return { warnings: [], remote_link: null };
    if (command === 'get_project_state_binary') {
      return projectStateEnvelope({ project: PROJECT, stations: [], stats: null, frozen_shots: [] });
    }
    if (crashes(command)) throw CRASH;
    switch (command) {
      case 'can_undo':
      case 'can_redo':
        return true;
      case 'check_project_integrity':
        return { messages: [], save_blocked: false };
      case 'set_workspace_mode':
        return null;
      default:
        // The warnings read: no warnings.
        return [];
    }
  });
}

const edits = () => invokeMock.mock.calls.filter(([command]) => command === 'move_section').length;

/** Two edits, the second issued once the first settled; both must settle and land. */
async function twoEdits() {
  const first = await useProjectStore.getState().moveSection(1, { x: 1, y: 0, z: 0 });
  expect(first.success, 'the first edit ran; a crashed read after it does not make it a failure').toBe(true);
  expect(edits()).toBe(1);
  const second = await useProjectStore.getState().moveSection(1, { x: 2, y: 0, z: 0 });
  expect(second.success).toBe(true);
  expect(edits(), 'the edit issued after the crashed read reached the backend').toBe(2);
}

let errorLog: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  invokeMock.mockReset();
  errorLog = vi.spyOn(console, 'error').mockImplementation(() => {});
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  useUIStore.setState({ showToast: vi.fn(), activeDialog: null } as never);
  useProjectStore.setState({ project: PROJECT as never, isDirty: false, canUndo: false, canRedo: false });
});

describe('a crashed read on the edit queue (#576 RV5-6)', () => {
  it.each([
    ['can_undo', 'Failed to refresh undo state:'],
    ['can_redo', 'Failed to refresh undo state:'],
    ['check_project_integrity', 'Failed to check project integrity:'],
  ])(
    'the edit after a crashed %s still runs',
    async (read, logged) => {
      let crashed = false;
      backend((command) => {
        if (command !== read || crashed) return false;
        crashed = true;
        return true;
      });

      await twoEdits();

      expect(crashed, `premise: ${read} crashed`).toBe(true);
      // Caught where it is awaited and logged; the internal-error banner is the user's notice.
      expect(errorLog).toHaveBeenCalledWith(logged, expect.objectContaining({ code: 'INTERNAL_ERROR' }));
      // The second edit's own reads answered, so the undo state is current again.
      expect(useProjectStore.getState().canUndo).toBe(true);
    },
  );

  it('the edits still run when every read after them crashes', async () => {
    backend((command) => command !== 'set_workspace_mode');

    await twoEdits();

    // Each edit still marked the project dirty; the crashed reads left the rest as it was.
    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(useProjectStore.getState().canUndo).toBe(false);
  });

  it('an edit issued after a crashed workspace-mode send still runs', async () => {
    backend((command) => command === 'set_workspace_mode');

    await expect(sendWorkspaceMode('plan')).resolves.toBeUndefined();
    expect(invokeMock.mock.calls.some(([command]) => command === 'set_workspace_mode')).toBe(true);

    const edit = await useProjectStore.getState().moveSection(1, { x: 1, y: 0, z: 0 });
    expect(edit.success).toBe(true);
    expect(edits()).toBe(1);
  });
});
