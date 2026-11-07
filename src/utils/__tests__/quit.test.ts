/**
 * [#270] The exit guards.
 *
 * Two independent "you would lose work" conditions meet at quit time:
 *  - `projectStore.isDirty`            - unsaved to DISK;
 *  - `link.hasUnpushedEdits` + lock    - local edits NOT on the SpeleoDB server,
 *                                        which survive a save and a restart.
 * Before this, `quitApp` looked only at the first, so a saved-but-unpushed
 * project quit silently. These lock the guard, its ordering (SpeleoDB first, so
 * the prompt that can handle BOTH runs first and neither swallows the other) and
 * the one-shot confirmation that keeps the composed flow from looping.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

const closeMock = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: closeMock, destroy: vi.fn(async () => {}) }),
}));
vi.mock('../ipc');
stubGetErrorMessage(fallbackOr());

import {
  quitApp,
  guardQuit,
  performQuit,
  consumeQuitConfirmation,
  resetQuitConfirmationForTests,
} from '../quit';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { stubGetErrorMessage, fallbackOr } from '../../test/factories/ipc';
import { makeSpeleoLink } from '../../test/factories/speleo';

const link = (o: Partial<SpeleoLinkState> = {}): SpeleoLinkState =>
  makeSpeleoLink({ editState: 'editing', hasUnpushedEdits: true, ...o });

const openDialog = vi.fn();

beforeEach(() => {
  closeMock.mockReset();
  closeMock.mockResolvedValue(undefined);
  openDialog.mockReset();
  resetQuitConfirmationForTests();
  useProjectStore.setState({ project: { name: 'p' } as never, isDirty: false });
  useSpeleoStore.setState({ link: null });
  useUIStore.setState({ openDialog: openDialog as never, activeDialog: null, dialogData: {} });
});

/**
 * [#270] `quitApp` decides NOTHING: it asks for a close, un-confirmed, and the
 * App-level `useBeforeUnload` listener runs the whole exit flow - the native
 * import / dive-plan confirm FIRST, then `guardQuit`.
 *
 * It used to call `guardQuit()` here and then `performQuit()`, which put the tiers
 * in the opposite order for menu Quit / Ctrl+Q: `SpeleoQuitDialog` ran and its
 * "Push, release & quit" released the server lock, and only then did the re-issued
 * close reach the native prompt - which the user could still cancel, keeping the
 * app open with the lock irreversibly surrendered. The guards below are therefore
 * asserted on `guardQuit` directly, which is where they live.
 */
describe('quitApp', () => {
  it('asks for a close and guards nothing itself, whatever the state', () => {
    // Both guard conditions are live, and quitApp still just requests the close -
    // the listener asks the native prompt first, THEN opens this dialog.
    useProjectStore.setState({ isDirty: true });
    useSpeleoStore.setState({ link: link() });
    quitApp();
    expect(openDialog).not.toHaveBeenCalled();
    expect(closeMock).toHaveBeenCalledTimes(1);
  });

  it('does not arm the one-shot confirmation - that close must still be guarded', () => {
    quitApp();
    expect(consumeQuitConfirmation()).toBe(false);
  });
});

