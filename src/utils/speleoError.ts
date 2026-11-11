import type {
  EndpointRefusal,
  NewerSchema,
  SpeleoDbAction,
  SpeleoErrorDto,
  TransportCause,
  UnpushedLocalEdits,
} from '../types/ipc.generated';
import i18n from '../i18n';
import { useSpeleoStore } from '../store/speleoStore';
import { getErrorMessage } from './ipc';
import { newerSchemaMessage } from './newerSchema';

/**
 * The typed transport cause of a rejected SpeleoDB IPC call, or `null` when the
 * rejection is not a transport failure.
 *
 * Read STRUCTURALLY off the DTO's `transport` discriminant, which the backend
 * sets only for `SpeleoDbError::Transport` (see `src-tauri/src/speleodb/
 * contract.rs`). Never sniffed from the message text: that text is the backend's
 * raw cause (an OS errno such as "Invalid argument"), which is locale- and
 * platform-dependent.
 */
function transportCause(e: unknown): TransportCause | null {
  if (e && typeof e === 'object' && 'transport' in e) {
    const cause = (e as Partial<SpeleoErrorDto>).transport;
    return typeof cause === 'string' ? (cause as TransportCause) : null;
  }
  return null;
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
 * know WHICH box). The raw cause is not dropped: it stays on the rejected
 * `SpeleoErrorDto` as `message` (the backend error's Display, request URLs
 * stripped), so a caller that logs the error keeps it. The backend client does
 * NOT log transport failures itself (`SdkClient::call` logs only contract
 * mismatches); a transport failure reaches the backend log only where an
 * orchestration path logs the error it is degrading around.
 */
function transportMessage(cause: TransportCause, tried?: string | null): string {
  return i18n.t(TRANSPORT_KEYS[cause], { endpoint: serverName(tried) });
}

/**
 * The server to name in a message: the one the call was made to when the caller
 * knows it (Settings' sign-in tries the address ON SCREEN, which may not be saved
 * yet), else the session endpoint - where we are actually authenticated, since the
 * config value only affects future sign-ins - else a neutral phrase.
 */
function serverName(tried?: string | null): string {
  return tried?.trim()
    || useSpeleoStore.getState().session?.endpoint?.trim()
    || i18n.t('speleodb.transport.thisServer');
}

/** Why the endpoint policy refused the server address, read structurally off the DTO. */
function endpointRefusalOf(e: unknown): EndpointRefusal | null {
  if (!e || typeof e !== 'object' || !('endpoint_refusal' in e)) return null;
  const refusal = (e as Partial<SpeleoErrorDto>).endpoint_refusal;
  return refusal === 'PlainHttp' || refusal === 'InvalidAddress' ? refusal : null;
}

/**
 * Turn a rejected SpeleoDB IPC error into a single user-facing line.
 *
 * The backend already provides a clean detail (auth failures parse
 * `non_field_errors`). This adds the operation frame the backend can't know -
 * e.g. `Could not connect to SpeleoDB: …`, `Unable to open the project: …` - except on
 * transport errors, which are replaced wholesale by the humanized #270 line (it
 * is already self-framed and would otherwise read as "Login failed: Network
 * error…"), and on an endpoint the policy refused, which says what to change
 * instead of the backend's English rule.
 *
 * `opts.endpoint` is the server the call was made to, when the caller knows it
 * better than the session does (see {@link serverName}).
 */
export function humanizeSpeleoError(
  e: unknown,
  contextPrefix?: string,
  opts: { endpoint?: string | null } = {},
): string {
  const cause = transportCause(e);
  if (cause) return transportMessage(cause, opts.endpoint);
  const refusal = endpointRefusalOf(e);
  if (refusal) {
    return i18n.t(refusal === 'PlainHttp'
      ? 'speleodb.endpointRefused.plainHttp'
      : 'speleodb.endpointRefused.invalidAddress', { endpoint: serverName(opts.endpoint) });
  }
  // #649: a download a newer Stalagmight saved. Self-framed like the transport
  // line, and worded exactly as the open dialog words it.
  const newer = newerSchemaOf(e);
  if (newer) return newerSchemaMessage(newer, getErrorMessage(e));
  // Strip a trailing period so "…credentials." reads cleanly after our prefix.
  const msg = getErrorMessage(e).replace(/\.\s*$/, '');
  if (!contextPrefix) return msg;
  return `${contextPrefix}: ${msg}`;
}

/**
 * #649: the newer-schema refusal a rejected SpeleoDB call carries, or `null`.
 * Read structurally off the DTO, and only trusted when it is shaped like one.
 */
function newerSchemaOf(e: unknown): NewerSchema | null {
  if (!e || typeof e !== 'object' || !('newer_schema' in e)) return null;
  const newer = (e as Partial<SpeleoErrorDto>).newer_schema;
  if (!newer || typeof newer !== 'object') return null;
  const { file, found, supported } = newer;
  if (typeof file !== 'string' || typeof found !== 'number' || typeof supported !== 'number') return null;
  return newer;
}

/**
 * Extract the typed {@link SpeleoDbAction} from a rejected SpeleoDB IPC call.
 *
 * The `speleodb_*` commands reject with a {@link SpeleoErrorDto} (`{ message,
 * action, status?, transport? }`), so the UI can branch on `action` (ReLogin /
 * ReadOnly / Fence / …) instead of scraping the display message. Returns `null`
 * for a non-DTO rejection (a plain string / Error), so callers fall back to a
 * generic message via `getErrorMessage`.
 */
export function speleoErrorAction(e: unknown): SpeleoDbAction | null {
  if (e && typeof e === 'object' && 'action' in e) {
    const action = (e as Partial<SpeleoErrorDto>).action;
    return typeof action === 'string' ? (action as SpeleoDbAction) : null;
  }
  return null;
}

/**
 * [#270] The un-pushed local edits an open was refused over, or `null`.
 *
 * `speleodb_open_project` refuses rather than overwrite a cached `.stalx` that
 * still holds edits the server has never seen; the refusal carries the project
 * id, the local path and when it was last edited so the UI can name exactly what
 * is at stake. Read structurally off the DTO (never sniffed from the message),
 * and only trusted when the payload is actually shaped like the record - a
 * refusal we cannot describe must fall back to the generic error path rather
 * than raise a prompt with blanks in it.
 */
export function unpushedLocalEdits(e: unknown): UnpushedLocalEdits | null {
  if (speleoErrorAction(e) !== 'UnpushedLocalEdits') return null;
  const payload = (e as Partial<SpeleoErrorDto>).unpushed_local;
  if (!payload || typeof payload !== 'object') return null;
  const { project_id, local_path } = payload;
  if (typeof project_id !== 'string' || typeof local_path !== 'string' || !local_path) return null;
  return payload;
}
