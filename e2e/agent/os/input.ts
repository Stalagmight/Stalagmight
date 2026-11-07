/**
 * Tier 2 input: real CGEvent clicks and key presses aimed at the app's window.
 * See ./README.md for the tier model, the grant, and the two TCC permissions.
 *
 * Every action here runs the same three gates in the same order, because each
 * one is cheaper than the one after it and each protects a different thing:
 *
 * 1. `requireGrant()` - consent. Refusing here costs the machine nothing.
 * 2. `preflight()` - capability. Accessibility denied means posted events are
 *    silently dropped, which reads downstream as "the app ignored the click".
 * 3. `ensureAppFrontmost()` - evidence quality. At most ONE activate attempt;
 *    a false result rides the return value instead of throwing, because
 *    "a human took the machine back" is a fact about the evidence, not a fault.
 *
 * The `deps` parameter every entry point ends with exists so the unit tests can
 * drive all of this without posting a single real event: the suite never moves
 * the cursor. Callers pass two arguments and get the real machine.
 */
import { execFile } from 'child_process';
import type { Browser } from 'webdriverio';

import { testIdSelector } from '../lib/controls';
import { evalInApp } from '../lib/page';
import { isPidAlive } from '../lib/procs';
import type { AgentSession } from '../sessionTypes';
import {
  runHelper,
  type ClickResult,
  type FrontmostResult,
  type HelperError,
  type HelperResult,
  type KeysResult,
  type PreflightResult,
  type RunHelperOptions,
  type WindowResult,
} from './helper';
import { permissionHelp, preflight, requireGrant } from './permissions';

/** Rides every os_* result: was this evidence collected under valid conditions? */
export interface OsActionEvidence {
  frontmost_ok: boolean;
  permissions: { accessibility: boolean; screenRecording: boolean };
}

/** Screen points, top-left origin of the main display. */
export interface OsPoint {
  x: number;
  y: number;
}

/** A window frame in screen points, as `window-id` reports it. */
export interface OsRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** The webview's own view of its content area, in CSS points. */
export interface OsInnerSize {
  width: number;
  height: number;
}

/** Seams the unit tests replace; production wires them to the real machine. */
export interface OsDeps {
  runHelper: <T extends HelperResult = HelperResult>(
    args: string[],
    options?: RunHelperOptions,
  ) => Promise<T>;
  requireGrant: () => void;
  preflight: () => Promise<PreflightResult>;
  evalInApp: <T>(browser: Browser, body: string, args?: unknown[]) => Promise<T>;
  /** One attempt at bringing `pid` forward. */
  activate: (pid: number) => Promise<void>;
  sleep: (ms: number) => Promise<void>;
}

