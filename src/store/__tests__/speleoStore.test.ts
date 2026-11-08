import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  useSpeleoStore,
  PUSH_FAILURE_DIALOGS,
  isUnwoundNotEdited,
  speleoProjectSessionIssuedFor,
  speleoProjectSessionStillCurrent,
} from '../speleoStore';
import type { SpeleoLinkStatus } from '../../types/ipc.generated';
import { invokeIpcMock, messageOr, stubGetErrorMessage } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');
// The store reports a failed picker retry through this, so the mock has to carry
// it or the error path throws inside the very catch that exists to report.
stubGetErrorMessage(messageOr('error'));

/** A logged-in session so `scanLocksOnOpen` skips the `refreshSession` prelude. */
const loggedInAs = (email: string) =>
  useSpeleoStore.setState({
    session: { loggedIn: true, userEmail: email, endpoint: 'https://x/', keychainBacked: true, storage: 'keychain', keychain: 'available' },
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
  exclude_geojson: false,
  // [#303] The hybrid undo stack's policy pair: Full, from the global default.
  undo_mode: 'full',
  force_editing: false,
  allow_force_edit: false,
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
    storageChoiceOpen: false,
    remoteProjectId: null,
    sessionGeneration: 0,
    chainTargetGeneration: 0,
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
        keychain_backed: true,
        storage: 'keychain',
        keychain: 'available',
        needs_storage_choice: false,
        replaced_session: false,
        locks_left_held: [],
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
        keychain_backed: false,
        storage: 'memory',
        keychain: 'unavailable',
      });
      expect(useSpeleoStore.getState().session).toEqual({
        loggedIn: true,
        userEmail: 'u@x',
        endpoint: 'https://x/',
        keychainBacked: false,
        // Both new fields cross intact: "durable?" and "where?" are different
        // questions, and Settings needs the second one to say anything useful.
        storage: 'memory',
        keychain: 'unavailable',
      });
    });

    it('[#258] a refresh that reports logged-out tears down an OPEN picker', () => {
      // Suppressing NEW pickers after a logged-out refresh is only half of it: one
      // already on screen offers to retry against an account nobody is signed in to,
      // and `setSession` is the benign action every project load and Settings open
      // calls, so nothing else was going to close it. The cancel has to fire too, or a
      // caller suspended on the question is left waiting forever.
      useSpeleoStore.getState().setSession({
        logged_in: true,
        user_email: 'u@x',
        endpoint: 'https://x/',
        keychain_backed: true,
        storage: 'keychain',
        keychain: 'available',
      });
      const onCancel = vi.fn();
      useSpeleoStore
        .getState()
        .openDumpPicker(
          [{ entry_path: 'a.stalx', size_bytes: 1, modified: null }],
          'stalx',
          async () => {},
          onCancel,
        );
      expect(useSpeleoStore.getState().dumpPickerOpen).toBe(true);

      useSpeleoStore.getState().setSession({
        logged_in: false,
        user_email: null,
        endpoint: 'https://x/',
        keychain_backed: false,
        storage: null,
        keychain: 'available',
      });

      expect(useSpeleoStore.getState().dumpPickerOpen).toBe(false);
      expect(onCancel).toHaveBeenCalled();
      // The refresh's own report still lands - this is a teardown, not a veto.
      expect(useSpeleoStore.getState().session?.loggedIn).toBe(false);
    });

    it('[#258 review] tears the picker down on a logged-out refresh even with no prior session', () => {
      // `sessionStillCurrent` lets a picker be raised while `session` is still null -
      // an authenticated call can beat the first `speleodb_session` reply - so for that
      // picker there is no true->false transition to detect. A transition test alone
      // left it open over a session the same refresh had just reported dead: Confirm
      // could only fail, and a flow suspended on the question stayed busy.
      useSpeleoStore.setState({ session: null });
      const onCancel = vi.fn();
      useSpeleoStore
        .getState()
        .openDumpPicker(
          [{ entry_path: 'a.stalx', size_bytes: 1, modified: null }],
          'stalx',
          async () => {},
          onCancel,
        );

      useSpeleoStore.getState().setSession({
        logged_in: false,
        user_email: null,
        endpoint: 'https://x/',
        keychain_backed: false,
        storage: null,
        keychain: 'available',
      });

      expect(useSpeleoStore.getState().dumpPickerOpen).toBe(false);
      expect(onCancel).toHaveBeenCalled();
    });

    it('[#258] a benign logged-in refresh leaves an open picker alone', () => {
      // The other half: a project load and reopening Settings both call this, and a
      // picker raised by an open in flight must survive them.
      useSpeleoStore.getState().setSession({
        logged_in: true,
        user_email: 'u@x',
        endpoint: 'https://x/',
        keychain_backed: true,
        storage: 'keychain',
        keychain: 'available',
      });
      useSpeleoStore
        .getState()
        .openDumpPicker(
          [{ entry_path: 'a.stalx', size_bytes: 1, modified: null }],
          'stalx',
          async () => {},
        );

      useSpeleoStore.getState().setSession({
        logged_in: true,
        user_email: 'u@x',
        endpoint: 'https://x/',
        keychain_backed: true,
        storage: 'keychain',
        keychain: 'available',
      });

      expect(useSpeleoStore.getState().dumpPickerOpen).toBe(true);
    });

    it('clearSession degrades an active edit to read-only', () => {
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
      useSpeleoStore
        .getState()
        .setSessionFromLogin({
          user_email: 'u@x',
          endpoint: 'https://x/',
          keychain_backed: true,
          storage: 'keychain',
          keychain: 'available',
          needs_storage_choice: false,
          replaced_session: false,
          locks_left_held: [],
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
      // unpushed-edits warning. The next authenticated poll re-seeds it.
      expect(s.link?.hasUnpushedEdits).toBe(true);
      expect(s.link?.remoteState).toBe('unknown');
      expect(s.link?.noAccess).toBe(false);
      expect(s.link?.serverError).toBeNull();
      expect(s.link?.permission).toBe('READ_AND_WRITE');
      expect(s.divergence).toBeNull();
      expect(s.banner).toBeNull();
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
          keychain_backed: true,
          storage: 'keychain',
          keychain: 'available',
          needs_storage_choice: false,
          replaced_session: false,
          locks_left_held: [],
        });
      useSpeleoStore.getState().setPendingAuth({ kind: 'logout' });
      // No lock held, so no sign-out prompt (#576 SIGNOUT-PROMPT-D1); the logout rejects.
      invokeIpcMock.mockImplementation(async (cmd: string) => {
        if (cmd === 'speleodb_sign_out_locks') return { releasable: [], unpushed: [], in_use_elsewhere: [] };
        throw {
          message: 'Logged out, but the stored SpeleoDB credential could not be removed',
          code: 'SPELEODB_FATAL',
          status: null,
        };
      });

      const result = await useSpeleoStore.getState().applyPendingAuth('https://x/');
      expect(result.ok).toBe(false);
      expect(useSpeleoStore.getState().session).toBeNull();
      expect(useSpeleoStore.getState().pendingAuth).toBeNull();
    });

    /**
     * [#576 FZ-SDB-3-D1] A login that replaced a live session for another account or
     * server: the backend ran logout's hand-back for the old one first (lock released
     * best-effort, edit gate shut, claim forgotten), so the store mirrors that logout
     * BEFORE the new session lands and re-seeds the link from the backend.
     */
    describe('applyPendingAuth over a replaced session', () => {
      const seenAtNewSession: Array<{
        editState: string | undefined;
        lockHolder: string | null | undefined;
        heldLocks: unknown;
      }> = [];
      const realSetSessionFromLogin = useSpeleoStore.getState().setSessionFromLogin;

      const loginReplacing = async (replaced_session: boolean) => {
        loggedInAs('old@x');
        useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true, lock_holder: 'old@x' }));
        useSpeleoStore.setState({
          heldLocks: [{ projectId: 'p1', projectName: 'Cenote Azul' }],
          remoteProjectId: 'p1',
          // What the new session looks like at the moment it is set.
          setSessionFromLogin: (r) => {
            const s = useSpeleoStore.getState();
            seenAtNewSession.push({
              editState: s.link?.editState,
              lockHolder: s.link?.lockHolder,
              heldLocks: s.heldLocks,
            });
            realSetSessionFromLogin(r);
          },
        });
        useSpeleoStore.getState().setPendingAuth({
          kind: 'login',
          email: 'new@y',
          password: 'pw',
          token: '',
          useToken: false,
        });
        invokeIpcMock.mockImplementation(async (cmd: string) => {
          if (cmd === 'speleodb_login')
            return {
              user_email: 'new@y',
              endpoint: 'https://y/',
              keychain_backed: true,
              storage: 'keychain',
              keychain: 'available',
              needs_storage_choice: false,
              replaced_session,
              locks_left_held: [],
            };
          // The re-seed: never answers, so what is asserted is the reset, not a status.
          if (cmd === 'speleodb_link_status') return new Promise(() => {});
          // No lock held: no sign-out prompt (#576 SIGNOUT-PROMPT-D1).
          if (cmd === 'speleodb_sign_in_hand_back_locks') return { releasable: [], unpushed: [], in_use_elsewhere: [] };
          return [];
        });
        return useSpeleoStore.getState().applyPendingAuth('https://y/');
      };

      beforeEach(() => {
        seenAtNewSession.length = 0;
      });

      afterEach(() => {
        useSpeleoStore.setState({ setSessionFromLogin: realSetSessionFromLogin });
      });

      it('drops the old link and held-lock state before the new session, then re-seeds', async () => {
        expect((await loginReplacing(true)).ok).toBe(true);

        expect(seenAtNewSession).toEqual([{ editState: 'read-only', lockHolder: null, heldLocks: null }]);
        expect(useSpeleoStore.getState().session?.userEmail).toBe('new@y');
        expect(useSpeleoStore.getState().remoteProjectId).toBeNull();
        expect(invokeIpcMock.mock.calls.map(([cmd]) => cmd)).toContain('speleodb_link_status');
      });

      it('leaves the link alone when no session was replaced', async () => {
        expect((await loginReplacing(false)).ok).toBe(true);

        expect(seenAtNewSession).toEqual([
          { editState: 'editing', lockHolder: 'old@x', heldLocks: [{ projectId: 'p1', projectName: 'Cenote Azul' }] },
        ]);
        expect(useSpeleoStore.getState().session?.userEmail).toBe('new@y');
        expect(invokeIpcMock.mock.calls.map(([cmd]) => cmd)).not.toContain('speleodb_link_status');
      });
    });
  });

  describe('credential storage', () => {
    const login = (needs_storage_choice: boolean) => {
      useSpeleoStore.getState().setPendingAuth({
        kind: 'login',
        email: 'u@x',
        password: 'pw',
        token: '',
        useToken: false,
      });
      invokeIpcMock.mockImplementation(async (cmd: string) =>
        cmd === 'speleodb_login'
          ? {
              user_email: 'u@x',
              endpoint: 'https://x/',
              keychain_backed: false,
              storage: 'memory',
              keychain: 'unavailable',
              needs_storage_choice,
              locks_left_held: [],
            }
          : cmd === 'speleodb_sign_in_hand_back_locks'
            ? { releasable: [], unpushed: [], in_use_elsewhere: [] }
            : [],
      );
      return useSpeleoStore.getState().applyPendingAuth('https://x/');
    };

    it('a login whose keychain failed raises the storage question', async () => {
      // Raised from applyPendingAuth so BOTH login paths reach it: the staged one
      // applied on Settings' Save, and the panel's "Log in now" button.
      await login(true);
      expect(useSpeleoStore.getState().storageChoiceOpen).toBe(true);
    });

    it('a login with a working keychain asks nothing', async () => {
      await login(false);
      expect(useSpeleoStore.getState().storageChoiceOpen).toBe(false);
    });

    it('setTokenStorage resolves the tier that took the key, not the one asked for', async () => {
      // The plaintext write can itself fail, and then the truthful thing to show is
      // session-only - so the answer is authoritative over what was asked for. It is
      // handed back rather than collapsed to `true` because the caller has to report
      // where the key went: a bare success over a fallback told the user their key
      // was written to a file that does not exist, and they get signed out at exit.
      useSpeleoStore.setState({ storageChoiceOpen: true });
      invokeIpcMock.mockResolvedValue({
        logged_in: true,
        user_email: 'u@x',
        endpoint: 'https://x/',
        keychain_backed: false,
        storage: 'memory',
        keychain: 'declined',
      });

      expect(await useSpeleoStore.getState().setTokenStorage('plaintext')).toBe('memory');
      expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_set_token_storage', {
        mode: 'plaintext',
      });
      expect(useSpeleoStore.getState().session?.storage).toBe('memory');
      expect(useSpeleoStore.getState().storageChoiceOpen).toBe(false);
    });

    it('a refused setTokenStorage leaves the prompt up', async () => {
      useSpeleoStore.setState({ storageChoiceOpen: true });
      invokeIpcMock.mockRejectedValue({ message: 'not signed in' });

      expect(await useSpeleoStore.getState().setTokenStorage('session_only')).toBeNull();
      expect(useSpeleoStore.getState().storageChoiceOpen).toBe(true);
    });

    it('retryKeychain re-seeds the session from the refreshed status', async () => {
      invokeIpcMock.mockResolvedValue({
        logged_in: true,
        user_email: 'u@x',
        endpoint: 'https://x/',
        keychain_backed: true,
        storage: 'keychain',
        keychain: 'available',
      });

      expect(await useSpeleoStore.getState().retryKeychain()).toBe(true);
      expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_retry_keychain');
      expect(useSpeleoStore.getState().session?.keychain).toBe('available');
      expect(useSpeleoStore.getState().session?.storage).toBe('keychain');
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

    it('clears a prior lockedByOther once a later poll resolves no holder (fix 7)', () => {
      // Someone else held it, then released: the next poll reports no holder, and the
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

    it('surfaces an unexpected server_error, and clears it on the next clean poll (fix 12)', () => {
      const err = { code: 'SPELEODB_FATAL' as const, message: 'No Project matches the given query.', action: 'Fatal' as const, status: 404, transport: null, unpushed_local: null, dump_candidates: null, dump_candidate_kind: null };
      useSpeleoStore.getState().seedFromStatus(status({ server_error: err }));
      expect(useSpeleoStore.getState().link?.serverError).toEqual(err);
      useSpeleoStore.getState().seedFromStatus(status({ server_error: null }));
      expect(useSpeleoStore.getState().link?.serverError).toBeNull();
    });

    describe('lock lost under an edit session (#576 B35(e))', () => {
      // The backend now switches editing off when SpeleoDB says the lock this session
      // was editing under is gone or someone else's. The toolbar going read-only on
      // its own said nothing about why, or that un-uploaded edits are stranded.
      const editingThenPoll = (o: Partial<SpeleoLinkStatus>) => {
        loggedInAs('me@x');
        useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true, lock_holder: 'me@x' }));
        expect(useSpeleoStore.getState().banner).toBeNull();
        useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: false, ...o }));
        return useSpeleoStore.getState();
      };

      it('names the account that took the lock', () => {
        const s = editingThenPoll({ lock_holder: 'colleague@x' });
        expect(s.banner).toEqual({ kind: 'lock_lost', holder: 'colleague@x' });
        expect(s.link?.editState).toBe('read-only');
      });

      it('says so when the lock is gone over un-pushed edits', () => {
        const s = editingThenPoll({ lock_holder: null, has_unpushed_edits: true });
        expect(s.banner).toEqual({ kind: 'lock_lost', holder: null });
      });

      it('outranks a push-failure banner left by a Done that was refused for it', () => {
        loggedInAs('me@x');
        useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true, lock_holder: 'me@x' }));
        useSpeleoStore.getState().raisePushFailedBanner({ message: 'm', origin: 'done', pushMessage: null });
        useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: false, lock_holder: 'colleague@x' }));
        expect(useSpeleoStore.getState().banner?.kind).toBe('lock_lost');
      });

      it('stays quiet for a clean copy whose lock simply went away (a release looks the same)', () => {
        expect(editingThenPoll({ lock_holder: null }).banner).toBeNull();
      });

      it('stays quiet when the server did not answer (offline / signed out)', () => {
        expect(editingThenPoll({ lock_holder: null, permission: null, has_unpushed_edits: true }).banner).toBeNull();
        reset();
        expect(editingThenPoll({ lock_holder: 'colleague@x', permission: null }).banner).toBeNull();
      });

      it('stays quiet when the holder is our own account in another case', () => {
        expect(editingThenPoll({ lock_holder: 'ME@X' }).banner).toBeNull();
      });

      it('stays quiet when this session never held the lock', () => {
        loggedInAs('me@x');
        useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: false }));
        useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: false, lock_holder: 'colleague@x' }));
        expect(useSpeleoStore.getState().banner).toBeNull();
      });

      it('stays quiet for a force-edit session, which never held the lock', () => {
        loggedInAs('me@x');
        useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true, force_editing: true }));
        useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: false, lock_holder: 'colleague@x' }));
        expect(useSpeleoStore.getState().banner).toBeNull();
      });
    });

    it('carries force_editing + allow_force_edit through (#213)', () => {
      useSpeleoStore
        .getState()
        .seedFromStatus(status({ edit_enabled: true, force_editing: true, allow_force_edit: true }));
      const link = useSpeleoStore.getState().link;
      expect(link?.forceEditing).toBe(true);
      expect(link?.forceEditAllowed).toBe(true);
      // A later poll that acquired the lock clears force-edit (backend-authoritative).
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true, force_editing: false }));
      expect(useSpeleoStore.getState().link?.forceEditing).toBe(false);
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

      // An edit and then a push land while that poll is still waiting on the network.
      useSpeleoStore
        .getState()
        .applyRemoteLink({ has_unpushed_edits: true, unwound: false }, useSpeleoStore.getState().linkPath);
      useSpeleoStore.getState().applyPush({ kind: 'pushed' });
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);

      // Now the stale reply arrives, still describing the world before the push.
      release(status({ edit_enabled: true, has_unpushed_edits: true }));
      await inFlight;

      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);
    });

    it('applies a reply when nothing mutated the link while it was in flight', async () => {
      // The guard must not be so strict that ordinary polling stops working.
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
      invokeIpcMock.mockResolvedValue(status({ edit_enabled: true, has_unpushed_edits: true }));
      await useSpeleoStore.getState().refreshLinkStatus();
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
    });

    it('keeps the FRESHER of two overlapping refreshes instead of dropping it', async () => {
      // `seedFromStatus` allocates a new link object every time, so a plain
      // identity guard treated a rival poll's seed as a local mutation - and the
      // reply it discarded was the LATER one, holding newer truth.
      //
      // The path that matters: `handleRelease` releases the lock and awaits its own
      // refresh. A periodic poll landing in that window reseeded `link`, the
      // release's own reply was thrown away, and the toolbar kept the green pen
      // over a lock that no longer existed - until the next poll, up to 24h out.
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));

      const pending: Array<(s: SpeleoLinkStatus) => void> = [];
      invokeIpcMock.mockImplementation(
        () => new Promise<SpeleoLinkStatus>((resolve) => pending.push(resolve)),
      );

      const poll = useSpeleoStore.getState().refreshLinkStatus();
      const afterRelease = useSpeleoStore.getState().refreshLinkStatus();
      expect(pending).toHaveLength(2);

      // The periodic poll answers first, still describing the held lock.
      pending[0]!(status({ edit_enabled: true }));
      await poll;
      // Then the release's own refresh answers: the lock is gone.
      pending[1]!(status({ edit_enabled: false }));
      await afterRelease;

      expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    });

    it('lets an older reply lose to a newer one that already landed', async () => {
      // Issue order, not arrival order. Without it the release-then-poll ordering
      // is fixed but the poll-then-release one regresses.
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
        forceEditing: false,
        forceEditAllowed: false,
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

    it('a successful reconnect clears force-edit (#213)', () => {
      // Enter force-edit, then reconnect (editing/fast_forwarded acquire the lock).
      useSpeleoStore
        .getState()
        .seedFromStatus(status({ edit_enabled: true, force_editing: true }));
      expect(useSpeleoStore.getState().link?.forceEditing).toBe(true);
      useSpeleoStore.getState().applyStartEditing({ kind: 'editing' });
      expect(useSpeleoStore.getState().link?.forceEditing).toBe(false);
      expect(useSpeleoStore.getState().link?.editState).toBe('editing');
    });

    it('a contended reconnect (locked_by) preserves the force-edit session (#213)', () => {
      // The acquire failed (someone else holds it): the force-editor keeps editing
      // offline, so force-edit is NOT cleared and the gate stays on.
      useSpeleoStore
        .getState()
        .seedFromStatus(status({ edit_enabled: true, force_editing: true }));
      useSpeleoStore.getState().applyStartEditing({ kind: 'locked_by', email: 'a@b' });
      const s = useSpeleoStore.getState();
      expect(s.link?.forceEditing).toBe(true);
      // Still editing (offline) - not downgraded to read-only - with the holder noted.
      expect(s.link?.editState).toBe('editing');
      expect(s.link?.lockedByOther).toBe(true);
      expect(s.link?.lockHolder).toBe('a@b');
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
      const generation = useSpeleoStore.getState().chainTargetGeneration;
      useSpeleoStore.getState().applyDone({ kind: 'released' });
      const s = useSpeleoStore.getState();
      expect(s.link?.editState).toBe('read-only');
      expect(s.link?.hasUnpushedEdits).toBe(false);
      // Regression: a released lock must not still read as held (button stayed green).
      expect(s.link?.lockHolder).toBeNull();
      expect(s.link?.lockedByOther).toBe(false);
      expect(s.divergence).toBeNull();
      expect(s.chainTargetGeneration).toBe(generation + 1);
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
      const generation = useSpeleoStore.getState().chainTargetGeneration;
      useSpeleoStore.getState().applyPush({ kind: 'pushed' });
      const s = useSpeleoStore.getState();
      // Push behaves like push&release WITHOUT releasing the lock - still editing.
      expect(s.link?.editState).toBe('editing');
      expect(s.link?.hasUnpushedEdits).toBe(false);
      expect(s.link?.remoteState).toBe('up_to_date');
      expect(s.banner).toBeNull();
      expect(s.chainTargetGeneration).toBe(generation + 1);
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

  // [#576 D22] The link bits a survey mutation answered with, applied as the backend
  // set them. They replace the hand-written mirrors (`markDirty` after an undo,
  // `markForwardEdit` after an edit); each case below is one those used to cover.
  describe('applyRemoteLink', () => {
    const apply = (has_unpushed_edits: boolean, unwound: boolean, linkPath = useSpeleoStore.getState().linkPath) =>
      useSpeleoStore.getState().applyRemoteLink({ has_unpushed_edits, unwound }, linkPath);

    it('flips the dirty bit on for an editing link', () => {
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
      apply(true, false);
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
    });

    it('is a no-op with no link', () => {
      apply(true, false);
      expect(useSpeleoStore.getState().link).toBeNull();
    });

    // [#303] An undo/redo does NOT drop the chain pointer (the backend keeps it, and a
    // tripwire in speleodb_revisions.rs asserts undo.rs never forgets it), so an undo
    // on an unwound copy answers still-unwound - and `unwound` must stay standing.
    it('leaves unwound standing when the backend kept it (an undo)', () => {
      useSpeleoStore
        .getState()
        .seedFromStatus(status({ edit_enabled: true, has_unpushed_edits: true, unwound: true }));
      apply(true, true);
      expect(useSpeleoStore.getState().link?.unwound).toBe(true);
      expect(isUnwoundNotEdited(useSpeleoStore.getState().link)).toBe(true);
    });

    // [#303] The bug the old `markForwardEdit` existed to prevent: an unwound copy is
    // ALWAYS already dirty, so a dirty-bit-only mirror left `unwound` standing until
    // the next poll - and `isUnwoundNotEdited` kept routing the user to a discard of
    // work they just made. A forward edit answers dirty AND no longer unwound.
    it('clears unwound on a forward edit even when already dirty', () => {
      useSpeleoStore
        .getState()
        .seedFromStatus(status({ edit_enabled: true, has_unpushed_edits: true, unwound: true }));
      const generation = useSpeleoStore.getState().chainTargetGeneration;
      apply(true, false);
      const link = useSpeleoStore.getState().link;
      expect(link?.hasUnpushedEdits).toBe(true);
      expect(link?.unwound).toBe(false);
      expect(isUnwoundNotEdited(link)).toBe(false);
      expect(useSpeleoStore.getState().chainTargetGeneration).toBe(generation + 1);
    });

    it('dirties a clean link and no-ops when nothing changes', () => {
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
      const generation = useSpeleoStore.getState().chainTargetGeneration;
      apply(true, false);
      const after = useSpeleoStore.getState().link;
      expect(after?.hasUnpushedEdits).toBe(true);
      expect(after?.unwound).toBe(false);
      // Identity is preserved on a true no-op: refreshLinkStatus uses it to detect a
      // concurrent mutation, so a gratuitous new object would discard live replies.
      apply(true, false);
      expect(useSpeleoStore.getState().link).toBe(after);
      expect(useSpeleoStore.getState().chainTargetGeneration).toBe(generation + 1);
    });

    it('leaves a clean copy clean when the backend says the edit dirtied nothing (a provable no-op)', () => {
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
      const before = useSpeleoStore.getState().link;
      const generation = useSpeleoStore.getState().chainTargetGeneration;
      apply(false, false);
      expect(useSpeleoStore.getState().link).toBe(before);
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);
      expect(useSpeleoStore.getState().chainTargetGeneration).toBe(generation);
    });

    it('takes the backend value both ways, a cleared bit included', () => {
      useSpeleoStore
        .getState()
        .seedFromStatus(status({ edit_enabled: true, has_unpushed_edits: true, unwound: true }));
      apply(false, false);
      const link = useSpeleoStore.getState().link;
      expect(link?.hasUnpushedEdits).toBe(false);
      expect(link?.unwound).toBe(false);
    });

    it('drops a reply issued for another link path (the user left that project)', () => {
      useSpeleoStore.getState().syncProjectLink(true, '/caves/now-open.stalx');
      const before = useSpeleoStore.getState().link;
      const generation = useSpeleoStore.getState().chainTargetGeneration;
      apply(true, false, '/caves/left-behind.stalx');
      expect(useSpeleoStore.getState().link).toBe(before);
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);
      expect(useSpeleoStore.getState().chainTargetGeneration).toBe(generation);
      // The same reply for the path that IS open applies.
      apply(true, false, '/caves/now-open.stalx');
      expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
    });
  });

  describe('clearLink', () => {
    it('drops link, divergence and banner', () => {
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
      useSpeleoStore.setState({ divergence: 'update', banner: { kind: 'concurrent_session' } });
      useSpeleoStore.getState().clearLink();
      const s = useSpeleoStore.getState();
      expect(s.link).toBeNull();
      expect(s.divergence).toBeNull();
      expect(s.banner).toBeNull();
    });
  });

  describe('setUndoMode', () => {
    it('invalidates chain targets after the backend accepts the policy change', async () => {
      useSpeleoStore.getState().seedFromStatus(status({ edit_enabled: true }));
      invokeIpcMock.mockImplementation((command: string) => {
        if (command === 'speleodb_set_undo_mode') return Promise.resolve(null);
        if (command === 'speleodb_link_status') {
          return Promise.resolve(status({ edit_enabled: true, undo_mode: 'disabled' }));
        }
        return Promise.resolve(null);
      });
      const generation = useSpeleoStore.getState().chainTargetGeneration;

      await useSpeleoStore.getState().setUndoMode('disabled');

      expect(useSpeleoStore.getState().chainTargetGeneration).toBe(generation + 1);
      expect(useSpeleoStore.getState().link?.undoMode).toBe('disabled');
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
    it('seedFromStatus raises the rendering-disabled banner when exclude_geojson is set', () => {
      useSpeleoStore.getState().seedFromStatus(status({ exclude_geojson: true }));
      expect(useSpeleoStore.getState().banner).toEqual({ kind: 'rendering_disabled' });
    });

    it('seedFromStatus clears its own rendering-disabled banner once the flag is off', () => {
      useSpeleoStore.getState().seedFromStatus(status({ exclude_geojson: true }));
      useSpeleoStore.getState().seedFromStatus(status({ exclude_geojson: false }));
      expect(useSpeleoStore.getState().banner).toBeNull();
    });

    it('a rendering-disabled poll never clobbers transient push_unconfirmed feedback', () => {
      useSpeleoStore.setState({ banner: { kind: 'push_unconfirmed' } });
      useSpeleoStore.getState().seedFromStatus(status({ exclude_geojson: true }));
      expect(useSpeleoStore.getState().banner).toEqual({ kind: 'push_unconfirmed' });
    });

    it('raiseNoAnchorBanner only claims a free banner slot', () => {
      useSpeleoStore.getState().raiseNoAnchorBanner();
      expect(useSpeleoStore.getState().banner).toEqual({ kind: 'no_anchor' });
      // With a transient banner already up, it must not overwrite it.
      useSpeleoStore.setState({ banner: { kind: 'push_unconfirmed' } });
      useSpeleoStore.getState().raiseNoAnchorBanner();
      expect(useSpeleoStore.getState().banner).toEqual({ kind: 'push_unconfirmed' });
    });

    it('rendering_disabled preempts a no_anchor banner (so Enable-rendering is reachable)', () => {
      // Both conditions true: no_anchor holds the slot, then a poll sees
      // exclude_geojson. rendering_disabled must take over - otherwise its
      // Enable-rendering action (gated on the banner kind) is unreachable.
      useSpeleoStore.setState({ banner: { kind: 'no_anchor' } });
      useSpeleoStore.getState().seedFromStatus(status({ exclude_geojson: true }));
      expect(useSpeleoStore.getState().banner).toEqual({ kind: 'rendering_disabled' });
    });

    it('a no_anchor notice is never clobbered by a poll when rendering is enabled', () => {
      // exclude_geojson off: a no_anchor notice must survive routine polls (only a
      // rendering_disabled banner that WE own is cleared when the flag goes off).
      useSpeleoStore.setState({ banner: { kind: 'no_anchor' } });
      useSpeleoStore.getState().seedFromStatus(status({ exclude_geojson: false }));
      expect(useSpeleoStore.getState().banner).toEqual({ kind: 'no_anchor' });
    });

    it('enableRendering clears the rendering-disabled banner and PATCHes exclude=false', async () => {
      useSpeleoStore.setState({ banner: { kind: 'rendering_disabled' } });
      // set_exclude_geojson returns null; the follow-up refreshLinkStatus reads status.
      invokeIpcMock.mockImplementation(async (cmd: string) =>
        cmd === 'speleodb_link_status' ? status({ exclude_geojson: false }) : null,
      );
      await useSpeleoStore.getState().enableRendering();
      expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_set_exclude_geojson', {
        exclude: false,
        expectedProject: null,
      });
      expect(useSpeleoStore.getState().banner).toBeNull();
    });

    it('enableRendering rethrows and leaves the banner intact so the caller can surface it', async () => {
      useSpeleoStore.setState({ banner: { kind: 'rendering_disabled' } });
      invokeIpcMock.mockRejectedValue(new Error('403'));
      await expect(useSpeleoStore.getState().enableRendering()).rejects.toThrow('403');
      // The banner must stay up so SpeleoBanner keeps itself open to show the error.
      expect(useSpeleoStore.getState().banner).toEqual({ kind: 'rendering_disabled' });
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
        session: { loggedIn: false, userEmail: null, endpoint: null, keychainBacked: false, storage: null, keychain: 'not_attempted' },
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
        session: { loggedIn: false, userEmail: null, endpoint: null, keychainBacked: false, storage: null, keychain: 'not_attempted' },
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
            session: { loggedIn: false, userEmail: null, endpoint: null, keychainBacked: false, storage: null, keychain: 'not_attempted' },
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

    // #270: `applyPendingAuth`'s logout branch is the only logout path. Its dead twin
    // swallowed a keychain-delete failure, so the user was never warned a credential
    // might still be stored on the machine.
    it('exposes no second logout path that could swallow a keychain failure', () => {
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
          forceEditing: false,
          forceEditAllowed: false,
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
      'speleodb-close',
      'speleodb-quit',
      'speleodb-revert',
      'speleodb-unwound-push',
    ])('covers %s', (id) => {
      expect(PUSH_FAILURE_DIALOGS).toContain(id);
    });
  });
});

describe('speleoStore - the dump picker reports its own failures (#258)', () => {
  it('keeps the picker open and records why, instead of rejecting into nothing', async () => {
    // The retry runs from the DIALOG - its caller invokes this with `void` and no
    // host receives an outcome - so a throw used to escape as an unhandled rejection
    // and leave the picker sitting there saying nothing about why confirming failed.
    const candidates = [
      { entry_path: 'a.stalx', size_bytes: 1, modified: null },
      { entry_path: 'b.stalx', size_bytes: 2, modified: null },
    ];
    useSpeleoStore
      .getState()
      .openDumpPicker(candidates, 'stalx', async () => {
        throw new Error('the session expired while you were choosing');
      });

    await useSpeleoStore.getState().resolveDumpPicker('a.stalx');

    const s = useSpeleoStore.getState();
    expect(s.dumpPickerOpen).toBe(true);
    expect(s.dumpPickerBusy).toBe(false);
    expect(s.dumpPickerError).toContain('the session expired');
    // The candidates survive, so the user can simply answer again.
    expect(s.dumpPickerCandidates).toEqual(candidates);
  });

  it('clears a previous failure when the next attempt starts', async () => {
    const candidates = [
      { entry_path: 'a.stalx', size_bytes: 1, modified: null },
      { entry_path: 'b.stalx', size_bytes: 2, modified: null },
    ];
    useSpeleoStore.getState().openDumpPicker(candidates, 'stalx', async () => {});
    useSpeleoStore.setState({ dumpPickerError: 'stale' });

    await useSpeleoStore.getState().resolveDumpPicker('b.stalx');

    expect(useSpeleoStore.getState().dumpPickerOpen).toBe(false);
    expect(useSpeleoStore.getState().dumpPickerError).toBeNull();
  });

  it('[#258 review] releases the displaced owner when a SECOND flow raises a picker', async () => {
    // Two flows really can be in flight: the revision history disables View while its
    // own download runs but leaves Revert live, so a revert probe can raise a picker
    // and the older View download can then raise another over it. The displaced revert
    // keeps `busy` deliberately - its cancel is what releases it - so dropping that
    // callback left a dialog with every control and every dismissal route disabled.
    const revertCancel = vi.fn();
    useSpeleoStore
      .getState()
      .openDumpPicker(
        [{ entry_path: 'a.stalx', size_bytes: 1, modified: null }],
        'stalx',
        async () => {},
        revertCancel,
      );

    // The older download lands and takes the picker over.
    useSpeleoStore
      .getState()
      .openDumpPicker(
        [{ entry_path: 'b.stalx', size_bytes: 2, modified: null }],
        'stalx',
        async () => {},
      );

    expect(revertCancel).toHaveBeenCalledTimes(1);
    // ...and the newcomer owns the picker, undisturbed.
    expect(useSpeleoStore.getState().dumpPickerOpen).toBe(true);
    expect(useSpeleoStore.getState().dumpPickerCandidates[0]?.entry_path).toBe('b.stalx');
  });

  it('[#258 review] does NOT release the flow when its OWN re-raise replaces it', async () => {
    // The other half, and why this cannot just always fire. A retry that meets a fresh
    // ambiguity re-raises from inside the same suspended flow; releasing its cancel
    // there would un-suspend it out from under the picker that just replaced it.
    const cancel = vi.fn();
    const raise = (path: string) =>
      useSpeleoStore
        .getState()
        .openDumpPicker(
          [{ entry_path: path, size_bytes: 1, modified: null }],
          'stalx',
          async () => {},
          cancel,
          'flow-a',
        );

    raise('a.stalx');
    raise('c.stalx');

    expect(cancel).not.toHaveBeenCalled();
    expect(useSpeleoStore.getState().dumpPickerCandidates[0]?.entry_path).toBe('c.stalx');
  });

  it('[#258 review] tells continuation from displacement by OWNER, not by timing', async () => {
    // Codex's interleaving, and the reason the first attempt at this was wrong. That
    // one marked a picker as a continuation when a retry happened to be in flight -
    // a process-wide window, not a relationship. So a picker raised by a DIFFERENT
    // flow during that window, and then displaced when the retry re-offered, had its
    // cancel suppressed as if it were the retry's own re-raise. It is the one dialog
    // that was actually suspended, and the cancel was the only thing that released it.
    const revertCancel = vi.fn();
    let releaseRetry = () => {};
    const retryRan = new Promise<void>((resolve) => {
      releaseRetry = resolve;
    });

    // A View picker: no owner, no cancel - it suspends nothing.
    useSpeleoStore.getState().openDumpPicker(
      [{ entry_path: 'view.stalx', size_bytes: 1, modified: null }],
      'stalx',
      async () => {
        // ... while its retry is out, the revert raises its own question and
        // suspends on the answer.
        useSpeleoStore
          .getState()
          .openDumpPicker(
            [{ entry_path: 'revert.stalx', size_bytes: 2, modified: null }],
            'stalx',
            async () => {},
            revertCancel,
            'speleodb-revert',
          );
        await retryRan;
        // ... and then the View's retry meets a moved tree and re-offers, taking the
        // overlay back from a revert that is still waiting to be released.
        useSpeleoStore
          .getState()
          .openDumpPicker(
            [{ entry_path: 'view2.stalx', size_bytes: 3, modified: null }],
            'stalx',
            async () => {},
          );
      },
      undefined,
      undefined,
    );

    const resolving = useSpeleoStore.getState().resolveDumpPicker('view.stalx');
    releaseRetry();
    await resolving;

    expect(revertCancel).toHaveBeenCalledTimes(1);
    expect(useSpeleoStore.getState().dumpPickerCandidates[0]?.entry_path).toBe('view2.stalx');
  });

  it('lets a retry that lost its picker settle without touching the one that replaced it', async () => {
    // Confirming is a network round trip of unbounded length, so the picker it began
    // on can be gone by the time it settles - closed, re-raised on a moved tree, or
    // torn down by a session loss. Writing the outcome anyway landed it on a
    // DIFFERENT dialog: the loser's error overwrote the newcomer's, and its `finally`
    // re-enabled the newcomer's buttons in the middle of its own confirm.
    const first = [
      { entry_path: 'a.stalx', size_bytes: 1, modified: null },
      { entry_path: 'b.stalx', size_bytes: 2, modified: null },
    ];
    const second = [
      { entry_path: 'c.stalx', size_bytes: 3, modified: null },
      { entry_path: 'd.stalx', size_bytes: 4, modified: null },
    ];
    let release = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    useSpeleoStore.getState().openDumpPicker(first, 'stalx', async () => {
      await gate;
      throw new Error('the loser finally settled');
    });

    const inFlight = useSpeleoStore.getState().resolveDumpPicker('a.stalx');
    expect(useSpeleoStore.getState().dumpPickerBusy).toBe(true);

    // A second picker takes over while the first confirm is still in flight, and is
    // itself mid-confirm with a failure of its own already on screen.
    useSpeleoStore.getState().openDumpPicker(second, 'stalx', async () => {});
    useSpeleoStore.setState({ dumpPickerBusy: true, dumpPickerError: 'the new one is working' });

    release();
    await inFlight;

    const s = useSpeleoStore.getState();
    expect(s.dumpPickerCandidates).toEqual(second);
    expect(s.dumpPickerError).toBe('the new one is working');
    expect(s.dumpPickerBusy).toBe(true);
    expect(s.dumpPickerOpen).toBe(true);
  });

  it('still clears busy when the picker it owns succeeds', async () => {
    // The ownership guard moved the un-busy off the unconditional `finally`, so the
    // ordinary close has to carry it - or a successful pick would leave the next
    // picker's buttons disabled from the moment it opened.
    const candidates = [
      { entry_path: 'a.stalx', size_bytes: 1, modified: null },
      { entry_path: 'b.stalx', size_bytes: 2, modified: null },
    ];
    useSpeleoStore.getState().openDumpPicker(candidates, 'stalx', async () => {});

    await useSpeleoStore.getState().resolveDumpPicker('a.stalx');

    expect(useSpeleoStore.getState().dumpPickerBusy).toBe(false);
    expect(useSpeleoStore.getState().dumpPickerOpen).toBe(false);
  });
});
