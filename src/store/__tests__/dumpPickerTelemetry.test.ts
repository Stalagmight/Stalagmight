/**
 * [#258] The ambiguous-dump picker's telemetry lifecycle.
 *
 * It is an independent overlay, so it never touches `uiStore.openDialog` - which is
 * where the whole lifecycle normally lives. Every call it would have made has to be
 * made by hand, and each one was missed at least once: the four registered events
 * were dead on arrival, a completion left a marker standing that swallowed the NEXT
 * picker's cancel, and a session-loss teardown recorded nothing at all.
 *
 * These assert the calls rather than the emitted events because the sequencing is
 * the thing that broke - particularly complete-then-consume, which is invisible in
 * any single interaction and only shows up on the one after it.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

const trackAction = vi.hoisted(() => vi.fn());
const setDwellState = vi.hoisted(() => vi.fn());
const checkDialogCancel = vi.hoisted(() => vi.fn());
const trackDialogComplete = vi.hoisted(() => vi.fn());
vi.mock('../../utils/telemetry', async () =>
  (await import('../../test/factories/telemetry')).telemetryStubs({
    trackAction,
    setDwellState,
    checkDialogCancel,
    trackDialogComplete,
  }),
);
vi.mock('../../utils/ipc');
stubGetErrorMessage((e: unknown) => (e as { message?: string })?.message ?? 'error');
invokeIpcMock.mockImplementation(async () => null);

import { useSpeleoStore } from '../speleoStore';
import { useUIStore } from '../uiStore';
import { invokeIpcMock, stubGetErrorMessage } from '../../test/factories/ipc';

const DIALOG = 'dialog.speleodb-dump-picker';
const KEY = 'speleodb-dump-picker';

const CANDIDATES = [
  { entry_path: 'a.stalx', size_bytes: 1, modified: null },
  { entry_path: 'b.stalx', size_bytes: 2, modified: null },
];

beforeEach(() => {
  trackAction.mockClear();
  setDwellState.mockClear();
  checkDialogCancel.mockClear();
  trackDialogComplete.mockClear();
  useSpeleoStore.getState().closeDumpPicker();
  trackAction.mockClear();
  checkDialogCancel.mockClear();
  setDwellState.mockClear();
  useUIStore.setState({ activeDialog: null, activeTool: 'select' });
});

describe('dump picker telemetry (#258)', () => {
  it('records the open, the way any dialog does', () => {
    useSpeleoStore.getState().openDumpPicker(CANDIDATES, 'stalx', async () => {});

    expect(trackAction).toHaveBeenCalledWith(DIALOG);
    expect(setDwellState).toHaveBeenCalledWith(DIALOG);
  });

  it('records a dismissal and hands dwell back', () => {
    useSpeleoStore.getState().openDumpPicker(CANDIDATES, 'stalx', async () => {});
    useSpeleoStore.getState().closeDumpPicker();

    expect(checkDialogCancel).toHaveBeenCalledWith(KEY);
    expect(setDwellState).toHaveBeenLastCalledWith('tool.select');
  });

  it('hands dwell back to the dialog underneath, not to the map tool', () => {
    // This overlay layers OVER other dialogs. Returning dwell to the tool - the way
    // `uiStore.closeDialog` does - would attribute it to a surface nobody is on.
    useUIStore.setState({ activeDialog: 'speleodb-project' });
    useSpeleoStore.getState().openDumpPicker(CANDIDATES, 'stalx', async () => {});
    useSpeleoStore.getState().closeDumpPicker();

    expect(setDwellState).toHaveBeenLastCalledWith('dialog.speleodb-project');
  });

  it('DO NOT REGRESS: consumes its own completion marker', async () => {
    // `trackDialogComplete` only SETS a module-global marker; `checkDialogCancel` is
    // what consumes it. Completing without consuming left it standing, so the next
    // picker's cancel was read as this one's completion and emitted nothing.
    useSpeleoStore.getState().openDumpPicker(CANDIDATES, 'stalx', async () => {});
    await useSpeleoStore.getState().resolveDumpPicker('a.stalx');

    expect(trackDialogComplete).toHaveBeenCalledWith(KEY);
    expect(checkDialogCancel).toHaveBeenCalledWith(KEY);
  });

  it('DO NOT REGRESS: records a session-loss teardown as a dismissal', () => {
    // Losing the session drops the picker without an answer, which owes the same
    // telemetry as any other dismissal - and dwell must not stay on a picker that
    // is no longer on screen.
    useSpeleoStore.getState().openDumpPicker(CANDIDATES, 'stalx', async () => {});
    checkDialogCancel.mockClear();
    setDwellState.mockClear();

    useSpeleoStore.getState().clearSession();

    expect(checkDialogCancel).toHaveBeenCalledWith(KEY);
    expect(setDwellState).toHaveBeenLastCalledWith('tool.select');
  });

  it('records nothing extra when the session ends with no picker up', () => {
    useSpeleoStore.getState().clearSession();
    expect(checkDialogCancel).not.toHaveBeenCalled();
  });
});
