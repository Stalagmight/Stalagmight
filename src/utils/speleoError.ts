import type {
  SpeleoDumpCandidate,
  TransportCause,
  UnpushedLocalEdits,
} from '../types/ipc.generated';
import i18n from '../i18n';
import type { DumpPickerKind } from '../store/speleoStore';
import { useSpeleoStore } from '../store/speleoStore';
import { commandErrorOf, errorCode } from './commandError';
import { isEditGateCode, READ_ONLY_EDIT_NOTICE } from './editErrors';
import { saveErrorKind } from './saveErrors';
import { autosaveFailureMessage } from './saveFeedback';
import { getErrorMessage } from './ipc';

/**
 * The typed transport cause of a rejected SpeleoDB IPC call, or `null` when the
 * rejection is not a transport failure.
 *
 * Read STRUCTURALLY off the CommandError's `transport` payload, which the backend
 * sets only for `SpeleoDbError::Transport` (see `src-tauri/src/speleodb/
 * contract.rs`). Never sniffed from the message text: that text is the backend's
 * raw cause (an OS errno such as "Invalid argument"), which is locale- and
 * platform-dependent.
 */
function transportCause(e: unknown): TransportCause | null {
  const cause = commandErrorOf(e)?.transport;
  return typeof cause === 'string' ? cause : null;
}

const TRANSPORT_KEYS: Record<TransportCause, string> = {
  Refused: 'speleodb.transport.refused',
  Timeout: 'speleodb.transport.timeout',
  Dns: 'speleodb.transport.dns',
  Tls: 'speleodb.transport.tls',
  Unknown: 'speleodb.transport.unreachable',
};

/**
 * [#270] The user-facing line for a transport failure.
 *
 * The backend's own text was passed straight through, so a blackholed host read
 * "Network error talking to SpeleoDB: Invalid argument" - a bare errno that says
 * neither what failed nor what to do, and does not even hint the server was
 * unreachable. The message is built here instead: translatable, cause-specific,
 * and naming the server we are actually signed in to (self-hosted users need to
 * know WHICH box). The raw cause is not dropped - it stays on the error and in
 * the backend log (`client::with_retry` warns with the cause + endpoint), which
 * is what makes a bug report actionable.
 */
function transportMessage(cause: TransportCause): string {
  // The session endpoint is where we are actually authenticated (the config value
  // only affects future logins), so it is the honest answer to "which server?".
  const endpoint =
    useSpeleoStore.getState().session?.endpoint?.trim() || i18n.t('speleodb.transport.thisServer');
  return i18n.t(TRANSPORT_KEYS[cause], { endpoint });
}

/** What the failed SpeleoDB verb did, where its wording depends on it. */
export interface SpeleoErrorContext {
  /**
   * The verb saves the project before anything else (Push, Done), so a refused
   * save is what failed (#576 NF-W9BE-1). Publish saves first too but does not
   * pass it: its newer-version refusal reads as the newer-version line, like
   * Start editing's (#576 NF-W9BE-1-D1).
   */
  saves?: boolean;
}

/**
 * Turn a rejected SpeleoDB IPC error into a single user-facing line.
 *
 * The backend already provides a clean detail (auth failures parse
 * `non_field_errors`). This adds the operation frame the backend can't know -
 * e.g. `Login to SpeleoDB failed: …`, `Unable to open the project: …` - except on
 * transport errors, which are replaced wholesale by the humanized #270 line (it
 * is already self-framed and would otherwise read as "Login failed: Network
 * error…").
 */
