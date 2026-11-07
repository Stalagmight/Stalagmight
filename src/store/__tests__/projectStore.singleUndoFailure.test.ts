/**
 * [#576 NF-W2S-2] A failed SINGLE undo/redo marks the frontend dirty right away.
 *
 * The backend marks the SpeleoDB copy un-pushed for every undo/redo failure past
 * its empty-stack check (#576 A18(a)): the step reports its failure only after
 * running its inverse, which mutates as it goes. The frontend mirrored that for
 * the multi-step jumps (`undo_to`/`redo_to`) but not for single steps, so its
 * two dirty bits stayed clean until the next status poll - long enough for the
 * quit guard and the push prompt to read "nothing to save / nothing to push".
 *
 * [#576 C4, D22] The backend now SAYS which it was: a single step that ran and
 * failed is `HISTORY_STEP_FAILED` (step 1 of 1), and every refusal that ran no step
 * has its own code. The SpeleoDB bit is the backend's own, on the failure's
 * `remote_link`, applied by `invokeIpc` - so the real `invokeIpc` runs here and only
 * Tauri's `invoke` is mocked.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useSpeleoStore } from '../speleoStore';
import { useUIStore } from '../uiStore';
import { projectStateEnvelope } from '../../test/projectStateEnvelope';
import type { CommandError } from '../../types/ipc/CommandError';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeIpcMock }));

/** Every history verb fails with `failure`; every refresh round trip answers an empty project. */
function failWith(failure: CommandError) {
  invokeIpcMock.mockImplementation(async (command: string) => {
    switch (command) {
      case 'undo':
      case 'redo':
      case 'undo_to':
      case 'redo_to':
        throw failure;
      case 'get_project_state_binary':
        return projectStateEnvelope({
          project: {
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
          },
          stations: [],
          stats: null,
          frozen_shots: [],
        });
      case 'can_undo':
      case 'can_redo':
        return false;
      case 'get_project_warnings':
        return [];
      case 'check_project_integrity':
        return { messages: [], save_blocked: false };
      default:
        return undefined;
    }
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  useUIStore.setState({ showToast: vi.fn() });
  useSpeleoStore.getState().setLinkOnOpen({ permission: 'ADMIN' });
  useProjectStore.setState({ isDirty: false, project: null });
});

describe('a failed single undo/redo (#576 NF-W2S-2)', () => {
  it.each(['undo', 'redo'] as const)(
    'marks both dirty bits when the %s step failed after it started',
    async (verb) => {
      failWith({
        code: 'HISTORY_STEP_FAILED',
        message: 'Failed to parse undo data',
        history_step: { failed_step: 1, total_steps: 1 },
        // The backend marked the copy (#576 A18(a)) and reports its link row.
        remote_link: { has_unpushed_edits: true, unwound: false },
      });
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);

      const result = await useProjectStore.getState()[verb]();

      expect(result.success).toBe(false);
      expect(useProjectStore.getState().isDirty).toBe(true);
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
    },
  );

  it.each([
    ['NOTHING_TO_UNDO', 'Nothing to undo'],
    ['NOTHING_TO_REDO', 'Nothing to redo'],
    ['REFUSED_AFTER_INTERNAL_ERROR', 'Undo is unavailable after an internal error. Save your work and restart Stalagmight.'],
    ['EDIT_DISABLED', 'Undo is disabled: this SpeleoDB project is open read-only. Acquire the lock to edit.'],
    ['WORKSPACE_READ_ONLY', 'Undo is disabled: the Plan workspace is read-only.'],
    ['NEWER_FILE_EDIT_DISABLED', 'Undo is disabled: this project was saved by a newer version of Stalagmight.'],
    ['EDIT_REVERIFY_DIVERGED', 'The SpeleoDB lock or server version changed - reconcile before editing.'],
  ] as const)('marks nothing on a refusal that ran no step: %s', async (code, message) => {
    // Nothing ran, so the link row the backend reports is unchanged (clean).
    failWith({ code, message, remote_link: { has_unpushed_edits: false, unwound: false } });

    await useProjectStore.getState().undo();

    expect(useProjectStore.getState().isDirty).toBe(false);
    expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);
  });

  // [#576 C4] A body that panicked on the backend's pool answers INTERNAL_ERROR with no
  // `remote_link` (the crash skipped the link read). Before the coded envelope it was an
  // untagged failure, which a single step counted as having run: the step may have half-run.
  const CRASH: CommandError = {
    code: 'INTERNAL_ERROR',
    message: 'The operation failed unexpectedly (task 7 panicked)',
  };

  it.each(['undo', 'redo'] as const)('marks the project dirty when the %s crashed', async (verb) => {
    failWith(CRASH);

    const result = await useProjectStore.getState()[verb]();

    expect(result).toMatchObject({ success: false, code: 'INTERNAL_ERROR' });
    expect(useProjectStore.getState().isDirty).toBe(true);
    // The SpeleoDB bits are the backend's alone, and a crash reported none.
    expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);
  });

  it.each(['undoTo', 'redoTo'] as const)('marks nothing when a %s jump crashed: no failed step was reported', async (verb) => {
    failWith(CRASH);

    const result = await useProjectStore.getState()[verb](0);

    expect(result).toMatchObject({ success: false, code: 'INTERNAL_ERROR' });
    expect(useProjectStore.getState().isDirty).toBe(false);
  });
});
