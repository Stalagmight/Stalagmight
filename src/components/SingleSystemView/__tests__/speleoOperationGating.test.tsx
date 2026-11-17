import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, renderHook, screen } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import { SpeleoWorkspace } from '../SpeleoWorkspace';
import { SpeleoDbSettings } from '../../Dialogs/settings/SpeleoDbSettings';
import OpenFromSpeleoDbDialog from '../../Dialogs/OpenFromSpeleoDbDialog';
import { isSpeleoOperationBusy, useSpeleoOperationBusy } from '../../../hooks/useSpeleoOperationBusy';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoNetworkStore } from '../../../store/speleoNetworkStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import { defaultConfig } from '../../../types/config';

const editingLink: SpeleoLinkState = {
  permission: 'READ_AND_WRITE', editState: 'editing', lockHolder: 'synthetic@example.com',
  lockedByOther: false, hasUnpushedEdits: false, unwound: false,
  undoMode: 'full', remoteState: 'up_to_date', noAccess: false, serverError: null,
};

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(() => Promise.resolve(null));
  useSpeleoNetworkStore.getState().reset();
  useProjectStore.setState({ project: { name: 'Synthetic cave', file_path: '/synthetic.stalx', zero_datums: [] } as never });
  useSpeleoStore.setState({
    busy: false,
    link: editingLink,
    projectLinkKind: 'remote',
    remoteProjectId: 'synthetic-project',
    session: { loggedIn: true, userEmail: 'synthetic@example.com', endpoint: 'https://speleodb.example' },
    pendingAuth: null,
    authError: null,
    refreshSession: vi.fn(async () => {}),
  });
  useUIStore.setState({ activeDialog: null, dialogData: {}, appMode: 'survey' });
});

/** Start one SpeleoDB call of `kind`, as `invokeIpc` would. */
function startCall(kind: 'read' | 'exclusive'): () => void {
  let end!: () => void;
  act(() => {
    end = useSpeleoNetworkStore.getState().begin(kind);
  });
  return () => act(() => end());
}

describe('useSpeleoOperationBusy', () => {
  it('is false while idle and while only reads are in flight', () => {
    const { result } = renderHook(() => useSpeleoOperationBusy());
    expect(result.current).toBe(false);
    const end = startCall('read');
    expect(result.current).toBe(false);
    expect(isSpeleoOperationBusy()).toBe(false);
    end();
  });

  it('is true while an exclusive call or a hand-off verb is running', () => {
    const { result } = renderHook(() => useSpeleoOperationBusy());
    const end = startCall('exclusive');
    expect(result.current).toBe(true);
    expect(isSpeleoOperationBusy()).toBe(true);
    end();
    expect(result.current).toBe(false);
    let token = 0;
    act(() => {
      token = useSpeleoStore.getState().beginBusy();
    });
    expect(result.current).toBe(true);
    act(() => useSpeleoStore.getState().endBusy(token));
    expect(result.current).toBe(false);
  });
});

describe('SpeleoDB workspace', () => {
  const actions = ['speleodb-workspace-upload', 'speleodb-workspace-editing', 'speleodb-workspace-latest', 'speleodb-workspace-close'];

  it('disables every lock, upload and reload control while an exclusive call runs', () => {
    render(<SpeleoWorkspace />);
    for (const id of actions) expect(screen.getByTestId(id)).toBeEnabled();
    const end = startCall('exclusive');
    for (const id of actions) expect(screen.getByTestId(id), id).toBeDisabled();
    // Navigation is never gated.
    expect(screen.getByTestId('speleodb-workspace-section-history')).toBeEnabled();
    end();
    for (const id of actions) expect(screen.getByTestId(id), id).toBeEnabled();
  });

  it('keeps the controls available during a status refresh', () => {
    render(<SpeleoWorkspace />);
    const end = startCall('read');
    for (const id of actions) expect(screen.getByTestId(id), id).toBeEnabled();
    end();
  });

  it('gates the conversion action of a local project too', () => {
    useSpeleoStore.setState({ link: null, projectLinkKind: 'local', remoteProjectId: null });
    render(<SpeleoWorkspace />);
    expect(screen.getByTestId('speleodb-workspace-convert')).toBeEnabled();
    const end = startCall('exclusive');
    expect(screen.getByTestId('speleodb-workspace-convert')).toBeDisabled();
    end();
  });
});

describe('SpeleoDB settings', () => {
  function renderSettings() {
    render(
      <SpeleoDbSettings
        config={defaultConfig}
        onChange={vi.fn()}
        changedFields={new Set()}
        onValidationError={vi.fn()}
        panelIdBase="settings-gating-test"
      />,
    );
  }

  it('locks Disconnect while an exclusive call runs; the server stays locked while connected', () => {
    renderSettings();
    // Connected: the server box is locked until Disconnect (#668).
    expect(screen.getByTestId('input-speleodb-endpoint')).toBeDisabled();
    expect(screen.getByTestId('speleodb-settings-logout')).toBeEnabled();
    const end = startCall('exclusive');
    expect(screen.getByTestId('speleodb-settings-logout')).toBeDisabled();
    end();
    expect(screen.getByTestId('speleodb-settings-logout')).toBeEnabled();
  });

  it('locks the sign-in form while an exclusive call runs', () => {
    useSpeleoStore.setState({
      session: null,
      pendingAuth: { kind: 'login', email: 'synthetic@example.com', password: 'synthetic', token: '', useToken: false },
    });
    renderSettings();
    expect(screen.getByTestId('speleodb-settings-try-login')).toBeEnabled();
    expect(screen.getByTestId('input-speleodb-endpoint')).toBeEnabled();
    const end = startCall('exclusive');
    expect(screen.getByTestId('speleodb-settings-try-login')).toBeDisabled();
    expect(screen.getByTestId('speleodb-settings-email')).toBeDisabled();
    expect(screen.getByTestId('input-speleodb-endpoint')).toBeDisabled();
    end();
    expect(screen.getByTestId('input-speleodb-endpoint')).toBeEnabled();
  });

  it('leaves the account controls alone during a read', () => {
    renderSettings();
    const end = startCall('read');
    expect(screen.getByTestId('speleodb-settings-logout')).toBeEnabled();
    end();
  });
});

describe('Open from SpeleoDB', () => {
  it('disables opening another project while an exclusive call runs', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_session') return Promise.resolve({ logged_in: true, user_email: 'synthetic@example.com', endpoint: 'https://speleodb.example' });
      if (cmd === 'speleodb_list_projects') {
        return Promise.resolve([{ id: 'p1', name: 'Synthetic Cave', permission: 'READ_AND_WRITE', type_tag: 'stalagmight', latest_commit_sha: null, lock_holder: null }]);
      }
      return Promise.resolve(null);
    });
    useSpeleoStore.setState({ setSession: vi.fn(), clearSession: vi.fn() });
    render(<OpenFromSpeleoDbDialog />);
    const open = await screen.findByTestId('open-from-speleodb-open-p1');
    expect(open).toBeEnabled();
    const end = startCall('exclusive');
    expect(open).toBeDisabled();
    end();
    expect(open).toBeEnabled();
  });
});
