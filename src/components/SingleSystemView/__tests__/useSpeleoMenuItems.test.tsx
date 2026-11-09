/**
 * Hook test for the SpeleoDB menu rows shared by the hamburger submenu and the
 * status button's right-click menu.
 *
 * Covers (#213) the force-edit gating - "Edit without the write lock" is behind the
 * `allow_force_edit` opt-in and only offered while read-only, and a force-editor
 * gets Reconnect rather than Unlock - plus the "open on the web" destination,
 * which must be disabled whenever no URL can be formed.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook } from '@testing-library/react';

vi.mock('../../../utils/ipc', () => ({
  invokeIpc: vi.fn(),
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import { useSpeleoMenuItems } from '../useSpeleoMenuItems';
import { useSpeleoStore, type SpeleoLinkState } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import type { AppMenuItem } from '../../common/AppMenu';

const LINK: SpeleoLinkState = {
  permission: 'READ_AND_WRITE',
  editState: 'read-only',
  lockHolder: null,
  lockedByOther: false,
  hasUnpushedEdits: false,
  unwound: false,
  forceEditing: false,
  forceEditAllowed: false,
  undoMode: 'full',
  remoteState: 'up_to_date',
  noAccess: false,
  serverError: null,
};

function items(o: Partial<SpeleoLinkState> = {}): AppMenuItem[] {
  useSpeleoStore.setState({ link: { ...LINK, ...o } });
  const { result } = renderHook(() => useSpeleoMenuItems());
  return result.current;
}

const ids = (o: Partial<SpeleoLinkState> = {}): string[] => items(o).map((it) => it.id);

beforeEach(() => {
  useSpeleoStore.setState({
    link: null,
    remoteProjectId: 'p1',
    session: { loggedIn: true, userEmail: 'me@x', endpoint: 'https://x/', },
  });
});

describe('useSpeleoMenuItems force-edit gating (#213)', () => {
  it('hides "Edit without the write lock" when the opt-in is off', () => {
    expect(ids({ forceEditAllowed: false, editState: 'read-only' })).not.toContain(
      'speleodb-menu-edit-without-lock',
    );
  });

  it('shows "Edit without the write lock" when opted-in and read-only', () => {
    expect(ids({ forceEditAllowed: true, editState: 'read-only' })).toContain(
      'speleodb-menu-edit-without-lock',
    );
  });

  it('hides "Edit without the write lock" once we are already editing (opted-in)', () => {
    expect(ids({ forceEditAllowed: true, editState: 'editing' })).not.toContain(
      'speleodb-menu-edit-without-lock',
    );
  });

  it('offers Reconnect (not Unlock) as the top action while force-editing', () => {
    const list = ids({ forceEditAllowed: true, editState: 'editing', forceEditing: true });
    expect(list).toContain('speleodb-menu-reconnect');
    expect(list).not.toContain('speleodb-menu-unlock');
    // A force-editor reconnects rather than force-editing again.
    expect(list).not.toContain('speleodb-menu-edit-without-lock');
  });
});

/**
 * The Push row needs the WRITE LOCK, not just an open edit gate. Gated on
 * `editState === 'editing'` alone it was ENABLED for a force-editor - who edits
 * without a lock (#213), so unpushed edits are the normal state there - and the
 * backend can only refuse it: "reconnect to acquire the write lock before pushing".
 */
describe('useSpeleoMenuItems push row', () => {
  const push = (o: Partial<SpeleoLinkState> = {}) =>
    items(o).find((it) => it.id === 'speleodb-menu-push');

  it('is enabled while holding the lock with edits to push', () => {
    expect(push({ editState: 'editing', hasUnpushedEdits: true })?.disabled).toBe(false);
  });

  it('is disabled while holding the lock with nothing to push', () => {
    expect(push({ editState: 'editing', hasUnpushedEdits: false })?.disabled).toBe(true);
  });

  it('is disabled for a force-editor even with unpushed edits (no lock to push under)', () => {
    expect(
      push({ editState: 'editing', forceEditing: true, hasUnpushedEdits: true })?.disabled,
    ).toBe(true);
  });

  it('is disabled while read-only', () => {
    expect(push({ editState: 'read-only', hasUnpushedEdits: true })?.disabled).toBe(true);
  });
});

describe('useSpeleoMenuItems destinations', () => {
  const openWeb = (o: Partial<SpeleoLinkState> = {}) =>
    items(o).find((it) => it.id === 'speleodb-menu-open-web');

  it('returns no rows at all for a local project', () => {
    useSpeleoStore.setState({ link: null });
    const { result } = renderHook(() => useSpeleoMenuItems());
    expect(result.current).toEqual([]);
  });

  it('enables "open on the web" with both an endpoint and a project id', () => {
    expect(openWeb()?.disabled).toBe(false);
  });

  it('disables "open on the web" when the session endpoint is missing', () => {
    // The URL comes from the SESSION endpoint, not the config, so a signed-out
    // shell has nothing to link to even with the project id in hand.
    useSpeleoStore.setState({ session: null });
    expect(openWeb()?.disabled).toBe(true);
  });

  it('disables "open on the web" when the remote project id is missing', () => {
    useSpeleoStore.setState({ remoteProjectId: null });
    expect(openWeb()?.disabled).toBe(true);
  });

  it('puts the destinations last, behind a separator', () => {
    const list = ids();
    expect(list.slice(-3)).toEqual([
      'speleodb-menu-sep-1',
      'speleodb-menu-open-web',
      'speleodb-menu-project-settings',
    ]);
  });
});

/**
 * [#303] The status button and this menu open the push DIALOG directly when the copy is
 * dirty, which walks straight past the guard inside `done()`. So an unwound copy has to be
 * branched here too, or the user meets "Push & release" before anything explains that
 * pushing it publishes a revert.
 */
describe('useSpeleoMenuItems - an unwound copy does not route to a push (#303)', () => {
  it('opens the unwound confirmation instead of the done dialog', () => {
    const openDialog = vi.fn();
    useUIStore.setState({ openDialog });
    const menu = items({ editState: 'editing', hasUnpushedEdits: true, unwound: true });

    menu.find((i) => i.id === 'speleodb-menu-unlock')?.onClick?.();

    expect(openDialog).toHaveBeenCalledWith('speleodb-unwound-push');
    expect(openDialog).not.toHaveBeenCalledWith('speleodb-done');
  });

  it('still routes an ordinary dirty copy to the done dialog', () => {
    const openDialog = vi.fn();
    useUIStore.setState({ openDialog });
    const menu = items({ editState: 'editing', hasUnpushedEdits: true, unwound: false });

    menu.find((i) => i.id === 'speleodb-menu-unlock')?.onClick?.();

    expect(openDialog).toHaveBeenCalledWith('speleodb-done');
  });
});
