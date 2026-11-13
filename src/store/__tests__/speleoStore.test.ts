import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  useSpeleoStore,
  PUSH_FAILURE_DIALOGS,
  isUnwoundNotEdited,
  speleoProjectSessionIssuedFor,
  speleoProjectSessionStillCurrent,
} from '../speleoStore';
import { useProjectStore } from '../projectStore';
import type { SpeleoLinkStatus } from '../../types/ipc.generated';

// The sign-out guard (uploads / discards / hands back locks before a sign-out) has
// its own suite; here it only has to say "go ahead", or "cancelled" where a case
// asks it to.
const guardSignOutMock = vi.hoisted(() => vi.fn(async (_kind: string) => true));
vi.mock('../../utils/speleoSignOut', () => ({ guardSpeleoSignOut: guardSignOutMock }));

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  // The store reports a failed picker retry through this, so the mock has to carry
  // it or the error path throws inside the very catch that exists to report.
  getErrorMessage: (e: unknown, fallback?: string) =>
    (e as { message?: string })?.message ?? fallback ?? 'error',
}));

/** A logged-in session so `scanLocksOnOpen` skips the `refreshSession` prelude. */
const loggedInAs = (email: string) =>
  useSpeleoStore.setState({
    session: { loggedIn: true, userEmail: email, endpoint: 'https://x/', },
  });

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

const reset = () => {
  invokeIpcMock.mockReset();
  useSpeleoStore.setState({
    session: null,
    link: null,
    divergence: null,
    banner: null,
    remoteLocks: {},
    remoteProjects: {},
    heldLocks: null,
    heldLockPromptOpen: false,
    heldLockPromptPending: true,
    remoteProjectId: null,
    sessionGeneration: 0,
  });
};

