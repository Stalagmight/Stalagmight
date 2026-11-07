/**
 * [#576 Q-W44-7-D1] The SpeleoDB undo-mode write is a BACKGROUND verb: refused
 * because the project changed under it (the backend's PROJECT_CHANGED), it shows
 * nothing - no inline error, no toast. Any other refusal still shows its inline error.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../../utils/ipc');
stubGetErrorMessage(errorMessageOrString);

import { SpeleoUndoModeControl } from '../SpeleoUndoModeControl';
import { useSpeleoStore } from '../../../../store/speleoStore';
import { useUIStore } from '../../../../store/uiStore';
import { IpcCommandError } from '../../../../utils/commandError';
import { invokeIpcMock, stubGetErrorMessage, errorMessageOrString } from '../../../../test/factories/ipc';
import { makeSpeleoLink } from '../../../../test/factories/speleo';

let showToast: ReturnType<typeof vi.fn>;

const refuseUndoMode = (failure: IpcCommandError) =>
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'speleodb_set_undo_mode') throw failure;
    return null;
  });

describe('SpeleoUndoModeControl: a refusal because the project changed (#576 Q-W44-7-D1)', () => {
  beforeEach(() => {
    invokeIpcMock.mockReset();
    showToast = vi.fn();
    useUIStore.setState({ showToast });
    useSpeleoStore.setState({
      projectLinkKind: 'remote',
      link: makeSpeleoLink({ editState: 'editing', undoMode: 'full' }),
    });
  });

  it('shows nothing: no inline error, no toast', async () => {
    refuseUndoMode(
      new IpcCommandError({
        code: 'PROJECT_CHANGED',
        message: 'Setting the undo mode was not applied: another project is open now.',
      }),
    );
    render(<SpeleoUndoModeControl />);

    await userEvent.click(screen.getByTestId('speleodb-undo-mode-disabled-input'));

    await waitFor(() =>
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_set_undo_mode')).toBe(true),
    );
    // The control is usable again once the refusal settled (busy cleared)...
    await waitFor(() => expect(screen.getByTestId('speleodb-undo-mode-full-input')).toBeEnabled());
    // ...and nothing said anything about it.
    expect(screen.queryByTestId('speleodb-undo-mode-error')).toBeNull();
    expect(showToast).not.toHaveBeenCalled();
  });

  it('any other refusal still shows its inline error', async () => {
    refuseUndoMode(new IpcCommandError({ code: 'SPELEODB_READ_ONLY', message: 'you do not hold the lock' }));
    render(<SpeleoUndoModeControl />);

    await userEvent.click(screen.getByTestId('speleodb-undo-mode-disabled-input'));

    expect(await screen.findByTestId('speleodb-undo-mode-error')).toHaveTextContent('you do not hold the lock');
  });
});
