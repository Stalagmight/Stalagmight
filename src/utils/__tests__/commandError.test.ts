/**
 * [#576 C4, A6, D22] The one IPC failure envelope and the one applier of the
 * SpeleoDB link bits, both in `invokeIpc`.
 *
 * Every fallible command rejects with the backend's `CommandError` (`{ code,
 * message, ...payload }`). `invokeIpc` re-throws it as an `IpcCommandError`, so the
 * message keeps reaching every surface the old string rejection reached
 * (`getErrorMessage`, `String(e)`, `${e}`, `instanceof Error`), while routing reads
 * the code. A survey mutation's answer - its outcome, or the CommandError of a
 * failure that still mutated - carries `remote_link`, and `invokeIpc` hands it to
 * the speleo store, unless the project changed while the call was in flight.
 *
 * The real `invokeIpc` runs here; only Tauri's `invoke` is mocked.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { getErrorMessage, invokeIpc } from '../ipc';
import { IpcCommandError, commandErrorOf, errorCode, toIpcError } from '../commandError';
import { refusedBy, refusedLocally } from '../../store/commandResult';
import { useSpeleoStore } from '../../store/speleoStore';
import type { SpeleoLinkStatus } from '../../types/ipc.generated';

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', async () => (await import('../../test/factories/tauri')).tauriCore(invokeMock));

const status = (o: Partial<SpeleoLinkStatus> = {}): SpeleoLinkStatus => ({
  is_remote: true,
  edit_enabled: true,
  has_unpushed_edits: false,
  unwound: false,
  permission: 'READ_AND_WRITE',
  lock_holder: null,
  remote_state: 'up_to_date',
  access_denied: false,
  server_error: null,
  exclude_geojson: false,
  undo_mode: 'full',
  force_editing: false,
  allow_force_edit: false,
  project_id: null,
  ...o,
});

const editOutcome = (remote_link: { has_unpushed_edits: boolean; unwound: boolean } | null) => ({
  warnings: [],
  remote_link,
});

/** Open a remote project at `path`, editing and clean, the way a load seeds it. */
function openRemote(path: string, o: Partial<SpeleoLinkStatus> = {}) {
  useSpeleoStore.getState().clearLink();
  useSpeleoStore.getState().syncProjectLink(true, path);
  useSpeleoStore.getState().seedFromStatus(status(o));
}

beforeEach(() => {
  invokeMock.mockReset();
  useSpeleoStore.getState().clearLink();
});

describe('IpcCommandError (#576 C4)', () => {
  it('is an Error whose message, toString and template string are the display message', () => {
    const e = new IpcCommandError({ code: 'EDIT_DISABLED', message: 'Move station is disabled.' });

    expect(e).toBeInstanceOf(Error);
    expect(e).toBeInstanceOf(IpcCommandError);
    expect(e.name).toBe('IpcCommandError');
    expect(e.code).toBe('EDIT_DISABLED');
    expect(e.message).toBe('Move station is disabled.');
    expect(e.toString()).toBe('Move station is disabled.');
    expect(String(e)).toBe('Move station is disabled.');
    expect(`${e}`).toBe('Move station is disabled.');
    expect(getErrorMessage(e)).toBe('Move station is disabled.');
  });

  it('carries every payload field under its wire name, and leaves an omitted one absent', () => {
    const e = new IpcCommandError({
      code: 'HISTORY_STEP_FAILED',
      message: 'Undo failed at step 2 of 3: boom',
      history_step: { failed_step: 2, total_steps: 3 },
      reversed_sections: [{ section_id: 4, shot_count: 7 }],
      remote_link: { has_unpushed_edits: true, unwound: false },
    });

    expect(e.history_step).toEqual({ failed_step: 2, total_steps: 3 });
    expect(e.reversed_sections).toEqual([{ section_id: 4, shot_count: 7 }]);
    expect(e.remote_link).toEqual({ has_unpushed_edits: true, unwound: false });
    expect('details' in e).toBe(false);
    expect('session_closed' in e).toBe(false);
  });
});

describe('the structural readers', () => {
  it('read the code off an IpcCommandError, a plain CommandError object and a refused outcome', () => {
    const plain = { code: 'FS_PERMISSION_DENIED', message: 'denied' } as const;

    expect(errorCode(new IpcCommandError(plain))).toBe('FS_PERMISSION_DENIED');
    expect(errorCode(plain)).toBe('FS_PERMISSION_DENIED');
    expect(errorCode(refusedBy(plain))).toBe('FS_PERMISSION_DENIED');
    expect(commandErrorOf(plain)).toBe(plain);
  });

  it('find no code on anything else', () => {
    expect(errorCode('EDIT_DISABLED: a plain-string rejection')).toBeNull();
    expect(errorCode(new Error('EDIT_DISABLED: an Error'))).toBeNull();
    expect(errorCode({ message: 'no code' })).toBeNull();
    expect(errorCode({ code: 42, message: 'not a string code' })).toBeNull();
    expect(errorCode(refusedLocally('The project changed before this action could run.'))).toBeNull();
    expect(errorCode(null)).toBeNull();
    expect(errorCode(undefined)).toBeNull();
    expect(commandErrorOf('a string')).toBeNull();
  });

  it('wrap only a CommandError-shaped rejection, once', () => {
    const wrapped = toIpcError({ code: 'FAILED', message: 'x' });
    expect(wrapped).toBeInstanceOf(IpcCommandError);
    expect(toIpcError(wrapped)).toBe(wrapped);
    expect(toIpcError('invalid args `sectionId` for command `delete_section`')).toBe(
      'invalid args `sectionId` for command `delete_section`',
    );
    const transport = new Error('transport down');
    expect(toIpcError(transport)).toBe(transport);
  });
});

