/**
 * Component test (#218) - the Project Settings revision-history section. Fetches
 * speleodb_get_revisions on mount, marks the current HEAD (not restorable), and
 * gates the Restore button on holding the lock.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/ipc');
stubGetErrorMessage(fallbackOr('error'));

import { SpeleoRevisionHistory } from './SpeleoRevisionHistory';
import { useSpeleoStore, type SpeleoLinkState } from '../../../store/speleoStore';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import type { SpeleoRevision } from '../../../types/ipc.generated';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../../test/factories/ipc';
import { makeSpeleoLink } from '../../../test/factories/speleo';

const LINK: SpeleoLinkState = makeSpeleoLink();

const REVISIONS: SpeleoRevision[] = [
  {
    id: 'aaaaaaaa1111',
    parent_ids: ['bbbbbbbb2222'],
    author_email: 'me@x',
    authored_date: '2026-07-03T10:00:00+00:00',
    message: 'second',
  },
  {
    id: 'bbbbbbbb2222',
    parent_ids: [],
    author_email: 'you@x',
    authored_date: null,
    message: 'root',
  },
];

let openDialogMock: ReturnType<typeof vi.fn>;
let showToastMock: ReturnType<typeof vi.fn>;

const seed = (editState: SpeleoLinkState['editState']) => {
  useSpeleoStore.setState({ remoteProjectId: 'p1', link: { ...LINK, editState } });
};

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(REVISIONS);
  openDialogMock = vi.fn();
  showToastMock = vi.fn();
  useSpeleoStore.setState({
    remoteProjectId: null,
    link: null,
    refreshLinkStatus: vi.fn(async () => {}),
  });
  useProjectStore.setState({
    refreshProjectState: vi.fn(async () => {}),
    checkProjectIntegrity: vi.fn(async () => []),
  });
  useUIStore.setState({ openDialog: openDialogMock, showToast: showToastMock, setView: vi.fn() });
});

describe('SpeleoRevisionHistory (#218)', () => {
  it('fetches the chain, marks the current HEAD, and offers Restore on past revisions', async () => {
    seed('editing');
    render(<SpeleoRevisionHistory />);

    await waitFor(() => expect(screen.getByTestId('speleodb-revisions-list')).toBeInTheDocument());
    const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_get_revisions');
    expect(call?.[1]).toEqual({ projectId: 'p1' });

    // Newest-first: index 0 is the current server HEAD (marked, no Restore); the
    // older commit is restorable.
    expect(screen.getByTestId('speleodb-revision-current-aaaaaaaa1111')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-revision-restore-aaaaaaaa1111')).not.toBeInTheDocument();
    expect(screen.getByTestId('speleodb-revision-restore-bbbbbbbb2222')).toBeEnabled();
    // While editing, the "lock first" note is not shown.
    expect(screen.queryByTestId('speleodb-revisions-lock-hint')).not.toBeInTheDocument();
  });

  it('offers View (not Restore) and shows the lock-first note when not editing', async () => {
    seed('read-only');
    render(<SpeleoRevisionHistory />);
    await waitFor(() => expect(screen.getByTestId('speleodb-revisions-list')).toBeInTheDocument());
    // Read-only: the past revision gets an enabled View, not a Restore.
    expect(screen.queryByTestId('speleodb-revision-restore-bbbbbbbb2222')).not.toBeInTheDocument();
    expect(screen.getByTestId('speleodb-revision-view-bbbbbbbb2222')).toBeEnabled();
    expect(screen.getByTestId('speleodb-revisions-lock-hint')).toBeInTheDocument();
  });

  it('View checks the revision out read-only (restore=false)', async () => {
    seed('read-only');
    render(<SpeleoRevisionHistory />);
    await waitFor(() => expect(screen.getByTestId('speleodb-revisions-list')).toBeInTheDocument());

    await userEvent.click(screen.getByTestId('speleodb-revision-view-bbbbbbbb2222'));
    await waitFor(() =>
      expect(
        invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_restore_revision'),
      ).toBe(true),
    );
    const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_restore_revision');
    expect(call?.[1]).toMatchObject({ projectId: 'p1', hexsha: 'bbbbbbbb2222', restore: false });
  });

  it('opens the restore confirm with the chosen revision', async () => {
    seed('editing');
    render(<SpeleoRevisionHistory />);
    await waitFor(() => expect(screen.getByTestId('speleodb-revisions-list')).toBeInTheDocument());

    await userEvent.click(screen.getByTestId('speleodb-revision-restore-bbbbbbbb2222'));
    expect(openDialogMock).toHaveBeenCalledWith(
      'speleodb-restore',
      expect.objectContaining({ projectId: 'p1', hexsha: 'bbbbbbbb2222' }),
    );
  });

  it('renders nothing for a local project (no open remote)', () => {
    useSpeleoStore.setState({ remoteProjectId: null, link: null });
    const { container } = render(<SpeleoRevisionHistory />);
    expect(container).toBeEmptyDOMElement();
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });

  it('shows the empty state when the project has no revisions', async () => {
    invokeIpcMock.mockResolvedValue([]);
    seed('read-only');
    render(<SpeleoRevisionHistory />);
    await waitFor(() =>
      expect(screen.getByTestId('speleodb-revisions-empty')).toBeInTheDocument(),
    );
    expect(screen.queryByTestId('speleodb-revisions-list')).not.toBeInTheDocument();
  });

  it('shows the error state when the fetch fails', async () => {
    invokeIpcMock.mockRejectedValue({ message: 'offline' });
    seed('read-only');
    render(<SpeleoRevisionHistory />);
    await waitFor(() =>
      expect(screen.getByTestId('speleodb-revisions-error')).toBeInTheDocument(),
    );
    expect(screen.queryByTestId('speleodb-revisions-list')).not.toBeInTheDocument();
  });

  it('marks a merge commit (more than one parent)', async () => {
    invokeIpcMock.mockResolvedValue([
      { id: 'aaaaaaaa1111', parent_ids: ['b', 'c'], author_email: 'me@x', authored_date: null, message: 'merge' },
      { id: 'bbbbbbbb2222', parent_ids: [], author_email: 'you@x', authored_date: null, message: 'root' },
    ] satisfies SpeleoRevision[]);
    seed('read-only');
    render(<SpeleoRevisionHistory />);
    await waitFor(() => expect(screen.getByTestId('speleodb-revisions-list')).toBeInTheDocument());
    expect(screen.getByTestId('speleodb-revision-merge-aaaaaaaa1111')).toBeInTheDocument();
  });

  it('offers Revert alongside View for a read-only project with write permission', async () => {
    seed('read-only');
    render(<SpeleoRevisionHistory />);
    await waitFor(() => expect(screen.getByTestId('speleodb-revisions-list')).toBeInTheDocument());
    expect(screen.getByTestId('speleodb-revision-revert-bbbbbbbb2222')).toBeEnabled();
    // Never on the current HEAD - reverting to where you already are is a no-op.
    expect(screen.queryByTestId('speleodb-revision-revert-aaaaaaaa1111')).not.toBeInTheDocument();
  });

  it('opens the revert confirm with the chosen revision and a way back', async () => {
    seed('read-only');
    render(<SpeleoRevisionHistory />);
    await waitFor(() => expect(screen.getByTestId('speleodb-revisions-list')).toBeInTheDocument());

    await userEvent.click(screen.getByTestId('speleodb-revision-revert-bbbbbbbb2222'));
    expect(openDialogMock).toHaveBeenCalledWith(
      'speleodb-revert',
      expect.objectContaining({
        projectId: 'p1',
        hexsha: 'bbbbbbbb2222',
        returnTo: 'speleodb-project',
      }),
    );
  });

  it('hides Revert for a known view-only permission', async () => {
    // A revert takes the write lock and uploads, so a WEB_VIEWER/READ_ONLY grant can
    // only be refused by the server - don't offer it.
    useSpeleoStore.setState({
      remoteProjectId: 'p1',
      link: { ...LINK, editState: 'read-only', permission: 'READ_ONLY' },
    });
    render(<SpeleoRevisionHistory />);
    await waitFor(() => expect(screen.getByTestId('speleodb-revisions-list')).toBeInTheDocument());
    expect(screen.queryByTestId('speleodb-revision-revert-bbbbbbbb2222')).not.toBeInTheDocument();
  });

  it('offers Revert for an UNKNOWN (null) permission', async () => {
    // Offline, so the grant is not known - treat it as potentially editable and let
    // the backend gate decide, the same way the status button does.
    useSpeleoStore.setState({
      remoteProjectId: 'p1',
      link: { ...LINK, editState: 'read-only', permission: null },
    });
    render(<SpeleoRevisionHistory />);
    await waitFor(() => expect(screen.getByTestId('speleodb-revisions-list')).toBeInTheDocument());
    expect(screen.getByTestId('speleodb-revision-revert-bbbbbbbb2222')).toBeEnabled();
  });

  it('offers Revert - and NOT View - when un-pushed edits block the read-only peek', async () => {
    // That state used to show no buttons at all. Revert is the route out of it: it
    // takes the lock for the user, with the armed discard as the explicit consent.
    useSpeleoStore.setState({
      remoteProjectId: 'p1',
      link: { ...LINK, editState: 'read-only', hasUnpushedEdits: true },
    });
    render(<SpeleoRevisionHistory />);
    await waitFor(() => expect(screen.getByTestId('speleodb-revisions-list')).toBeInTheDocument());
    expect(screen.getByTestId('speleodb-revisions-unpushed-hint')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-revision-view-bbbbbbbb2222')).not.toBeInTheDocument();
    expect(screen.getByTestId('speleodb-revision-revert-bbbbbbbb2222')).toBeEnabled();
  });

  it('offers NEITHER View nor Restore nor Revert while force-editing (no lock), with a hint', async () => {
    // A force-editor holds unsaved, lock-less edits; swapping the tree would clobber
    // them, so the backend refuses both and the UI offers no action.
    useSpeleoStore.setState({
      remoteProjectId: 'p1',
      link: { ...LINK, editState: 'editing', forceEditing: true },
    });
    render(<SpeleoRevisionHistory />);
    await waitFor(() => expect(screen.getByTestId('speleodb-revisions-list')).toBeInTheDocument());
    expect(screen.queryByTestId('speleodb-revision-restore-bbbbbbbb2222')).not.toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-revision-view-bbbbbbbb2222')).not.toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-revision-revert-bbbbbbbb2222')).not.toBeInTheDocument();
    expect(screen.getByTestId('speleodb-revisions-forceedit-hint')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-revisions-lock-hint')).not.toBeInTheDocument();
  });
});

/**
 * [#576 W55 NF-W53-1] A View refused because the cached copy could not be read
 * (`state_unknown`) must say that, not repeat the un-pushed-changes line - which is
 * still right for a copy that was read and found dirty.
 */
