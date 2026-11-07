/**
 * [#270 - bug 1] The OS window X must go through the app's OWN exit guard.
 *
 * The window X never reaches `quitApp`: it arrives as a `tauri://close-requested`
 * event, and this hook was the only listener. Its `shouldBlock` argument covered
 * an in-flight import and a dirty dive plan and nothing else, so closing via X
 * destroyed the window - discarding unsaved-to-disk work and unpushed SpeleoDB
 * edits with no prompt at all, while the menu Quit prompted for both.
 *
 * These lock the wiring: the guard runs on every close request, an opened dialog
 * aborts the close, and the quit that dialog then issues is not re-guarded (which
 * would loop forever, since a discard deliberately leaves the store dirty).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/react';

const listeners = vi.hoisted(() => [] as Array<() => Promise<void> | void>);
const unlisten = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_name: string, cb: () => Promise<void>) => {
    listeners.push(cb);
    return unlisten;
  }),
}));

const destroyMock = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ destroy: destroyMock, close: vi.fn(async () => {}) }),
}));

const confirmMock = vi.hoisted(() => vi.fn(async () => true));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: confirmMock }));

import { useBeforeUnload } from '../useBeforeUnload';
import { performQuit, resetQuitConfirmationForTests } from '../../utils/quit';
import { nextTimer } from '../../test/factories/async';

function Harness({
  shouldBlock,
  guard,
  onConfirm,
}: {
  shouldBlock: boolean;
  guard: () => boolean;
  onConfirm?: () => Promise<void>;
}) {
  useBeforeUnload(shouldBlock, 'discard?', onConfirm, guard);
  return null;
}

/** Fire the close-requested event the OS window X produces. */
async function pressWindowX(): Promise<void> {
  for (const cb of [...listeners]) await cb();
}

beforeEach(() => {
  listeners.length = 0;
  destroyMock.mockClear();
  destroyMock.mockResolvedValue(undefined);
  unlisten.mockClear();
  confirmMock.mockClear();
  confirmMock.mockResolvedValue(true);
  resetQuitConfirmationForTests();
});

describe('useBeforeUnload - the in-app exit guard', () => {
  it('destroys the window when nothing needs guarding', async () => {
    const guard = vi.fn(() => false);
    render(<Harness shouldBlock={false} guard={guard} />);
    await nextTimer();

    await pressWindowX();

    expect(guard).toHaveBeenCalledTimes(1);
    expect(destroyMock).toHaveBeenCalledTimes(1);
  });

  it('aborts the close when the guard opens a dialog', async () => {
    const guard = vi.fn(() => true);
    render(<Harness shouldBlock={false} guard={guard} />);
    await nextTimer();

    await pressWindowX();

    expect(destroyMock).not.toHaveBeenCalled();
  });

  it('skips the guard for a quit the user already confirmed in-app', async () => {
    // The guard still reports "dirty" - a discard leaves it that way on purpose.
    const guard = vi.fn(() => true);
    render(<Harness shouldBlock={false} guard={guard} />);
    await nextTimer();

    performQuit(); // what the dialog's Quit button does
    await pressWindowX();

    expect(guard).not.toHaveBeenCalled();
    expect(destroyMock).toHaveBeenCalledTimes(1);
  });

  it('re-guards the next close request - the confirmation is one-shot', async () => {
    const guard = vi.fn(() => true);
    render(<Harness shouldBlock={false} guard={guard} />);
    await nextTimer();

    performQuit();
    await pressWindowX();
    destroyMock.mockClear();

    // A second X (in a hypothetical still-running window) is guarded again.
    listeners.length = 0;
    render(<Harness shouldBlock={false} guard={guard} />);
    await nextTimer();
    await pressWindowX();

    expect(guard).toHaveBeenCalledTimes(1);
    expect(destroyMock).not.toHaveBeenCalled();
  });

  it('runs the import/dive-plan confirm FIRST, then the guard', async () => {
    const guard = vi.fn(() => true);
    render(<Harness shouldBlock guard={guard} />);
    await nextTimer();

    await pressWindowX();

    expect(confirmMock).toHaveBeenCalledTimes(1);
    expect(guard).toHaveBeenCalledTimes(1);
    expect(destroyMock).not.toHaveBeenCalled();
  });

  it('never reaches the guard when the import/dive-plan confirm is declined', async () => {
    confirmMock.mockResolvedValue(false);
    const guard = vi.fn(() => false);
    render(<Harness shouldBlock guard={guard} />);
    await nextTimer();

    await pressWindowX();

    expect(guard).not.toHaveBeenCalled();
    expect(destroyMock).not.toHaveBeenCalled();
  });

  it('still prompts for a dirty dive plan on a confirmed in-app quit', async () => {
    // `preConfirmed` covers the PROJECT decision only; the dive plan is separate
    // in-memory work the same close would drop. (Skipped only when THIS attempt
    // already asked - see the ordering suite below.)
    const guard = vi.fn(() => false);
    render(<Harness shouldBlock guard={guard} />);
    await nextTimer();

    performQuit();
    await pressWindowX();

    expect(confirmMock).toHaveBeenCalledTimes(1);
    expect(destroyMock).toHaveBeenCalledTimes(1);
  });
});

/**
 * [#270] Nothing that can still CANCEL the quit may run after something
 * irreversible.
 *
 * A tier-2 dialog aborts the close and re-issues it once the user acts - and
 * `SpeleoQuitDialog`'s "Push, release & quit" pushes and RELEASES the server lock
 * before doing so. The tier-1 native prompt used to be asked on that second pass,
 * after the release: cancelling it left the user in the app with the lock
 * irreversibly surrendered and the project silently read-only. Tier 1 is asked on
 * the FIRST pass instead, and its answer carries across the dialog.
 */
