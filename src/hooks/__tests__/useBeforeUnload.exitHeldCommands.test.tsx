/**
 * The quit gate counts every command the backend's exit waits for.
 *
 * `close_project` and the sign-out's `speleodb_discard_cached_copy` register an
 * exit-holding transfer in the backend (`commands/exit_wait.rs`), but the
 * frontend gate used to miss them: a quit during a slow close-release or a
 * sign-out discard destroyed the window while the backend kept the process alive,
 * windowless, until the command finished. Both are now counted at the IPC
 * boundary (`speleoNetworkCommands.ts`), whoever calls them. These run the real
 * `invokeIpc` against a pending Tauri `invoke`.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';

const pending = vi.hoisted(() => new Map<string, (value: unknown) => void>());
const HELD = vi.hoisted(() => new Set(['close_project', 'speleodb_discard_cached_copy']));
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn((command: string) =>
    HELD.has(command)
      ? new Promise((resolve) => {
          pending.set(command, resolve);
        })
      : Promise.resolve(null),
  ),
}));
vi.mock('../../utils/telemetry', () => ({ trackCommand: vi.fn() }));

const listeners = vi.hoisted(() => [] as Array<() => Promise<void> | void>);
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_name: string, cb: () => Promise<void>) => {
    listeners.push(cb);
    return vi.fn();
  }),
}));
async function closeRequested(): Promise<void> {
  for (const cb of [...listeners]) await cb();
}

const destroyMock = vi.hoisted(() => vi.fn(async () => {}));
const closeMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ destroy: destroyMock, close: closeMock }),
}));
vi.mock('../../utils/exitConfirm', () => ({ askExitConfirm: vi.fn(async () => true) }));

import { useBeforeUnload } from '../useBeforeUnload';
import QuitWaitDialog from '../../components/Dialogs/QuitWaitDialog';
import { invokeIpc } from '../../utils/ipc';
import { resetQuitConfirmationForTests, resetUpdateRestartForTests } from '../../utils/quit';
import { QUIT_SETTLE_MS, resetQuitWaitForTests, useQuitWait } from '../../utils/quitAfterWork';
import { useSpeleoNetworkStore } from '../../store/speleoNetworkStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';

function Harness() {
  useBeforeUnload(false, 'unused', undefined, () => false);
  return <QuitWaitDialog />;
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, QUIT_SETTLE_MS + 60)));

/** Answer the pending backend call. */
async function finish(command: string, value: unknown): Promise<void> {
  const resolve = pending.get(command);
  expect(resolve, `${command} is pending`).toBeDefined();
  pending.delete(command);
  await act(async () => {
    resolve?.(value);
    await flush();
  });
}

beforeEach(async () => {
  pending.clear();
  listeners.length = 0;
  destroyMock.mockClear();
  closeMock.mockReset().mockImplementation(async () => { await closeRequested(); });
  resetQuitConfirmationForTests();
  resetUpdateRestartForTests();
  resetQuitWaitForTests();
  useSpeleoNetworkStore.getState().reset();
  useSpeleoStore.setState({ busy: false, link: null, remoteProjectId: null });
  useUIStore.setState({ showToast: vi.fn() as never, openDialog: vi.fn() as never });
  useProjectStore.setState({ project: null, isDirty: false, isClosing: false });
  render(<Harness />);
  await flush();
});

afterEach(() => resetQuitWaitForTests());

describe('a quit waits for the commands the backend exit waits for', () => {
  it('a project close still releasing its lock keeps the window and shows the waiting prompt', async () => {
    let closing!: Promise<unknown>;
    act(() => { closing = useProjectStore.getState().closeProject(); });
    await act(flush);
    expect(pending.has('close_project'), 'the close is in flight').toBe(true);

    await act(closeRequested);
    expect(destroyMock).not.toHaveBeenCalled();
    expect(useQuitWait.getState().waiting).toBe(true);
    expect(screen.getByTestId('quit-wait-dialog')).toBeVisible();

    await finish('close_project', { success: true });
    await act(async () => { await closing; });
    await settle();
    // Re-issued once the close has settled: nothing is open any more, so it quits.
    expect(destroyMock).toHaveBeenCalledOnce();
  });

  it("sign-out's discard of a cached copy keeps the window and shows the waiting prompt", async () => {
    let discarding!: Promise<unknown>;
    act(() => {
      discarding = invokeIpc('speleodb_discard_cached_copy', { projectId: 'synthetic', discardUnpushed: true });
    });

    await act(closeRequested);
    expect(destroyMock).not.toHaveBeenCalled();
    expect(useQuitWait.getState().waiting).toBe(true);
    expect(screen.getByTestId('quit-wait-dialog')).toBeVisible();

    await finish('speleodb_discard_cached_copy', { success: true });
    await act(async () => { await discarding; });
    await settle();
    expect(destroyMock).toHaveBeenCalledOnce();
  });
});
