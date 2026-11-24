/**
 * Demo item 1 (screenshot 28): two live sessions of the SAME account. SpeleoDB's
 * lock belongs to the account, so the second session's sign-in scan finds the lock
 * the first one is editing under, and the held-lock prompt offered a bare Release
 * "perhaps from a session that did not exit cleanly". Releasing it lets anyone else
 * lock the project and change it under the session still editing.
 *
 * The app cannot tell a live session from a crashed one, so the prompt now says
 * both are possible and what releasing does, offers the release only as a worded
 * "Release anyway" (a crashed session's lock must stay releasable), and makes Not
 * now the default. Where this computer can positively not have taken the lock (no
 * claim of this run, no copy, no recent file linked to the project), the row says
 * it is held by this account, not by this computer.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, screen } from '@testing-library/react';
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

const ELSEWHERE = 'cccccccc-3333-3333-3333-333333333333';
const COPY_HERE = 'dddddddd-4444-4444-4444-444444444444';
const CLAIMED = 'eeeeeeee-5555-5555-5555-555555555555';
const RECENT = 'ffffffff-6666-6666-6666-666666666666';
const UNREADABLE = '99999999-7777-7777-7777-777777777777';

/** Nothing on this computer: no copy, no claim of this run. */
const nothingHere = {
  has_local_copy: false,
  has_unpushed_edits: false,
  last_edit_at: null,
  is_open: false,
  state_unknown: false,
  lock_claimed_here: false,
};

const recentRow = (id: string) => ({
  name: 'Recent Cave',
  location: null,
  file_path: '/elsewhere/recent.stalx',
  modified_at: '2026-11-21T00:00:00Z',
  line_count: 1,
  total_length: 10,
  station_count: 2,
  gps_center: null,
  gps_bounds: null,
  is_remote: true,
  speleodb_project_id: id,
});

const wireIpc = (localStates: Record<string, unknown> | Error) => {
  invokeIpcMock.mockImplementation((command: string) => {
    if (command === 'speleodb_local_project_states') {
      return localStates instanceof Error ? Promise.reject(localStates) : Promise.resolve(localStates);
    }
    return Promise.resolve(null);
  });
};

const hold = (...ids: string[]) =>
  useSpeleoStore.setState({
    link: null,
    heldLocks: ids.map((projectId) => ({ projectId, projectName: `Cave ${projectId.slice(0, 4)}` })),
    heldLockPromptOpen: true,
    session: { loggedIn: true, userEmail: 'me@example.com', endpoint: null },
  });

beforeEach(() => {
  invokeIpcMock.mockReset();
  useProjectStore.setState({ project: null, isDirty: false, recentProjects: [] });
  useUIStore.setState({ activeDialog: null, dialogData: {} });
});

const row = (id: string) => screen.getByTestId(`speleodb-held-lock-${id}`);

