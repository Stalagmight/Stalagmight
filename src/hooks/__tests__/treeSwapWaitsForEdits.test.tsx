/**
 * [#576 D23-SWAP] (decided "SpeleoDB tree swaps also wait for queued edits")
 *
 * Edits run on one queue (`runSerialized`, src/store/mutationQueue.ts). A
 * project switch waits for the edits queued before it (`runProjectSwitch`,
 * #576 D23); the SpeleoDB verbs that swap the open survey tree IN PLACE did
 * not. Their frontend reconciliation (`applySpeleoTreeReplacement`) is
 * serialized, but it runs AFTER the backend verb already swapped the tree - so
 * an edit still queued (behind the previous edit's refresh, say) when the user
 * pressed Update reached the backend after the swap and ran against the new
 * tree, where the same small section and shot ids mostly resolve.
 *
 * Each case: one edit in flight, one queued behind it, then the action. The
 * backend verb must not be invoked until both have settled. The verb itself is
 * made to fail - only WHEN it is invoked is under test here, and each action's
 * own failure path is covered by its own suite.
 *
 * Modelled on src/utils/projectSwitchWaitsForEdits.test.ts. `.tsx` so vitest runs
 * it under jsdom (renderHook).
 */
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { renderHook } from '@testing-library/react';

vi.mock('../../utils/ipc');
stubGetErrorMessage(fallbackOr('error'));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: vi.fn(async () => {}), destroy: vi.fn(async () => {}) }),
}));

import { runSerialized } from '../../store/mutationQueue';
import { afterQueuedEdits } from '../../utils/projectTransition';
import { useSpeleoActions } from '../useSpeleoActions';
import { useRestoreRevision } from '../useRestoreRevision';
import { useOpenRemoteProject } from '../useOpenRemoteProject';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import type { SpeleoProjectSummary } from '../../types/ipc.generated';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../test/factories/ipc';
import { deferred, microtasks } from '../../test/factories/async';

const SUMMARY: SpeleoProjectSummary = {
  id: 'p1',
  name: 'CaveA',
  permission: 'READ_AND_WRITE',
  lock_holder: null,
  type_tag: 'stalx',
  latest_commit_sha: 'abc123',
};

beforeEach(() => {
  invokeIpcMock.mockReset();
  useProjectStore.setState({ project: null, isDirty: false, loadProject: vi.fn(async () => ({ success: true })) });
  useSpeleoStore.setState({ busy: false, link: null, remoteProjectId: null, bumpPoll: vi.fn(), setLinkOnOpen: vi.fn() });
  useUIStore.setState({ showToast: vi.fn(), openDialog: vi.fn() });
  vi.spyOn(console, 'error').mockImplementation(() => {});
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});

const speleo = () => renderHook(() => useSpeleoActions()).result.current;
const remote = () => renderHook(() => useOpenRemoteProject()).result.current;
const restore = () => renderHook(() => useRestoreRevision()).result.current;

// [backend verb, the user action that issues it]
const SWAPS: Array<[string, () => Promise<unknown>]> = [
  ['speleodb_update', () => speleo().update()],
  ['speleodb_start_editing', () => speleo().startEditing()],
  ['speleodb_discard_changes', () => speleo().discardChanges(false)],
  ['speleodb_take_remote', () => speleo().takeRemote()],
  [
    'speleodb_restore_revision',
    () => restore()({ projectId: 'p1', projectName: 'CaveA', hexsha: 'abc123', shortSha: 'abc123', restore: false }),
  ],
  ['speleodb_open_project', () => remote().openRemote(SUMMARY)],
  ['speleodb_open_project_discarding_local', () => remote().takeRemoteDiscardingLocal(SUMMARY)],
  ['speleodb_adopt_ariane', () => remote().adoptRemote(SUMMARY)],
];

describe('in-place SpeleoDB tree swaps wait for the edits queued before them (#576 D23-SWAP)', () => {
  it.each(SWAPS)('%s is invoked only after the edits queued before it settled', async (verb, issue) => {
    const trace: string[] = [];
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === verb) {
        trace.push(`${verb} invoked`);
        throw new Error('verb refused (test)');
      }
      return null;
    });
    const editInFlight = deferred();
    const edit = runSerialized(async () => {
      trace.push('edit starts');
      await editInFlight.promise;
      trace.push('edit finishes');
    });
    const queuedEdit = runSerialized(async () => {
      trace.push('queued edit runs');
    });

    const action = issue().catch(() => undefined);

    try {
      await microtasks(10);
      // The verb must still be waiting: an edit ahead of it is mid-flight.
      expect(trace).toEqual(['edit starts']);
    } finally {
      // Released whatever the verdict: the queue is module state, and a wedged
      // edit would stall every later case in this file.
      editInFlight.resolve();
    }
    await Promise.all([edit, queuedEdit, action]);
    expect(trace).toEqual(['edit starts', 'edit finishes', 'queued edit runs', `${verb} invoked`]);
  });
});

describe('afterQueuedEdits', () => {
  it('does not make the verb wait for an edit issued after it', async () => {
    const trace: string[] = [];
    const verbInFlight = deferred();
    const swap = afterQueuedEdits(async () => {
      trace.push('verb starts');
      await verbInFlight.promise;
      trace.push('verb finishes');
    });
    await microtasks(10);
    // Issued while the verb runs: its own queue, not held behind the verb.
    await runSerialized(async () => {
      trace.push('edit during the swap');
    });
    expect(trace).toEqual(['verb starts', 'edit during the swap']);
    verbInFlight.resolve();
    await swap;
    expect(trace).toEqual(['verb starts', 'edit during the swap', 'verb finishes']);
  });

  it('still runs the verb when a queued edit failed', async () => {
    const failing = runSerialized(async () => {
      throw new Error('edit refused');
    });
    await expect(failing).rejects.toThrow('edit refused');
    await expect(afterQueuedEdits(async () => 'swapped')).resolves.toBe('swapped');
  });
});
