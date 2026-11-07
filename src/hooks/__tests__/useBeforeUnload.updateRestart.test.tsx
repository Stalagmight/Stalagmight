/**
 * [#579 x #576] An update restart gets every guarantee a quit gets.
 *
 * `requestUpdateRestart` asks for an ordinary window close, so the restart runs
 * the same listener as the window X and menu Quit. These drive that listener
 * with the REAL exit guards (`guardQuit`, the exit prompts' store state, the
 * autosave hold) rather than a stub, and pin what #576 made of a quit:
 *
 * - tier 1 (an import or a dirty dive plan) is asked before any in-app prompt;
 * - the SpeleoDB prompt comes before the unsaved-changes one (`guardQuit` order);
 * - nothing is installed while a prompt is open, and a cancelled prompt abandons
 *   the restart on both sides;
 * - autosave is held from "Discard & Quit" until the install ends (#576 D34),
 *   however long the install takes - not just for the quit confirmation's TTL.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render } from '@testing-library/react';

const listeners = vi.hoisted(() => [] as Array<() => Promise<void> | void>);
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_name: string, cb: () => Promise<void>) => {
    listeners.push(cb);
    return vi.fn();
  }),
}));

const destroyMock = vi.hoisted(() => vi.fn(async () => {}));
const closeMock = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ destroy: destroyMock, close: closeMock }),
}));

const confirmMock = vi.hoisted(() => vi.fn(async () => true));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: confirmMock }));

vi.mock('../../utils/ipc');

import { useBeforeUnload } from '../useBeforeUnload';
import {
  abandonUpdateRestart,
  guardQuit,
  performQuit,
  requestUpdateRestart,
  resetQuitConfirmationForTests,
  resetUpdateRestartForTests,
} from '../../utils/quit';
import { isAutosaveHeld, useAutosaveHoldStore } from '../../utils/autosaveHold';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { useUpdateStore } from '../../store/updateStore';
import { deferred } from '../../test/factories/async';
import { invokeIpcMock } from '../../test/factories/ipc';
import { makeSpeleoLink } from '../../test/factories/speleo';

const INSTALL = 'install_app_update_and_restart';

function App({ shouldBlock }: { shouldBlock: boolean }) {
  // As App.tsx wires it: tier 1 for an import / dive plan, `guardQuit` as tier 2.
  useBeforeUnload(shouldBlock, 'Import is in progress. Discard all import data and close?', undefined, guardQuit);
  return null;
}

/** The backend prevents the close and re-emits it; this is that re-emission. */
async function closeRequested(): Promise<void> {
  for (const cb of [...listeners]) await cb();
}

function calls(cmd: string): number {
  return invokeIpcMock.mock.calls.filter((c) => c[0] === cmd).length;
}

/** Mount, then arm and request the restart exactly as the banner's Restart does. */
async function restartFrom(shouldBlock = false): Promise<void> {
  render(<App shouldBlock={shouldBlock} />);
  await vi.advanceTimersByTimeAsync(0);
  await requestUpdateRestart();
  expect(calls('arm_app_update_restart')).toBe(1);
  expect(closeMock).toHaveBeenCalledTimes(1);
  await closeRequested();
}

beforeEach(() => {
  vi.useFakeTimers();
  listeners.length = 0;
  destroyMock.mockClear();
  closeMock.mockClear();
  confirmMock.mockReset();
  confirmMock.mockResolvedValue(true);
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  resetQuitConfirmationForTests();
  resetUpdateRestartForTests();
  useAutosaveHoldStore.setState({ holds: 0 });
  useUpdateStore.getState().reset();
  useUIStore.setState({ activeDialog: null, dialogData: {} });
  useSpeleoStore.setState({ link: null });
  useProjectStore.setState({ project: { name: 'CaveA' } as never, isDirty: false });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('an update restart runs the exit guards a quit runs', () => {
  it('asks tier 1 first, and declining it abandons the restart on both sides', async () => {
    useProjectStore.setState({ isDirty: true });
    confirmMock.mockResolvedValue(false);

    await restartFrom(true);

    expect(confirmMock).toHaveBeenCalledTimes(1);
    // The in-app guard never ran: no prompt is up.
    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(calls('unarm_app_update_restart')).toBe(1);
    expect(calls(INSTALL)).toBe(0);
    expect(destroyMock).not.toHaveBeenCalled();
  });

  it('puts the SpeleoDB prompt before the unsaved-changes one, and installs nothing while it is up', async () => {
    // Both conditions at once: unpushed edits AND unsaved to disk. The SpeleoDB
    // prompt is the one that can settle both, so it must be the one asked.
    useProjectStore.setState({ isDirty: true });
    useSpeleoStore.setState({ link: makeSpeleoLink({ editState: 'editing', hasUnpushedEdits: true }) });

    await restartFrom();

    expect(useUIStore.getState().activeDialog).toBe('speleodb-quit');
    expect(calls(INSTALL)).toBe(0);
    expect(destroyMock).not.toHaveBeenCalled();
    expect(isAutosaveHeld()).toBe(true);

    // Its Cancel (`SpeleoQuitDialog.dismiss`) abandons the exit, restart included.
    abandonUpdateRestart();
    expect(calls('unarm_app_update_restart')).toBe(1);
  });

  it('after Discard & Quit installs instead of destroying, with autosave held for the whole install', async () => {
    useProjectStore.setState({ isDirty: true });
    const install = deferred<null>();
    invokeIpcMock.mockImplementation((cmd: string) => (cmd === INSTALL ? install.promise : Promise.resolve(null)));

    await restartFrom();

    // The unsaved-changes prompt, in its quit form; nothing installed yet.
    expect(useUIStore.getState().activeDialog).toBe('unsaved-changes');
    expect(useUIStore.getState().dialogData.pendingQuit).toBe(true);
    expect(calls(INSTALL)).toBe(0);

    // "Discard & Quit" (UnsavedChangesDialog.handleDiscard with pendingQuit):
    // close the prompt, then the confirmed quit re-issues the close.
    useUIStore.getState().closeDialog();
    performQuit();
    // Not awaited: the listener waits on the install, which is still running.
    const closing = closeRequested();
    await vi.advanceTimersByTimeAsync(0);

    expect(calls(INSTALL)).toBe(1);
    expect(destroyMock).not.toHaveBeenCalled();

    // The quit confirmation's own hold lapses after its TTL; an install that runs
    // longer must still keep autosave off the discarded edits.
    await vi.advanceTimersByTimeAsync(60_000);
    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(isAutosaveHeld()).toBe(true);

    // A failed install leaves the app running - and autosave with it.
    const err = vi.spyOn(console, 'error').mockImplementation(() => {});
    install.reject({ code: 'FAILED', message: 'Update install failed: disk full' });
    await closing;
    expect(isAutosaveHeld()).toBe(false);
    expect(destroyMock).not.toHaveBeenCalled();
    err.mockRestore();
  });

  it('a clean project installs straight away, past every guard', async () => {
    await restartFrom();

    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(calls(INSTALL)).toBe(1);
    expect(destroyMock).not.toHaveBeenCalled();
  });
});