describe('speleoStore', () => {
  beforeEach(reset);

  describe('open-project request identity', () => {
    it('rejects a reply after a different remote project opens', () => {
      useSpeleoStore.setState({ remoteProjectId: 'p1' });
      const issuedFor = speleoProjectSessionIssuedFor('p1');

      useSpeleoStore.setState({ remoteProjectId: 'p2' });

      expect(speleoProjectSessionStillCurrent(issuedFor)).toBe(false);
    });

    it('rejects a reply after a new login session starts', () => {
      useSpeleoStore.setState({ remoteProjectId: 'p1' });
      const issuedFor = speleoProjectSessionIssuedFor('p1');

      useSpeleoStore.getState().setSessionFromLogin({
        user_email: 'u@x',
        endpoint: 'https://x/',
      });

      expect(speleoProjectSessionStillCurrent(issuedFor)).toBe(false);
    });
  });

  describe('session', () => {
    it('setSession maps the backend status', () => {
      useSpeleoStore.getState().setSession({
        logged_in: true,
        user_email: 'u@x',
        endpoint: 'https://x/',
        });
      expect(useSpeleoStore.getState().session).toEqual({
        loggedIn: true,
        userEmail: 'u@x',
        endpoint: 'https://x/',
        // Both new fields cross intact: "durable?" and "where?" are different
        // questions, and Settings needs the second one to say anything useful.
        });
    });

    it('does not let a delayed cold-session read overwrite a newer login', async () => {
      let resolveStatus!: (value: unknown) => void;
      invokeIpcMock.mockReturnValue(new Promise((resolve) => { resolveStatus = resolve; }));
      const pending = useSpeleoStore.getState().refreshSession();
      useSpeleoStore.getState().setSessionFromLogin({ user_email: 'new@x', endpoint: 'https://new.example' });
      resolveStatus({ logged_in: true, user_email: 'old@x', endpoint: 'https://old.example' });
      await pending;
      expect(useSpeleoStore.getState().session?.userEmail).toBe('new@x');
    });

    it('does not resurrect a session after logout during a cold-session read', async () => {
      let resolveStatus!: (value: unknown) => void;
      invokeIpcMock.mockReturnValue(new Promise((resolve) => { resolveStatus = resolve; }));
      const pending = useSpeleoStore.getState().refreshSession();
      useSpeleoStore.getState().clearSession();
      resolveStatus({ logged_in: true, user_email: 'old@x', endpoint: 'https://old.example' });
      await pending;
      expect(useSpeleoStore.getState().session).toBeNull();
    });

    it('clearSession degrades an active edit to read-only', () => {
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
      useSpeleoStore
        .getState()
        .setSessionFromLogin({
          user_email: 'u@x',
          endpoint: 'https://x/',
        });
      useSpeleoStore.getState().clearSession();
      const s = useSpeleoStore.getState();
      expect(s.session).toBeNull();
      expect(s.link?.editState).toBe('read-only');
    });

    it('clearSession fully resets stale link fields, keeping the permission tier (fix 4)', () => {
      // Mid-edit with a held lock, un-pushed edits, a raised banner and divergence:
      // a logout must clear ALL of it (not just editState) so the button can't render
      // a self-contradictory state for a session that no longer exists.
      useSpeleoStore
        .getState()
        .seedFromStatus(
          status({ edit_enabled: true, has_unpushed_edits: true, lock_holder: 'me@x' }),
        );
      useSpeleoStore.setState({ divergence: 'push', banner: { kind: 'push_unconfirmed' } });
      useSpeleoStore.getState().clearSession();
      const s = useSpeleoStore.getState();
      expect(s.link?.editState).toBe('read-only');
      expect(s.link?.lockHolder).toBeNull();
      expect(s.link?.lockedByOther).toBe(false);
      // NOT reset: `hasUnpushedEdits` is a fact about the local `.stalx` - edits
      // that never reached the server - and losing the token does not upload them.
      // Clearing it claimed "nothing outstanding" at the exact moment the work
      // became most stranded, which disarmed the quit guard and hid the Toolbar's
      // unpushed-edits warning. The next authenticated refresh re-seeds it.
      expect(s.link?.hasUnpushedEdits).toBe(true);
      expect(s.link?.remoteState).toBe('unknown');
      expect(s.link?.noAccess).toBe(false);
      expect(s.link?.serverError).toBeNull();
      expect(s.link?.permission).toBe('READ_AND_WRITE');
      expect(s.divergence).toBeNull();
      expect(s.banner).toBeNull();
    });

    it.each([false, true])('refreshes project references after logout (failure: %s)', async (fails) => {
      const recent = vi.spyOn(useProjectStore.getState(), 'loadRecentProjects').mockResolvedValue();
      const config = vi.spyOn(useProjectStore.getState(), 'loadConfig').mockResolvedValue();
      try {
        useSpeleoStore.getState().setPendingAuth({ kind: 'logout' });
        if (fails) invokeIpcMock.mockRejectedValueOnce(new Error('cleanup failed'));
        else invokeIpcMock.mockResolvedValueOnce(null);
        const result = await useSpeleoStore.getState().applyPendingAuth(null);
        expect(result.ok).toBe(!fails);
        expect(recent).toHaveBeenCalledOnce();
        expect(config).toHaveBeenCalledOnce();
      } finally {
        recent.mockRestore();
        config.mockRestore();
      }
    });

    it('a sign-out the guard cancels never reaches the backend logout', async () => {
      useSpeleoStore.getState().setSessionFromLogin({ user_email: 'u@x', endpoint: 'https://x/' });
      useSpeleoStore.getState().setPendingAuth({ kind: 'logout' });
      invokeIpcMock.mockReset();
      guardSignOutMock.mockResolvedValueOnce(false);
      expect(await useSpeleoStore.getState().applyPendingAuth('https://x/')).toEqual({ ok: false, cancelled: true });
      expect(guardSignOutMock).toHaveBeenCalledWith('signOut');
      expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_logout');
      expect(useSpeleoStore.getState().session?.userEmail).toBe('u@x');
    });

    it('logging in as SOMEONE ELSE signs the previous account out through the guard first', async () => {
      useSpeleoStore.getState().setSessionFromLogin({ user_email: 'u@x', endpoint: 'https://x/' });
      useSpeleoStore.getState().setPendingAuth({ kind: 'login', email: 'other@x', password: 'pw', token: '', useToken: false } as never);
      guardSignOutMock.mockResolvedValueOnce(false);
      expect(await useSpeleoStore.getState().applyPendingAuth('https://x/')).toEqual({ ok: false, cancelled: true });
      expect(guardSignOutMock).toHaveBeenCalledWith('switch');
      expect(invokeIpcMock.mock.calls.map((c) => c[0])).not.toContain('speleodb_login');
    });

    it('applyPendingAuth clears the session even when the logout command rejects', async () => {
      // #270: `speleodb_logout` rejects only to WARN that the credential could not be
      // removed from this machine - the backend session is gone either way. The store
      // must mirror that (and still report the warning), never leave the UI claiming a
      // session the backend dropped.
      useSpeleoStore
        .getState()
        .setSessionFromLogin({
          user_email: 'u@x',
          endpoint: 'https://x/',
        });
      useSpeleoStore.getState().setPendingAuth({ kind: 'logout' });
      invokeIpcMock.mockRejectedValue({
        message: 'Logged out, but the stored SpeleoDB credential could not be removed',
        action: 'Fatal',
        status: null,
      });

      const result = await useSpeleoStore.getState().applyPendingAuth('https://x/');
      expect(result.ok).toBe(false);
      expect(useSpeleoStore.getState().session).toBeNull();
      expect(useSpeleoStore.getState().pendingAuth).toBeNull();
    });
  });

  describe('seedFromStatus', () => {
    it('clears the link for a local (non-remote) project', () => {
      useSpeleoStore.getState().setLinkOnOpen({ permission: 'ADMIN' });
      useSpeleoStore.getState().seedFromStatus(status({ is_remote: false }));
      expect(useSpeleoStore.getState().link).toBeNull();
    });

    it('maps the edit gate to read-only / editing', () => {
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: false }));
      expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
      expect(useSpeleoStore.getState().link?.editState).toBe('editing');
    });

    it('keeps the last-known permission when the network field is null (offline)', () => {
      useSpeleoStore.getState().seedFromStatus(status({ permission: 'READ_AND_WRITE' }));
      useSpeleoStore.getState().seedFromStatus(status({ permission: null }));
      expect(useSpeleoStore.getState().link?.permission).toBe('READ_AND_WRITE');
    });

    it('carries lock holder and dirty bit through', () => {
      useSpeleoStore
        .getState()
        .seedFromStatus(status({ lock_holder: 'a@b', has_unpushed_edits: true }));
      const link = useSpeleoStore.getState().link;
      expect(link?.lockHolder).toBe('a@b');
      expect(link?.hasUnpushedEdits).toBe(true);
    });

    it('read-only + a holder ⟹ lockedByOther; editing ⟹ not', () => {
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: false, lock_holder: 'a@b' }));
      expect(useSpeleoStore.getState().link?.lockedByOther).toBe(true);
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true, lock_holder: 'me@x' }));
      expect(useSpeleoStore.getState().link?.lockedByOther).toBe(false);
    });

    it('does not call our own lock another user when editing is not enabled yet', () => {
      useSpeleoStore.setState({ session: { loggedIn: true, userEmail: 'me@x', endpoint: 'https://example.com' } });
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: false, lock_holder: 'ME@X' }));
      expect(useSpeleoStore.getState().link?.lockedByOther).toBe(false);
      expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    });

    it('clears a prior lockedByOther once a later refresh resolves no holder (fix 7)', () => {
      // Someone else held it, then released: the next refresh reports no holder, and the
      // flag must drop (no "preserve prior" carry keeping the button stuck orange).
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: false, lock_holder: 'a@b' }));
      expect(useSpeleoStore.getState().link?.lockedByOther).toBe(true);
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: false, lock_holder: null }));
      expect(useSpeleoStore.getState().link?.lockedByOther).toBe(false);
    });

    it('carries the access_denied flag through as noAccess', () => {
      useSpeleoStore.getState().seedFromStatus(status({ permission: null, access_denied: true }));
      expect(useSpeleoStore.getState().link?.noAccess).toBe(true);
    });

    it('surfaces an unexpected server_error, and clears it on the next clean refresh (fix 12)', () => {
      const err = { message: 'No Project matches the given query.', action: 'Fatal' as const, status: 404, transport: null, unpushed_local: null, dump_candidates: null, dump_candidate_kind: null, newer_schema: null };
      useSpeleoStore.getState().seedFromStatus(status({ server_error: err }));
      expect(useSpeleoStore.getState().link?.serverError).toEqual(err);
      useSpeleoStore.getState().seedFromStatus(status({ server_error: null }));
      expect(useSpeleoStore.getState().link?.serverError).toBeNull();
    });

  });

  describe('refreshLinkStatus staleness guard', () => {
    it('drops a reply whose link changed under it, so a push cannot be un-cleared', async () => {
      // The self-conflict race, frontend half. The backend reads the link row and
      // THEN makes two network round trips, and nothing holds a command-level
      // lock, so a push lands inside that window. The reply therefore carries the
      // PRE-push snapshot (`has_unpushed_edits: true`); applying it would
      // resurrect a dirty bit the push had just cleared, re-arming the quit guard
      // and re-painting the un-pushed warning over work that IS on the server.
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));

      let release!: (s: SpeleoLinkStatus) => void;
      invokeIpcMock.mockImplementation(
        () =>
          new Promise<SpeleoLinkStatus>((resolve) => {
            release = resolve;
          }),
      );
      const inFlight = useSpeleoStore.getState().refreshLinkStatus();

      // The push lands while that refresh is still waiting on the network.
      useSpeleoStore.getState().markDirty();
      useSpeleoStore.getState().applyPush({ kind: 'pushed' });
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);

      // Now the stale reply arrives, still describing the world before the push.
      release(status({ edit_enabled: true, has_unpushed_edits: true }));
      await inFlight;

      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);
    });

    it('applies a reply when nothing mutated the link while it was in flight', async () => {
      // The guard must not be so strict that ordinary refreshing stops working.
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
      invokeIpcMock.mockResolvedValue(status({ edit_enabled: true, has_unpushed_edits: true }));
      await useSpeleoStore.getState().refreshLinkStatus();
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
    });

    it('keeps the FRESHER of two overlapping refreshes instead of dropping it', async () => {
      // `seedFromStatus` allocates a new link object every time, so a plain
      // identity guard treated a rival refresh's seed as a local mutation - and the
      // reply it discarded was the LATER one, holding newer truth.
      //
      // The path that matters: `handleRelease` releases the lock and awaits its own
      // refresh. An overlapping refresh landing in that window reseeded `link`, the
      // release's own reply was thrown away, and the toolbar kept the green pen
      // over a lock that no longer existed - until another status refresh.
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));

      const pending: Array<(s: SpeleoLinkStatus) => void> = [];
      invokeIpcMock.mockImplementation(
        () => new Promise<SpeleoLinkStatus>((resolve) => pending.push(resolve)),
      );

      const refresh = useSpeleoStore.getState().refreshLinkStatus();
      const afterRelease = useSpeleoStore.getState().refreshLinkStatus();
      expect(pending).toHaveLength(2);

      // The overlapping refresh answers first, still describing the held lock.
      pending[0]!(status({ edit_enabled: true }));
      await refresh;
      // Then the release's own refresh answers: the lock is gone.
      pending[1]!(status({ edit_enabled: false }));
      await afterRelease;

      expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    });

    it('lets an older reply lose to a newer one that already landed', async () => {
      // Issue order, not arrival order. Without it the release-then-refresh ordering
      // is fixed but the refresh-then-release one regresses.
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));

      const pending: Array<(s: SpeleoLinkStatus) => void> = [];
      invokeIpcMock.mockImplementation(
        () => new Promise<SpeleoLinkStatus>((resolve) => pending.push(resolve)),
      );

      const older = useSpeleoStore.getState().refreshLinkStatus();
      const newer = useSpeleoStore.getState().refreshLinkStatus();

      pending[1]!(status({ edit_enabled: false }));
      await newer;
      pending[0]!(status({ edit_enabled: true }));
      await older;

      expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    });
  });

  describe('setLinkOnOpen', () => {
    it('seeds a read-only link with the summary permission', () => {
      useSpeleoStore.getState().setLinkOnOpen({ permission: 'READ_ONLY' });
      expect(useSpeleoStore.getState().link).toEqual({
        permission: 'READ_ONLY',
        editState: 'read-only',
        lockHolder: null,
        lockedByOther: false,
        hasUnpushedEdits: false,
        unwound: false,

        undoMode: 'full',
        remoteState: 'up_to_date',
        noAccess: false,
        serverError: null,
      });
    });
  });

  describe('applyStartEditing', () => {
    beforeEach(() => useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: false })));

    it('editing → editing', () => {
      useSpeleoStore.getState().applyStartEditing({ kind: 'editing' });
      expect(useSpeleoStore.getState().link?.editState).toBe('editing');
    });

    it('fast_forwarded → editing (optimistic; reload re-seeds)', () => {
      useSpeleoStore.getState().applyStartEditing({ kind: 'fast_forwarded', local_path: '/x' });
      expect(useSpeleoStore.getState().link?.editState).toBe('editing');
    });

    it('locked_by → read-only + lock holder + lockedByOther', () => {
      useSpeleoStore.getState().applyStartEditing({ kind: 'locked_by', email: 'a@b' });
      const s = useSpeleoStore.getState();
      expect(s.link?.editState).toBe('read-only');
      expect(s.link?.lockHolder).toBe('a@b');
      expect(s.link?.lockedByOther).toBe(true);
    });

    it('locked_by with a null email still marks lockedByOther (not free/unlocked)', () => {
      useSpeleoStore.getState().applyStartEditing({ kind: 'locked_by', email: null });
      expect(useSpeleoStore.getState().link?.lockedByOther).toBe(true);
    });

    it('divergence_needed → divergence origin start', () => {
      useSpeleoStore.getState().applyStartEditing({ kind: 'divergence_needed' });
      expect(useSpeleoStore.getState().divergence).toBe('start');
      // Stays read-only (not enabled on divergence).
      expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    });

  });

  describe('applyUpdate', () => {
    beforeEach(() => useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: false })));

    it('divergence_needed → divergence origin update', () => {
      useSpeleoStore.getState().applyUpdate({ kind: 'divergence_needed' });
      expect(useSpeleoStore.getState().divergence).toBe('update');
    });

    it('up_to_date clears a stale remote_ahead indicator', () => {
      useSpeleoStore.getState().seedFromStatus(status({ remote_state: 'remote_ahead' }));
      expect(useSpeleoStore.getState().link?.remoteState).toBe('remote_ahead');
      useSpeleoStore.getState().applyUpdate({ kind: 'up_to_date' });
      expect(useSpeleoStore.getState().link?.remoteState).toBe('up_to_date');
      expect(useSpeleoStore.getState().divergence).toBeNull();
    });
  });

  describe('applyDone', () => {
    beforeEach(() =>
      useSpeleoStore
        .getState()
        .seedFromStatus(
          status({ edit_enabled: true, has_unpushed_edits: true, lock_holder: 'me@x' }),
        ),
    );

    it('released → read-only, dirty + lock holder cleared, divergence cleared', () => {
      useSpeleoStore.getState().applyDone({ kind: 'released' });
      const s = useSpeleoStore.getState();
      expect(s.link?.editState).toBe('read-only');
      expect(s.link?.hasUnpushedEdits).toBe(false);
      // Regression: a released lock must not still read as held (button stayed green).
      expect(s.link?.lockHolder).toBeNull();
      expect(s.link?.lockedByOther).toBe(false);
      expect(s.divergence).toBeNull();
    });

    it('released clears a lingering push-unconfirmed banner', () => {
      useSpeleoStore.setState({ banner: { kind: 'push_unconfirmed' } });
      useSpeleoStore.getState().applyDone({ kind: 'released' });
      expect(useSpeleoStore.getState().banner).toBeNull();
    });

    it('push_unconfirmed → raises the banner, stays editing', () => {
      useSpeleoStore.getState().applyDone({ kind: 'push_unconfirmed' });
      const s = useSpeleoStore.getState();
      expect(s.banner).toEqual({ kind: 'push_unconfirmed' });
      expect(s.link?.editState).toBe('editing');
    });

    it('divergence_needed → divergence origin done, stays editing', () => {
      useSpeleoStore.getState().applyDone({ kind: 'divergence_needed' });
      const s = useSpeleoStore.getState();
      expect(s.divergence).toBe('done');
      expect(s.link?.editState).toBe('editing');
    });
  });

  describe('applyPush', () => {
    beforeEach(() =>
      useSpeleoStore
        .getState()
        .seedFromStatus(
          status({ edit_enabled: true, has_unpushed_edits: true, lock_holder: 'me@x' }),
        ),
    );

    it('pushed → keeps the lock, clears dirty + banner, resets remote state', () => {
      useSpeleoStore.setState({ banner: { kind: 'push_unconfirmed' } });
      useSpeleoStore.getState().applyPush({ kind: 'pushed' });
      const s = useSpeleoStore.getState();
      // Push behaves like push&release WITHOUT releasing the lock - still editing.
      expect(s.link?.editState).toBe('editing');
      expect(s.link?.hasUnpushedEdits).toBe(false);
      expect(s.link?.remoteState).toBe('up_to_date');
      expect(s.banner).toBeNull();
    });

    it('divergence_needed → divergence origin push, stays editing', () => {
      useSpeleoStore.getState().applyPush({ kind: 'divergence_needed' });
      const s = useSpeleoStore.getState();
      expect(s.divergence).toBe('push');
      expect(s.link?.editState).toBe('editing');
    });

    it('push_unconfirmed → raises the banner, stays editing', () => {
      useSpeleoStore.getState().applyPush({ kind: 'push_unconfirmed' });
      const s = useSpeleoStore.getState();
      expect(s.banner).toEqual({ kind: 'push_unconfirmed' });
      expect(s.link?.editState).toBe('editing');
    });
  });

  describe('markDirty', () => {
    it('flips the dirty bit on for an editing link', () => {
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
      useSpeleoStore.getState().markDirty();
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
    });

    it('is a no-op with no link', () => {
      useSpeleoStore.getState().markDirty();
      expect(useSpeleoStore.getState().link).toBeNull();
    });

    // [#303] The bug this pair exists to prevent: `markDirty` short-circuits on an
    // already-dirty link, and an unwound copy is ALWAYS already dirty, so mirroring a
    // forward edit through it left `unwound` standing until the next refresh - and
    // `isUnwoundNotEdited` kept routing the user to a discard of work they just made.
    it('markDirty leaves unwound standing on an already-dirty link', () => {
      useSpeleoStore
        .getState()
        .seedFromStatus(status({ edit_enabled: true, has_unpushed_edits: true, unwound: true }));
      useSpeleoStore.getState().markDirty();
      expect(useSpeleoStore.getState().link?.unwound).toBe(true);
      expect(isUnwoundNotEdited(useSpeleoStore.getState().link)).toBe(true);
    });

    it('markForwardEdit clears unwound even when already dirty', () => {
      useSpeleoStore
        .getState()
        .seedFromStatus(status({ edit_enabled: true, has_unpushed_edits: true, unwound: true }));
      useSpeleoStore.getState().markForwardEdit();
      const link = useSpeleoStore.getState().link;
      expect(link?.hasUnpushedEdits).toBe(true);
      expect(link?.unwound).toBe(false);
      expect(isUnwoundNotEdited(link)).toBe(false);
    });

    it('markForwardEdit dirties a clean link and no-ops when nothing changes', () => {
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
      useSpeleoStore.getState().markForwardEdit();
      const after = useSpeleoStore.getState().link;
      expect(after?.hasUnpushedEdits).toBe(true);
      expect(after?.unwound).toBe(false);
      // Identity is preserved on a true no-op: refreshLinkStatus uses it to detect a
      // concurrent mutation, so a gratuitous new object would discard live replies.
      useSpeleoStore.getState().markForwardEdit();
      expect(useSpeleoStore.getState().link).toBe(after);
    });

    it('markForwardEdit is a no-op with no link', () => {
      useSpeleoStore.getState().markForwardEdit();
      expect(useSpeleoStore.getState().link).toBeNull();
    });
  });

  describe('clearLink', () => {
    it('drops link, divergence and banner', () => {
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
      useSpeleoStore.setState({ divergence: 'update', banner: { kind: 'push_unconfirmed' } });
      useSpeleoStore.getState().clearLink();
      const s = useSpeleoStore.getState();
      expect(s.link).toBeNull();
      expect(s.divergence).toBeNull();
      expect(s.banner).toBeNull();
    });
  });

  /**
   * [#270] `busy` is one shared boolean with three writers - the verb's own
   * release, `clearLink` (close/switch) and `restoreLink` (a rolled-back remote
   * open) - so releases are ownership-checked. See `beginBusy` for why a stale
   * clear is a data-safety problem and not just a stuck spinner.
   */
  describe('busy ownership', () => {
    it('raises and clears the flag for the claim that owns it', () => {
      const token = useSpeleoStore.getState().beginBusy();
      expect(useSpeleoStore.getState().busy).toBe(true);
      useSpeleoStore.getState().endBusy(token);
      expect(useSpeleoStore.getState().busy).toBe(false);
    });

    it('ignores a release from a claim that has been superseded', () => {
      const stale = useSpeleoStore.getState().beginBusy();
      const current = useSpeleoStore.getState().beginBusy();
      useSpeleoStore.getState().endBusy(stale);
      expect(useSpeleoStore.getState().busy).toBe(true);
      useSpeleoStore.getState().endBusy(current);
      expect(useSpeleoStore.getState().busy).toBe(false);
    });

    it.each([
      ['clearLink', () => useSpeleoStore.getState().clearLink()],
      ['restoreLink', () => useSpeleoStore.getState().restoreLink(useSpeleoStore.getState().snapshotLink())],
    ])('%s disowns the in-flight claim, so its late release is a no-op', (_name, reset) => {
      const abandoned = useSpeleoStore.getState().beginBusy();
      reset();
      expect(useSpeleoStore.getState().busy).toBe(false);

      // A NEWER verb (on a newer project) claims the flag ...
      useSpeleoStore.getState().beginBusy();
      // ... and the abandoned one finally lands. It must not clear this.
      useSpeleoStore.getState().endBusy(abandoned);
      expect(useSpeleoStore.getState().busy).toBe(true);
    });
  });

  describe('notifyReverifyDivergence (#202)', () => {
    it('drops an active edit to read-only + conflict and raises the divergence prompt', () => {
      // Mid-edit (lock held), then the lazy 20-min re-verify finds a lost lock /
      // moved tip: the shell must go read-only + conflict and open the prompt.
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
      expect(useSpeleoStore.getState().link?.editState).toBe('editing');
      useSpeleoStore.getState().notifyReverifyDivergence();
      const s = useSpeleoStore.getState();
      expect(s.divergence).toBe('reverify');
      expect(s.link?.editState).toBe('read-only');
      expect(s.link?.remoteState).toBe('conflict');
    });

    it('is a safe no-op on link state when no project is linked', () => {
      useSpeleoStore.getState().notifyReverifyDivergence();
      const s = useSpeleoStore.getState();
      expect(s.divergence).toBe('reverify');
      expect(s.link).toBeNull();
    });
  });

  describe('#200 banners', () => {
    it('a status refresh never raises a banner of its own', () => {
      useSpeleoStore.getState().seedFromStatus(status());
      expect(useSpeleoStore.getState().banner).toBeNull();
    });

    it.each([
      { kind: 'push_unconfirmed' as const },
      { kind: 'no_anchor' as const },
    ])('a status refresh keeps the $kind banner', (banner) => {
      useSpeleoStore.setState({ banner });
      useSpeleoStore.getState().seedFromStatus(status());
      expect(useSpeleoStore.getState().banner).toEqual(banner);
    });

    it('raiseNoAnchorBanner only claims a free banner slot', () => {
      useSpeleoStore.getState().raiseNoAnchorBanner();
      expect(useSpeleoStore.getState().banner).toEqual({ kind: 'no_anchor' });
      // With a transient banner already up, it must not overwrite it.
      useSpeleoStore.setState({ banner: { kind: 'push_unconfirmed' } });
      useSpeleoStore.getState().raiseNoAnchorBanner();
      expect(useSpeleoStore.getState().banner).toEqual({ kind: 'push_unconfirmed' });
    });

    it('the no_anchor notice survives the refreshes that follow it', () => {
      // The retired rendering-disabled banner used to claim this slot from
      // no_anchor on every refresh; nothing may mask the notice now.
      useSpeleoStore.getState().seedFromStatus(status());
      useSpeleoStore.getState().raiseNoAnchorBanner();
      useSpeleoStore.getState().seedFromStatus(status());
      expect(useSpeleoStore.getState().banner).toEqual({ kind: 'no_anchor' });
    });


  });

  // #201 refinements: the resume/release prompt fires only on login + app-open (not
  // every recents visit), and individual locks can be released one at a time.
  describe('held-lock scan + release (#201)', () => {
    it('the first app-open scan is prompt-worthy: it opens the prompt when a lock is held', async () => {
      loggedInAs('me@x');
      invokeIpcMock.mockResolvedValue([{ id: 'p1', name: 'Held', lock_holder: 'me@x' }]);
      await useSpeleoStore.getState().scanLocksOnOpen();
      const s = useSpeleoStore.getState();
      expect(s.heldLocks).toEqual([{ projectId: 'p1', projectName: 'Held' }]);
      expect(s.remoteLocks).toEqual({ p1: 'me@x' });
      // The SpeleoDB project name is captured for the recents "SpeleoDB: {name}" label.
      expect(s.remoteProjects).toEqual({ p1: 'Held' });
      expect(s.heldLockPromptOpen).toBe(true);
      // Consumed, so a later routine scan won't retro-prompt.
      expect(s.heldLockPromptPending).toBe(false);
    });

    it('does not prompt to recover the project opened while the startup scan was pending', async () => {
      loggedInAs('me@x');
      useProjectStore.setState({ project: null });
      let finish!: (value: unknown) => void;
      invokeIpcMock.mockReturnValue(new Promise(resolve => { finish = resolve; }));
      const scan = useSpeleoStore.getState().scanLocksOnOpen();
      useProjectStore.setState({ project: { file_path: '/current.stalx' } as never });
      useSpeleoStore.setState({ remoteProjectId: 'p1', heldLockPromptOpen: true });
      finish([{ id: 'p1', name: 'Current', lock_holder: 'me@x' }]);
      await scan;
      expect(useSpeleoStore.getState().heldLocks).toBeNull();
      expect(useSpeleoStore.getState().heldLockPromptOpen).toBe(false);
      expect(useSpeleoStore.getState().remoteLocks).toEqual({ p1: 'me@x' });
      useProjectStore.setState({ project: null });
    });

    it('defers other held projects until returning home while retaining the current project badge', async () => {
      loggedInAs('me@x');
      useProjectStore.setState({ project: { file_path: '/current.stalx' } as never });
      useSpeleoStore.setState({ remoteProjectId: 'p1' });
      invokeIpcMock.mockResolvedValue([
        { id: 'p1', name: 'Current', lock_holder: 'me@x' },
        { id: 'p2', name: 'Stranded', lock_holder: 'me@x' },
      ]);
      await useSpeleoStore.getState().scanLocksOnOpen();
      expect(useSpeleoStore.getState().heldLocks).toEqual([{ projectId: 'p2', projectName: 'Stranded' }]);
      expect(useSpeleoStore.getState().heldLockPromptOpen).toBe(false);
      expect(useSpeleoStore.getState().heldLockPromptPending).toBe(true);
      expect(useSpeleoStore.getState().remoteLocks).toEqual({ p1: 'me@x', p2: 'me@x' });
      useProjectStore.setState({ project: null });
      await useSpeleoStore.getState().scanLocksOnOpen();
      expect(useSpeleoStore.getState().heldLockPromptOpen).toBe(true);
    });

    it('defers recovery while the open project id is still resolving', async () => {
      loggedInAs('me@x');
      useProjectStore.setState({ project: { file_path: '/current.stalx' } as never });
      useSpeleoStore.setState({ remoteProjectId: null });
      invokeIpcMock.mockResolvedValue([{ id: 'p1', name: 'Current', lock_holder: 'me@x' }]);
      await useSpeleoStore.getState().scanLocksOnOpen();
      expect(useSpeleoStore.getState().heldLockPromptOpen).toBe(false);
      expect(useSpeleoStore.getState().heldLockPromptPending).toBe(true);
      useProjectStore.setState({ project: null });
    });

    it('a routine badge refresh keeps the badges but does NOT re-open the prompt', async () => {
      loggedInAs('me@x');
      useSpeleoStore.setState({ heldLockPromptPending: false, heldLockPromptOpen: false });
      invokeIpcMock.mockResolvedValue([{ id: 'p1', name: 'Held', lock_holder: 'me@x' }]);
      await useSpeleoStore.getState().scanLocksOnOpen();
      const s = useSpeleoStore.getState();
      expect(s.heldLocks).toEqual([{ projectId: 'p1', projectName: 'Held' }]); // badges still fresh
      expect(s.heldLockPromptOpen).toBe(false); // but no new prompt
    });

    it('an explicit prompt scan (login) re-opens the prompt even after the pending flag is gone', async () => {
      loggedInAs('me@x');
      useSpeleoStore.setState({ heldLockPromptPending: false, heldLockPromptOpen: false });
      invokeIpcMock.mockResolvedValue([{ id: 'p1', name: 'Held', lock_holder: 'me@x' }]);
      await useSpeleoStore.getState().scanLocksOnOpen({ prompt: true });
      expect(useSpeleoStore.getState().heldLockPromptOpen).toBe(true);
    });

    it('a scan that finds no lock held by us never opens the prompt (case-insensitive match)', async () => {
      loggedInAs('Me@X');
      invokeIpcMock.mockResolvedValue([
        { id: 'p2', name: 'Other', lock_holder: 'someone@x' },
        { id: 'p3', name: 'Free', lock_holder: null },
      ]);
      await useSpeleoStore.getState().scanLocksOnOpen({ prompt: true });
      const s = useSpeleoStore.getState();
      expect(s.heldLocks).toBeNull();
      expect(s.heldLockPromptOpen).toBe(false);
      expect(s.remoteLocks).toEqual({ p2: 'someone@x', p3: null });
    });

    it('a logged-out scan clears the badges and consumes the app-open pending without prompting', async () => {
      useSpeleoStore.setState({
        session: { loggedIn: false, userEmail: null, endpoint: null, },
        remoteLocks: { p1: 'x' },
        remoteProjects: { p1: 'Stale' },
      });
      await useSpeleoStore.getState().scanLocksOnOpen();
      const s = useSpeleoStore.getState();
      expect(s.remoteLocks).toEqual({});
      expect(s.remoteProjects).toEqual({});
      expect(s.heldLocks).toBeNull();
      expect(s.heldLockPromptPending).toBe(false);
      expect(s.heldLockPromptOpen).toBe(false);
      expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_list_projects');
    });

    it('a logged-out scan also forces an already-open prompt overlay shut', async () => {
      // The prompt was open under the prior session; logging out must not leave it
      // lingering over a logged-out shell (not just consume the pending flag).
      useSpeleoStore.setState({
        session: { loggedIn: false, userEmail: null, endpoint: null, },
        heldLocks: [{ projectId: 'p1', projectName: 'A' }],
        heldLockPromptOpen: true,
      });
      await useSpeleoStore.getState().scanLocksOnOpen();
      const s = useSpeleoStore.getState();
      expect(s.heldLockPromptOpen).toBe(false);
      expect(s.heldLocks).toBeNull();
    });

    it('a failed scan keeps prior badges, never prompts, and does not consume the pending flag', async () => {
      loggedInAs('me@x');
      useSpeleoStore.setState({
        remoteLocks: { p1: 'me@x' },
        remoteProjects: { p1: 'Prev' },
        heldLockPromptPending: true,
      });
      invokeIpcMock.mockRejectedValue(new Error('offline'));
      await useSpeleoStore.getState().scanLocksOnOpen({ prompt: true });
      const s = useSpeleoStore.getState();
      // Best-effort: prior badges survive, no prompt, and pending is NOT burned - so
      // a later successful scan can still raise the first real prompt.
      expect(s.remoteLocks).toEqual({ p1: 'me@x' });
      expect(s.remoteProjects).toEqual({ p1: 'Prev' });
      expect(s.heldLockPromptOpen).toBe(false);
      expect(s.heldLockPromptPending).toBe(true);
    });

    it('a scan whose session ends mid-flight does not repaint the ended session badges', async () => {
      loggedInAs('me@x');
      // The list_projects GET resolves, but by then the user has logged out. The
      // post-await staleness guard must drop the stale result rather than repaint.
      invokeIpcMock.mockImplementation(async (cmd: string) => {
        if (cmd === 'speleodb_list_projects') {
          useSpeleoStore.setState({
            session: { loggedIn: false, userEmail: null, endpoint: null, },
          });
          return [{ id: 'p1', name: 'Held', lock_holder: 'me@x' }];
        }
        return null;
      });
      await useSpeleoStore.getState().scanLocksOnOpen({ prompt: true });
      const s = useSpeleoStore.getState();
      expect(s.remoteLocks).toEqual({});
      expect(s.heldLocks).toBeNull();
      expect(s.heldLockPromptOpen).toBe(false);
    });

    it('ignores a prior server scan after the same email signs in to another server', async () => {
      loggedInAs('me@x');
      invokeIpcMock.mockImplementation(async (cmd: string) => {
        if (cmd === 'speleodb_list_projects') {
          useSpeleoStore.getState().clearSession();
          useSpeleoStore.getState().setSessionFromLogin({ user_email: 'me@x', endpoint: 'https://new.example' });
          return [{ id: 'old', name: 'Old server', lock_holder: 'me@x' }];
        }
        return null;
      });
      await useSpeleoStore.getState().scanLocksOnOpen({ prompt: true });
      expect(useSpeleoStore.getState().remoteSummaries).toEqual({});
      expect(useSpeleoStore.getState().heldLocks).toBeNull();
    });

    it('releaseLock drops one lock and frees it in the badge map, emptying to null on the last', async () => {
      loggedInAs('me@x');
      useSpeleoStore.setState({
        heldLocks: [
          { projectId: 'p1', projectName: 'A' },
          { projectId: 'p2', projectName: 'B' },
        ],
        remoteLocks: { p1: 'me@x', p2: 'me@x' },
      });
      // release_lock returns null; the follow-up refreshLinkStatus reads link_status.
      invokeIpcMock.mockImplementation(async (cmd: string) =>
        cmd === 'speleodb_link_status' ? status() : null,
      );

      await useSpeleoStore.getState().releaseLock('p1');
      let s = useSpeleoStore.getState();
      expect(s.heldLocks).toEqual([{ projectId: 'p2', projectName: 'B' }]);
      expect(s.remoteLocks.p1).toBeNull();
      expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_release_lock', { projectId: 'p1' });

      await useSpeleoStore.getState().releaseLock('p2');
      s = useSpeleoStore.getState();
      expect(s.heldLocks).toBeNull(); // empty → the dialog's effect auto-closes it
      expect(s.remoteLocks.p2).toBeNull();
    });

    // #270: the "Release all locks" button was deleted (the server verb is
    // all-or-nothing and this prompt is exactly where locks over un-pushed work
    // surface), and the store action it drove went with it - dead code that still
    // offered a one-call way to strand every project's work.
    it('exposes no bulk release action at all', () => {
      expect('releaseAllLocks' in useSpeleoStore.getState()).toBe(false);
    });

    it('exposes no second logout path that could swallow a credential-file failure', () => {
      expect('logout' in useSpeleoStore.getState()).toBe(false);
    });
  });

  // #270: `speleodb_push_only` / `speleodb_done` answer `not_editing` when the edit
  // gate was already closed - nothing uploaded, nothing released. Folded into
  // Pushed/Released, that cleared `hasUnpushedEdits`, which is exactly the flag
  // `guardQuit` and the status button's warning read: the app claimed the work had
  // landed and stopped protecting it, over edits still only on this computer.
  describe('a no-op push/release must not clear the dirty bit (#270)', () => {
    const dirtyReadOnly = () =>
      useSpeleoStore.setState({
        link: {
          permission: 'READ_AND_WRITE',
          // How this state is reached: a 401 runs `clearSession()`, which drops to
          // read-only while deliberately CARRYING the dirty bit through.
          editState: 'read-only',
          lockHolder: null,
          lockedByOther: false,
          hasUnpushedEdits: true,
          unwound: false,

          undoMode: 'full',
          remoteState: 'unknown',
          noAccess: false,
          serverError: null,
        },
      });

    it('applyPush leaves the dirty bit set on not_editing', () => {
      dirtyReadOnly();
      useSpeleoStore.getState().applyPush({ kind: 'not_editing' } as never);
      const link = useSpeleoStore.getState().link;
      expect(link?.hasUnpushedEdits).toBe(true);
      // And it must not claim the copy is in sync either.
      expect(link?.remoteState).toBe('unknown');
    });

    it('applyDone leaves the dirty bit AND the read-only state alone on not_editing', () => {
      dirtyReadOnly();
      useSpeleoStore.getState().applyDone({ kind: 'not_editing' } as never);
      const link = useSpeleoStore.getState().link;
      expect(link?.hasUnpushedEdits).toBe(true);
      expect(link?.remoteState).toBe('unknown');
    });

    it('an unrecognised kind is treated conservatively rather than as success', () => {
      dirtyReadOnly();
      const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
      useSpeleoStore.getState().applyPush({ kind: 'something_new' } as never);
      useSpeleoStore.getState().applyDone({ kind: 'something_new' } as never);
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
      warn.mockRestore();
    });

    it('still clears the dirty bit when the work genuinely landed', () => {
      dirtyReadOnly();
      useSpeleoStore.getState().applyPush({ kind: 'pushed' });
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);
      dirtyReadOnly();
      useSpeleoStore.getState().applyDone({ kind: 'released' });
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);
    });
  });

  // #270: the optimistic open seed hardcoded `hasUnpushedEdits: false` under the
  // comment "a freshly pulled project always opens read-only + up-to-date". True for
  // a pull; false for "Keep the local copy", which reuses the same tail and pulls
  // nothing.
  describe('setLinkOnOpen carries the dirty bit explicitly (#270)', () => {
    it('defaults to clean, preserving the fresh-pull case', () => {
      useSpeleoStore.getState().setLinkOnOpen({ permission: 'READ_AND_WRITE', forPath: '/a.stalx' });
      const link = useSpeleoStore.getState().link;
      expect(link?.hasUnpushedEdits).toBe(false);
      expect(link?.remoteState).toBe('up_to_date');
    });

    it('seeds a dirty copy as dirty, and stops claiming it is up to date', () => {
      useSpeleoStore
        .getState()
        .setLinkOnOpen({ permission: 'READ_AND_WRITE', forPath: '/a.stalx', hasUnpushedEdits: true });
      const link = useSpeleoStore.getState().link;
      expect(link?.hasUnpushedEdits).toBe(true);
      expect(link?.remoteState).toBe('unknown');
    });

    it('syncProjectLink keeps that seed for the path being loaded', () => {
      useSpeleoStore
        .getState()
        .setLinkOnOpen({ permission: 'READ_AND_WRITE', forPath: '/a.stalx', hasUnpushedEdits: true });
      useSpeleoStore.getState().syncProjectLink(true, '/a.stalx');
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
    });
  });

  // #270: the revert dialog raises the push-failure banner (via `done`) and stays
  // open pointing the user at it - but it renders through AlertDialog's
  // full-viewport scrim, so the banner behind it is covered and unclickable. It
  // belongs on the suppress list with the other dialogs that own a failure.
  describe('PUSH_FAILURE_DIALOGS', () => {
    it.each([
      'speleodb-done',
      'speleodb-revert',
      'speleodb-unwound-push',
    ])('covers %s', (id) => {
      expect(PUSH_FAILURE_DIALOGS).toContain(id);
    });

    // No dialog opens these ids any more: the close and quit questions are the
    // exit prompt's overlay, which the banner checks directly.
    it('lists no dead dialog ids', () => {
      expect(PUSH_FAILURE_DIALOGS).toEqual(['speleodb-done', 'speleodb-revert', 'speleodb-unwound-push']);
    });
  });
});

