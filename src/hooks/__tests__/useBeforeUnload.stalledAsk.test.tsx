/**
 * Pre-PR delta review finding 1, at the window X: the close guard awaits the
 * backend's unsaved-changes flag while the close listener's latch
 * (`askingQuestions`) is held, and every close request arriving meanwhile
 * joins that attempt. With no bound on the ask, a backend that never answered
 * held the latch for good: the X did nothing, then or ever again. The ask is
 * bounded now, so the prompt opens once the bound runs out and the latch is
 * released for the next attempt.
 *
 * Injection: awaiting `has_unsaved_changes` without the bound leaves the
 * first close pending, no prompt opens, and the second X is swallowed by the
 * attempt that never finishes (the test times out).
 */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from '@testing-library/react';

const listeners = vi.hoisted(() => [] as Array<() => Promise<void> | void>);
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_name: string, cb: () => Promise<void>) => {
    listeners.push(cb);
    return vi.fn();
  }),
}));

const destroyMock = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ destroy: destroyMock, close: vi.fn(async () => {}) }),
}));

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock, getErrorMessage: String }));

import { useBeforeUnload } from '../useBeforeUnload';
import { guardQuit, resetQuitConfirmationForTests, resetUpdateRestartForTests } from '../../utils/quit';
import { resetQuitWaitForTests } from '../../utils/quitAfterWork';
import { UNSAVED_ASK_BOUND_MS } from '../../utils/unsavedChanges';
import { useSpeleoNetworkStore } from '../../store/speleoNetworkStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';

function Harness() {
  useBeforeUnload(false, 'Import is in progress. Discard?', undefined, guardQuit);
  return null;
}

/** Fire the close-requested event, as the window X does. */
function closeRequested(): Promise<void> {
  return Promise.all(listeners.map((cb) => cb())).then(() => undefined);
}

const openDialog = vi.fn();
const asks = () => invokeIpcMock.mock.calls.filter(([name]) => name === 'has_unsaved_changes').length;

beforeEach(async () => {
  vi.useFakeTimers();
  listeners.length = 0;
  destroyMock.mockClear();
  openDialog.mockClear();
  invokeIpcMock.mockReset().mockImplementation(((command: string) =>
    command === 'has_unsaved_changes' ? new Promise(() => {}) : Promise.resolve(null)) as never);
  resetQuitConfirmationForTests();
  resetUpdateRestartForTests();
  resetQuitWaitForTests();
  useSpeleoNetworkStore.getState().reset();
  useSpeleoStore.setState({ busy: false, link: null });
  useUIStore.setState({ showToast: vi.fn() as never, openDialog: openDialog as never, activeDialog: null, dialogData: {} });
  useProjectStore.setState({ project: { name: 'Cave', file_path: '/cave/stalled.stalx' } as never, isDirty: false });
  render(<Harness />);
  await vi.advanceTimersByTimeAsync(0);
  expect(listeners).toHaveLength(1);
});

afterEach(() => {
  vi.useRealTimers();
  resetQuitWaitForTests();
});

it('a backend that never answers still gets the X its prompt, and the next X a fresh attempt', async () => {
  const first = closeRequested();
  await vi.advanceTimersByTimeAsync(UNSAVED_ASK_BOUND_MS - 1);
  expect(openDialog, 'the ask is still within its bound').not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  await first;
  expect(openDialog).toHaveBeenCalledWith('unsaved-changes', { pendingQuit: true });
  expect(destroyMock).not.toHaveBeenCalled();

  // The latch was released: the next X asks again instead of joining an
  // attempt that never finishes.
  const second = closeRequested();
  await vi.advanceTimersByTimeAsync(UNSAVED_ASK_BOUND_MS);
  await second;
  expect(asks()).toBe(2);
  expect(openDialog).toHaveBeenCalledTimes(2);
  expect(destroyMock).not.toHaveBeenCalled();
});