export function humanizeSpeleoError(e: unknown, contextPrefix?: string, context: SpeleoErrorContext = {}): string {
  const cause = transportCause(e);
  if (cause) return transportMessage(cause);
  const code = errorCode(e);
  const raw = getErrorMessage(e);
  // [#576 RV2-R-15] An edit-gate refusal (e.g. Restore revision / Unwind in the
  // read-only Plan or Export workspace) gets the same notice a refused edit gets.
  if (isEditGateCode(code)) return READ_ONLY_EDIT_NOTICE(code);
  const kind = saveErrorKind(code);
  // [#576 NF-W2SAVE-1] A Done/Push whose pre-push save refused carries the save's
  // own code: worded like any refused save's line. [#576 NF-W9BE-1] Except the
  // newer-version code outside a verb that saves: a download that would install
  // such a project (open, View, Restore, Update) and Start editing / Force edit
  // are refused with it too, and they saved nothing.
  const saved =
    kind === 'newer-file' && !context.saves
      ? i18n.t('banners.newerFile.lead')
      : kind === 'other'
        ? raw
        : autosaveFailureMessage({ code, error: raw });
  // Strip a trailing period so "…credentials." reads cleanly after our prefix.
  const msg = saved.replace(/\.\s*$/, '');
  if (!contextPrefix) return msg;
  return `${contextPrefix}: ${msg}`;
}

/**
 * [#270] The un-pushed local edits an open was refused over, or `null`.
 *
 * `speleodb_open_project` refuses (code `SPELEODB_UNPUSHED_LOCAL_EDITS`) rather
 * than overwrite a cached `.stalx` that still holds edits the server has never
 * seen; the refusal carries the project id, the local path and when it was last
 * edited so the UI can name exactly what is at stake. Read structurally off the
 * CommandError (never sniffed from the message), and only trusted when the payload
 * is actually shaped like the record - a refusal we cannot describe must fall back
 * to the generic error path rather than raise a prompt with blanks in it.
 */
export function unpushedLocalEdits(e: unknown): UnpushedLocalEdits | null {
  if (errorCode(e) !== 'SPELEODB_UNPUSHED_LOCAL_EDITS') return null;
  const payload = commandErrorOf(e)?.unpushed_local;
  if (!payload || typeof payload !== 'object') return null;
  const { project_id, local_path } = payload;
  if (typeof project_id !== 'string' || typeof local_path !== 'string' || !local_path) return null;
  return payload;
}

/**
 * [#258] The openable files an ambiguous dump offered, or `null`.
 *
 * A dump holding more than one `.stalx` (or, with no `.stalx`, more than one
 * `ariane.tml`) is a question rather than a failure: the backend rejects with
 * code `SPELEODB_AMBIGUOUS` and attaches the candidates so the UI can offer a
 * choice instead of a dead end.
 *
 * Validated structurally, like `unpushedLocalEdits`, and for the same reason: a
 * list we cannot render must fall through to the generic error path, whose
 * message names the remedy, rather than raise an empty picker. Note the
 * fast-forward verbs (Start editing / Update / Take remote) carry no way to send
 * a choice back, so they legitimately produce this code with nothing to act on
 * - the empty check below is what routes those to the message.
 */
export function dumpCandidates(e: unknown): SpeleoDumpCandidate[] | null {
  if (errorCode(e) !== 'SPELEODB_AMBIGUOUS') return null;
  const payload = commandErrorOf(e)?.dump_candidates;
  if (!Array.isArray(payload) || payload.length === 0) return null;
  const usable = payload.every(
    (c) => c && typeof c === 'object' && typeof c.entry_path === 'string' && c.entry_path !== '',
  );
  return usable ? payload : null;
}

/**
 * [#258] Which ambiguity `dumpCandidates` returned, defaulting to `'stalx'`.
 *
 * Read off the CommandError rather than inferred from the candidates' extensions: which
 * kind a set is, is the backend classifier's decision, and re-deriving it here
 * would be a second copy of that rule with nothing keeping the two in step. The
 * kind picks the VERB - extract this `.stalx` versus adopt this `ariane.tml` are
 * different operations - so it is not merely cosmetic.
 *
 * Defaults to `'stalx'` for an older backend that sends no kind, which is the
 * commoner ambiguity and the one whose flow works without adoption.
 */
export function dumpCandidateKind(e: unknown): DumpPickerKind {
  return commandErrorOf(e)?.dump_candidate_kind === 'ariane' ? 'ariane' : 'stalx';
}