describe('successful sign-in refresh', () => {
  beforeEach(reset);

  it('fetches project rows immediately without waiting for Settings to close', async () => {
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'speleodb_login') return { user_email: 'me@x', endpoint: 'https://x/' };
      if (cmd === 'speleodb_list_projects') return [{ id: 'p1', name: 'New cave', lock_holder: null }];
      if (cmd === 'speleodb_link_status') return status();
      return null;
    });
    useSpeleoStore.getState().setPendingAuth({
      kind: 'login', email: 'me@x', password: 'synthetic', token: '', useToken: false,
    });
    expect(await useSpeleoStore.getState().applyPendingAuth('https://x/')).toEqual({ ok: true });
    await vi.waitFor(() => {
      expect(useSpeleoStore.getState().remoteProjects).toEqual({ p1: 'New cave' });
    });
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_list_projects');
  });

  it.each([true, false])('logs in over the given insecure-HTTP choice (%s) and reloads the saved settings', async (insecureHttp) => {
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'speleodb_login') return { user_email: 'me@x', endpoint: 'http://lab.example/' };
      if (cmd === 'get_config') return null;
      return null;
    });
    useSpeleoStore.getState().setPendingAuth({
      kind: 'login', email: '', password: '', token: '<your_token_here>', useToken: true,
    });
    expect(await useSpeleoStore.getState().applyPendingAuth('http://lab.example/', insecureHttp)).toEqual({ ok: true });
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_login', expect.objectContaining({
      endpoint: 'http://lab.example/', insecureHttp,
    }));
    // The backend saved that connection with the session; the app's copy follows.
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'get_config')).toBe(true);
  });

  it('leaves the insecure-HTTP choice to the saved one when none is given', async () => {
    invokeIpcMock.mockImplementation(async (cmd: string) =>
      cmd === 'speleodb_login' ? { user_email: 'me@x', endpoint: 'https://x/' } : null);
    useSpeleoStore.getState().setPendingAuth({
      kind: 'login', email: '', password: '', token: '<your_token_here>', useToken: true,
    });
    await useSpeleoStore.getState().applyPendingAuth('https://x/');
    const login = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_login');
    expect(login?.[1]).not.toHaveProperty('insecureHttp');
  });
});

