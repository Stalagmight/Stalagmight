/**
 * "Resume editing" on both read-only map pills: the SpeleoDB pill retakes the
 * write lock through the start-editing flow; the Closures Computed View pill goes
 * back to the Raw Data View. Each shows only where it can apply.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const ipc = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: ipc, getErrorMessage: String }));

import { MapTopLeftOverlay } from './MapTopLeftOverlay';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoReadOnlyNotice } from '../../utils/autoAcquireSpeleoLock';

const LINK: SpeleoLinkState = {
  permission: 'READ_AND_WRITE', editState: 'read-only', lockHolder: null,
  lockedByOther: false, hasUnpushedEdits: false, unwound: false,
  undoMode: 'full', remoteState: 'up_to_date', noAccess: false, serverError: null,
};
const SESSION = { loggedIn: true, userEmail: 'me@example.com', endpoint: 'https://synthetic.example' };
const base = { focused: false, onFitOrExitFocus: vi.fn(), showSpeleoReadOnly: false, showClosureReadOnly: false, showClosureComputing: false };

beforeEach(() => {
  ipc.mockReset();
  ipc.mockResolvedValue({ kind: 'editing' });
  useProjectStore.setState({ project: { file_path: '/synthetic.stalx', name: 'Synthetic', zero_datums: [] } as never });
  useSpeleoStore.setState({ link: LINK, session: SESSION, busy: false, remoteProjectId: 'synthetic', sessionGeneration: 0 });
  useSpeleoReadOnlyNotice.setState({ notice: null });
  useUIStore.setState({ geometryMode: 'closureAdjusted', appMode: 'survey', toasts: [] });
});

describe('SpeleoDB read-only pill: Resume editing', () => {
  it('takes the write lock through the start-editing flow', async () => {
    render(<MapTopLeftOverlay {...base} showSpeleoReadOnly />);
    const resume = screen.getByTestId('speleodb-readonly-resume');
    expect(resume).toHaveTextContent('Resume editing');
    expect(screen.getByTestId('speleodb-readonly-banner')).toContainElement(resume);
    await userEvent.click(resume);
    expect(ipc).toHaveBeenCalledWith('speleodb_start_editing');
    await waitFor(() => expect(useSpeleoStore.getState().link?.editState).toBe('editing'));
  });

  it('reports someone else editing through the existing read-only notice', async () => {
    ipc.mockResolvedValue({ kind: 'locked_by', email: 'editor@example.com' });
    render(<MapTopLeftOverlay {...base} showSpeleoReadOnly />);
    await userEvent.click(screen.getByTestId('speleodb-readonly-resume'));
    await waitFor(() => expect(useSpeleoReadOnlyNotice.getState().notice?.message).toContain('editor@example.com'));
    expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
  });

  it.each([
    ['signed out', () => useSpeleoStore.setState({ session: null })],
    ['read-only permission', () => useSpeleoStore.setState({ link: { ...LINK, permission: 'READ_ONLY' } })],
    ['web-viewer permission', () => useSpeleoStore.setState({ link: { ...LINK, permission: 'WEB_VIEWER' } })],
    ['no project access', () => useSpeleoStore.setState({ link: { ...LINK, noAccess: true } })],
  ])('is hidden when it cannot apply: %s', (_label, seed) => {
    seed();
    render(<MapTopLeftOverlay {...base} showSpeleoReadOnly />);
    expect(screen.getByTestId('speleodb-readonly-banner')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-readonly-resume')).not.toBeInTheDocument();
  });

  it('is disabled while another SpeleoDB action is in flight', () => {
    useSpeleoStore.setState({ busy: true });
    render(<MapTopLeftOverlay {...base} showSpeleoReadOnly />);
    expect(screen.getByTestId('speleodb-readonly-resume')).toBeDisabled();
  });
});

describe('SpeleoDB read-only pill: why it cannot be edited', () => {
  it('signed out, says it is a read-only copy and offers Connect, which opens SpeleoDB settings', async () => {
    useSpeleoStore.setState({ session: { loggedIn: false, userEmail: null, endpoint: null } });
    render(<MapTopLeftOverlay {...base} showSpeleoReadOnly />);
    expect(screen.getByTestId('speleodb-readonly-reason')).toHaveTextContent(/^Read-only copy - connect to SpeleoDB to edit$/);
    const login = screen.getByTestId('speleodb-readonly-login');
    expect(login).toHaveTextContent(/^Connect$/);
    expect(login).toHaveAttribute('title', 'Open the SpeleoDB settings to connect');
    expect(screen.getByTestId('speleodb-readonly-banner')).toContainElement(login);
    await userEvent.click(login);
    expect(useUIStore.getState().activeDialog).toBe('settings');
    expect(useUIStore.getState().dialogData).toMatchObject({ initialCategory: 'speleodb' });
    expect(ipc).not.toHaveBeenCalledWith('speleodb_start_editing');
  });

  it('signed in without write access, says so and offers neither action', () => {
    useSpeleoStore.setState({ link: { ...LINK, permission: 'READ_ONLY' } });
    render(<MapTopLeftOverlay {...base} showSpeleoReadOnly />);
    expect(screen.getByTestId('speleodb-readonly-reason'))
      .toHaveTextContent(/^Read-only - your SpeleoDB access does not allow editing$/);
    expect(screen.queryByTestId('speleodb-readonly-login')).not.toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-readonly-resume')).not.toBeInTheDocument();
  });

  it('signed in with write access, keeps the plain badge and Resume editing', () => {
    render(<MapTopLeftOverlay {...base} showSpeleoReadOnly />);
    expect(screen.getByTestId('speleodb-readonly-reason')).toHaveTextContent(/^SpeleoDB · read-only$/);
    expect(screen.queryByTestId('speleodb-readonly-login')).not.toBeInTheDocument();
  });
});

describe('Closures Computed View pill: Resume editing', () => {
  it('switches back to the Raw Data View', async () => {
    render(<MapTopLeftOverlay {...base} showClosureReadOnly />);
    const resume = screen.getByTestId('geometry-readonly-resume');
    expect(screen.getByTestId('geometry-readonly-banner')).toContainElement(resume);
    await userEvent.click(resume);
    expect(useUIStore.getState().geometryMode).toBe('measured');
    expect(ipc).toHaveBeenCalledWith('save_display_setting', { key: 'geometryMode', value: 'measured' }, expect.anything());
  });

  it('is offered while the solve is still computing in Survey', () => {
    render(<MapTopLeftOverlay {...base} showClosureReadOnly showClosureComputing />);
    expect(screen.getByTestId('geometry-readonly-resume')).toBeInTheDocument();
  });

  it('is absent where the computed view is forced (Plan/Export: computing chip only)', () => {
    render(<MapTopLeftOverlay {...base} showClosureComputing />);
    expect(screen.queryByTestId('geometry-readonly-resume')).not.toBeInTheDocument();
  });
});
