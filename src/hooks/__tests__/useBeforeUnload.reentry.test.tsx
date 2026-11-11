/**
 * The close listener runs one attempt's questions at a time, and a forced quit
 * whose window survives takes its backend arm back.
 *
 * The in-app question (`utils/exitConfirm`) is the real one here: a second
 * question answers the pending one with "no". A close request arriving while an
 * attempt was still asking (the user pressed X again) started a second attempt,
 * whose own question cancelled the first; the first then dropped the update
 * restart or the forced quit riding on the quit the user was about to confirm.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render } from '@testing-library/react';

const listeners = vi.hoisted(() => [] as Array<() => Promise<void> | void>);
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_name: string, cb: () => Promise<void>) => {
    listeners.push(cb);
    return vi.fn();
  }),
}));

/** Fire the close-requested event, as the window X (or a re-issued quit) does. */
async function closeRequested(): Promise<void> {
  for (const cb of [...listeners]) await cb();
}

const destroyMock = vi.hoisted(() => vi.fn(async () => {}));
// A requested close is fired by the test itself, so it can be fired mid-question.
const closeMock = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ destroy: destroyMock, close: closeMock }),
}));

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock, getErrorMessage: String }));

import { useBeforeUnload } from '../useBeforeUnload';
import { useExitConfirm } from '../../utils/exitConfirm';
import { requestUpdateRestart, resetQuitConfirmationForTests, resetUpdateRestartForTests } from '../../utils/quit';
import { forceQuitNow, resetQuitWaitForTests, setQuitWaitStep, useQuitWait } from '../../utils/quitAfterWork';
import { registerWorkspaceDraft } from '../../utils/workspaceDrafts';
import { useSpeleoNetworkStore } from '../../store/speleoNetworkStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';

const guard = vi.fn(() => false);

