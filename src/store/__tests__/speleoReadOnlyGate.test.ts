/**
 * #270: the SpeleoDB edit gate must FAIL CLOSED.
 *
 * `selectSpeleoReadOnly` used to read `link === null` as "editable", which
 * conflated a confirmed-local project with "we have not established what this
 * project is yet". Opening a SpeleoDB project from recents therefore showed every
 * edit affordance enabled until `speleodb_link_status` answered, letting the user
 * start actions the backend then silently refused (`AppState::guard_mutation`).
 *
 * These pin the inverted default: the unresolved state is read-only, only a
 * CONFIRMED-local project or a held write lock opens the gate, and the resolution
 * happens on both success and failure of the status call.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { useSpeleoStore, selectSpeleoReadOnly } from '../speleoStore';
import { useUIStore } from '../uiStore';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import type { SpeleoLinkStatus } from '../../types/ipc.generated';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));

const status = (o: Partial<SpeleoLinkStatus> = {}): SpeleoLinkStatus => ({
  is_remote: true,
  edit_enabled: false,
  has_unpushed_edits: false,
  unwound: false,
  permission: 'READ_AND_WRITE',
  lock_holder: null,
  remote_state: 'up_to_date',
  access_denied: false,
  server_error: null,
  // [#303] The hybrid undo stack's policy pair: Full, from the global default.
  undo_mode: 'full',

  project_id: null,
  ...o,
});

const readOnly = () => selectSpeleoReadOnly(useSpeleoStore.getState());

beforeEach(() => {
  invokeIpcMock.mockReset();
  useSpeleoStore.setState({
    session: null,
    link: null,
    projectLinkKind: 'unknown',
    remoteProjectId: null,
    divergence: null,
    banner: null,
  });
  // The map itself is in its one fully-editable state, so every verdict below is
  // attributable to the SpeleoDB term alone.
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
});

describe('selectSpeleoReadOnly - unresolved means read-only (#270)', () => {
  it('is read-only before any project has resolved (the default state)', () => {
    expect(useSpeleoStore.getState().projectLinkKind).toBe('unknown');
    expect(readOnly()).toBe(true);
  });

  it('flips to editable only once the project is CONFIRMED local', () => {
    expect(readOnly()).toBe(true);
    useSpeleoStore.getState().syncProjectLink(false);
    expect(useSpeleoStore.getState().projectLinkKind).toBe('local');
    expect(useSpeleoStore.getState().link).toBeNull();
    expect(readOnly()).toBe(false);
  });

  it('is read-only again after the project closes', () => {
    useSpeleoStore.getState().syncProjectLink(false);
    expect(readOnly()).toBe(false);
    useSpeleoStore.getState().clearLink();
    expect(useSpeleoStore.getState().projectLinkKind).toBe('unknown');
    expect(readOnly()).toBe(true);
  });

  it('drives the imperative twin identically (isEditingBlockedNow)', () => {
    expect(isEditingBlockedNow()).toBe(true);
    useSpeleoStore.getState().syncProjectLink(false);
    expect(isEditingBlockedNow()).toBe(false);
  });
});

describe('a remote project without the write lock (#270)', () => {
  it('is read-only BEFORE and AFTER the status resolves - no editable window', () => {
    // The instant the load commits, before any status round trip.
    useSpeleoStore.getState().syncProjectLink(true);
    expect(useSpeleoStore.getState().projectLinkKind).toBe('remote');
    expect(readOnly()).toBe(true);

    // ... and once the status confirms no lock.
    useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: false }));
    expect(readOnly()).toBe(true);
  });

  it('seeds a pessimistic link so the SpeleoDB tab still offers the way out', () => {
    useSpeleoStore.getState().syncProjectLink(true);
    const link = useSpeleoStore.getState().link;
    // A null link hides the SpeleoDB workspace's actions entirely (the ones that
    // allow manual recovery), so a remote project must never be left without one.
    expect(link).not.toBeNull();
    expect(link?.editState).toBe('read-only');
    // The button derives its 'unlocked' kind (bare mark -> Start editing) from
    // exactly these: no access denial, no server error, no other holder.
    expect(link?.noAccess).toBe(false);
    expect(link?.serverError).toBeNull();
    expect(link?.lockedByOther).toBe(false);
    // Nothing has been asked of the server yet - say so rather than claiming
    // up-to-date.
    expect(link?.remoteState).toBe('unknown');
    expect(link?.permission).toBeNull();
  });

  it('does not clobber a link the browser already seeded for THIS project', () => {
    useSpeleoStore
      .getState()
      .setLinkOnOpen({ permission: 'READ_AND_WRITE', forPath: '/cache/a.stalx' });
    expect(useSpeleoStore.getState().projectLinkKind).toBe('remote');
    useSpeleoStore.getState().syncProjectLink(true, '/cache/a.stalx');
    expect(useSpeleoStore.getState().link?.permission).toBe('READ_AND_WRITE');
    expect(readOnly()).toBe(true);
  });

  it('DOES replace a link left over from a different project', () => {
    // The leak: opening remote project B while A's link was still live rendered B
    // with A's permission, A's lock holder and A's dirty bit. File > Open never
    // clears the link, and `refreshLinkStatus` swallows its errors, so offline the
    // previous project's state simply stayed on screen.
    useSpeleoStore
      .getState()
      .setLinkOnOpen({ permission: 'READ_AND_WRITE', forPath: '/cache/a.stalx' });
    useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
    expect(useSpeleoStore.getState().link?.editState).toBe('editing');

    useSpeleoStore.getState().syncProjectLink(true, '/cache/b.stalx');

    expect(useSpeleoStore.getState().link?.permission).toBeNull();
    expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    expect(useSpeleoStore.getState().link?.remoteState).toBe('unknown');
    expect(useSpeleoStore.getState().remoteProjectId).toBeNull();
    expect(readOnly()).toBe(true);
  });

  it('keeps the link across a reload of the SAME file (fast-forward / revision swap)', () => {
    useSpeleoStore
      .getState()
      .setLinkOnOpen({ permission: 'READ_AND_WRITE', forPath: '/cache/a.stalx' });
    useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
    useSpeleoStore.getState().syncProjectLink(true, '/cache/a.stalx');
    expect(useSpeleoStore.getState().link?.editState).toBe('editing');
    expect(readOnly()).toBe(false);
  });

  it('becomes editable once the status confirms the write lock is held', () => {
    useSpeleoStore.getState().syncProjectLink(true);
    expect(readOnly()).toBe(true);
    useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
    expect(useSpeleoStore.getState().link?.editState).toBe('editing');
    expect(readOnly()).toBe(false);
  });
});

describe('a failed or never-answered status call (#270)', () => {
  it('leaves a remote project READ-ONLY, with the link (and its way out) intact', async () => {
    useSpeleoStore.getState().syncProjectLink(true);
    invokeIpcMock.mockRejectedValueOnce(new Error('offline'));

    await useSpeleoStore.getState().refreshLinkStatus();

    // Decided behavior: a failure resolves to the SAFE state, not to editable.
    // The project stays exactly as the synchronous open left it - read-only,
    // known-remote, with a link so the SpeleoDB tab can still offer
    // "Start editing" or a manual refresh.
    expect(useSpeleoStore.getState().projectLinkKind).toBe('remote');
    expect(useSpeleoStore.getState().link).not.toBeNull();
    expect(readOnly()).toBe(true);
  });

  it('leaves a local project EDITABLE - a failure never strands it read-only', async () => {
    useSpeleoStore.getState().syncProjectLink(false);
    invokeIpcMock.mockRejectedValueOnce(new Error('offline'));

    await useSpeleoStore.getState().refreshLinkStatus();

    expect(useSpeleoStore.getState().projectLinkKind).toBe('local');
    expect(readOnly()).toBe(false);
  });
});

describe('switching between projects (#270)', () => {
  it('drops a previous remote project link when a LOCAL one opens (no read-only flash)', () => {
    useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true, project_id: 'p1' }));
    expect(readOnly()).toBe(false);
    useSpeleoStore.setState({ divergence: 'start', banner: { kind: 'no_anchor' } });

    useSpeleoStore.getState().syncProjectLink(false);

    // Synchronously, not one status round trip later: no window in which the
    // local project wears the previous project's read-only shell.
    expect(useSpeleoStore.getState().link).toBeNull();
    expect(useSpeleoStore.getState().remoteProjectId).toBeNull();
    expect(useSpeleoStore.getState().divergence).toBeNull();
    expect(useSpeleoStore.getState().banner).toBeNull();
    expect(readOnly()).toBe(false);
  });

  it('a status reply resolves the tri-state both ways', () => {
    useSpeleoStore.getState().seedFromStatus(status());
    expect(useSpeleoStore.getState().projectLinkKind).toBe('remote');
    useSpeleoStore.getState().seedFromStatus(status({ is_remote: false }));
    expect(useSpeleoStore.getState().projectLinkKind).toBe('local');
  });
});