describe('SpeleoRevisionHistory - a View refused over a copy that could not be read (#576 W55 NF-W53-1)', () => {
  const refusal = (stateUnknown: boolean) => ({
    message: 'could not be read',
    code: 'SPELEODB_UNPUSHED_LOCAL_EDITS',
    unpushed_local: {
      project_id: 'p1',
      local_path: '/cache/p1.stalx',
      last_edit_at: null,
      state_unknown: stateUnknown,
    },
  });

  const viewRefusedWith = async (error: unknown): Promise<string> => {
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'speleodb_restore_revision') throw error;
      return command === 'speleodb_get_revisions' ? REVISIONS : null;
    });
    seed('read-only');
    render(<SpeleoRevisionHistory />);
    await waitFor(() => expect(screen.getByTestId('speleodb-revisions-list')).toBeInTheDocument());

    await userEvent.click(screen.getByTestId('speleodb-revision-view-bbbbbbbb2222'));
    await waitFor(() => expect(showToastMock).toHaveBeenCalled());
    const [message, , severity] = showToastMock.mock.calls[0] as [string, string, string];
    expect(severity).toBe('error');
    return message;
  };

  it('says the copy could not be read instead of claiming un-pushed changes', async () => {
    const message = await viewRefusedWith(refusal(true));
    console.log('[W55] revision View toast (state_unknown):', message);
    expect(message).toContain('could not be read');
    expect(message).toContain('A save may have been interrupted, or another program may be using it.');
    expect(message).not.toContain('This copy has changes that are not on SpeleoDB');
  });

  it('keeps the un-pushed line for a copy that was read and found dirty', async () => {
    const message = await viewRefusedWith(refusal(false));
    expect(message).toContain('This copy has changes that are not on SpeleoDB');
    expect(message).not.toContain('could not be read');
  });

  /**
   * [#576 W59 Q-W53-2-D1] The View route has no discard step of its own (a refused View
   * replaces nothing and offers no "replace" button), so the decision's neutral wording
   * comes down to the toast and the row's buttons: neither may speak of "my/your
   * changes" for a copy nobody could read. The dirty line keeps its "your changes".
   */
  describe('neutral wording (#576 W59)', () => {
    const MY_CHANGES = /\b(my|your) changes\b/i;

    it('says nothing about "my changes" for a copy that could not be read', async () => {
      const message = await viewRefusedWith(refusal(true));
      const section = screen.getByTestId('speleodb-revisions').textContent ?? '';
      console.log('[W59] revision View toast (state_unknown):', message);
      expect(message).not.toMatch(MY_CHANGES);
      expect(section).not.toMatch(MY_CHANGES);
    });

    it('keeps "your changes" in the line for a copy that was read and found dirty', async () => {
      const message = await viewRefusedWith(refusal(false));
      expect(message).toMatch(MY_CHANGES);
    });
  });
});

