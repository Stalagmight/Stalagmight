import type { MockCommandFamily, MockReply } from './types';

/**
 * SpeleoDB commands of the Playwright IPC mock: deterministic logged-out / local defaults.
 *
 * Split out of `../tauri-ipc.ts` unchanged (#576 E8). Runs IN THE PAGE: `setupTauriMocks`
 * serializes this factory with `Function.prototype.toString` into its init script, so it
 * may use only `ctx` and browser globals - never an import (type-only imports are erased).
 */
export const speleodbCommands: MockCommandFamily = (ctx) => {
  return (cmd, _args) => {
    switch (cmd) {
      // SpeleoDB (#203/#204/#205): deterministic logged-out / local defaults so
      // the settings tab + browser render without a live server. No e2e spec
      // drives the hand-off yet; these keep the mounts from erroring on null.
      case 'speleodb_session':
        return {
          logged_in: false,
          user_email: null,
          endpoint: null,
          keychain_backed: false,
          // No login this session, so nothing has touched the keychain - which
          // is exactly what the UI must be able to say without probing.
          storage: null,
          keychain: 'not_attempted',
        } satisfies MockReply<'speleodb_session'>;
      // The credential-storage verbs both answer with a refreshed session. A
      // mocked run is signed out, so they echo the logged-out shape above
      // rather than inventing a session the rest of the mock does not have.
      case 'speleodb_set_token_storage':
      case 'speleodb_retry_keychain':
        return {
          logged_in: false,
          user_email: null,
          endpoint: null,
          keychain_backed: false,
          storage: null,
          keychain: 'not_attempted',
        } satisfies MockReply<'speleodb_set_token_storage'> & MockReply<'speleodb_retry_keychain'>;
      case 'speleodb_link_status':
        // Full SpeleoLinkStatus shape (remote_state / access_denied / server_error
        // included) so a spec that flips is_remote:true still matches the DTO.
        return {
          is_remote: false,
          edit_enabled: true,
          has_unpushed_edits: false,
          permission: null,
          lock_holder: null,
          remote_state: 'unknown',
          access_denied: false,
          server_error: null,
          exclude_geojson: false,
          // [#303] Dirty ONLY because a chain step walked this copy back to an older
          // revision - the user has edited nothing. Every consumer composes it with
          // has_unpushed_edits (`isUnwoundNotEdited`), and it is what makes the close
          // and quit prompts stop calling an old copy "unpushed changes" and stop
          // pre-focusing the push. It was missing here, so it read as `undefined`
          // rather than false and no spec could reach the state at all - the state
          // this whole feature is built around.
          unwound: false,
          // [#303] The hybrid undo stack's policy. Full is the default - the shape a
          // spec sees unless it overrides this.
          undo_mode: 'full',
          // #213 / #218: no force-edit session, the opt-in off, no remote project open.
          force_editing: false,
          allow_force_edit: false,
          project_id: null,
          // A spec opts into a different link state by assigning this before the app
          // asks; anything it carries wins over the defaults above.
          ...((window as unknown as Record<string, unknown>)
            .__speleoLinkStatusOverrides as Record<string, unknown> | undefined),
        } satisfies MockReply<'speleodb_link_status'>;
      case 'speleodb_open_file_flags':
        // [#576 NF-W2F-1] The open file's flags, re-asked after an in-place tree swap.
        // The mock never swaps a tree in place, so the open file is whatever the last
        // open said: an ordinary current-format file unless a spec overrides it.
        return {
          newerSchemaVersion: null,
          speleoLinkUnreadable: false,
          unreadableProjectSettings: [],
          ...((window as unknown as Record<string, unknown>)
            .__openFileFlagsOverrides as Record<string, unknown> | undefined),
        } satisfies MockReply<'speleodb_open_file_flags'>;
      case 'speleodb_list_projects':
        return [] satisfies MockReply<'speleodb_list_projects'>;
      // #270: local (un-pushed) state per project id, keyed by id. Nothing is
      // cached in a mocked run, so every list renders without a local-edit
      // accent; specs that need one override this per spec.
      case 'speleodb_local_project_states':
        return {} satisfies MockReply<'speleodb_local_project_states'>;
      case 'speleodb_take_remote':
        // Returns the reloaded .stalx path (#198); no e2e spec drives it yet.
        return '/mock/project.stalx' satisfies MockReply<'speleodb_take_remote'>;
      // [#576 FZ-SDB-5] The move out of the cache answers with the copy's SaveOutcome:
      // the app loads `saved_path` next, so it names a path under the mock documents dir.
      case 'speleodb_unlink':
        return {
          saved_path: '/mock/documents/stalagmight/unlinked.stalx',
          unlinked_copy_path: null,
          warnings: [],
        } satisfies MockReply<'speleodb_unlink'>;
      // [#576 Q-W61-3-D1] Every release landed: no lock left held to warn about. A
      // spec seeds a failed one with an override.
      case 'speleodb_logout':
        return { locks_left_held: [] } satisfies MockReply<'speleodb_logout'>;
      // [#576 SIGNOUT-PROMPT-D1] No lock held, so no sign-out prompt. A spec seeds
      // held locks with an override to raise it.
      case 'speleodb_sign_out_locks':
        return [] satisfies MockReply<'speleodb_sign_out_locks'>;
      case 'speleodb_sign_in_hand_back_locks':
        return [] satisfies MockReply<'speleodb_sign_in_hand_back_locks'>;
      // Strip half of the managed-file prompt: deletes a foreign copy's
      // speleo_link row. No payload; specs assert the invocation itself.
      case 'speleodb_remove_link_at':
      case 'speleodb_release_all_locks':
      case 'speleodb_release_lock':
      // [#258 review] The provenance-scoped unwind. No payload, and no argument:
      // it releases whatever THIS process recorded acquiring, or nothing.
      case 'speleodb_release_lock_acquired_here':
      case 'speleodb_set_exclude_geojson':
      // [#303] The per-project undo-history policy write: no payload. The FE
      // re-reads `speleodb_link_status` afterwards for the new value.
      case 'speleodb_set_undo_mode':
      // #215 lock release + delete: no payload.
      case 'speleodb_release_current_lock':
      case 'speleodb_delete_project':
        return null satisfies MockReply<'speleodb_remove_link_at'> &
          MockReply<'speleodb_release_all_locks'> &
          MockReply<'speleodb_release_lock'> &
          MockReply<'speleodb_release_lock_acquired_here'> &
          MockReply<'speleodb_set_exclude_geojson'> &
          MockReply<'speleodb_set_undo_mode'> &
          MockReply<'speleodb_release_current_lock'> &
          MockReply<'speleodb_delete_project'>;
      // [#303] Which revision a chain step would load, backward and forward. `null`
      // by default - a mocked project has no revision history to walk - so neither
      // dropdown shows its step entry unless a spec overrides this.
      case 'speleodb_unwind_target':
      case 'speleodb_rewind_target':
        return null satisfies MockReply<'speleodb_unwind_target'> & MockReply<'speleodb_rewind_target'>;
      // [#303] Discard the local copy and re-lock. Mocked as a plain successful
      // acquire: the specs that reach it care that the app returns to an editing
      // state, not about the download it does on the way.
      case 'speleodb_discard_changes':
        return { kind: 'editing' } satisfies MockReply<'speleodb_discard_changes'>;
      // #215 collaborator management: the roster (users + teams). The mutating
      // commands echo the refreshed roster; the picker lists no teams by default.
      case 'speleodb_list_collaborators':
      case 'speleodb_add_collaborator':
      case 'speleodb_set_collaborator':
      case 'speleodb_remove_collaborator':
      case 'speleodb_add_team':
      case 'speleodb_set_team':
      case 'speleodb_remove_team':
        return { users: [], teams: [] } satisfies MockReply<'speleodb_list_collaborators'> &
          MockReply<'speleodb_add_collaborator'> &
          MockReply<'speleodb_set_collaborator'> &
          MockReply<'speleodb_remove_collaborator'> &
          MockReply<'speleodb_add_team'> &
          MockReply<'speleodb_set_team'> &
          MockReply<'speleodb_remove_team'>;
      case 'speleodb_list_teams':
        return [] satisfies MockReply<'speleodb_list_teams'>;
      case 'speleodb_search_users':
        return [] satisfies MockReply<'speleodb_search_users'>;
      case 'speleodb_get_project_metadata':
      case 'speleodb_patch_project':
        // Deterministic project metadata for the #206 settings tab. patch echoes
        // the same shape (a real server would return post-edit truth).
        return {
          name: 'Mock Cave',
          description: '',
          country: 'US',
          visibility: 'PRIVATE',
          latitude: null,
          longitude: null,
          exclude_geojson: false,
        } satisfies MockReply<'speleodb_get_project_metadata'> & MockReply<'speleodb_patch_project'>;
      case 'speleodb_publish':
        // #207 publish result: the reloaded (now-remote) local .stalx path, with
        // what publish's saves could not keep (#576 NF-W4M-2: nothing here).
        return {
          result: {
            project_id: 'mock-project-id',
            endpoint: 'https://speleodb.example',
            local_path: '/mock/project.stalx',
          },
          save_warnings: [],
        } satisfies MockReply<'speleodb_publish'>;
      case 'speleodb_login':
        return {
          user_email: 'test@example.com',
          endpoint: 'https://speleodb.example',
          keychain_backed: true,
          storage: 'keychain',
          keychain: 'available',
          needs_storage_choice: false,
          // [#576 FZ-SDB-3-D1] A mocked run starts signed out, so no session is replaced.
          replaced_session: false,
          locks_left_held: [],
        } satisfies MockReply<'speleodb_login'>;

      // The write workflow (#576 E3): canned happy-path answers - the open lands on the
      // mock's cached copy, a push/done is accepted, an update finds nothing new, and the
      // project has no revisions or dump candidates. The lock, divergence and cache
      // semantics behind them are the backend's (crates/speleodb, core/state/speleodb*.rs
      // and the env-gated live tests, docs/speleodb-dev-testing.md); a spec that needs
      // another outcome seeds it with __MOCK_IPC_OVERRIDES__.
      case 'speleodb_open_project':
      case 'speleodb_open_project_discarding_local':
      case 'speleodb_restore_revision':
        return { local_path: '/mock/project.stalx', concurrent_session: false } satisfies MockReply<'speleodb_open_project'> &
          MockReply<'speleodb_open_project_discarding_local'> &
          MockReply<'speleodb_restore_revision'>;
      case 'speleodb_adopt_ariane':
        return {
          local_path: '/mock/project.stalx',
          type_flipped: false,
          concurrent_session: false,
          upload: { kind: 'pushed' },
        } satisfies MockReply<'speleodb_adopt_ariane'>;
      case 'speleodb_push':
        return { result: { kind: 'pushed' }, save_warnings: [] } satisfies MockReply<'speleodb_push'>;
      case 'speleodb_done':
        return { result: { kind: 'released' }, save_warnings: [] } satisfies MockReply<'speleodb_done'>;
      case 'speleodb_start_editing':
        return { kind: 'editing' } satisfies MockReply<'speleodb_start_editing'>;
      case 'speleodb_update':
        return { kind: 'up_to_date' } satisfies MockReply<'speleodb_update'>;
      case 'speleodb_force_edit':
        return null satisfies MockReply<'speleodb_force_edit'>;
      case 'speleodb_get_revisions':
        return [] satisfies MockReply<'speleodb_get_revisions'>;
      case 'speleodb_dump_candidates':
        return null satisfies MockReply<'speleodb_dump_candidates'>;

      default:
        return ctx.unhandled;
    }
  };
};