describe('useBeforeUnload - every cancellable prompt precedes the irreversible act', () => {
  it('asks tier 1 before the tier-2 dialog, and not again after it', async () => {
    const guard = vi.fn(() => true);
    render(<Harness shouldBlock guard={guard} />);
    await nextTimer();

    // Pass 1: the quit attempt (menu Quit now arrives here too, un-confirmed).
    await pressWindowX();
    expect(confirmMock).toHaveBeenCalledTimes(1);
    expect(guard).toHaveBeenCalledTimes(1);
    expect(destroyMock).not.toHaveBeenCalled();

    // Pass 2: the dialog has done its irreversible work and re-issued the close.
    performQuit();
    await pressWindowX();

    expect(confirmMock).toHaveBeenCalledTimes(1); // asked once, and asked FIRST
    expect(destroyMock).toHaveBeenCalledTimes(1);
  });

  it('declining tier 1 aborts before the tier-2 dialog can act at all', async () => {
    confirmMock.mockResolvedValue(false);
    const guard = vi.fn(() => true);
    render(<Harness shouldBlock guard={guard} />);
    await nextTimer();

    await pressWindowX();

    // The dialog that would push and release never even opens.
    expect(guard).not.toHaveBeenCalled();
    expect(destroyMock).not.toHaveBeenCalled();
  });

  it('re-asks on a later attempt when the first was abandoned at the dialog', async () => {
    // The user answered tier 1, then cancelled the tier-2 dialog. That answer was
    // for an attempt that never happened, so a fresh Quit must ask again.
    const guard = vi.fn(() => true);
    render(<Harness shouldBlock guard={guard} />);
    await nextTimer();

    await pressWindowX();
    expect(confirmMock).toHaveBeenCalledTimes(1);

    // ...dialog cancelled (no signal reaches us) ... user quits again:
    await pressWindowX();
    expect(confirmMock).toHaveBeenCalledTimes(2);
  });

  it('still runs the tier-1 cleanup on the pass that skips the prompt', async () => {
    // The cleanup is keyed on `shouldBlock`, not on "we prompted this pass" - it
    // must run exactly once, when the window actually goes away.
    const onConfirm = vi.fn(async () => {});
    const guard = vi.fn(() => true);
    render(<Harness shouldBlock guard={guard} onConfirm={onConfirm} />);
    await nextTimer();

    await pressWindowX();
    expect(onConfirm).not.toHaveBeenCalled();

    performQuit();
    await pressWindowX();

    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(destroyMock).toHaveBeenCalledTimes(1);
  });
});

/**
 * [#270] The tier-1 cleanup is destructive by contract ("e.g. delete placed
 * sections"), so it may only run once the close is actually going ahead. It used to
 * run inside the tier-1 confirmation, ABOVE the tier-2 guard - so a close the guard
 * then aborted had already performed the deletions: the user gets their prompt,
 * cancels, keeps working, and the sections are gone anyway.
 */
describe('useBeforeUnload - the tier-1 cleanup waits for the tier-2 guard', () => {
  it('does NOT run the cleanup when the guard aborts the close', async () => {
    const onConfirm = vi.fn(async () => {});
    const guard = vi.fn(() => true);
    render(<Harness shouldBlock guard={guard} onConfirm={onConfirm} />);
    await nextTimer();

    await pressWindowX();

    expect(confirmMock).toHaveBeenCalledTimes(1);
    expect(guard).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
    expect(destroyMock).not.toHaveBeenCalled();
  });

  it('runs the cleanup once the close survives every guard', async () => {
    const onConfirm = vi.fn(async () => {});
    const guard = vi.fn(() => false);
    render(<Harness shouldBlock guard={guard} onConfirm={onConfirm} />);
    await nextTimer();

    await pressWindowX();

    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(destroyMock).toHaveBeenCalledTimes(1);
  });

  it('does not run the cleanup when the tier-1 confirm itself is declined', async () => {
    confirmMock.mockResolvedValue(false);
    const onConfirm = vi.fn(async () => {});
    render(<Harness shouldBlock guard={() => false} onConfirm={onConfirm} />);
    await nextTimer();

    await pressWindowX();

    expect(onConfirm).not.toHaveBeenCalled();
    expect(destroyMock).not.toHaveBeenCalled();
  });

  // A failed destroy leaves the window on screen, so it must leave the app quittable
  // too. Latching `isClosing` and unregistering the listener BEFORE the destroy made
  // one rejection permanent: the early return swallowed every later close request,
  // and there was no listener left for the OS X to reach anyway.
  it('a rejecting destroy still lets a second X try', async () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => {});
    destroyMock.mockRejectedValueOnce(new Error('capability denied'));
    render(<Harness shouldBlock={false} guard={() => false} />);
    await nextTimer();

    await pressWindowX();
    expect(destroyMock).toHaveBeenCalledTimes(1);
    // The listener is still registered - it was never torn down.
    expect(unlisten).not.toHaveBeenCalled();

    await pressWindowX();

    expect(destroyMock).toHaveBeenCalledTimes(2);
    err.mockRestore();
  });

  it('a throwing cleanup still lets the window close', async () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => {});
    const onConfirm = vi.fn(async () => {
      throw new Error('cleanup boom');
    });
    render(<Harness shouldBlock guard={() => false} onConfirm={onConfirm} />);
    await nextTimer();

    await pressWindowX();

    expect(destroyMock).toHaveBeenCalledTimes(1);
    err.mockRestore();
  });
});
