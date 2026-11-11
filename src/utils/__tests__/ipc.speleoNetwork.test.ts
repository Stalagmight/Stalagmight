import { beforeEach, describe, expect, it, vi } from 'vitest';

const pending = new Map<string, { resolve: (v: unknown) => void; reject: (e: unknown) => void }>();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(
    (command: string) =>
      new Promise((resolve, reject) => {
        pending.set(command, { resolve, reject });
      }),
  ),
}));
vi.mock('../telemetry', () => ({ trackCommand: vi.fn() }));

import { invokeIpc } from '../ipc';
import { speleoNetworkKind } from '../speleoNetworkCommands';
import { useSpeleoNetworkStore } from '../../store/speleoNetworkStore';

const counts = () => {
  const s = useSpeleoNetworkStore.getState();
  return { inFlight: s.inFlight, exclusive: s.exclusiveInFlight };
};

beforeEach(() => {
  pending.clear();
  useSpeleoNetworkStore.getState().reset();
});

describe('invokeIpc counts SpeleoDB network calls', () => {
  it('counts an upload as exclusive while it runs, then clears on success', async () => {
    const call = invokeIpc('speleodb_push', { message: 'Synthetic update' });
    expect(counts()).toEqual({ inFlight: 1, exclusive: 1 });
    pending.get('speleodb_push')!.resolve({ kind: 'pushed' });
    await call;
    expect(counts()).toEqual({ inFlight: 0, exclusive: 0 });
  });

  it('clears the count when the call fails', async () => {
    const call = invokeIpc('speleodb_start_editing');
    expect(counts().exclusive).toBe(1);
    pending.get('speleodb_start_editing')!.reject({ message: 'offline', action: 'Retry' });
    await expect(call).rejects.toEqual({ message: 'offline', action: 'Retry' });
    expect(counts()).toEqual({ inFlight: 0, exclusive: 0 });
  });

  it('counts a status refresh as a read, not a conflicting operation', async () => {
    const call = invokeIpc('speleodb_link_status');
    expect(counts()).toEqual({ inFlight: 1, exclusive: 0 });
    pending.get('speleodb_link_status')!.resolve({});
    await call;
    expect(counts().inFlight).toBe(0);
  });

  it('does not count local commands', async () => {
    const config = invokeIpc('get_config');
    const session = invokeIpc('speleodb_session');
    expect(counts()).toEqual({ inFlight: 0, exclusive: 0 });
    pending.get('get_config')!.resolve({});
    pending.get('speleodb_session')!.resolve({});
    await Promise.all([config, session]);
  });
});

describe('speleoNetworkKind', () => {
  it('classifies every lock, upload, download and account call as exclusive', () => {
    for (const command of [
      'speleodb_login', 'speleodb_logout', 'speleodb_open_project',
      'speleodb_open_project_discarding_local', 'speleodb_start_editing', 'speleodb_done',
      'speleodb_push', 'speleodb_update', 'speleodb_take_remote', 'speleodb_discard_changes',
      'speleodb_restore_revision', 'speleodb_release_lock', 'speleodb_release_lock_acquired_here',
      'speleodb_release_current_lock', 'speleodb_unlink', 'speleodb_publish',
      'speleodb_remove_cached_copy', 'speleodb_purge_cached_copies', 'speleodb_settle_open_lock',
    ]) {
      expect(speleoNetworkKind(command), command).toBe('exclusive');
    }
  });

  it('counts the exit / sign-out actions the backend exit waits for as exclusive', () => {
    // `close_project` releases (or keeps, or discards) the open project's lock and
    // the sign-out's discard removes a cached copy; the backend holds the app's exit
    // for both (`exit_wait.rs`), so the quit gate must count them too.
    expect(speleoNetworkKind('close_project', { releaseLock: false, discardLocal: false })).toBe('exclusive');
    expect(speleoNetworkKind('speleodb_discard_cached_copy', { projectId: 'x', discardUnpushed: true })).toBe('exclusive');
  });

  it('classifies server reads as read', () => {
    for (const command of [
      'speleodb_list_projects', 'speleodb_link_status', 'speleodb_get_revisions',
      'speleodb_unwind_target', 'speleodb_rewind_target', 'speleodb_get_project_metadata',
      'speleodb_project_access_at',
    ]) {
      expect(speleoNetworkKind(command), command).toBe('read');
    }
  });

  it('leaves purely local calls uncounted', () => {
    expect(speleoNetworkKind('speleodb_session')).toBeNull();
    expect(speleoNetworkKind('speleodb_local_project_states')).toBeNull();
    expect(speleoNetworkKind('speleodb_remove_link_at')).toBeNull();
    expect(speleoNetworkKind('speleodb_project_access_at', { path: '/x.stalx', localOnly: true })).toBeNull();
    expect(speleoNetworkKind('save_project')).toBeNull();
    expect(speleoNetworkKind('toString')).toBeNull();
  });
});
