import { invoke } from '@tauri-apps/api/core';
import { trackCommand } from './telemetry';
import { trackInvoke } from './debugLockup';
import { IpcCommandError, toIpcError } from './commandError';
import { speleoState } from '../store/speleoStoreHandle';
import type { IpcCommands } from '../types/ipc/IpcCommands';
import type { RemoteLinkBits } from '../types/ipc/RemoteLinkBits';

const IPC_TRACE = import.meta.env.VITE_IPC_TRACE === 'true';

/** Commands that should not be tracked (telemetry's own IPC calls). */
const UNTRACKED_COMMANDS: ReadonlySet<string> = new Set([
  'track_events',
  'flush_telemetry',
  'is_telemetry_available',
  'set_telemetry_enabled',
] satisfies IpcCommandName[]);

/** Every command the backend registers (`generate_handler!`), per the generated manifest (#576 C3). */
export type IpcCommandName = keyof IpcCommands;
/** The arguments `command` takes: camelCase keys, `Option` arguments omittable. */
export type IpcArgs<K extends IpcCommandName> = IpcCommands[K]['args'];
/** What `command` resolves with. */
export type IpcResult<K extends IpcCommandName> = IpcCommands[K]['result'];

type OptionalKey<T, P extends keyof T> = Partial<Pick<T, P>> extends Pick<T, P> ? true : false;
/** On the wire an omitted key and an explicit `undefined` are the same (JSON drops it). */
type WireArgs<T> = { [P in keyof T]: OptionalKey<T, P> extends true ? T[P] | undefined : T[P] };

/** `args` may be left out when the command takes none, or only optional ones. */
type IpcRest<K extends IpcCommandName> =
  Record<string, never> extends IpcArgs<K>
    ? [args?: WireArgs<IpcArgs<K>> | undefined, meta?: IpcMeta]
    : [args: WireArgs<IpcArgs<K>>, meta?: IpcMeta];

/** The args object `command` takes, as a caller writes it (an omittable key may be `undefined`). */
export type IpcCallArgs<K extends IpcCommandName> = WireArgs<IpcArgs<K>>;

/**
 * [#576 RV5-PI-6] The args a caller hands a dispatcher that forwards `command` (the
 * mutation queue's `executeCommand` / `executeUndoRedo`): omittable only when the
 * command takes no required argument - the rule {@link IpcRest} gives `invokeIpc`.
 */
export type QueuedCallArgs<K extends IpcCommandName> =
  Record<string, never> extends IpcArgs<K> ? IpcCallArgs<K> | undefined : IpcCallArgs<K>;

/**
 * The survey mutations projectStore's mutation queue runs: every command whose outcome
 * carries its validation warnings and the SpeleoDB link bits (`remote_link`, #576 D22).
 * (`repair_project_integrity` answers the link bits too, but no warnings: it is not
 * run through the queue.)
 */
export type MutationCommand = {
  [K in IpcCommandName]: IpcResult<K> extends { warnings: unknown[]; remote_link: RemoteLinkBits | null }
    ? K
    : never;
}[IpcCommandName];

/** Tauri plugin commands (`plugin:<name>|<command>`) are not in the manifest: untyped. */
type PluginCommand = `plugin:${string}|${string}`;

export type IpcMeta = {
  purpose?: string;
  fields?: Record<string, unknown>;
  level?: 'debug' | 'info';
};

function nowMs(): number {
  if (typeof performance !== 'undefined' && typeof performance.now === 'function') {
    return performance.now();
  }
  return Date.now();
}

function errorToString(err: unknown): string {
  if (err instanceof IpcCommandError) return `${err.name} ${err.code}: ${err.message}`;
  if (err instanceof Error) return `${err.name}: ${err.message}`;
  return String(err);
}

/**
 * [#576 D22] Hand the SpeleoDB link bits a survey mutation answered with - on its
 * outcome, or on the CommandError of a failure that still mutated - to the speleo
 * store, the one place they are applied. `null` (or no field) is a local project or
 * a command that is no survey mutation: nothing to do.
 */
function applyRemoteLink(answer: unknown, linkPathAtIssue: string | null): void {
  if (typeof answer !== 'object' || answer === null || !('remote_link' in answer)) return;
  const bits = (answer as { remote_link?: RemoteLinkBits | null }).remote_link;
  if (bits) speleoState()?.applyRemoteLink(bits, linkPathAtIssue);
}

