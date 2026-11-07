/**
 * Component test (#215) - the Admin tab of the SpeleoDB project dialog: lock
 * release and the type-to-confirm project delete (which unlinks + closes on
 * success).
 *
 * The tab is deliberately NOT admin-only: only the danger zone is. Self-release is
 * the sole in-app stuck-lock recovery, so a write-tier user must still reach it.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/ipc');
stubGetErrorMessage(errorMessageOrString);

import { SpeleoAdminTab } from './SpeleoAdminTab';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import type { Permission } from '../../../types/ipc.generated';
import { invokeIpcMock, stubGetErrorMessage, errorMessageOrString } from '../../../test/factories/ipc';

let refreshLinkStatus: ReturnType<typeof vi.fn>;
let syncProjectLink: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;
let showToast: ReturnType<typeof vi.fn>;

const seed = (permission: Permission, lockHolder: string | null = null, editState = 'read-only') => {
  useSpeleoStore.setState({
    link: {
      permission,
      editState,
      lockHolder,
      lockedByOther: lockHolder != null && editState !== 'editing',
      hasUnpushedEdits: false,
      unwound: false,
      remoteState: 'up_to_date',
      noAccess: false,
      serverError: null,
    } as never,
    refreshLinkStatus,
    syncProjectLink,
  });
};

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  refreshLinkStatus = vi.fn(async () => {});
  syncProjectLink = vi.fn();
  closeDialog = vi.fn();
  showToast = vi.fn();
  useUIStore.setState({ showToast, closeDialog });
  useProjectStore.setState({ project: { name: 'My Cave' } as never });
});

describe('SpeleoAdminTab', () => {
  it('releases a held lock and refreshes status', async () => {
    seed('ADMIN', 'bob@x.com');
    render(<SpeleoAdminTab />);
    expect(screen.getByTestId('speleodb-lock-holder')).toHaveTextContent('bob@x.com');
    await userEvent.click(screen.getByTestId('speleodb-lock-release'));
    await waitFor(() =>
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_release_current_lock')).toBe(
        true,
      ),
    );
    await waitFor(() => expect(refreshLinkStatus).toHaveBeenCalled());
  });

  it('warns and offers release when the caller holds the lock (holder still null)', () => {
    // The stuck-lock recovery case: after acquiring a lock the fast path flips
    // editState to 'editing' but leaves lockHolder null. The Lock section must still
    // show "held by you" + a release button, not a dead-end "no lock held".
    seed('ADMIN', null, 'editing');
    render(<SpeleoAdminTab />);
    expect(screen.getByTestId('speleodb-lock-self-warning')).toBeInTheDocument();
    expect(screen.getByTestId('speleodb-lock-release')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-lock-none')).toBeNull();
  });

  it('keeps self-release reachable for a non-admin, without the danger zone', () => {
    seed('READ_AND_WRITE', null, 'editing');
    render(<SpeleoAdminTab />);
    expect(screen.getByTestId('speleodb-lock-release')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-delete-button')).toBeNull();
  });

  it('gates delete on typing the project name, then unlinks + closes', async () => {
    seed('ADMIN');
    render(<SpeleoAdminTab />);
    const del = screen.getByTestId('speleodb-delete-button');
    expect(del).toBeDisabled();
    await userEvent.type(screen.getByTestId('speleodb-delete-confirm-input'), 'My Cave');
    expect(del).toBeEnabled();
    await userEvent.click(del);
    await waitFor(() =>
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_delete_project')).toBe(true),
    );
    // Confirmed LOCAL, not merely unlinked: `clearLink()` would leave
    // `projectLinkKind: 'unknown'`, which `selectSpeleoReadOnly` reads as
    // read-only, stranding the still-open (now plain local) project uneditable
    // with no poll left to re-resolve it.
    await waitFor(() => expect(syncProjectLink).toHaveBeenCalledWith(false));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
  });

  it('a FAILED delete does NOT unlink or close (no false success)', async () => {
    seed('ADMIN');
    invokeIpcMock.mockImplementation((cmd: string) =>
      cmd === 'speleodb_delete_project'
        ? Promise.reject(new Error('server 500'))
        : Promise.resolve(null),
    );
    render(<SpeleoAdminTab />);
    await userEvent.type(screen.getByTestId('speleodb-delete-confirm-input'), 'My Cave');
    const del = screen.getByTestId('speleodb-delete-button');
    await userEvent.click(del);
    await waitFor(() => expect(showToast).toHaveBeenCalled());
    expect(syncProjectLink).not.toHaveBeenCalled();
    expect(closeDialog).not.toHaveBeenCalled();
    // The button re-enables so the user can retry / cancel.
    await waitFor(() => expect(del).toBeEnabled());
  });
});