describe('guardQuit', () => {
  it('lets a clean project quit immediately', () => {
    expect(guardQuit()).toBe(false);
    expect(openDialog).not.toHaveBeenCalled();
  });

  it('quits immediately with no project open at all', () => {
    useProjectStore.setState({ project: null, isDirty: true });
    useSpeleoStore.setState({ link: link() });
    expect(guardQuit()).toBe(false);
    expect(openDialog).not.toHaveBeenCalled();
  });

  it('blocks on unsaved-to-disk changes', () => {
    useProjectStore.setState({ isDirty: true });
    expect(guardQuit()).toBe(true);
    expect(openDialog).toHaveBeenCalledWith('unsaved-changes', { pendingQuit: true });
  });

  // The regression this file exists for: saved to disk, so the old guard passed.
  it('blocks on unpushed SpeleoDB edits even when the project is saved to disk', () => {
    useSpeleoStore.setState({ link: link() });
    expect(guardQuit()).toBe(true);
    expect(openDialog).toHaveBeenCalledWith('speleodb-quit');
  });

  it('does NOT block while holding the lock with nothing unpushed', () => {
    useSpeleoStore.setState({ link: link({ hasUnpushedEdits: false }) });
    expect(guardQuit()).toBe(false);
    expect(openDialog).not.toHaveBeenCalled();
  });

  // The guard keys off the WORK AT RISK, not the lock: edits the server has never
  // seen are what quitting can strand, and whether we still hold the write lock
  // only changes which choices the dialog offers (it adapts to a lock-less state).
  it('blocks a force-edit session with unpushed edits - they are the most stranded of all', () => {
    // A force-editor edits WITHOUT the lock (#213), so there is nothing to
    // push/keep/release - which is exactly why walking away silently is wrong: the
    // edits cannot reach the server until a reconnect.
    useSpeleoStore.setState({ link: link({ forceEditing: true }) });
    expect(guardQuit()).toBe(true);
    expect(openDialog).toHaveBeenCalledWith('speleodb-quit');
  });

  it('blocks a read-only remote project that still has unpushed edits', () => {
    // How this is reached: `notifyReverifyDivergence()` drops to read-only while
    // KEEPING the unpushed edits, and any ReLogin error runs `clearSession()`,
    // which also drops to read-only. Requiring editState === 'editing' disarmed
    // the guard in precisely those states.
    useSpeleoStore.setState({ link: link({ editState: 'read-only' }) });
    expect(guardQuit()).toBe(true);
    expect(openDialog).toHaveBeenCalledWith('speleodb-quit');
  });

  it('does NOT block a read-only remote project with nothing unpushed', () => {
    useSpeleoStore.setState({
      link: link({ editState: 'read-only', hasUnpushedEdits: false }),
    });
    expect(guardQuit()).toBe(false);
    expect(openDialog).not.toHaveBeenCalled();
  });

  it('uses the SpeleoDB prompt (not the disk one) when a force-edit session is also disk-dirty', () => {
    // The SpeleoDB prompt subsumes the disk one - it carries its own "save
    // locally" checkbox - so it must be the one that runs, or "Discard & Quit"
    // would walk straight past the unpushed edits.
    useProjectStore.setState({ isDirty: true });
    useSpeleoStore.setState({ link: link({ forceEditing: true }) });
    expect(guardQuit()).toBe(true);
    expect(openDialog).toHaveBeenCalledWith('speleodb-quit');
  });

  // Composition: BOTH dirty. The SpeleoDB prompt runs first because it is the only
  // one that also handles the disk side (its "Save changes locally" checkbox); the
  // reverse order would let "Discard & Quit" walk straight past the unpushed edits.
  it('runs the SpeleoDB prompt first when the project is both disk-dirty and unpushed', () => {
    useProjectStore.setState({ isDirty: true });
    useSpeleoStore.setState({ link: link() });
    expect(guardQuit()).toBe(true);
    expect(openDialog).toHaveBeenCalledTimes(1);
    expect(openDialog).toHaveBeenCalledWith('speleodb-quit');
  });
});

/**
 * [#270] `uiStore` has ONE `activeDialog` slot and `openDialog` replaces it
 * unconditionally, so raising an exit prompt evicts whatever was on screen. That is
 * fine for the exit; it is not fine for CANCEL, which leaves the user in the app with
 * the evicted dialog gone. Press X mid-DMP-placement and the wizard was swapped out
 * for the exit prompt - cancelling then dropped the user back on a bare map with the
 * sections they had placed discarded, a discard they consented to only as part of
 * quitting.
 */