describe('HeldLockResumeDialog: another live session of the same account (demo item 1)', () => {
  it('says the lock may be Stalagmight on another computer, and what releasing it does', async () => {
    wireIpc({ [ELSEWHERE]: nothingHere });
    hold(ELSEWHERE);
    render(<HeldLockResumeDialog />);

    const dialog = await screen.findByTestId('speleodb-held-lock-dialog');
    expect(dialog).toHaveTextContent('another computer with this account');
    expect(dialog).toHaveTextContent('change the project under it');
    // The old copy assumed a crash.
    expect(dialog).not.toHaveTextContent('perhaps from a session that did not exit cleanly');
  });

  it('offers the release only as "Release anyway", and it still hands the lock back', async () => {
    wireIpc({ [ELSEWHERE]: nothingHere });
    hold(ELSEWHERE);
    render(<HeldLockResumeDialog />);

    const release = await screen.findByTestId(`speleodb-held-lock-release-${ELSEWHERE}`);
    expect(release).toHaveTextContent('Release anyway');
    expect(release).toHaveAccessibleName(/anyway/);
    // A crashed session's lock must stay releasable: the action is worded, not hidden.
    await userEvent.click(release);
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_release_lock', { projectId: ELSEWHERE });
  });

  it('makes Not now the default: focus lands on it, never on Release anyway', async () => {
    // DialogWrapper places the initial focus in a frame after mount, skipping
    // anything without layout. jsdom gives nothing layout, so give every element a
    // box and run that frame by hand once the rows have loaded (in the app the
    // local states can land before or after it). Without a default the heuristic
    // falls to the first enabled button, which is the first row's Release.
    // Injection: drop `data-autofocus` from Not now and focus lands on Release anyway.
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => frames.push(cb));
    vi.stubGlobal('cancelAnimationFrame', () => {});
    const rects = vi
      .spyOn(HTMLElement.prototype, 'getClientRects')
      .mockReturnValue([{}] as unknown as DOMRectList);
    try {
      wireIpc({ [ELSEWHERE]: nothingHere });
      hold(ELSEWHERE);
      render(<HeldLockResumeDialog />);

      await screen.findByTestId(`speleodb-held-lock-release-${ELSEWHERE}`);
      act(() => frames.splice(0).forEach((frame) => frame(0)));
      const notNow = screen.getByTestId('speleodb-held-lock-keep');
      expect(document.activeElement).toBe(notNow);
      // Enter on the default leaves the lock alone.
      await userEvent.keyboard('{Enter}');
      expect(useSpeleoStore.getState().heldLockPromptOpen).toBe(false);
      expect(invokeIpcMock.mock.calls.some((c) => String(c[0]).startsWith('speleodb_release'))).toBe(
        false,
      );
    } finally {
      rects.mockRestore();
      vi.unstubAllGlobals();
    }
  });

  it('says a lock this computer never took is held by this account, not by this computer', async () => {
    wireIpc({ [ELSEWHERE]: nothingHere });
    hold(ELSEWHERE);
    render(<HeldLockResumeDialog />);

    expect(await screen.findByTestId(`speleodb-held-lock-elsewhere-${ELSEWHERE}`)).toHaveTextContent(
      'Held by this account, not by this computer',
    );
    expect(row(ELSEWHERE)).toHaveAttribute('data-lock-origin', 'elsewhere');
  });

  it('does not say so where this computer may have taken the lock', async () => {
    wireIpc({
      // A clean copy here: a session on this computer may have crashed over it.
      [COPY_HERE]: { ...nothingHere, has_local_copy: true },
      // This run still points at the lock (a kept lock, a release that failed).
      [CLAIMED]: { ...nothingHere, lock_claimed_here: true },
      // No cached copy, but a recent file linked to the project (a published copy
      // kept outside the cache).
      [RECENT]: nothingHere,
      // A copy that could not be read.
      [UNREADABLE]: { ...nothingHere, state_unknown: true },
    });
    useProjectStore.setState({ recentProjects: [recentRow(RECENT)] });
    hold(COPY_HERE, CLAIMED, RECENT, UNREADABLE);
    render(<HeldLockResumeDialog />);

    await screen.findByTestId(`speleodb-held-lock-release-${COPY_HERE}`);
    for (const id of [COPY_HERE, CLAIMED, RECENT, UNREADABLE]) {
      expect(row(id)).toHaveAttribute('data-lock-origin', 'unknown');
      expect(screen.queryByTestId(`speleodb-held-lock-elsewhere-${id}`)).not.toBeInTheDocument();
    }
  });

  it('does not say so when it could not look', async () => {
    wireIpc(new Error('Synthetic IPC failure'));
    hold(ELSEWHERE);
    render(<HeldLockResumeDialog />);

    await screen.findByTestId(`speleodb-held-lock-continue-${ELSEWHERE}`);
    expect(row(ELSEWHERE)).toHaveAttribute('data-lock-origin', 'unknown');
    expect(screen.queryByTestId(`speleodb-held-lock-elsewhere-${ELSEWHERE}`)).not.toBeInTheDocument();
  });
});