/**
 * #576 RV4-FE-SP4 (decided RV2-R-15: Restore revision is refused in the read-only Plan and
 * Export workspaces). The hamburger's SpeleoDB menu reaches this tab outside Survey, where
 * Restore (lock held) and Revert (read-only) were offered with no mode check. Neither is
 * offered there now, and the hint says why; View, a read-only peek, stays.
 */
describe('SpeleoRevisionHistory in the read-only Plan/Export workspace (#576 RV4-FE-SP4)', () => {
  afterEach(() => {
    useUIStore.setState({ appMode: 'survey' });
  });

  it('offers no Restore while holding the lock, and says why', async () => {
    seed('editing');
    useUIStore.setState({ appMode: 'plan' });
    render(<SpeleoRevisionHistory />);
    await waitFor(() => expect(screen.getByTestId('speleodb-revisions-list')).toBeInTheDocument());

    console.log('[SP4] Plan, lock held: restore button present =', screen.queryByTestId('speleodb-revision-restore-bbbbbbbb2222') !== null);
    expect(screen.queryByTestId('speleodb-revision-restore-bbbbbbbb2222')).not.toBeInTheDocument();
    expect(screen.getByTestId('speleodb-revisions-workspace-hint')).toHaveTextContent(
      'Plan and Export are read-only - switch to Survey to edit the survey.',
    );
  });

  it('offers no Revert while read-only, but keeps the View peek', async () => {
    seed('read-only');
    useUIStore.setState({ appMode: 'export' });
    render(<SpeleoRevisionHistory />);
    await waitFor(() => expect(screen.getByTestId('speleodb-revisions-list')).toBeInTheDocument());

    console.log('[SP4] Export, read-only: revert button present =', screen.queryByTestId('speleodb-revision-revert-bbbbbbbb2222') !== null);
    expect(screen.queryByTestId('speleodb-revision-revert-bbbbbbbb2222')).not.toBeInTheDocument();
    expect(screen.getByTestId('speleodb-revision-view-bbbbbbbb2222')).toBeInTheDocument();
    expect(screen.getByTestId('speleodb-revisions-workspace-hint')).toBeInTheDocument();
  });
});