describe('guardQuit puts back the dialog it displaces', () => {
  it('carries the displaced dialog and its data into the SpeleoDB prompt', () => {
    useSpeleoStore.setState({ link: link() });
    useUIStore.setState({
      activeDialog: 'dmp-selection',
      dialogData: { dmpFile: { sections: 3 } },
    });

    expect(guardQuit()).toBe(true);
    expect(openDialog).toHaveBeenCalledWith('speleodb-quit', {
      cancelReturnsTo: 'dmp-selection',
      cancelReturnsData: { dmpFile: { sections: 3 } },
    });
  });

  it('carries it into the unsaved-changes prompt alongside pendingQuit', () => {
    useProjectStore.setState({ isDirty: true });
    useUIStore.setState({ activeDialog: 'dmp-import-new', dialogData: { dmpFile: {} } });

    expect(guardQuit()).toBe(true);
    expect(openDialog).toHaveBeenCalledWith('unsaved-changes', {
      pendingQuit: true,
      cancelReturnsTo: 'dmp-import-new',
      cancelReturnsData: { dmpFile: {} },
    });
  });

  it('adds nothing when no dialog was on screen', () => {
    useSpeleoStore.setState({ link: link() });
    expect(guardQuit()).toBe(true);
    expect(openDialog).toHaveBeenCalledWith('speleodb-quit');
  });

  it('does not ask a prompt to return to ITSELF - that is a loop, not a hop', () => {
    // Reachable: the dialog is up, the user presses X again rather than answering it.
    useSpeleoStore.setState({ link: link() });
    useUIStore.setState({ activeDialog: 'speleodb-quit', dialogData: {} });

    expect(guardQuit()).toBe(true);
    expect(openDialog).toHaveBeenCalledWith('speleodb-quit');
  });
});

describe('the one-shot quit confirmation', () => {
  it('is armed by performQuit and consumed exactly once', () => {
    expect(consumeQuitConfirmation()).toBe(false);
    performQuit();
    expect(consumeQuitConfirmation()).toBe(true);
    expect(consumeQuitConfirmation()).toBe(false);
  });

  it('is NOT armed by a guarded quit that never reached performQuit', () => {
    useProjectStore.setState({ isDirty: true });
    expect(guardQuit()).toBe(true);
    expect(consumeQuitConfirmation()).toBe(false);
  });

  // Terminates: the dialog's own action calls performQuit while the store is still
  // dirty/unpushed, and the close listener must NOT re-guard it into a loop.
  it('lets a dialog quit past a still-dirty store', async () => {
    useProjectStore.setState({ isDirty: true });
    useSpeleoStore.setState({ link: link() });
    performQuit();
    expect(closeMock).toHaveBeenCalledTimes(1);
    expect(consumeQuitConfirmation()).toBe(true);
  });

  // Hardening, not a fix for a reachable trigger. Every un-arming path is on the
  // emitting side (a rejected `close()`) or the receiving side (a consume), so a
  // request that IS emitted but whose event is never DELIVERED would leave the flag
  // armed forever - and the next genuine close would consume it and skip `guardQuit`
  // entirely, quitting silently over unpushed edits.
  it('expires instead of arming a much later close request', () => {
    vi.useFakeTimers();
    try {
      performQuit();
      vi.advanceTimersByTime(60_000);
      expect(consumeQuitConfirmation()).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it('survives the round trip it was armed for', () => {
    vi.useFakeTimers();
    try {
      performQuit();
      // A close request -> Rust prevent -> re-emit -> listener round trip is a single
      // IPC hop; the deadline must not be tight enough to lose a real one.
      vi.advanceTimersByTime(250);
      expect(consumeQuitConfirmation()).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('un-arms itself when the window close is rejected, so the next X is guarded again', async () => {
    closeMock.mockRejectedValueOnce(new Error('capability denied'));
    const err = vi.spyOn(console, 'error').mockImplementation(() => {});
    performQuit();
    await Promise.resolve();
    await Promise.resolve();
    expect(consumeQuitConfirmation()).toBe(false);
    err.mockRestore();
  });
});
