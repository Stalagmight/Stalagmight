/**
 * [#576 RV6b-F1] The third caller of `discardChanges`: in Plan and Export the
 * survey is read-only, so "Discard changes" is refused before any IPC with the
 * read-only notice (`workspace_read_only`). The unwound-push dialog stopped
 * counting that refusal as the dialog completing (NF-FIX4FE-1); the explicit
 * Discard-changes dialog still did. Its only opener is disabled outside Survey,
 * so this is hard to reach, but the telemetry must not say the user discarded.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import { invokeIpcMock, stubGetErrorMessage, messageOr } from '../../../test/factories/ipc';

vi.mock('../../../utils/ipc');
stubGetErrorMessage(messageOr('e'));
const trackDialogCompleteMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/telemetry', async () =>
  (await import('../../../test/factories/telemetry')).telemetryWith({ trackDialogComplete: trackDialogCompleteMock }),
);

import SpeleoDiscardChangesDialog from '../SpeleoDiscardChangesDialog';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';

let closeDialog: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  trackDialogCompleteMock.mockReset();
  closeDialog = vi.fn(() => useUIStore.setState({ activeDialog: null }));
  useUIStore.setState({ closeDialog, openDialog: vi.fn(), showToast: vi.fn() });
  useSpeleoStore.setState({ remoteProjectId: 'p1', banner: null });
});

describe.each(['plan', 'export'] as const)('Discard changes in the read-only %s workspace', (appMode) => {
  it('closes without recording the dialog as completed', async () => {
    useUIStore.setState({ appMode, activeDialog: 'speleodb-discard-changes', dialogData: {} });
    render(<SpeleoDiscardChangesDialog />);
    await userEvent.click(screen.getByTestId('speleodb-discard-changes-keep'));

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    console.log(
      `[RV6b-F1] ${appMode}: IPC calls ${invokeIpcMock.mock.calls.length}; ` +
        `trackDialogComplete calls ${JSON.stringify(trackDialogCompleteMock.mock.calls)}`,
    );
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(trackDialogCompleteMock).not.toHaveBeenCalled();
  });
});
