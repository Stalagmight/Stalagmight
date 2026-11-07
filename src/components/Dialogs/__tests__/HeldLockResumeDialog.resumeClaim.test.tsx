/**
 * Component test (#576 RV2-R-13-RESTART): the held-lock prompt's Continue editing
 * claims the lock it resumes. Harness copied from HeldLockResumeDialog.localEdits.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/ipc');
stubGetErrorMessage(fallbackOr('error'));

import HeldLockResumeDialog from '../HeldLockResumeDialog';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../../test/factories/ipc';

const DIRTY = 'aaaaaaaa-1111-1111-1111-111111111111';
const CLEAN = 'bbbbbbbb-2222-2222-2222-222222222222';

const local = (opts: { unpushed: boolean; open?: boolean }) => ({
  has_local_copy: true,
  has_unpushed_edits: opts.unpushed,
  last_edit_at: opts.unpushed ? '2026-07-26T00:00:00Z' : null,
  is_open: opts.open ?? false,
  state_unknown: false,
});

/** Route the mock by command; `localStates` is the reply (or a thrown failure). */
const wireIpc = (localStates: Record<string, unknown> | Error) => {
  invokeIpcMock.mockImplementation((command: string) => {
    if (command === 'speleodb_local_project_states') {
      return localStates instanceof Error
        ? Promise.reject(localStates)
        : Promise.resolve(localStates);
    }
    if (command === 'speleodb_start_editing') return Promise.resolve({ kind: 'editing' });
    if (command === 'speleodb_link_status') return Promise.resolve(null);
    return Promise.resolve(null);
  });
};

const loadProjectMock = vi.fn(async () => ({ success: true }));

beforeEach(() => {
  invokeIpcMock.mockReset();
  loadProjectMock.mockClear();
  loadProjectMock.mockResolvedValue({ success: true });
  useSpeleoStore.setState({
    link: null,
    heldLocks: [
      { projectId: DIRTY, projectName: 'Dirty Cave' },
      { projectId: CLEAN, projectName: 'Clean Cave' },
    ],
    heldLockPromptOpen: true,
    session: { loggedIn: true, userEmail: 'me@example.com', endpoint: null, keychainBacked: true, storage: 'keychain', keychain: 'available' },
  });
  useProjectStore.setState({
    loadProject: loadProjectMock,
    // No project open and nothing unsaved unless a case says so - `continueEditing`
    // now reads BOTH live (the open path decides whether a load is even needed; the
    // dirty flag decides whether a load is allowed to close what is open).
    project: null,
    isDirty: false,
    recentProjects: [
      {
        name: 'Dirty Cave',
        location: null,
        file_path: '/cache/dirty.stalx',
        modified_at: '2026-07-26T00:00:00Z',
        section_count: 1,
        total_length: 10,
        station_count: 2,
        gps_center: null,
        gps_bounds: null,
        is_remote: true,
        speleodb_project_id: DIRTY,
      },
    ],
  });
  useUIStore.setState({ activeDialog: null, dialogData: {} });
});


/**
 * #576 RV2-R-13-RESTART (decided: "'Continue editing' at open records the lock claim
 * for this session"). Done releases only a lock this app session holds a claim on, so
 * a lock resumed here after a restart stayed held through Done. Continue editing now
 * asks the backend to claim the lock it resumes; plain Start editing does not.
 */
describe('HeldLockResumeDialog Continue editing claims the lock (#576 RV2-R-13-RESTART)', () => {
  it('starts editing with resumeHeldLock, so the backend records the claim', async () => {
    wireIpc({ [DIRTY]: local({ unpushed: true }), [CLEAN]: local({ unpushed: false }) });
    render(<HeldLockResumeDialog />);

    await userEvent.click(await screen.findByTestId(`speleodb-held-lock-continue-${DIRTY}`));

    await waitFor(() =>
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_start_editing')).toBe(true),
    );
    const start = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_start_editing');
    expect(start?.[1]).toEqual({ resumeHeldLock: true });
  });
});
