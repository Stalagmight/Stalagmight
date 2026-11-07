/**
 * [#576 Q-W44-7-D1] Release lock and Delete project on the Admin tab are verbs the
 * user started: refused because the project changed under them (the backend's
 * PROJECT_CHANGED), they toast the short "the project changed" line - a warning under
 * its own key - instead of their ordinary "Failed to ..." error toast.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../../utils/ipc');
stubGetErrorMessage(errorMessageOrString);

import { SpeleoAdminTab } from '../SpeleoAdminTab';
import { useSpeleoStore } from '../../../../store/speleoStore';
import { useProjectStore } from '../../../../store/projectStore';
import { useUIStore } from '../../../../store/uiStore';
import { IpcCommandError } from '../../../../utils/commandError';
import { TOAST_KEYS } from '../../../../utils/toastKeys';
import { invokeIpcMock, stubGetErrorMessage, errorMessageOrString } from '../../../../test/factories/ipc';
import { makeSpeleoLink } from '../../../../test/factories/speleo';

const SHORT = 'The project changed before this finished.';
const BACKEND_TEXT = 'Releasing the lock was not applied: another project is open now.';

let showToast: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  showToast = vi.fn();
  useUIStore.setState({ showToast, closeDialog: vi.fn() });
  useProjectStore.setState({ project: { name: 'My Cave' } as never });
  useSpeleoStore.setState({
    link: makeSpeleoLink({ permission: 'ADMIN', editState: 'editing' }),
    refreshLinkStatus: vi.fn(async () => {}),
    syncProjectLink: vi.fn(),
  });
});

describe('SpeleoAdminTab: a refusal because the project changed (#576 Q-W44-7-D1)', () => {
  it('Release lock toasts the short line as a warning, not the release-failed error', async () => {
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'speleodb_release_current_lock') {
        throw new IpcCommandError({ code: 'PROJECT_CHANGED', message: BACKEND_TEXT });
      }
      return null;
    });
    render(<SpeleoAdminTab />);

    await userEvent.click(screen.getByTestId('speleodb-lock-release'));

    await waitFor(() => expect(showToast).toHaveBeenCalled());
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(SHORT, TOAST_KEYS.PROJECT_CHANGED, 'warning');
    expect(showToast.mock.calls.some((c) => String(c[0]).includes('another project is open now'))).toBe(false);
    expect(showToast.mock.calls.some((c) => c[2] === 'error')).toBe(false);
  });

  it('any other release refusal keeps its error toast', async () => {
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'speleodb_release_current_lock') {
        throw new IpcCommandError({ code: 'SPELEODB_FATAL', message: 'server said no' });
      }
      return null;
    });
    render(<SpeleoAdminTab />);

    await userEvent.click(screen.getByTestId('speleodb-lock-release'));

    await waitFor(() => expect(showToast).toHaveBeenCalled());
    const [message, key, kind] = showToast.mock.calls[0] ?? [];
    expect(String(message)).toContain('server said no');
    expect(key).toBe(TOAST_KEYS.SPELEODB_LOCK);
    expect(kind).toBe('error');
  });
});