function Harness() {
  useBeforeUnload(false, 'Import is in progress. Discard?', undefined, guard);
  return null;
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const calls = (command: string) => invokeIpcMock.mock.calls.filter(([name]) => name === command);

let unregisterDraft: () => void = () => {};

beforeEach(async () => {
  listeners.length = 0;
  destroyMock.mockReset().mockResolvedValue(undefined);
  closeMock.mockReset().mockResolvedValue(undefined);
  invokeIpcMock.mockReset().mockResolvedValue(null);
  guard.mockClear();
  resetQuitConfirmationForTests();
  resetUpdateRestartForTests();
  resetQuitWaitForTests();
  useExitConfirm.setState({ request: null });
  useSpeleoNetworkStore.getState().reset();
  useSpeleoStore.setState({ busy: false });
  useUIStore.setState({ showToast: vi.fn() as never, openDialog: vi.fn() as never });
  useProjectStore.setState({ project: null, isDirty: false });
  render(<Harness />);
  await flush();
});

afterEach(() => {
  unregisterDraft();
  unregisterDraft = () => {};
  resetQuitWaitForTests();
});

/** Unsent SpeleoDB input, so every quit asks the unsent-input question. */
function withUnsentInput(): void {
  unregisterDraft = registerWorkspaceDraft({ label: () => 'the synthetic form', dirty: () => true, discard: vi.fn() });
}

/** Start a close whose question stays on screen; returns the attempt and the question. */
async function closeThatAsks(): Promise<{ attempt: Promise<void>; questionId: number }> {
  let attempt!: Promise<void>;
  act(() => { attempt = closeRequested(); });
  await act(flush);
  const question = useExitConfirm.getState().request;
  expect(question?.message, 'the unsent-input question is on screen').toContain('the synthetic form');
  return { attempt, questionId: question?.id ?? -1 };
}

/** The user presses X again while the question is on screen; returns that request. */
async function pressXAgain(): Promise<{ again: Promise<void> }> {
  let again!: Promise<void>;
  act(() => { again = closeRequested(); });
  await act(flush);
  return { again };
}

describe('a close request while an attempt is still asking joins it', () => {
  it('an armed update restart survives a second X and installs once the user says yes', async () => {
    withUnsentInput();
    await act(() => requestUpdateRestart());
    invokeIpcMock.mockClear();
    const { attempt, questionId } = await closeThatAsks();

    const { again } = await pressXAgain();
    expect(useExitConfirm.getState().request?.id, 'the same question, not answered "no"').toBe(questionId);
    expect(calls('unarm_app_update_restart')).toHaveLength(0);

    await act(async () => {
      useExitConfirm.getState().request?.answer('confirm');
      await attempt;
      await again;
    });
    expect(calls('install_app_update_and_restart')).toHaveLength(1);
    expect(calls('unarm_app_update_restart')).toHaveLength(0);
    // The install takes the destroy's place.
    expect(destroyMock).not.toHaveBeenCalled();
  });

  it('a forced quit survives a second X: the backend is armed once and the window destroyed once', async () => {
    withUnsentInput();
    useSpeleoNetworkStore.getState().begin('exclusive');
    await act(closeRequested);
    expect(useQuitWait.getState().waiting).toBe(true);
    await act(async () => {
      setQuitWaitStep('force');
      await forceQuitNow();
    });
    const { attempt, questionId } = await closeThatAsks();

    const { again } = await pressXAgain();
    expect(useExitConfirm.getState().request?.id).toBe(questionId);

    await act(async () => {
      useExitConfirm.getState().request?.answer('confirm');
      await attempt;
      await again;
    });
    expect(calls('arm_forced_exit')).toHaveLength(1);
    expect(destroyMock).toHaveBeenCalledOnce();
    expect(calls('disarm_forced_exit'), 'the window went: nothing to take back').toHaveLength(0);
    // The quit the user confirmed in the waiting prompt is not re-guarded.
    expect(guard).not.toHaveBeenCalled();
  });

  it('declining the pending question still abandons the attempt, and the next close asks afresh', async () => {
    withUnsentInput();
    await act(() => requestUpdateRestart());
    invokeIpcMock.mockClear();
    const { attempt } = await closeThatAsks();
    const { again } = await pressXAgain();
    await act(async () => {
      useExitConfirm.getState().request?.answer('cancel');
      await attempt;
      await again;
    });
    expect(calls('unarm_app_update_restart')).toHaveLength(1);
    expect(destroyMock).not.toHaveBeenCalled();

    const next = await closeThatAsks();
    await act(async () => {
      useExitConfirm.getState().request?.answer('confirm');
      await next.attempt;
    });
    expect(calls('install_app_update_and_restart')).toHaveLength(0);
    expect(destroyMock).toHaveBeenCalledOnce();
  });
});

describe('a forced quit whose window survives takes the backend arm back', () => {
  async function quitNowOverAnUpload(): Promise<void> {
    useSpeleoNetworkStore.getState().begin('exclusive');
    await act(closeRequested);
    await act(async () => {
      setQuitWaitStep('force');
      await forceQuitNow();
    });
  }

  it('a failed destroy disarms the forced exit, and the next quit waits for the work again', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    destroyMock.mockRejectedValueOnce(new Error('synthetic destroy failure'));
    await quitNowOverAnUpload();
    await act(closeRequested);
    error.mockRestore();

    expect(destroyMock).toHaveBeenCalledOnce();
    expect(calls('arm_forced_exit')).toHaveLength(1);
    expect(calls('disarm_forced_exit')).toHaveLength(1);
    const order = (command: string) =>
      invokeIpcMock.mock.invocationCallOrder[invokeIpcMock.mock.calls.findIndex(([name]) => name === command)] ?? 0;
    expect(order('disarm_forced_exit')).toBeGreaterThan(destroyMock.mock.invocationCallOrder[0] ?? Infinity);
    expect(order('arm_forced_exit')).toBeLessThan(order('disarm_forced_exit'));

    // Still quittable, and no longer forced: the transfer is waited for.
    await act(closeRequested);
    expect(useQuitWait.getState().waiting).toBe(true);
    expect(destroyMock).toHaveBeenCalledOnce();
  });

  it('a failed update install disarms the forced exit too', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    await act(() => requestUpdateRestart());
    await quitNowOverAnUpload();
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'install_app_update_and_restart') throw new Error('synthetic install failure');
      return null;
    });
    await act(closeRequested);
    error.mockRestore();

    expect(calls('install_app_update_and_restart')).toHaveLength(1);
    expect(calls('arm_forced_exit')).toHaveLength(1);
    expect(calls('disarm_forced_exit')).toHaveLength(1);
    expect(destroyMock).not.toHaveBeenCalled();
  });

  it('an ordinary quit whose destroy fails disarms nothing', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    destroyMock.mockRejectedValueOnce(new Error('synthetic destroy failure'));
    await act(closeRequested);
    error.mockRestore();
    expect(destroyMock).toHaveBeenCalledOnce();
    expect(calls('disarm_forced_exit')).toHaveLength(0);
  });
});
