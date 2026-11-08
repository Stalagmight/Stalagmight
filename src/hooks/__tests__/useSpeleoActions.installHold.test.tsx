/**
 * [#576 W85] The post-push save respects the update-install hold.
 *
 * A confirmed push or Done saves the project from the frontend once it answers
 * (`flushToDiskAfterPush`, to clear `isDirty`). It goes straight to `saveProject`,
 * outside the mutation queue, so the queue's install refusal does not cover it. A
 * Done started from the status button (not modal) can still be on the network when
 * Restart is pressed: the exit guards find nothing unpushed and a clean project,
 * the install starts, and the answer then arrived and wrote the project while the
 * install ran - over edits the user may just have chosen to discard. Nothing may
 * change the project while an update installs, so the save is skipped; the
 * restart ends the app either way, and a failed install gives autosave back.
 *
 * `.tsx` so this runs in the jsdom tier (renderHook).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';

vi.mock('../../utils/ipc');

import { useSpeleoActions } from '../useSpeleoActions';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { useUpdateStore } from '../../store/updateStore';
import { deferred } from '../../test/factories/async';
import { invokeIpcMock } from '../../test/factories/ipc';

let saveProject: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  saveProject = vi.fn(async () => ({ success: true }));
  useUpdateStore.getState().reset();
  useProjectStore.setState({ project: { zero_datums: [{}] } as never, saveProject: saveProject as never });
  useSpeleoStore.setState({
    busy: false,
    applyDone: vi.fn(),
    applyPush: vi.fn(),
    bumpPoll: vi.fn(),
    clearSession: vi.fn(),
    raiseNoAnchorBanner: vi.fn(),
  });
  useUIStore.setState({ showToast: vi.fn() });
});

/** Hold `cmd` on the network until the test answers it with `reply`. */
function holdSpeleoAnswer(cmd: string, reply: unknown): () => void {
  const answer = deferred<unknown>();
  invokeIpcMock.mockImplementation((c: string) => (c === cmd ? answer.promise : Promise.resolve(null)));
  return () => answer.resolve(reply);
}

describe('the post-push save while an update installs', () => {
  it.each([
    ['Done', 'speleodb_done', { result: { kind: 'released' }, save_warnings: [] }, 'done'],
    ['Push', 'speleodb_push', { result: { kind: 'pushed' }, save_warnings: [] }, 'push'],
  ] as const)('%s answering after the install started saves nothing', async (_label, cmd, reply, verb) => {
    const answer = holdSpeleoAnswer(cmd, reply);
    const { result } = renderHook(() => useSpeleoActions());

    let running!: Promise<unknown>;
    act(() => {
      running = result.current[verb]();
    });
    // Restart pressed while the answer is on its way: the install starts.
    useUpdateStore.getState().setInstalling(true);
    await act(async () => {
      answer();
      await running;
    });

    expect(saveProject).not.toHaveBeenCalled();
  });

  it.each([
    ['Done', 'speleodb_done', { result: { kind: 'released' }, save_warnings: [] }, 'done'],
    ['Push', 'speleodb_push', { result: { kind: 'pushed' }, save_warnings: [] }, 'push'],
  ] as const)('control: %s with no install running saves once', async (_label, cmd, reply, verb) => {
    const answer = holdSpeleoAnswer(cmd, reply);
    const { result } = renderHook(() => useSpeleoActions());

    let running!: Promise<unknown>;
    act(() => {
      running = result.current[verb]();
    });
    await act(async () => {
      answer();
      await running;
    });

    expect(saveProject).toHaveBeenCalledTimes(1);
  });
});