/** Wall-clock budget for the one activate attempt. */
const ACTIVATE_TIMEOUT_MS = 5_000;
/** Time the window server gets to finish the activation before we re-check. */
export const ACTIVATE_SETTLE_MS = 250;

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Bring `pid` forward through System Events. Accessibility (already verified by
 * the caller's preflight) is what makes this permitted; the pid is interpolated
 * into the script text, so `requireAppPid` validating it as a positive integer
 * is what keeps that interpolation safe.
 */
function activateApp(pid: number): Promise<void> {
  const script = `tell application "System Events" to set frontmost of (first process whose unix id is ${pid}) to true`;
  return new Promise((resolve, reject) => {
    execFile('osascript', ['-e', script], { timeout: ACTIVATE_TIMEOUT_MS }, (error, _out, errOut) => {
      if (error) reject(new Error(`could not activate pid ${pid}: ${errOut.trim() || error.message}`));
      else resolve();
    });
  });
}

export const defaultOsDeps: OsDeps = {
  runHelper,
  requireGrant,
  preflight,
  evalInApp,
  activate: activateApp,
  sleep: delay,
};

/**
 * Gates 1 and 2, shared by every os_* action. Returns the preflight state so it
 * can ride the result: a caller that scored a click as real evidence must be
 * able to see which permissions were actually in force when it was posted.
 */
export async function requireOsAccess(
  deps: OsDeps,
  needScreenRecording: boolean,
): Promise<OsActionEvidence['permissions']> {
  deps.requireGrant();
  const state = await deps.preflight();
  if (!state.accessibility) throw new Error(permissionHelp('accessibility'));
  if (needScreenRecording && !state.screenRecording) throw new Error(permissionHelp('screenRecording'));
  return { accessibility: state.accessibility, screenRecording: state.screenRecording };
}

/**
 * Gate 3 for INPUT only: refuse to post real events when we could not prove the
 * app is frontmost. A CGEvent is desktop-wide - it goes wherever focus is - so
 * a lost focus check means the next chord or click lands in the human's app.
 * `force` is the deliberate escape hatch for the rare case where the caller
 * really does want to drive whatever is in front.
 */
export function requireFrontmostForInput(
  frontmostOk: boolean,
  force: boolean | undefined,
  what: string,
): void {
  if (frontmostOk || force) return;
  throw new Error(
    `refusing to post os ${what}: the app could not be brought frontmost, and OS input goes to ` +
      'whatever app IS frontmost - posting now would type into the human\'s window. Bring the app ' +
      'forward (or close whatever stole focus) and retry; pass force: true only if you really mean ' +
      'to drive the frontmost app, whatever it is.',
  );
}

/**
 * The pid of the app this session launched. Every os_* action needs it to
 * locate the app's window and to check that the app is frontmost before
 * posting. Note this gates EXISTENCE, not aim: CGEvent input is desktop-wide
 * and a coordinate click is never clamped to the window, which is why
 * requireFrontmostForInput exists.
 */
export function requireAppPid(session: AgentSession): number {
  // A stopped or crashed session keeps its recorder (and therefore its
  // pidfile), so appPid() still answers with the pid of an app that is gone.
  // Posting desktop-wide input off a dead session is never right: the pid can
  // even have been reused by an unrelated process by now.
  if (session.state !== 'ready' && session.state !== 'busy') {
    throw new Error(
      `refusing OS input: the session is ${session.state}, not ready. Start (or restart) a macOS ` +
        'session first - a stopped session still remembers its old app pid, which may since have ' +
        'been reused by another process.',
    );
  }
  const pid = session.appPid?.() ?? null;
  if (pid === null || !Number.isInteger(pid) || pid <= 0) {
    throw new Error(
      'no app pid is known for this session, so OS input cannot target or verify the app window. ' +
        'Start a macOS session (session_start) and check that its result reports appPidKnown: true.',
    );
  }
  if (!isPidAlive(pid)) {
    throw new Error(
      `refusing OS input: the recorded app pid ${pid} is not running. The app died without the ` +
        'session noticing; restart it before driving the desktop.',
    );
  }
  return pid;
}

/** The app's frontmost on-screen window, or a thrown helper error. */
export async function windowForPid(pid: number, deps: OsDeps = defaultOsDeps): Promise<WindowResult> {
  const result = await deps.runHelper<WindowResult | HelperError>(['window-id', '--pid', String(pid)]);
  if (!result.ok) throw new Error(`os helper window-id --pid ${pid} failed: ${result.error}`);
  return result;
}

/**
 * Where the webview's content area starts on screen.
 *
 * `window-id` reports `kCGWindowBounds`: the full window FRAME in screen
 * points, title bar included. The webview reports `window.innerWidth/Height`:
 * its content area in CSS points, which are the same size as screen points at
 * zoom 1 (Retina backing scale is a pixel concern, never a point one). macOS
 * windows have no side or bottom border, so the only offset between the two is
 * the title bar, and its height is what the two heights disagree by.
 *
 * A negative title bar (a content area taller than its own frame) can only come
 * from a stale probe or a zoomed webview; shifting the click UP by it would be
 * worse than not shifting, so it clamps to 0. `innerSize.width` is not read:
 * with no side borders the content origin's x IS the frame's x.
 */
export function contentOriginFromFrame(frame: OsRect, innerSize: OsInnerSize): OsPoint {
  return { x: frame.x, y: frame.y + Math.max(0, frame.h - innerSize.height) };
}

interface ElementCenterProbe {
  cx: number;
  cy: number;
  innerWidth: number;
  innerHeight: number;
}

/**
 * Element center plus the content size, read in ONE round trip: two probes
 * could straddle a resize and compose a point that was never on the element.
 */
const ELEMENT_CENTER_BODY = `
  const element = document.querySelector(args[0]);
  if (!element) throw new Error('no element matches ' + args[0]);
  const rect = element.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) throw new Error(args[0] + ' has a zero-size rect');
  return {
    cx: rect.left + rect.width / 2,
    cy: rect.top + rect.height / 2,
    innerWidth: window.innerWidth,
    innerHeight: window.innerHeight,
  };
`;

/**
 * The screen point an os_* action will hit.
 *
 * `x`/`y` are already screen points and pass straight through. A `testid`
 * composes: element rect center (CSS points, content-relative, via `evalInApp`)
 * + the window content origin (screen points, via the helper). `testid` wins
 * when both are given, matching the Tier 1 `click` tool's precedence - it is
 * the more specific intent and it survives the window moving.
 *
 * Rounded: CGEvent takes doubles, but a sub-point click target is noise, and
 * rounding keeps the point in the recorded evidence identical to the one posted.
 */
export async function resolveTargetPoint(
  session: AgentSession,
  target: { testid?: string | undefined; x?: number | undefined; y?: number | undefined },
  deps: OsDeps = defaultOsDeps,
): Promise<OsPoint> {
  if (target.testid !== undefined) {
    const selector = testIdSelector(target.testid);
    const frame = await windowForPid(requireAppPid(session), deps);
    const probe = await deps.evalInApp<ElementCenterProbe>(session.browser, ELEMENT_CENTER_BODY, [selector]);
    const origin = contentOriginFromFrame(frame.bounds, {
      width: probe.innerWidth,
      height: probe.innerHeight,
    });
    return { x: Math.round(origin.x + probe.cx), y: Math.round(origin.y + probe.cy) };
  }
  if (typeof target.x === 'number' && typeof target.y === 'number') {
    if (!Number.isFinite(target.x) || !Number.isFinite(target.y)) {
      throw new Error('an OS input target needs finite x and y screen points');
    }
    return { x: Math.round(target.x), y: Math.round(target.y) };
  }
  throw new Error('an OS input target needs a testid or an x+y screen point');
}

/** How a result names what it aimed at. Same precedence as resolveTargetPoint. */
function describeTarget(target: { testid?: string | undefined; x?: number | undefined; y?: number | undefined }): string {
  if (target.testid !== undefined) return testIdSelector(target.testid);
  return `screen ${target.x},${target.y}`;
}

/** The pid owning the frontmost window, or null if that cannot be established. */
async function frontmostPid(deps: OsDeps): Promise<number | null> {
  const result = await deps.runHelper<FrontmostResult | HelperError>(['frontmost']);
  // A helper-level error here ("no on-screen application window found") is not
  // an infrastructure fault, and either way we cannot PROVE the app is in
  // front - which is exactly what frontmost_ok claims.
  return result.ok ? result.pid : null;
}

/**
 * Make the app frontmost if it is not already, with EXACTLY ONE attempt.
 *
 * Never loops: repeatedly stealing focus back from a human at the same keyboard
 * is always wrong. Returns whether the app is in front now.
 *
 * What the caller does with `false` differs by action, and the difference is
 * safety, not style. A capture is harmless when focus was lost - it just
 * photographs the wrong thing, so it proceeds and labels the evidence. REAL
 * INPUT is not: a CGEvent goes to whatever is frontmost, so posting `cmd+s`
 * (or a click at a point now covered by another window) after a failed focus
 * check types into the human's editor, not into a bad screenshot. Input
 * therefore ABORTS by default and only proceeds under an explicit `force`.
 */
export async function ensureAppFrontmost(appPid: number, deps: OsDeps = defaultOsDeps): Promise<boolean> {
  if ((await frontmostPid(deps)) === appPid) return true;
  try {
    await deps.activate(appPid);
  } catch {
    // Deliberately not rethrown; see above.
  }
  await deps.sleep(ACTIVATE_SETTLE_MS);
  return (await frontmostPid(deps)) === appPid;
}

/**
 * A real trusted click. The hover move that precedes the press, its settle, and
 * the press/release gap all live inside the helper.
 */
export async function osClick(
  session: AgentSession,
  opts: {
    testid?: string | undefined;
    x?: number | undefined;
    y?: number | undefined;
    button?: 'left' | 'right' | undefined;
    double?: boolean | undefined;
    force?: boolean | undefined;
  },
  deps: OsDeps = defaultOsDeps,
): Promise<{ clicked: OsPoint; target: string } & OsActionEvidence> {
  const permissions = await requireOsAccess(deps, false);
  const appPid = requireAppPid(session);
  await ensureAppFrontmost(appPid, deps);
  // Resolved AFTER the activation: bringing a window forward can move it.
  const point = await resolveTargetPoint(session, opts, deps);
  // Re-checked AFTER resolution and immediately before posting: resolving a
  // testid costs a window lookup plus a webview round-trip, and focus can move
  // during it. Checking only up front would post into whatever stole focus in
  // that window while reporting frontmost_ok: true - a lie in the one field
  // that exists to make the evidence trustworthy.
  const frontmostOk = (await frontmostPid(deps)) === appPid;
  requireFrontmostForInput(frontmostOk, opts.force, 'click');

  const args = ['click', String(point.x), String(point.y)];
  if (opts.button === 'right') args.push('--right');
  if (opts.double) args.push('--double');
  const result = await deps.runHelper<ClickResult | HelperError>(args);
  if (!result.ok) throw new Error(`os helper click failed: ${result.error}`);

  return {
    clicked: point,
    target: describeTarget(opts),
    frontmost_ok: frontmostOk,
    permissions,
  };
}

/**
 * A real key press with real modifier flags - the only tier that can fire a
 * native menu accelerator. Spec grammar: README.md, or the helper's `--help`.
 */
export async function osKeys(
  session: AgentSession,
  spec: string,
  deps: OsDeps = defaultOsDeps,
  opts: { force?: boolean | undefined } = {},
): Promise<{ spec: string } & OsActionEvidence> {
  const permissions = await requireOsAccess(deps, false);
  const appPid = requireAppPid(session);
  const frontmostOk = await ensureAppFrontmost(appPid, deps);
  requireFrontmostForInput(frontmostOk, opts.force, `keys ${spec}`);

  const result = await deps.runHelper<KeysResult | HelperError>(['keys', spec]);
  if (!result.ok) throw new Error(`os helper keys ${spec} failed: ${result.error}`);

  return { spec: result.spec, frontmost_ok: frontmostOk, permissions };
}
