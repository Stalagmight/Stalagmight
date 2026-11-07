/**
 * [#576 NF-FIX4FE-1] In Plan and Export the survey is read-only, so "Take the
 * SpeleoDB version" (divergence dialog) and "Discard and keep editing" (unwound-push
 * dialog) are refused before any IPC, with the read-only notice (RV4-FE-SP4). The
 * dialog that asked must then close: the remedy is the workspace switch, not a
 * retry from a dialog still sitting over the map. The divergence dialog stayed open
 * on its destroy step, and the unwound-push dialog counted the refusal as the
 * dialog completing.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/ipc');
stubGetErrorMessage(messageOr('e'));
const trackDialogCompleteMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/telemetry', async () =>
  (await import('../../../test/factories/telemetry')).telemetryWith({ trackDialogComplete: trackDialogCompleteMock }),
);

import i18n from '../../../i18n';
import SpeleoDivergenceDialog from '../SpeleoDivergenceDialog';
import SpeleoUnwoundPushDialog from '../SpeleoUnwoundPushDialog';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import { TOAST_KEYS } from '../../../utils/toastKeys';
import { invokeIpcMock, stubGetErrorMessage, messageOr } from '../../../test/factories/ipc';

let closeDialog: ReturnType<typeof vi.fn>;
let showToast: ReturnType<typeof vi.fn>;
let clearDivergence: ReturnType<typeof vi.fn>;

const NOTICE = () => i18n.t('editGate.workspaceReadOnlyNotice');

beforeEach(() => {
  invokeIpcMock.mockReset();
  trackDialogCompleteMock.mockReset();
  closeDialog = vi.fn(() => useUIStore.setState({ activeDialog: null }));
  showToast = vi.fn();
  clearDivergence = vi.fn();
  useUIStore.setState({ closeDialog, openDialog: vi.fn(), showToast });
  useSpeleoStore.setState({ remoteProjectId: 'p1', clearDivergence, banner: null });
});

describe.each(['plan', 'export'] as const)('in the read-only %s workspace', (appMode) => {
  beforeEach(() => {
    useUIStore.setState({ appMode });
  });

  it('Take the SpeleoDB version is refused with the notice and the divergence dialog closes', async () => {
    useUIStore.setState({ activeDialog: 'speleodb-divergence', dialogData: {} });
    render(<SpeleoDivergenceDialog />);
    await userEvent.click(screen.getByTestId('speleodb-divergence-take-remote'));
    await userEvent.click(screen.getByTestId('speleodb-take-remote-destroy'));

    await waitFor(() => expect(showToast).toHaveBeenCalled());
    console.log(
      `${appMode}: take remote -> toast ${JSON.stringify(showToast.mock.calls[0]?.[0])}; ` +
        `IPC calls ${invokeIpcMock.mock.calls.length}; dialog closed: ${closeDialog.mock.calls.length > 0}`,
    );
    expect(showToast).toHaveBeenCalledWith(NOTICE(), TOAST_KEYS.EDIT_DISABLED, 'warning');
    expect(invokeIpcMock).not.toHaveBeenCalled();
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    // Like Cancel: the divergence is left for later, and the App effect must not
    // re-open the dialog straight away.
    expect(clearDivergence).toHaveBeenCalled();
  });

  it('Discard is refused with the notice and the unwound-push dialog closes, not completed', async () => {
    useUIStore.setState({ activeDialog: 'speleodb-unwound-push', dialogData: { message: 'm' } });
    render(<SpeleoUnwoundPushDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unwound-push-discard'));

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    console.log(
      `${appMode}: discard -> toast ${JSON.stringify(showToast.mock.calls[0]?.[0])}; ` +
        `IPC calls ${invokeIpcMock.mock.calls.length}; completion tracked: ${trackDialogCompleteMock.mock.calls.length}`,
    );
    expect(showToast).toHaveBeenCalledWith(NOTICE(), TOAST_KEYS.EDIT_DISABLED, 'warning');
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(trackDialogCompleteMock).not.toHaveBeenCalled();
  });
});
