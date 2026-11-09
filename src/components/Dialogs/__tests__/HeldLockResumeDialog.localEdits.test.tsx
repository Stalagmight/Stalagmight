/**
 * Component test (#270): the held-lock prompt joining LOCAL state onto the
 * server-only lock scan.
 *
 * The scan (`list_projects`) knows only who holds which lock, so every row used to
 * offer the same bare "Release" - including rows whose lock sits on top of work that
 * never reached SpeleoDB. `speleodb_local_project_states` supplies the missing half;
 * a row with un-pushed edits offers "Continue editing" (open + keep the lock) and
 * does not offer Release at all, and the bulk release stops being a silent
 * work-discarder.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import HeldLockResumeDialog from '../HeldLockResumeDialog';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';

const DIRTY = 'aaaaaaaa-1111-1111-1111-111111111111';
const CLEAN = 'bbbbbbbb-2222-2222-2222-222222222222';

const local = (opts: { unpushed: boolean; open?: boolean }) => ({
  has_local_copy: true,
  has_unpushed_edits: opts.unpushed,
  last_edit_at: opts.unpushed ? '2026-07-26T00:00:00Z' : null,
  is_open: opts.open ?? false,
  state_unknown: false,
});

/**
 * A row the backend found but could NOT read. The listing fails open per row, so it
 * reports `has_unpushed_edits: false` - byte-identical to a clean row - and only
 * `state_unknown` tells them apart.
 */
