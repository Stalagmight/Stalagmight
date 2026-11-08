/**
 * #576 RV20-13: a developer's local opt-in to the text the shareable forms
 * withhold. With `STALAGMIGHT_AGENT_PRIVATE_ERRORS=1` the harness also keeps
 * the whole text of an error it prints withheld - a session attach failure,
 * `ipc-payload-probe.ts`'s rejections - in a private file only: the session's
 * `actions.jsonl` (an `attach-failed` event) and the probe's
 * `ipc-probe-errors.jsonl` in the session directory, both under "Not for
 * sharing" in docs/agent-harness.md. Never on stdout, stderr or in report.md:
 * what a run prints is the same with it set or not.
 */
import * as fs from 'fs';
import * as path from 'path';

export const PRIVATE_ERRORS_ENV = 'STALAGMIGHT_AGENT_PRIVATE_ERRORS';

/** Did the developer opt in (`STALAGMIGHT_AGENT_PRIVATE_ERRORS=1`)? */
export function privateErrorsOn(env: NodeJS.ProcessEnv = process.env): boolean {
  return env[PRIVATE_ERRORS_ENV] === '1';
}

/** Opted in, with a recorder: `err`'s whole message as an `<event>` event of the session's actions.jsonl. */
export function recordPrivateError(
  recorder: { event(event: string, detail?: unknown): void } | undefined,
  event: string,
  err: unknown,
  env: NodeJS.ProcessEnv = process.env,
): void {
  if (recorder === undefined || !privateErrorsOn(env)) return;
  recorder.event(event, { message: err instanceof Error ? err.message : String(err) });
}

/** `ipc-payload-probe.ts`'s private log, in its session directory. */
export const PROBE_ERRORS_FILE = 'ipc-probe-errors.jsonl';

/** Opted in: one probe call's whole error as a line of `<sessionDir>/ipc-probe-errors.jsonl`. */
export function appendProbeError(
  sessionDir: string,
  call: string,
  error: string | null,
  env: NodeJS.ProcessEnv = process.env,
): void {
  if (sessionDir === '' || error === null || !privateErrorsOn(env)) return;
  fs.appendFileSync(path.join(sessionDir, PROBE_ERRORS_FILE), `${JSON.stringify({ call, error })}\n`);
}
