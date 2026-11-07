import { describe, it, expect } from 'vitest';
import { invokeIpc, type QueuedCallArgs } from '../ipc';
import { IPC_EVENTS, listenIpc } from '../ipcEvents';
import type { BatchCommand } from '../../types/commands';
import type { EditOutcome } from '../../types/ipc/EditOutcome';
import type { ProjectState } from '../../types/ipc/ProjectState';

/**
 * The IPC typing contract (#576 C3, C13), checked by `tsc` (`pnpm tsc --noEmit`
 * covers this file): command names, argument objects and result types come from
 * the manifest generated out of `generate_handler!`, event names and payloads
 * from `core::events`. Every `@ts-expect-error` below is a mistake that used to
 * compile - `invokeIpc` took any string and a caller-asserted result type, the
 * hand-written `BatchCommand` was the only statement of the nested keys, and
 * `listen()` took any event name. Never called: the calls would hit the real
 * Tauri bridge.
 */
function typeContract(): void {
  // @ts-expect-error not a registered command
  void invokeIpc('no_such_command');
  // @ts-expect-error top-level arguments are camelCase (Tauri renames them)
  void invokeIpc('delete_section', { section_id: 1 });
  // @ts-expect-error a required argument is missing
  void invokeIpc('delete_section', {});
  // @ts-expect-error an argument of the wrong type
  void invokeIpc('delete_section', { sectionId: 'one' });
  const deleted: Promise<EditOutcome> = invokeIpc('delete_section', { sectionId: 1 });
  // @ts-expect-error a command's success value is its own outcome: delete_section has no section_id
  const deletedId: Promise<{ section_id: number }> = invokeIpc('delete_section', { sectionId: 1 });
  // @ts-expect-error get_project_state resolves with ProjectState | null
  const state: Promise<ProjectState> = invokeIpc('get_project_state');
  // An `Option` argument may be omitted.
  void invokeIpc('close_project', {});
  // @ts-expect-error a nested batch payload keeps the Rust snake_case keys
  const batch: BatchCommand = { type: 'update_shot', sectionId: 1, shot_id: 2, updates: {} };
  // @ts-expect-error not an event the backend declares
  void listenIpc('no-such-event', () => {});
  void listenIpc(IPC_EVENTS.LOADING_PROGRESS, (event) => {
    // @ts-expect-error the loading id is a plain number on the wire (#576 C6)
    const id: bigint = event.payload.loadingId;
    void id;
  });
  void deleted;
  void deletedId;
  void state;
  void batch;
}

/**
 * [#576 RV5-PI-6] The mutation queue's entry (`executeCommand` / `executeUndoRedo`
 * in projectStore) types its args with `QueuedCallArgs<K>`. It used to be
 * `IpcCallArgs<K> | undefined` for every command, so a command with a required
 * argument could be queued with none and tsc stayed green, while `invokeIpc`
 * itself refuses that. Never called.
 *
 * [#576 RV6a-12] Each `@ts-expect-error` below guards its own regression: `none`
 * fails if the args become omittable for every command again (the RV5-PI-6
 * typing); `empty` fails if they become partial (`Partial<IpcCallArgs<K>>` for a
 * command with required arguments), which the RV5-PI-6 typing did not do.
 */
function queueContract(): void {
  // @ts-expect-error delete_section takes a required sectionId: it cannot be queued with no args
  const none: QueuedCallArgs<'delete_section'> = undefined;
  // @ts-expect-error the queue takes the same required keys invokeIpc does: none may be left out
  const empty: QueuedCallArgs<'delete_section'> = {};
  const given: QueuedCallArgs<'delete_section'> = { sectionId: 1 };
  // A command with no required argument may still be queued with none.
  const undo: QueuedCallArgs<'undo'> = undefined;
  void none;
  void empty;
  void given;
  void undo;
}

describe('IPC typing contract (#576 C3, C13)', () => {
  it('is enforced by tsc, not at runtime', () => {
    expect(typeof typeContract).toBe('function');
    expect(typeof queueContract).toBe('function');
  });
});