describe('refresh progress', () => {
  beforeEach(reset);

  it('stays active across overlapping listing requests and clears after failure', async () => {
    loggedInAs('me@x');
    let finish!: (value: unknown) => void;
    let fail!: (error: Error) => void;
    invokeIpcMock.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    invokeIpcMock.mockImplementationOnce(() => new Promise((_resolve, reject) => { fail = reject; }));
    const first = useSpeleoStore.getState().scanLocksOnOpen();
    const second = useSpeleoStore.getState().scanLocksOnOpen();
    expect(useSpeleoStore.getState().projectRefreshCount).toBe(2);
    finish([]);
    await first;
    expect(useSpeleoStore.getState().projectRefreshCount).toBe(1);
    fail(new Error('offline'));
    await second;
    expect(useSpeleoStore.getState().projectRefreshCount).toBe(0);
  });

  it('clears status progress when a request fails', async () => {
    let fail!: (error: Error) => void;
    invokeIpcMock.mockImplementationOnce(() => new Promise((_resolve, reject) => { fail = reject; }));
    const request = useSpeleoStore.getState().refreshLinkStatus();
    expect(useSpeleoStore.getState().statusRefreshCount).toBe(1);
    fail(new Error('offline'));
    await request;
    expect(useSpeleoStore.getState().statusRefreshCount).toBe(0);
  });
});

it('does not apply an old account status after a successful replacement login', async () => {
  reset();
  useSpeleoStore.getState().seedFromStatus(status({ permission: 'READ_AND_WRITE' }));
  const original = useSpeleoStore.getState().link;
  let finish!: (value: SpeleoLinkStatus) => void;
  invokeIpcMock.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  const request = useSpeleoStore.getState().refreshLinkStatus();
  useSpeleoStore.getState().setSessionFromLogin({ user_email: 'new@example.com', endpoint: 'https://x/' });
  finish(status({ permission: 'ADMIN', edit_enabled: true }));
  await request;
  expect(useSpeleoStore.getState().link).toBe(original);
  expect(useSpeleoStore.getState().statusRefreshCount).toBe(0);
});