const unreadable = () => ({
  has_local_copy: false,
  has_unpushed_edits: false,
  last_edit_at: null,
  is_open: false,
  state_unknown: true,
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
    session: { loggedIn: true, userEmail: 'me@example.com', endpoint: null, },
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

const row = (id: string) => screen.getByTestId(`speleodb-held-lock-${id}`);

describe('HeldLockResumeDialog local-edit join (#270)', () => {
  it('offers Continue editing (and no Release) for a project with local changes', async () => {
    wireIpc({ [DIRTY]: local({ unpushed: true }), [CLEAN]: local({ unpushed: false }) });
    render(<HeldLockResumeDialog />);

    await screen.findByTestId(`speleodb-held-lock-continue-${DIRTY}`);
    expect(row(DIRTY)).toHaveAttribute('data-local-edits', 'true');
    // Release is the one action that would strand the work - it must be gone.
    expect(screen.queryByTestId(`speleodb-held-lock-release-${DIRTY}`)).not.toBeInTheDocument();
  });

  it('keeps plain Release for a project with nothing outstanding', async () => {
    wireIpc({ [DIRTY]: local({ unpushed: true }), [CLEAN]: local({ unpushed: false }) });
    render(<HeldLockResumeDialog />);

    await screen.findByTestId(`speleodb-held-lock-release-${CLEAN}`);
    expect(row(CLEAN)).toHaveAttribute('data-local-edits', 'false');
    expect(screen.queryByTestId(`speleodb-held-lock-continue-${CLEAN}`)).not.toBeInTheDocument();
  });

  it('Continue editing opens the local copy and re-acquires, keeping the lock', async () => {
    wireIpc({ [DIRTY]: local({ unpushed: true }), [CLEAN]: local({ unpushed: false }) });
    render(<HeldLockResumeDialog />);

    await userEvent.click(await screen.findByTestId(`speleodb-held-lock-continue-${DIRTY}`));

    await waitFor(() => expect(loadProjectMock).toHaveBeenCalledWith('/cache/dirty.stalx'));
    await waitFor(() =>
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_start_editing')).toBe(true),
    );
    // No release of any kind: the lock is deliberately retained.
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_release_lock')).toBe(false);
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_release_all_locks')).toBe(false);
    await waitFor(() => expect(useSpeleoStore.getState().heldLockPromptOpen).toBe(false));
  });

  it('skips the reload when that project is already the open one', async () => {
    // Decided from the LIVE open path, not from the `is_open` field of a snapshot
    // taken when this prompt mounted and never revalidated since.
    useProjectStore.setState({ project: { file_path: '/cache/dirty.stalx' } as never });
    wireIpc({
      [DIRTY]: local({ unpushed: true, open: true }),
      [CLEAN]: local({ unpushed: false }),
    });
    render(<HeldLockResumeDialog />);

    await userEvent.click(await screen.findByTestId(`speleodb-held-lock-continue-${DIRTY}`));

    await waitFor(() =>
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_start_editing')).toBe(true),
    );
    expect(loadProjectMock).not.toHaveBeenCalled();
  });

  it('refuses to open over a dirty project instead of dropping its in-memory edits', async () => {
    // This prompt is raised on LOGIN as well as app-open, and login is reachable from
    // Settings with a dirty project open. `loadProject` for a DIFFERENT project closes
    // the current one backend-side, dropping unsaved edits - which is why
    // `useProject.handleOpenProject` routes through the unsaved-changes prompt. Here
    // there is no such route (the prompt is an app-wide overlay), so it must refuse.
    useProjectStore.setState({
      project: { file_path: '/other/open.stalx' } as never,
      isDirty: true,
    });
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    wireIpc({ [DIRTY]: local({ unpushed: true }), [CLEAN]: local({ unpushed: false }) });
    render(<HeldLockResumeDialog />);

    await userEvent.click(await screen.findByTestId(`speleodb-held-lock-continue-${DIRTY}`));

    await waitFor(() => expect(showToast).toHaveBeenCalled());
    expect(loadProjectMock).not.toHaveBeenCalled();
    // Nothing was acquired and nothing was discarded: the prompt stays up with every
    // lock still listed, so the user can save first and come back to it.
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_start_editing')).toBe(false);
    expect(useSpeleoStore.getState().heldLockPromptOpen).toBe(true);
    expect(useSpeleoStore.getState().heldLocks).toHaveLength(2);
  });

  it('refuses to open over a disk-clean project that still owes SpeleoDB work', async () => {
    // The sibling above covers disk-dirty. This is the case that made this a FIFTH
    // exit: the open project is saved to disk - so `isDirty` is false and the old
    // guard waved it through - while still holding un-pushed edits and our write
    // lock. Closing it backend-side hands neither back; it hides the local copy and
    // drops the lock. Every other way out of a project routes through
    // `needsSpeleoHandBackPrompt`, and this one has to as well.
    useProjectStore.setState({
      project: { file_path: '/other/open.stalx' } as never,
      isDirty: false,
    });
    useSpeleoStore.setState({
      link: { editState: 'editing', forceEditing: false, hasUnpushedEdits: true } as never,
    });
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    wireIpc({ [DIRTY]: local({ unpushed: true }), [CLEAN]: local({ unpushed: false }) });
    render(<HeldLockResumeDialog />);

    await userEvent.click(await screen.findByTestId(`speleodb-held-lock-continue-${DIRTY}`));

    await waitFor(() => expect(showToast).toHaveBeenCalled());
    expect(showToast.mock.calls[0]?.[1]).toBe('speleodb-continue-editing-unpushed');
    expect(loadProjectMock).not.toHaveBeenCalled();
    // Same shape as the dirty case: nothing acquired, nothing discarded, every lock
    // still listed so the user can hand the open project back and return here.
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_start_editing')).toBe(false);
    expect(useSpeleoStore.getState().heldLockPromptOpen).toBe(true);
    expect(useSpeleoStore.getState().heldLocks).toHaveLength(2);
  });

  it('refuses to open over a project whose only outstanding thing is our write lock', async () => {
    // The sibling above sets BOTH halves of `needsSpeleoHandBackPrompt` at once, so it
    // passes just as well against a guard that only looked at `hasUnpushedEdits`. This
    // is the OTHER disjunct on its own: everything is saved and everything is pushed,
    // and the single thing the open project still owes is the write lock we hold on it.
    // `loadProject` closes it backend-side, which drops that lock without handing it
    // back - the very outcome this prompt exists to resolve, done silently to a second
    // project. `holdsWriteLock` is why the shared predicate has two terms.
    useProjectStore.setState({
      project: { file_path: '/other/open.stalx' } as never,
      isDirty: false,
    });
    useSpeleoStore.setState({
      link: { editState: 'editing', forceEditing: false, hasUnpushedEdits: false } as never,
    });
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    wireIpc({ [DIRTY]: local({ unpushed: true }), [CLEAN]: local({ unpushed: false }) });
    render(<HeldLockResumeDialog />);

    await userEvent.click(await screen.findByTestId(`speleodb-held-lock-continue-${DIRTY}`));

    await waitFor(() => expect(showToast).toHaveBeenCalled());
    expect(showToast.mock.calls[0]?.[1]).toBe('speleodb-continue-editing-unpushed');
    expect(loadProjectMock).not.toHaveBeenCalled();
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_start_editing')).toBe(false);
    expect(useSpeleoStore.getState().heldLockPromptOpen).toBe(true);
    expect(useSpeleoStore.getState().heldLocks).toHaveLength(2);
  });

  // DO-NOT-REGRESS. The hand-back guard must stay INSIDE the same-project branch.
  // This prompt exists to rescue stranded locks, and the row that reaches
  // `continueEditing` is offered Continue INSTEAD of Release precisely because the
  // project has un-pushed edits - so when the lock being resumed is the one on screen,
  // `needsSpeleoHandBackPrompt` is true BY CONSTRUCTION (we hold that lock, and the
  // work is what withheld Release). Hoisting the guard above the `openPath !== path`
  // test therefore does not protect anything: it refuses every resume of the open
  // project, permanently, on the strength of the very facts that made the row
  // rescuable. Nothing is being replaced here - `loadProject` is not even called -
  // so there is nothing to hand back. Same exemption `openRemote` makes for
  // re-opening the project already on screen.
  it('still resumes the lock on the OPEN project, which by definition owes SpeleoDB work', async () => {
    useProjectStore.setState({ project: { file_path: '/cache/dirty.stalx' } as never });
    useSpeleoStore.setState({
      link: { editState: 'editing', forceEditing: false, hasUnpushedEdits: true } as never,
    });
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    wireIpc({
      [DIRTY]: local({ unpushed: true, open: true }),
      [CLEAN]: local({ unpushed: false }),
    });
    render(<HeldLockResumeDialog />);

    await userEvent.click(await screen.findByTestId(`speleodb-held-lock-continue-${DIRTY}`));

    await waitFor(() =>
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_start_editing')).toBe(true),
    );
    expect(loadProjectMock).not.toHaveBeenCalled();
    // Neither refusal fired: this is the resume, not a project switch.
    expect(
      showToast.mock.calls.some(
        (c) =>
          c[1] === 'speleodb-continue-editing-unpushed' || c[1] === 'speleodb-continue-editing-dirty',
      ),
    ).toBe(false);
    await waitFor(() => expect(useSpeleoStore.getState().heldLockPromptOpen).toBe(false));
  });

  // DO-NOT-REGRESS. The guard reads what the open project OWES, not merely that one is
  // open: a different project that is saved, pushed and holds no lock is safe to swap
  // out, and refusing it would break the ordinary resume this prompt is for.
  it('opens over a different project that owes nothing', async () => {
    useProjectStore.setState({
      project: { file_path: '/other/open.stalx' } as never,
      isDirty: false,
    });
    useSpeleoStore.setState({
      link: { editState: 'read-only', forceEditing: false, hasUnpushedEdits: false } as never,
    });
    wireIpc({ [DIRTY]: local({ unpushed: true }), [CLEAN]: local({ unpushed: false }) });
    render(<HeldLockResumeDialog />);

    await userEvent.click(await screen.findByTestId(`speleodb-held-lock-continue-${DIRTY}`));

    await waitFor(() => expect(loadProjectMock).toHaveBeenCalledWith('/cache/dirty.stalx'));
    await waitFor(() =>
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_start_editing')).toBe(true),
    );
  });

  it('offers no bulk release at all', async () => {
    // Removed deliberately, not overlooked. The server verb is all-or-nothing and
    // this prompt is exactly where locks sitting over un-pushed work surface, so a
    // single button freeing every lock is the one action most able to strand the
    // work the per-row rules exist to protect.
    wireIpc({ [DIRTY]: local({ unpushed: true }), [CLEAN]: local({ unpushed: false }) });
    render(<HeldLockResumeDialog />);
    await screen.findByTestId(`speleodb-held-lock-continue-${DIRTY}`);

    expect(screen.queryByTestId('speleodb-held-lock-release-all')).not.toBeInTheDocument();
    expect(invokeIpcMock).not.toHaveBeenCalledWith(
      'speleodb_release_all_locks',
      expect.anything(),
    );
    expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_release_all_locks');
  });

  it('withholds Release while the local-state call is still in flight', async () => {
    // The scan is raised at app-open and walks one cache dir per listed id, so this
    // window is real - and a Release landing in it acts on a project we have not yet
    // been told anything about. No row may offer the irreversible action yet.
    let settle: (v: Record<string, unknown>) => void = () => {};
    invokeIpcMock.mockImplementation((command: string) => {
      if (command === 'speleodb_local_project_states') {
        return new Promise<Record<string, unknown>>((resolve) => {
          settle = resolve;
        });
      }
      return Promise.resolve(null);
    });
    render(<HeldLockResumeDialog />);

    await screen.findByTestId(`speleodb-held-lock-checking-${DIRTY}`);
    expect(screen.queryByTestId(`speleodb-held-lock-release-${DIRTY}`)).not.toBeInTheDocument();
    expect(screen.queryByTestId(`speleodb-held-lock-release-${CLEAN}`)).not.toBeInTheDocument();

    // ...and it appears only once the clean verdict actually arrives.
    settle({ [DIRTY]: local({ unpushed: true }), [CLEAN]: local({ unpushed: false }) });
    await screen.findByTestId(`speleodb-held-lock-release-${CLEAN}`);
    expect(screen.queryByTestId(`speleodb-held-lock-release-${DIRTY}`)).not.toBeInTheDocument();
  });

  it('does not offer Release when the local-state call fails, and says it does not know', async () => {
    // A failure resets `states` to {}, which reads as "every project is clean" -
    // so the old fallback flipped even the dirty row to Release, deterministically,
    // exactly when the local half could not be consulted. Unknown must resolve to
    // the safe action, not the destructive one.
    wireIpc(new Error('backend down'));
    render(<HeldLockResumeDialog />);

    await screen.findByTestId('speleodb-held-lock-local-unknown');
    expect(screen.queryByTestId(`speleodb-held-lock-release-${DIRTY}`)).not.toBeInTheDocument();
    expect(screen.queryByTestId(`speleodb-held-lock-release-${CLEAN}`)).not.toBeInTheDocument();
    // Continue editing keeps the lock held, so nothing can be stranded behind it.
    expect(screen.getByTestId(`speleodb-held-lock-continue-${DIRTY}`)).toBeInTheDocument();
    expect(screen.getByTestId(`speleodb-held-lock-continue-${CLEAN}`)).toBeInTheDocument();
  });

  it('withholds Release for a copy it could not read, and says why', async () => {
    // The per-row twin of the whole-call failure above. An unreadable copy comes back
    // `has_unpushed_edits: false` because the listing fails open, so before
    // `state_unknown` existed this row was indistinguishable from clean and offered
    // the one irreversible button - over a file whose likely problem is a hot journal
    // left by the same crash that stranded the lock. The Rust guard fails CLOSED on
    // this input, so offering Release here also meant offering what the backend
    // would refuse.
    wireIpc({ [DIRTY]: unreadable(), [CLEAN]: local({ unpushed: false }) });
    render(<HeldLockResumeDialog />);

    // The clean row still resolves to Release: this must not become a blanket veto.
    await screen.findByTestId(`speleodb-held-lock-release-${CLEAN}`);
    expect(screen.queryByTestId(`speleodb-held-lock-release-${DIRTY}`)).not.toBeInTheDocument();
    expect(screen.getByTestId(`speleodb-held-lock-continue-${DIRTY}`)).toBeInTheDocument();
    // A withheld button with no explanation reads as a bug, so the row says why.
    expect(
      screen.getByTestId(`speleodb-held-lock-state-unknown-${DIRTY}`),
    ).toBeInTheDocument();
    expect(screen.getByTestId(`speleodb-held-lock-${DIRTY}`)).toHaveAttribute(
      'data-local-state-unknown',
      'true',
    );
    expect(screen.getByTestId(`speleodb-held-lock-${CLEAN}`)).toHaveAttribute(
      'data-local-state-unknown',
      'false',
    );
  });

  // The X, Escape and a backdrop click all route to DialogWrapper's single `onClose`,
  // which is the same handler as the Keep button - and Keep is `disabled={busy}`, so
  // ungated they did what that button refuses. It matters more here than in most
  // dialogs: Keep runs `clearHeldLocks()`, which drops the WHOLE list including rows
  // that were cleanly releasable, and nothing re-raises this prompt until the next
  // login scan. Asserted through the X, the one of the three that is unambiguously
  // reachable mid-verb (the row button that started it takes the focus with it when
  // it disables, so Escape has nowhere to land).
  it('ignores a dismissal while a row verb is in flight', async () => {
    let settleRelease: () => void = () => {};
    useSpeleoStore.setState({
      releaseLock: vi.fn(
        () =>
          new Promise<void>((resolve) => {
            settleRelease = resolve;
          }),
      ) as never,
    });
    wireIpc({ [DIRTY]: local({ unpushed: true }), [CLEAN]: local({ unpushed: false }) });
    render(<HeldLockResumeDialog />);

    await userEvent.click(await screen.findByTestId(`speleodb-held-lock-release-${CLEAN}`));
    // The Keep button says no; the dismissals must agree.
    await waitFor(() => expect(screen.getByTestId('speleodb-held-lock-keep')).toBeDisabled());

    await userEvent.click(screen.getByLabelText('Close dialog'));
    expect(useSpeleoStore.getState().heldLockPromptOpen).toBe(true);
    expect(useSpeleoStore.getState().heldLocks).toHaveLength(2);

    settleRelease();
  });

  it('keeps the prompt (and the other locks) when the re-acquire is refused', async () => {
    // A contended lock must not read as success: dismissing here also DISCARDS the
    // remaining locks, and the prompt cannot be re-raised until the next login scan.
    invokeIpcMock.mockImplementation((command: string) => {
      if (command === 'speleodb_local_project_states') {
        return Promise.resolve({ [DIRTY]: local({ unpushed: true }), [CLEAN]: local({ unpushed: false }) });
      }
      if (command === 'speleodb_start_editing') {
        return Promise.resolve({ kind: 'locked_by', email: 'someone@else.test' });
      }
      return Promise.resolve(null);
    });
    render(<HeldLockResumeDialog />);

    await userEvent.click(await screen.findByTestId(`speleodb-held-lock-continue-${DIRTY}`));

    await waitFor(() =>
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_start_editing')).toBe(true),
    );
    expect(useSpeleoStore.getState().heldLockPromptOpen).toBe(true);
    expect(useSpeleoStore.getState().heldLocks).toHaveLength(2);
  });
});
