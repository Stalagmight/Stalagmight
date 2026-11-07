import type { CommandError } from '../types/ipc/CommandError';
import type { CommandErrorCode } from '../types/ipc/CommandErrorCode';

/**
 * How a command failure reaches the frontend (#576 C4, A6): every fallible command
 * rejects with the backend's `CommandError` - a `code` to route on, a `message` that
 * is display-only, and the payload fields of the failures that carry one (the load's
 * `details`, SpeleoDB's `status` / `transport` / `unpushed_local` / ..., a history
 * jump's `history_step`, the link bits in `remote_link`). The message never carries
 * a routing marker, and nothing on the frontend reads it to decide anything: every
 * decision goes through the code, read here.
 *
 * `invokeIpc` re-throws that rejection as an `IpcCommandError`, so the message keeps
 * reaching every surface that shows an error the way it always did:
 * `getErrorMessage(e)`, `String(e)`, `${e}` and `e instanceof Error` all see it. The
 * readers below work STRUCTURALLY - on an `IpcCommandError`, on the plain
 * `{ code, message }` object a test (or `invoke` called directly, as the load does)
 * rejects with, and on a refused store outcome (`CommandRefused`, which carries the
 * same `code`) - so a caller never needs to know which of the three it holds.
 */
/** A payload field's type, as the generated `CommandError` declares it (optional). */
type Payload<K extends keyof CommandError> = Exclude<CommandError[K], undefined>;

export class IpcCommandError extends Error implements CommandError {
  override readonly name = 'IpcCommandError';
  // Set by the constructor from the wire object (`declare`: no field initializer may
  // overwrite them, and an omitted payload stays absent rather than `undefined`).
  declare readonly code: CommandErrorCode;
  declare readonly details?: Payload<'details'>;
  declare readonly speleodb_managed?: Payload<'speleodb_managed'>;
  declare readonly status?: Payload<'status'>;
  declare readonly transport?: Payload<'transport'>;
  declare readonly unpushed_local?: Payload<'unpushed_local'>;
  declare readonly dump_candidates?: Payload<'dump_candidates'>;
  declare readonly dump_candidate_kind?: Payload<'dump_candidate_kind'>;
  declare readonly history_step?: Payload<'history_step'>;
  declare readonly reversed_sections?: Payload<'reversed_sections'>;
  declare readonly session_closed?: Payload<'session_closed'>;
  declare readonly remote_link?: Payload<'remote_link'>;

  constructor(wire: CommandError) {
    super(wire.message);
    // Every field but `message` (which `super` set), exactly as the backend sent it:
    // an omitted payload stays absent rather than becoming a present `undefined`.
    const { message: _message, ...fields } = wire;
    Object.assign(this, fields);
  }

  /** The display message alone, so `String(e)` / `${e}` read like the old string rejection. */
  override toString(): string {
    return this.message;
  }
}

/**
 * The backend's `CommandError` that `e` is, or null: an object with a string `code`
 * and a string `message` - an `IpcCommandError`, or the plain object the IPC
 * transport rejects with. A plain-string rejection (Tauri's own argument
 * deserialization errors), an ordinary `Error`, or anything else is not one.
 */
export function commandErrorOf(e: unknown): CommandError | null {
  if (typeof e !== 'object' || e === null) return null;
  const { code, message } = e as { code?: unknown; message?: unknown };
  return typeof code === 'string' && typeof message === 'string' ? (e as CommandError) : null;
}

/**
 * The code a failure carries, or null when it carries none: read off a
 * `CommandError` (see `commandErrorOf`) or off a refused store outcome
 * (`CommandRefused.code`, null for a refusal the frontend made itself).
 */
export function errorCode(e: unknown): CommandErrorCode | null {
  const failure = commandErrorOf(e);
  if (failure) return failure.code;
  if (typeof e !== 'object' || e === null) return null;
  const { code, success } = e as { code?: unknown; success?: unknown };
  return success === false && typeof code === 'string' ? (code as CommandErrorCode) : null;
}

/** Extract error message from unknown catch value */
export function getErrorMessage(e: unknown, fallback?: string): string {
  if (e instanceof Error) return e.message;
  if (typeof e === 'string') return e;
  // [Track A] #186/D-B2: a structured IPC error that was not wrapped (a plain
  // CommandError object, e.g. from `invoke` called directly) carries a `message`
  // string - surface it instead of "[object Object]". Its `code` is branched on
  // separately (`errorCode`).
  if (e && typeof e === 'object' && 'message' in e && typeof e.message === 'string') {
    return e.message;
  }
  return fallback ?? String(e);
}

/**
 * What `invokeIpc` throws for a rejection `e`: a `CommandError`-shaped object becomes
 * an `IpcCommandError` (one already wrapped is returned as is); anything else - a
 * plain string from Tauri's argument deserialization, a transport `Error` - is
 * passed through unchanged, so the callers that handle those keep seeing them.
 */
export function toIpcError(e: unknown): unknown {
  if (e instanceof IpcCommandError) return e;
  const failure = commandErrorOf(e);
  return failure ? new IpcCommandError(failure) : e;
}