/**
 * The transport both implementations share: `invoke`, then the one applier of the
 * link bits (on a resolve and on a reject), and a CommandError rejection re-thrown as
 * an `IpcCommandError` (#576 C4). The speleo store's `linkPath` is read when the call
 * is ISSUED: a reply for a project the user has since left must not touch the one
 * now open, and `applyRemoteLink` drops it when the two differ.
 */
async function send(command: string, args: Record<string, unknown> | undefined): Promise<unknown> {
  const linkPathAtIssue = speleoState()?.linkPath ?? null;
  let answer: unknown;
  try {
    answer = await trackInvoke(command, invoke<unknown>(command, args));
  } catch (e) {
    const err = toIpcError(e);
    applyRemoteLink(err, linkPathAtIssue);
    throw err;
  }
  applyRemoteLink(answer, linkPathAtIssue);
  return answer;
}

/**
 * Typed by the generated command manifest (#576 C3): the command name must be one
 * the backend registers, `args` must match its `#[tauri::command]` parameters,
 * and the promise resolves with the type the command returns - no caller-asserted
 * result type.
 */
export type InvokeIpc = {
  (command: PluginCommand, args?: Record<string, unknown>, meta?: IpcMeta): Promise<unknown>;
  <K extends IpcCommandName>(command: K, ...rest: IpcRest<K>): Promise<IpcResult<K>>;
};

/** The untyped transport both implementations share. */
type RawInvoke = (command: string, args?: Record<string, unknown>, meta?: IpcMeta) => Promise<unknown>;

const invokeWithTrace: RawInvoke = async (command, args, meta) => {
  if (!UNTRACKED_COMMANDS.has(command)) trackCommand(command);
  const start = nowMs();
  const purpose = meta?.purpose ?? 'unspecified purpose';
  const fields = meta?.fields ?? {};
  const logStart = meta?.level === 'info' ? console.info : console.debug;

  logStart(`[IPC] invoking ${command} for ${purpose}`, { command, purpose, ...fields });

  try {
    const result = await send(command, args);
    const durationMs = Math.round(nowMs() - start);
    console.debug(`[IPC] success ${command}`, { command, purpose, duration_ms: durationMs, ...fields });
    return result;
  } catch (err) {
    const durationMs = Math.round(nowMs() - start);
    console.error(`[IPC] failed ${command} for ${purpose}`, {
      command,
      purpose,
      duration_ms: durationMs,
      reason: errorToString(err),
      ...fields,
    });
    throw err;
  }
};

const invokeNoTrace: RawInvoke = (command, args, _meta) => {
  if (!UNTRACKED_COMMANDS.has(command)) trackCommand(command);
  return send(command, args);
};

/**
 * Invoke a Tauri IPC command with optional trace logging.
 * Logs are compiled out in production unless VITE_IPC_TRACE is enabled.
 *
 * A command that fails rejects with an `IpcCommandError` (its `code` is what to route
 * on, see `commandError.ts`); a plain-string rejection (Tauri could not deserialize
 * the arguments) stays a string. A survey mutation's SpeleoDB link bits are applied
 * here before the caller sees the answer (#576 D22).
 */
export const invokeIpc = (IPC_TRACE ? invokeWithTrace : invokeNoTrace) as InvokeIpc;

/**
 * How a dispatcher that forwards a command name it was handed (the mutation
 * queue) calls `invokeIpc`: `(invokeIpc as ForwardedInvoke)(command, args, meta)`.
 * The name is still checked and the result is still the one the command
 * declares; the arguments were checked where the dispatcher itself was called,
 * against `QueuedCallArgs<K>`. A type, not a second export, so a test that mocks
 * this module with `{ invokeIpc }` alone still mocks every call.
 */
export type ForwardedInvoke = <K extends IpcCommandName>(
  command: K,
  args: Record<string, unknown> | undefined,
  meta?: IpcMeta
) => Promise<IpcResult<K>>;

// Lives beside the CommandError readers (a module no test mocks, so the store reads a
// failure's message through it even where `utils/ipc` is mocked with `invokeIpc` alone);
// re-exported here, where every caller has always imported it from.
export { getErrorMessage } from './commandError';