describe('invokeIpc and a rejection', () => {
  it('re-throws a CommandError rejection as an IpcCommandError with its code and message', async () => {
    invokeMock.mockRejectedValue({ code: 'WORKSPACE_READ_ONLY', message: 'Move station is disabled: Plan.' });

    const caught = await invokeIpc('delete_section', { sectionId: 1 }).catch((e: unknown) => e);

    expect(caught).toBeInstanceOf(IpcCommandError);
    expect(errorCode(caught)).toBe('WORKSPACE_READ_ONLY');
    expect(getErrorMessage(caught)).toBe('Move station is disabled: Plan.');
  });

  it("leaves a plain-string rejection (Tauri's own argument errors) a string", async () => {
    invokeMock.mockRejectedValue('invalid args `sectionId` for command `delete_section`');

    const caught = await invokeIpc('delete_section', { sectionId: 1 }).catch((e: unknown) => e);

    expect(caught).toBe('invalid args `sectionId` for command `delete_section`');
  });
});

describe('invokeIpc applies the SpeleoDB link bits (#576 D22)', () => {
  it('on a resolve', async () => {
    openRemote('/caves/a.stalx');
    invokeMock.mockResolvedValue(editOutcome({ has_unpushed_edits: true, unwound: false }));

    await invokeIpc('delete_section', { sectionId: 1 });

    expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
  });

  it('on a reject that still mutated', async () => {
    openRemote('/caves/a.stalx', { has_unpushed_edits: true, unwound: true });
    invokeMock.mockRejectedValue({
      code: 'HISTORY_STEP_FAILED',
      message: 'Undo failed at step 2 of 2: boom',
      history_step: { failed_step: 2, total_steps: 2 },
      remote_link: { has_unpushed_edits: true, unwound: false },
    });

    await expect(invokeIpc('undo_to', { index: 1 })).rejects.toBeInstanceOf(IpcCommandError);

    expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
    expect(useSpeleoStore.getState().link?.unwound).toBe(false);
  });

  it('ignores a null remote_link (a local project) and an answer without one', async () => {
    openRemote('/caves/a.stalx');
    const before = useSpeleoStore.getState().link;
    invokeMock.mockResolvedValueOnce(editOutcome(null));
    await invokeIpc('delete_section', { sectionId: 1 });
    invokeMock.mockResolvedValueOnce(true);
    await invokeIpc('can_undo');
    invokeMock.mockRejectedValueOnce({ code: 'FAILED', message: 'no bits on this one' });
    await invokeIpc('delete_section', { sectionId: 1 }).catch(() => undefined);

    expect(useSpeleoStore.getState().link).toBe(before);
  });

  it('ignores a reply that lands after the project (its link path) changed', async () => {
    openRemote('/caves/a.stalx');
    let answer!: (value: unknown) => void;
    invokeMock.mockImplementation(
      () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    );
    const call = invokeIpc('delete_section', { sectionId: 1 });
    // The user opens another remote project while the edit is in flight.
    openRemote('/caves/b.stalx');
    const generation = useSpeleoStore.getState().chainTargetGeneration;
    answer(editOutcome({ has_unpushed_edits: true, unwound: false }));
    await call;

    expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);
    expect(useSpeleoStore.getState().chainTargetGeneration).toBe(generation);
  });

  it('preserves the link identity when nothing changes, and bumps chainTargetGeneration when anything does', async () => {
    openRemote('/caves/a.stalx');
    const clean = useSpeleoStore.getState().link;
    const generation = useSpeleoStore.getState().chainTargetGeneration;

    invokeMock.mockResolvedValueOnce(editOutcome({ has_unpushed_edits: false, unwound: false }));
    await invokeIpc('delete_section', { sectionId: 1 });
    expect(useSpeleoStore.getState().link).toBe(clean);
    expect(useSpeleoStore.getState().chainTargetGeneration).toBe(generation);

    invokeMock.mockResolvedValueOnce(editOutcome({ has_unpushed_edits: true, unwound: false }));
    await invokeIpc('delete_section', { sectionId: 1 });
    expect(useSpeleoStore.getState().link).not.toBe(clean);
    expect(useSpeleoStore.getState().chainTargetGeneration).toBe(generation + 1);
  });
});
