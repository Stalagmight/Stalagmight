import { commandErrorOf } from '../utils/commandError';
import type { CommandResult } from '../types/commands';
import type { CommandError } from '../types/ipc/CommandError';
import type { CommandErrorCode } from '../types/ipc/CommandErrorCode';

/**
 * A `CommandResult` for an outcome the frontend decided itself (a cancelled or
 * superseded request, a caught IPC rejection): no warnings unless given, no
 * sections, and `error` only on failure.
 *
 * A leaf module so the store slices can share it without importing
 * projectStore (#576 D27-32(b): projectConfigActions importing it from
 * projectStore, which imports projectConfigActions, was part of a value-import
 * cycle).
 */
export function emptyCommandResult(
  success: boolean,
  error?: string | null,
  warnings: CommandResult['warnings'] = []
): CommandResult {
  return {
    success,
    warnings,
    error: success ? null : (error ?? null),
    section_ids: [],
  };
}

/**
 * A store mutation that did not happen (#576 C4, A12): refused by the backend, or by
 * the frontend before it was sent. `error` is the line to show (display only, never
 * parsed); `code` is what to route on - the backend's `CommandErrorCode`, or `null`
 * for a refusal the frontend made itself (the project changed under the request, a
 * move carry, a cancelled load). `failure` is the backend's whole `CommandError`
 * (payloads included: `history_step`, `reversed_sections`, `session_closed`, ...),
 * null exactly when `code` is.
 */
export interface CommandRefused {
  success: false;
  error: string;
  code: CommandErrorCode | null;
  failure: CommandError | null;
}

/** A store mutation that happened: the command's own outcome `T`, flagged. */
export type CommandAccepted<T> = T & { success: true; error: null };

/**
 * What a store mutation answers with: the per-command outcome on success (read its
 * fields after narrowing on `success`), or the refusal. `success` / `error` read the
 * same on both arms, so a caller that only asks "did it work, and if not why" keeps
 * doing exactly that.
 */
export type CommandOutcome<T> = CommandAccepted<T> | CommandRefused;

/** The accepted arm for a command's resolved outcome. */
export function accepted<T extends object>(outcome: T): CommandAccepted<T> {
  return { ...outcome, success: true, error: null };
}

/** The refused arm for the backend's `CommandError`. */
export function refusedBy(failure: CommandError): CommandRefused {
  return { success: false, error: failure.message, code: failure.code, failure };
}

/** The refused arm for a refusal the frontend made itself (no backend code). */
export function refusedLocally(error: string): CommandRefused {
  return { success: false, error, code: null, failure: null };
}

/** What every seam returns for a request whose project changed out from under it. */
export const PROJECT_CHANGED_REFUSAL = 'The project changed before this action could run.';

/**
 * [#576 FZ-FZ-INT-4, FZ-FZ-INT-7] Whether a refusal says the project changed - or
 * was being switched - under the request: the backend's PROJECT_CHANGED (on a
 * refused outcome, a thrown `CommandError` or a `SaveFailure`), or the frontend's
 * own refusal of that kind, which is {@link PROJECT_CHANGED_REFUSAL} with no code
 * (compared to this module's own constant, never to a backend message). Not a
 * failure to report: the switch is the user's own action, and what they asked of
 * the old project no longer applies.
 */
export function isProjectChangedRefusal(value: unknown): boolean {
  if (typeof value !== 'object' || value === null) return false;
  const { code, error } = value as { code?: unknown; error?: unknown };
  return code === 'PROJECT_CHANGED' || (code === null && error === PROJECT_CHANGED_REFUSAL);
}

/**
 * Settle a command's promise into its store outcome: a resolve is accepted, a
 * rejection carrying the backend's `CommandError` is refused with it, and any other
 * rejection - a transport failure, Tauri's own argument-deserialization string - is
 * re-thrown for the caller to handle as it always has.
 */
export async function settleCommand<T extends object>(call: Promise<T>): Promise<CommandOutcome<T>> {
  try {
    return accepted(await call);
  } catch (e) {
    const failure = commandErrorOf(e);
    if (failure) return refusedBy(failure);
    throw e;
  }
}
