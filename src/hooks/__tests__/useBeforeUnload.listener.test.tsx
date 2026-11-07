/**
 * [#576 D27-32(f), NET-2] The `tauri://close-requested` listener's lifecycle.
 *
 * The backend always prevents the close and emits this event, and an emit with no
 * listener is simply dropped - so the window X does nothing at all while the hook
 * has no listener registered.
 *
 * - D27-32(f): the listener effect used to depend on `message` and `onConfirm`, and
 *   `App.tsx` flips the message whenever an import starts or ends. Every flip tore
 *   the listener down synchronously and re-registered it only after `listen()`'s IPC
 *   round trip, so a close request landing in between was lost.
 * - NET-2: the unlisten function `listen()` resolves to is async (it awaits the
 *   `plugin:event|unlisten` IPC call) and the hook called it without awaiting or
 *   catching, so a failed release escaped as an unhandled rejection - the gap
 *   useNetworkStatus closed for its own listeners.
 *
 * The mock below models the real API's shape: the handler is registered only when
 * the `listen()` promise resolves (after a tick), and the unlisten it resolves to
 * removes the handler synchronously and then returns the IPC promise.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render } from '@testing-library/react';

type Handler = () => Promise<void> | void;
// Plain functions, not vi.fn(): a spy attaches its own handler to every promise its
// implementation returns, which would hide the unhandled rejection NET-2 is about.
const tauri = vi.hoisted(() => ({
  handlers: new Set<() => Promise<void> | void>(),
  listenCalls: 0,
  unlistenCalls: 0,
  unlistenResult: (): Promise<void> => Promise.resolve(),
  // Holds each registration until the test releases it; null resolves after a tick.
  pending: null as Array<() => void> | null,
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: (_event: string, handler: Handler) => {
    tauri.listenCalls += 1;
    return new Promise((resolve) => {
      const register = () => {
        tauri.handlers.add(handler);
        resolve(() => {
          tauri.unlistenCalls += 1;
          tauri.handlers.delete(handler);
          return tauri.unlistenResult();
        });
      };
      if (tauri.pending) tauri.pending.push(register);
      else setTimeout(register, 0);
    });
  },
}));

const destroyMock = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ destroy: destroyMock, close: vi.fn(async () => {}) }),
}));

const confirmMock = vi.hoisted(() => vi.fn(async (_message: string) => true));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: confirmMock }));

import { useBeforeUnload } from '../useBeforeUnload';
import { resetQuitConfirmationForTests } from '../../utils/quit';
import { timerTurns } from '../../test/factories/async';

function Harness({
  shouldBlock,
  message,
  onConfirm,
}: {
  shouldBlock: boolean;
  message: string;
  onConfirm?: () => Promise<void>;
}) {
  useBeforeUnload(shouldBlock, message, onConfirm, () => false);
  return null;
}

/** What the OS window X produces: the event, delivered to whoever is listening. */
async function pressWindowX(): Promise<void> {
  for (const handler of [...tauri.handlers]) await handler();
}

let unhandled: string[] = [];
const onUnhandled = (reason: unknown) => {
  unhandled.push(reason instanceof Error ? reason.message : String(reason));
};

beforeEach(async () => {
  // A registration the previous test left in flight resolves here, not inside the
  // next test's counters.
  await timerTurns(2);
  tauri.handlers.clear();
  tauri.listenCalls = 0;
  tauri.unlistenCalls = 0;
  tauri.unlistenResult = () => Promise.resolve();
  tauri.pending = null;
  destroyMock.mockClear();
  destroyMock.mockResolvedValue(undefined);
  confirmMock.mockClear();
  confirmMock.mockResolvedValue(true);
  resetQuitConfirmationForTests();
  unhandled = [];
  process.on('unhandledRejection', onUnhandled);
});

afterEach(() => {
  process.off('unhandledRejection', onUnhandled);
  vi.restoreAllMocks();
});

