/**
 * lock#6 / frontend#4: a quit never cuts a SpeleoDB transfer in flight.
 *
 * Every quit route requests a window close and lands in the close listener, whose
 * first gate is `deferQuitUntilWorkDone`: with work in flight it keeps the
 * window, toasts, shows the waiting prompt and quits once the work has finished,
 * re-running every guard. "Quit now" is an explicit choice that saves locally,
 * skips the wait and tells the backend it may cut the operation.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

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
const closeMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ destroy: destroyMock, close: closeMock }),
}));

const confirmMock = vi.hoisted(() => vi.fn(async () => true));
vi.mock('../../utils/exitConfirm', () => ({ askExitConfirm: confirmMock }));

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock, getErrorMessage: String }));

import { useBeforeUnload } from '../useBeforeUnload';
import QuitWaitDialog from '../../components/Dialogs/QuitWaitDialog';
import { resetQuitConfirmationForTests, requestUpdateRestart, resetUpdateRestartForTests } from '../../utils/quit';
import {
  QUIT_SETTLE_MS,
  forceQuitNow,
  holdQuitDuring,
  resetQuitWaitForTests,
  setQuitWaitStep,
  useQuitWait,
} from '../../utils/quitAfterWork';
import { useSpeleoNetworkStore } from '../../store/speleoNetworkStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoExitPrompt } from '../../utils/speleoExit';
import { registerWorkspaceDraft } from '../../utils/workspaceDrafts';

function Harness({ guard, shouldBlock = false }: { guard: () => boolean; shouldBlock?: boolean }) {
  useBeforeUnload(shouldBlock, 'Import is in progress. Discard?', undefined, guard);
  return <QuitWaitDialog />;
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, QUIT_SETTLE_MS + 60)));

let showToast: ReturnType<typeof vi.fn>;
let guard: ReturnType<typeof vi.fn>;

beforeEach(async () => {
  listeners.length = 0;
  destroyMock.mockClear();
  // A requested close comes back as the close-requested event, as Tauri does.
  closeMock.mockReset().mockImplementation(async () => { await closeRequested(); });
  confirmMock.mockClear().mockResolvedValue(true);
  invokeIpcMock.mockReset().mockResolvedValue(null);
  resetQuitConfirmationForTests();
  resetUpdateRestartForTests();
  resetQuitWaitForTests();
  useSpeleoNetworkStore.getState().reset();
  useSpeleoStore.setState({ busy: false });
  useSpeleoExitPrompt.setState({ request: null, working: false });
  showToast = vi.fn();
  useUIStore.setState({ showToast: showToast as never, openDialog: vi.fn() as never });
  useProjectStore.setState({ project: null, isDirty: false });
  guard = vi.fn(() => false);
  render(<Harness guard={guard} />);
  await flush();
});

afterEach(() => resetQuitWaitForTests());

describe('quit after in-flight work', () => {
  it('nothing in flight: the quit goes straight on', async () => {
    await closeRequested();
    expect(destroyMock).toHaveBeenCalledOnce();
    expect(useQuitWait.getState().waiting).toBe(false);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('an upload in flight keeps the window, toasts, shows progress, then closes when it finishes', async () => {
    const endUpload = useSpeleoNetworkStore.getState().begin('exclusive');
    await act(closeRequested);
    expect(destroyMock).not.toHaveBeenCalled();
    // Nothing is asked against a state that is about to change.
    expect(guard).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('will close when it finishes'), 'quit-after-work', 'info');
    expect(screen.getByTestId('quit-wait-dialog')).toBeVisible();
    expect(screen.getByTestId('quit-wait-cancel')).toHaveAttribute('data-primary-action', '');

    // Pressing X again only says so again.
    await act(closeRequested);
    expect(showToast).toHaveBeenCalledTimes(2);

    act(() => endUpload());
    await settle();
    // Re-issued from the top: the guards run against the settled state.
    expect(closeMock).toHaveBeenCalled();
    expect(guard).toHaveBeenCalledOnce();
    expect(destroyMock).toHaveBeenCalledOnce();
    expect(screen.queryByTestId('quit-wait-dialog')).toBeNull();
  });

  it('a step that starts right after the previous one (save, then upload) is not raced', async () => {
    const endSave = useSpeleoNetworkStore.getState().begin('exclusive');
    await act(closeRequested);
    let endUpload!: () => void;
    act(() => {
      endSave();
      endUpload = useSpeleoNetworkStore.getState().begin('exclusive');
    });
    await settle();
    expect(destroyMock).not.toHaveBeenCalled();
    act(() => endUpload());
    await settle();
    expect(destroyMock).toHaveBeenCalledOnce();
  });

  it('a hand-off verb or an exit-prompt answer being carried out counts as work too', async () => {
    useSpeleoStore.setState({ busy: true });
    await act(closeRequested);
    expect(useQuitWait.getState().waiting).toBe(true);
    act(() => useSpeleoStore.setState({ busy: false }));
    act(() => useSpeleoExitPrompt.setState({ working: true }));
    await settle();
    expect(destroyMock).not.toHaveBeenCalled();
    act(() => useSpeleoExitPrompt.setState({ working: false }));
    await settle();
    expect(destroyMock).toHaveBeenCalledOnce();
  });

  it('a held multi-step flow (Save Changes, Publish) is waited for between its calls too', async () => {
    let finish!: () => void;
    let held!: Promise<void>;
    act(() => { held = holdQuitDuring(() => new Promise<void>((resolve) => { finish = resolve; })); });
    await act(closeRequested);
    expect(useQuitWait.getState().waiting).toBe(true);
    await settle();
    expect(destroyMock).not.toHaveBeenCalled();
    await act(async () => { finish(); await held; });
    await settle();
    expect(destroyMock).toHaveBeenCalledOnce();
  });

  // The second tier-0 check: work that STARTS while a question is on screen (here
  // the import / dive-plan question) is waited for the same way, before anything
  // one-way happens - not cut by the destroy that would otherwise follow the "yes".
  it('work started while the tier-1 question is open is waited for after the answer', async () => {
    cleanup();
    listeners.length = 0;
    render(<Harness guard={guard} shouldBlock />);
    await flush();
    let answer!: (yes: boolean) => void;
    confirmMock.mockImplementationOnce(() => new Promise<boolean>((resolve) => { answer = resolve; }));

    let closing!: Promise<void>;
    act(() => { closing = closeRequested(); });
    await act(flush);
    expect(confirmMock).toHaveBeenCalledOnce();
    expect(useQuitWait.getState().waiting).toBe(false);

    // An upload starts while the question is still on screen...
    const endUpload = useSpeleoNetworkStore.getState().begin('exclusive');
    // ...and the user answers yes.
    await act(async () => {
      answer(true);
      await closing;
    });
    expect(destroyMock).not.toHaveBeenCalled();
    expect(useQuitWait.getState().waiting).toBe(true);
    expect(screen.getByTestId('quit-wait-dialog')).toBeVisible();

    // Once it has finished, the re-issued quit asks again and closes.
    act(() => endUpload());
    await settle();
    expect(confirmMock).toHaveBeenCalledTimes(2);
    expect(destroyMock).toHaveBeenCalledOnce();
  });

  it('Keep Stalagmight open calls the quit off; the transfer carries on', async () => {
    const endUpload = useSpeleoNetworkStore.getState().begin('exclusive');
    await act(closeRequested);
    await userEvent.click(screen.getByTestId('quit-wait-cancel'));
    expect(screen.queryByTestId('quit-wait-dialog')).toBeNull();
    act(() => endUpload());
    await settle();
    expect(destroyMock).not.toHaveBeenCalled();
    expect(closeMock).not.toHaveBeenCalled();
  });

  it('Quit now is behind a warning, saves unsaved edits locally, and tells the backend it may cut the transfer', async () => {
    const save = vi.fn(async () => {
      useProjectStore.setState({ isDirty: false });
      return { success: true };
    });
    useProjectStore.setState({ project: { file_path: '/synthetic/cache/project.stalx' } as never, isDirty: true, saveProject: save as never });
    useSpeleoNetworkStore.getState().begin('exclusive');
    await act(closeRequested);
    await userEvent.click(screen.getByTestId('quit-wait-force'));
    expect(screen.getByTestId('quit-wait-force-warning')).toHaveTextContent('cut off');
    // The warning's default action is Back, never the forced quit.
    expect(screen.getByTestId('quit-wait-back')).toHaveAttribute('data-primary-action', '');
    await userEvent.click(screen.getByTestId('quit-wait-force-confirm'));
    await act(flush);
    expect(save).toHaveBeenCalledOnce();
    expect(invokeIpcMock).toHaveBeenCalledWith('arm_forced_exit', undefined, expect.anything());
    expect(destroyMock).toHaveBeenCalledOnce();
    // The answered quit is not re-guarded.
    expect(guard).not.toHaveBeenCalled();
  });

  it('Quit now does not quit when the local save fails, and says why', async () => {
    useProjectStore.setState({
      project: { file_path: '/synthetic/cache/project.stalx' } as never,
      isDirty: true,
      saveProject: vi.fn(async () => ({ success: false, error: 'Synthetic disk failure' })) as never,
    });
    useSpeleoNetworkStore.getState().begin('exclusive');
    await act(closeRequested);
    await userEvent.click(screen.getByTestId('quit-wait-force'));
    await userEvent.click(screen.getByTestId('quit-wait-force-confirm'));
    expect(await screen.findByTestId('quit-wait-error')).toHaveTextContent('Synthetic disk failure');
    expect(destroyMock).not.toHaveBeenCalled();
    expect(invokeIpcMock).not.toHaveBeenCalledWith('arm_forced_exit', undefined, expect.anything());
  });

  describe('Quit now is a one-shot, not a deadline', () => {
    /** Put the app in the waiting prompt with an upload in flight, then press Quit now. */
    async function quitNowOverAnUpload(): Promise<void> {
      useSpeleoNetworkStore.getState().begin('exclusive');
      await act(closeRequested);
      expect(useQuitWait.getState().waiting).toBe(true);
      await act(async () => {
        setQuitWaitStep('force');
        await forceQuitNow();
      });
    }

    /** A question that stays on screen until the test answers it. */
    function heldQuestion(): (yes: boolean) => void {
      let answer: ((yes: boolean) => void) | null = null;
      confirmMock.mockImplementation(() => new Promise<boolean>((resolve) => { answer = resolve; }));
      return (yes) => {
        expect(answer, 'the question is on screen').not.toBeNull();
        answer?.(yes);
      };
    }

    const armed = () => invokeIpcMock.mock.calls.filter(([command]) => command === 'arm_forced_exit').length;

    afterEach(() => vi.useRealTimers());

    // The forced quit still asks these two questions, and a wall-clock arm used to
    // run out while one was open: the close that came back after a slow "yes" fell
    // into the waiting prompt again. The backend is armed only as the window goes.
    for (const question of ['unsent SpeleoDB input', 'import / dive plan'] as const) {
      it(`a ${question} question answered after a minute still quits and arms the backend at the exit`, async () => {
        let unregister = () => {};
        if (question === 'import / dive plan') {
          cleanup();
          listeners.length = 0;
          render(<Harness guard={guard} shouldBlock />);
          await flush();
        } else {
          unregister = registerWorkspaceDraft({ label: () => 'the synthetic form', dirty: () => true, discard: vi.fn() });
        }
        // The real guard would raise the exit prompt for saved, un-uploaded work
        // under the lock; the answered quit must not reach it, however late.
        guard.mockReturnValue(true);
        const answer = heldQuestion();
        vi.useFakeTimers();
        await quitNowOverAnUpload();
        expect(confirmMock).toHaveBeenCalledOnce();

        await act(() => vi.advanceTimersByTimeAsync(60_000));
        expect(destroyMock).not.toHaveBeenCalled();
        expect(armed(), 'not armed at the click').toBe(0);

        await act(async () => {
          answer(true);
          await vi.advanceTimersByTimeAsync(50);
        });
        unregister();
        expect(armed()).toBe(1);
        expect(destroyMock).toHaveBeenCalledOnce();
        const armedAt = invokeIpcMock.mock.invocationCallOrder[
          invokeIpcMock.mock.calls.findIndex(([command]) => command === 'arm_forced_exit')
        ];
        expect(armedAt).toBeLessThan(destroyMock.mock.invocationCallOrder[0] ?? 0);
        expect(guard).not.toHaveBeenCalled();
        expect(useQuitWait.getState().waiting).toBe(false);
      });
    }

    it('declining a question drops the arm: the next quit waits for the work again', async () => {
      const unregister = registerWorkspaceDraft({ label: () => 'the synthetic form', dirty: () => true, discard: vi.fn() });
      const answer = heldQuestion();
      await quitNowOverAnUpload();
      await act(async () => {
        answer(false);
        await flush();
      });
      unregister();
      expect(destroyMock).not.toHaveBeenCalled();

      confirmMock.mockResolvedValue(true);
      await act(closeRequested);
      expect(useQuitWait.getState().waiting).toBe(true);
      expect(destroyMock).not.toHaveBeenCalled();
      expect(armed()).toBe(0);
    });

    it('a refused close drops the arm: the next quit waits for the work again', async () => {
      closeMock.mockRejectedValueOnce(new Error('synthetic close refusal'));
      const error = vi.spyOn(console, 'error').mockImplementation(() => {});
      await quitNowOverAnUpload();
      await act(flush);
      error.mockRestore();
      expect(destroyMock).not.toHaveBeenCalled();

      await act(closeRequested);
      expect(useQuitWait.getState().waiting).toBe(true);
      expect(destroyMock).not.toHaveBeenCalled();
      expect(armed()).toBe(0);
    });
  });

  it('an ordinary quit never arms the forced exit', async () => {
    const endUpload = useSpeleoNetworkStore.getState().begin('exclusive');
    await act(closeRequested);
    act(() => endUpload());
    await settle();
    expect(destroyMock).toHaveBeenCalledOnce();
    expect(invokeIpcMock).not.toHaveBeenCalledWith('arm_forced_exit', undefined, expect.anything());
  });

  it('restart-to-update waits the same way, then installs', async () => {
    const endUpload = useSpeleoNetworkStore.getState().begin('exclusive');
    await act(() => requestUpdateRestart());
    expect(invokeIpcMock).not.toHaveBeenCalledWith('install_app_update_and_restart', undefined, expect.anything());
    expect(useQuitWait.getState().waiting).toBe(true);
    act(() => endUpload());
    await settle();
    expect(invokeIpcMock).toHaveBeenCalledWith('install_app_update_and_restart', undefined, expect.anything());
  });

  it('cancelling the wait abandons a restart riding on it', async () => {
    useSpeleoNetworkStore.getState().begin('exclusive');
    await act(() => requestUpdateRestart());
    await userEvent.click(screen.getByTestId('quit-wait-cancel'));
    expect(invokeIpcMock).toHaveBeenCalledWith('unarm_app_update_restart', undefined, expect.anything());
  });

  // frontend#10: the unsent-SpeleoDB-input question comes AFTER the wait, so the
  // re-issued quit asks it against the settled state; declining it abandons the
  // quit and the restart riding on it.
  it('asks about unsent SpeleoDB input only once the work has finished', async () => {
    const unregister = registerWorkspaceDraft({ label: () => 'the synthetic form', dirty: () => true, discard: vi.fn() });
    const endUpload = useSpeleoNetworkStore.getState().begin('exclusive');
    await act(() => requestUpdateRestart());
    expect(confirmMock).not.toHaveBeenCalled();
    confirmMock.mockResolvedValue(false);
    act(() => endUpload());
    await settle();
    unregister();
    expect(confirmMock).toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringContaining('the synthetic form') }));
    expect(guard).not.toHaveBeenCalled();
    expect(destroyMock).not.toHaveBeenCalled();
    expect(invokeIpcMock).not.toHaveBeenCalledWith('install_app_update_and_restart', undefined, expect.anything());
    expect(invokeIpcMock).toHaveBeenCalledWith('unarm_app_update_restart', undefined, expect.anything());
  });
});
