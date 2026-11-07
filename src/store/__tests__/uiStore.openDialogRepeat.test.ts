/**
 * `openDialog` treats a repeat open of the ALREADY-ACTIVE dialog as a data
 * refresh, not a chain and not a new open.
 *
 * The state half must stay unconditional: several reachable repeats carry a
 * different payload (a background re-verify moving `speleodb-divergence` on, a
 * second async failure re-messaging `error`, `openExitPrompt` re-raising a quit
 * prompt over itself), and `context-menu` is opened straight off a pointer
 * gesture with a per-gesture target. Suppressing the state update would show the
 * user stale content.
 *
 * The bookkeeping half must be gated: chaining is A -> B, so completing A
 * against itself only clobbers a pending completion, and a refresh is not a
 * second `dialog.<name>` open - counting it twice also burned the 50 ms
 * `setSource` hint via `consumeSource`, mis-attributing the user's next action.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';

const trackActionMock = vi.hoisted(() => vi.fn());
const trackDialogCompleteMock = vi.hoisted(() => vi.fn());
const checkDialogCancelMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/telemetry', async () =>
  (await import('../../test/factories/telemetry')).telemetryWith({
    trackAction: trackActionMock,
    trackDialogComplete: trackDialogCompleteMock,
    checkDialogCancel: checkDialogCancelMock,
  }),
);

import { useUIStore } from '../uiStore';

describe('uiStore.openDialog repeat-open handling', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useUIStore.setState({ activeDialog: null, dialogData: {} } as never);
  });

  it('still replaces dialogData when the same dialog is re-opened', () => {
    useUIStore.getState().openDialog('error', { message: 'first' });
    useUIStore.getState().openDialog('error', { message: 'second' });

    expect(useUIStore.getState().activeDialog).toBe('error');
    expect(useUIStore.getState().dialogData).toEqual({ message: 'second' });
  });

  it('does not count a repeat open as a second dialog action', () => {
    useUIStore.getState().openDialog('save-permission', { context: 'save' });
    expect(trackActionMock).toHaveBeenCalledTimes(1);

    useUIStore.getState().openDialog('save-permission', { context: 'save' });
    expect(trackActionMock).toHaveBeenCalledTimes(1);
  });

  it('does not run chaining bookkeeping against the dialog itself', () => {
    useUIStore.getState().openDialog('speleodb-divergence');
    trackDialogCompleteMock.mockClear();
    checkDialogCancelMock.mockClear();

    useUIStore.getState().openDialog('speleodb-divergence');

    expect(trackDialogCompleteMock).not.toHaveBeenCalled();
    expect(checkDialogCancelMock).not.toHaveBeenCalled();
  });

  it('still chains, and still counts, when the dialog id actually changes', () => {
    useUIStore.getState().openDialog('context-menu', { x: 1 });
    trackActionMock.mockClear();

    useUIStore.getState().openDialog('edit-station', { stationIndex: 2 });

    expect(trackDialogCompleteMock).toHaveBeenCalledWith('context-menu');
    expect(checkDialogCancelMock).toHaveBeenCalledWith('context-menu');
    expect(trackActionMock).toHaveBeenCalledWith('dialog.edit-station');
    expect(useUIStore.getState().dialogData).toEqual({ stationIndex: 2 });
  });
});