describe('useBeforeUnload - one close listener for the life of the hook', () => {
  it('a close request during a message change still reaches a listener', async () => {
    const { rerender } = render(<Harness shouldBlock={false} message="first" />);
    await timerTurns(2);

    // The message flips (an import started); the X lands before any new listen()
    // could have completed its round trip.
    tauri.pending = [];
    rerender(<Harness shouldBlock={false} message="second" />);
    await pressWindowX();

    expect(destroyMock, 'the close request was dropped').toHaveBeenCalledTimes(1);
  });

  it('registers once across message and onConfirm changes, and uses the latest of both', async () => {
    const firstCleanup = vi.fn(async () => {});
    const latestCleanup = vi.fn(async () => {});
    const { rerender } = render(
      <Harness shouldBlock message="Import is in progress" onConfirm={firstCleanup} />,
    );
    await timerTurns(2);
    rerender(<Harness shouldBlock message="Dive plan has unsaved changes" onConfirm={latestCleanup} />);
    await timerTurns(2);

    expect(tauri.listenCalls, 'listen() calls').toBe(1);
    expect(tauri.unlistenCalls, 'unlisten() calls').toBe(0);

    await pressWindowX();

    expect(confirmMock).toHaveBeenCalledTimes(1);
    expect(confirmMock.mock.calls[0]?.[0]).toBe('Dive plan has unsaved changes');
    expect(firstCleanup).not.toHaveBeenCalled();
    expect(latestCleanup).toHaveBeenCalledTimes(1);
    expect(destroyMock).toHaveBeenCalledTimes(1);
  });

  it('the dev-mode beforeunload prompt carries the latest message', async () => {
    const added = vi.spyOn(window, 'addEventListener');
    const { rerender } = render(<Harness shouldBlock message="first" />);
    await timerTurns(2);
    rerender(<Harness shouldBlock message="second" />);

    // jsdom's Event.returnValue is the legacy boolean, so call the handler directly.
    const handlers = added.mock.calls
      .filter(([type]) => type === 'beforeunload')
      .map(([, handler]) => handler as (e: BeforeUnloadEvent) => unknown);
    const event = { preventDefault: vi.fn(), returnValue: '' } as unknown as BeforeUnloadEvent;
    const returned = handlers[handlers.length - 1]?.(event);

    expect(returned).toBe('second');
    expect(event.returnValue).toBe('second');
    expect(event.preventDefault).toHaveBeenCalledTimes(1);
  });

  it('still releases the listener on unmount', async () => {
    const { unmount } = render(<Harness shouldBlock={false} message="m" />);
    await timerTurns(2);
    unmount();
    await timerTurns(2);

    expect(tauri.unlistenCalls).toBe(1);
    expect(tauri.handlers.size).toBe(0);
  });
});

describe('useBeforeUnload - a rejected unlisten() is handled', () => {
  it('on unmount', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    tauri.unlistenResult = () => Promise.reject(new Error('unlisten failed on unmount'));

    const { unmount } = render(<Harness shouldBlock={false} message="m" />);
    await timerTurns(2);
    unmount();
    await timerTurns(2);

    expect(unhandled, 'unhandled rejections seen').toEqual([]);
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it('after the window is destroyed', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    tauri.unlistenResult = () => Promise.reject(new Error('unlisten failed after destroy'));

    render(<Harness shouldBlock={false} message="m" />);
    await timerTurns(2);
    await pressWindowX();
    await timerTurns(2);

    expect(destroyMock).toHaveBeenCalledTimes(1);
    expect(unhandled, 'unhandled rejections seen').toEqual([]);
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it('for a registration that resolves only after unmount', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    tauri.unlistenResult = () => Promise.reject(new Error('unlisten failed for a late registration'));
    tauri.pending = [];

    const { unmount } = render(<Harness shouldBlock={false} message="m" />);
    unmount();
    tauri.pending.forEach((register) => register());
    await timerTurns(2);

    expect(tauri.unlistenCalls, 'the late listener is released').toBe(1);
    expect(unhandled, 'unhandled rejections seen').toEqual([]);
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it('when unlisten() throws synchronously', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    tauri.unlistenResult = () => {
      throw new Error('unlisten threw');
    };

    const { unmount } = render(<Harness shouldBlock={false} message="m" />);
    await timerTurns(2);
    expect(() => unmount()).not.toThrow();
    await timerTurns(2);

    expect(unhandled).toEqual([]);
    expect(warn).toHaveBeenCalledTimes(1);
  });
});
