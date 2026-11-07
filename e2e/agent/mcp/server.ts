// hygiene MUST be first: it silences wdio + routes console.* off stdout before
// webdriverio (imported transitively via ../createSession) can log there.
import './hygiene';
import * as fs from 'fs';
import * as path from 'path';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { z } from 'zod';
import { createSession } from '../createSession';
import { isDarwinTarget } from '../lib/platform';
import { osClick, osKeys } from '../os/input';
import { grant as osGrant, preflight as osPreflight } from '../os/permissions';
import { osScreenshot } from '../os/screenshot';
import type { AgentSession } from '../sessionTypes';
import { readStore, callStoreAction, postDigest, type StoreName } from '../lib/stores';
import { LogCursor, type LogLevel } from '../lib/logs';
import { invokeInApp, evalInApp, capJson } from '../lib/page';
import { newOracleState, runSweep, type OracleState } from '../lib/oracles';
import { listScenarios, runScenario } from '../scenarios';
import { publishableErrorMessage, redactCorpusValue } from '../lib/fixtureCorpus';
import { DEFAULT_FIXTURE, materializeFixture } from '../lib/fixtures';
import {
  waitForAppReady,
  waitForUiIdle,
  waitForMapIdle,
  waitForActiveTool,
} from '../lib/waiters';
import {
  dragPointer,
  hoverElement,
  mapClick,
  queryElementState,
  answerFileDialog,
  scrollTarget,
  selectOption,
  targetSelector,
} from '../lib/controls';
import { mapFeatureClick } from '../lib/mapControls';
import { REACT_SAFE_SET_VALUE_SCRIPT, SYNTHETIC_KEY_SCRIPT } from '../lib/reactInput';

const STORE_NAMES = ['ui', 'project', 'globalView', 'gasPlan', 'speleo', 'export', 'network'] as const;

const session = createSession();
let logCursor: LogCursor | null = null;
let oracleState: OracleState = newOracleState();

type ToolResult = { content: Array<Record<string, unknown>>; isError?: boolean };

/**
 * A tool's answer, folded through the corpus redaction like every error the server
 * returns (`asMessage`): the `logs` tool, for one, returns the backend's line for a
 * failed parse, which quotes the corpus copy it was handed (#576 RV13-4).
 */
function textResult(value: unknown): ToolResult {
  return { content: [{ type: 'text', text: JSON.stringify(redactCorpusValue(value) ?? null) }] };
}
function errorResult(message: string): ToolResult {
  return { content: [{ type: 'text', text: JSON.stringify({ error: message }) }], isError: true };
}
/**
 * A tool's result capped at 16 KB, folded through the corpus redaction FIRST
 * (#576 RV18-W23-6), as `record` folds before its own cap. `textResult` folds
 * the answer again, but a cut through a corpus name leaves a stub no fold
 * recognises, and the agent was handed its start. (The action log was not:
 * `record` keeps the first 4 KB of a result.)
 */
function capFolded(value: unknown): { value: unknown; truncated: boolean } {
  return capJson(redactCorpusValue(value), 16384);
}
/** Every error the server records or returns, worded publishably (#576 RV12-F1). */
function asMessage(err: unknown): string {
  return publishableErrorMessage(err);
}

interface RunOutput<T> {
  result: T;
  resolved?: unknown;
  artifacts?: { screenshot?: string; stateSnapshot?: string };
}

/** Time + record a tool call to the session action log, then return its result. */
async function record<T>(
  tool: string,
  params: unknown,
  fn: (seq: number) => Promise<RunOutput<T>>,
  opts: { digest?: boolean | undefined; mutates?: boolean } = {},
): Promise<T> {
  const start = Date.now();
  const seq = session.hasRecorder ? session.recorder.nextSeq() : 0;
  try {
    const { result, resolved, artifacts } = await fn(seq);
    if (session.hasRecorder) {
      let post: unknown;
      // Under the command mutex like every other WebDriver call - a raw
      // browser.execute here was the one path that could interleave with a
      // concurrent tool's exclusive block.
      if (opts.digest) {
        post = await session.exclusive(() => postDigest(session.browser)).catch(() => undefined);
      }
      session.recorder.action({
        seq,
        ts: new Date().toISOString(),
        tool,
        mutating: opts.mutates === true,
        params,
        resolved,
        outcome: { ok: true, durationMs: Date.now() - start },
        // Folded BEFORE the cap: a cut through a corpus name would leave a stub the
        // recorder's own folding cannot recognise (#576 RV13-4).
        result: capJson(redactCorpusValue(result), 4096).value,
        artifacts,
        post,
      });
    }
    return result;
  } catch (err) {
    if (session.hasRecorder) {
      session.recorder.action({
        seq,
        ts: new Date().toISOString(),
        tool,
        // A mutation that THREW is still a repro step - often the decisive one -
        // and the report renders it with a "(failed)" flag.
        mutating: opts.mutates === true,
        params,
        outcome: { ok: false, durationMs: Date.now() - start, error: asMessage(err) },
      });
    }
    throw err;
  }
}

/**
 * Record a tool call, then (for mutating tools) run a cheap oracle sweep and
 * merge any new findings into the JSON envelope so the agent sees problems the
 * moment they appear. The tool's own result object is spread at the top level;
 * findings ride under `oracle`.
 */
async function run(
  tool: string,
  params: unknown,
  fn: (seq: number) => Promise<RunOutput<unknown>>,
  opts: { digest?: boolean; sweep?: boolean; mutates?: boolean } = {},
): Promise<ToolResult> {
  try {
    // `sweep` is the default answer to "does this mutate?" because the oracle
    // sweep exists to run after state changes - the two coincide for all but a
    // few tools that mutate without being worth a sweep, which pass `mutates`
    // explicitly. Deriving it this way means a new mutating tool is classified
    // correctly by default rather than being forgotten.
    const result = await record(tool, params, fn, {
      digest: opts.digest,
      mutates: opts.mutates ?? opts.sweep ?? false,
    });
    const envelope: Record<string, unknown> =
      result && typeof result === 'object' ? { ...(result as Record<string, unknown>) } : { value: result };
    if (opts.sweep) {
      const oracle = await sweepBestEffort();
      if (oracle) envelope.oracle = oracle;
    }
    return textResult(envelope);
  } catch (err) {
    // A failed action is the highest-signal moment for the oracle (the failure
    // may BE the crash), so sweep on the error path too.
    const payload: Record<string, unknown> = { error: asMessage(err) };
    if (opts.sweep) {
      const oracle = await sweepBestEffort();
      if (oracle) payload.oracle = oracle;
    }
    return { content: [{ type: 'text', text: JSON.stringify(payload) }], isError: true };
  }
}

/**
 * Reset oracle state and align its ring cursor to the CURRENT ring tail, so
 * the first post-action sweep only reports lines emitted after this point.
 * Startup noise (benign boot-time warnings) predates any agent interaction
 * and must not be attributed to the first tool call - this mirrors what
 * LogCursor.checkpoint() already does for the `logs` tool.
 */
async function resetOracles(): Promise<void> {
  oracleState = newOracleState();
  try {
    const logs = await session.exclusive(() =>
      invokeInApp<{ latest_seq: number }>(session.browser, 'get_recent_logs_since', { afterSeq: 0 }),
    );
    oracleState.logCursor = logs.latest_seq;
  } catch {
    // Best-effort: a failed checkpoint only means the first sweep may include
    // startup lines.
  }
}

/** Cheap sweep that never throws; returns an `oracle` block or null. */
async function sweepBestEffort(): Promise<{ worst: unknown; findings: unknown[] } | null> {
  try {
    const sweep = await runSweep(session, oracleState, {
      recorder: session.hasRecorder ? session.recorder : undefined,
    });
    return sweep.newFindings.length ? { worst: sweep.worst, findings: sweep.newFindings } : null;
  } catch {
    return null;
  }
}

const server = new McpServer({ name: 'stalagmight-app', version: '0.1.0' });

// The MCP SDK does NOT serialize tool handlers: protocol.js's _onrequest fires
// each handler via Promise.resolve().then(...) without awaiting it, so a client
// that pipelines two tool calls runs both handlers concurrently. The session's
// exclusive() mutex only covers the WebDriver leg inside each handler - the
// recorder seq allocation, the oracle sweep bookkeeping (oracleState.reported /
// logCursor / findingCount), and postDigest would still interleave across
// tools. Serialize WHOLE handlers here so one tool call fully completes
// (mutation + digest + sweep) before the next begins.
let toolChain: Promise<unknown> = Promise.resolve();

function define<S extends z.ZodRawShape>(
  name: string,
  description: string,
  schema: S,
  handler: (args: z.infer<z.ZodObject<S>>) => Promise<ToolResult>,
  opts: { serialize?: boolean } = {},
): void {
  const wrapped =
    opts.serialize === false
      ? handler
      : (args: z.infer<z.ZodObject<S>>): Promise<ToolResult> => {
          const run = toolChain.then(() => handler(args));
          toolChain = run.catch(() => undefined);
          return run;
        };
  // eslint-disable-next-line @typescript-eslint/no-explicit-any -- registerTool infers its callback type from a concrete schema; a generic S cannot satisfy it
  server.registerTool(name, { description, inputSchema: schema }, wrapped as any);
}

// ---------------------------------------------------------------- lifecycle

define(
  'app_start',
  'Build (if stale) and launch the real Tauri app under WebDriver, returning session info.',
  {
    build: z.enum(['auto', 'skip', 'force']).optional(),
    headed: z.boolean().optional(),
    freshProfile: z.boolean().optional(),
  },
  async (args) => {
    try {
      const res = await session.start({
        build: args.build ?? 'auto',
        headed: args.headed,
        freshProfile: args.freshProfile,
      });
      await resetOracles();
      logCursor = new LogCursor(session.browser);
      await logCursor.checkpoint().catch(() => undefined);
      return textResult(res);
    } catch (err) {
      return errorResult(asMessage(err));
    }
  },
);

define('app_stop', 'Close the app and driver, freeing the session.', {}, async () => {
  try {
    await session.stop('app_stop');
    logCursor = null;
    return textResult({ stopped: true });
  } catch (err) {
    return errorResult(asMessage(err));
  }
});

define(
  'app_restart',
  'Stop and restart the session. By default keeps the profile (to reproduce persisted-state bugs).',
  { freshProfile: z.boolean().optional() },
  async (args) => {
    try {
      const fresh = args.freshProfile ?? false;
      // Keep the profile dir across the stop unless a fresh one was requested;
      // this is what makes "restart with persisted state" actually persist.
      await session.stop('app_restart', { keepProfile: !fresh });
      const res = await session.start({ build: 'skip', freshProfile: fresh });
      await resetOracles();
      logCursor = new LogCursor(session.browser);
      await logCursor.checkpoint().catch(() => undefined);
      return textResult(res);
    } catch (err) {
      return errorResult(asMessage(err));
    }
  },
);

// serialize:false - a pure in-process read with no WebDriver/recorder/oracle
// touch, kept unserialized so it stays a usable liveness probe even while a
// long build or scenario holds the tool chain.
define(
  'app_status',
  'Report session liveness and basic info.',
  {},
  async () => {
    return textResult({
      state: session.state,
      uptimeMs: session.uptimeMs(),
      driverAlive: session.driverAlive(),
      sessionDir: session.sessionDir,
      node: process.versions.node,
    });
  },
  { serialize: false },
);

// ------------------------------------------------------------- observation

define('screenshot', 'Capture a PNG screenshot (returned as image + saved to the session dir).', {
  name: z.string().optional(),
}, async (args) => {
  try {
    const { base64, path } = await record('screenshot', args, async (seq) => {
      const b64 = await session.exclusive(() => session.browser.takeScreenshot());
      const p = session.recorder.writeScreenshot(seq, args.name ?? 'shot', b64);
      return { result: { base64: b64, path: p }, artifacts: { screenshot: p } };
    });
    // darwin: the in-app WebDriver plugin rasterizes the DOM (SVG foreignObject),
    // so canvas/WebGL content - the survey map - is BLANK in the capture. Honest
    // flag rather than a silent lie; real pixels are Tier 2's os_screenshot (#421).
    const domOnly = isDarwinTarget();
    return {
      content: [
        { type: 'image', data: base64, mimeType: 'image/png' },
        {
          type: 'text',
          text: JSON.stringify(
            domOnly
              ? { path, domOnly, note: 'macOS capture is DOM-only: canvas/WebGL (the map) renders blank' }
              : { path },
          ),
        },
      ],
    };
  } catch (err) {
    return errorResult(asMessage(err));
  }
});

define(
  'store_state',
  'Read a Zustand store. With no path, returns a key/type summary; with a dot path, the value (JSON-capped).',
  { store: z.enum(STORE_NAMES), path: z.string().optional(), maxBytes: z.number().optional() },
  (args) =>
    run('store_state', args, async () => ({
      result: await session.exclusive(() =>
        readStore(session.browser, args.store as StoreName, args.path ?? null, args.maxBytes ?? 16384),
      ),
    })),
);

define(
  'logs',
  'Return ring-buffer log lines since the last read. level filters output; peek does not advance the cursor.',
  { level: z.enum(['all', 'warn', 'error']).optional(), peek: z.boolean().optional() },
  (args) =>
    run('logs', args, async () => {
      if (!logCursor) throw new Error('no session (call app_start)');
      return { result: await session.exclusive(() => logCursor!.read((args.level ?? 'error') as LogLevel, args.peek)) };
    }),
);

define(
  'wait_for',
  'Wait for a condition: selector visible/hidden, app-ready, ui-idle, map-idle, active-tool, or a store dot-path equalling a value.',
  {
    condition: z.enum(['selector', 'hidden', 'app-ready', 'ui-idle', 'map-idle', 'active-tool', 'store-path']),
    selector: z.string().optional(),
    tool: z.string().optional(),
    store: z.enum(STORE_NAMES).optional(),
    path: z.string().optional(),
    equals: z.unknown().optional(),
    timeoutMs: z.number().optional(),
  },
  (args) =>
    run('wait_for', args, async () => {
      const timeout = args.timeoutMs ?? 10000;
      await session.exclusive(async () => {
        const b = session.browser;
        switch (args.condition) {
          case 'selector':
            if (!args.selector) throw new Error('wait_for selector needs selector');
            await b.$(args.selector).waitForDisplayed({ timeout });
            break;
          case 'hidden':
            if (!args.selector) throw new Error('wait_for hidden needs selector');
            await b.$(args.selector).waitForDisplayed({ timeout, reverse: true });
            break;
          case 'app-ready':
            await waitForAppReady(b, timeout);
            break;
          case 'ui-idle':
            await waitForUiIdle(b, 0, timeout);
            break;
          case 'map-idle':
            await waitForMapIdle(b, 0, timeout);
            break;
          case 'active-tool':
            if (!args.tool) throw new Error('wait_for active-tool needs tool');
            await waitForActiveTool(b, args.tool, timeout);
            break;
          case 'store-path':
            if (!args.store || !args.path) throw new Error('wait_for store-path needs store and path');
            await b.waitUntil(
              async () => {
                const v = await readStore(b, args.store as StoreName, args.path!);
                return JSON.stringify((v as { value?: unknown }).value) === JSON.stringify(args.equals);
              },
              { timeout, timeoutMsg: 'store-path did not match' },
            );
            break;
          default:
            throw new Error(`unknown condition ${args.condition}`);
        }
      });
      return { result: { met: true } };
    }),
);

// ------------------------------------------------------------- interaction

define(
  'click',
  'Click an element by data-testid, CSS selector, or canvas coordinates. Optional right/middle button, double, and modifier keys.',
  {
    testid: z.string().optional(),
    selector: z.string().optional(),
    x: z.number().optional(),
    y: z.number().optional(),
    button: z.enum(['left', 'right', 'middle']).optional(),
    double: z.boolean().optional(),
    modifiers: z.array(z.enum(['Control', 'Shift', 'Alt'])).optional(),
  },
  (args) =>
    run(
      'click',
      args,
      async () => {
        const b = session.browser;
        const selector = targetSelector(args);
        const button = args.button ?? 'left';
        return session.exclusive(async () => {
          let point: { x: number; y: number } | undefined;
          const simple = button === 'left' && !args.double && !(args.modifiers && args.modifiers.length);
          if (selector) {
            const el = b.$(selector);
            await el.waitForClickable({ timeout: 5000 });
            if (simple) {
              await el.click();
            } else {
              await el.moveTo();
              await clickWithOptions(b, button, args.double, args.modifiers);
            }
          } else if (typeof args.x === 'number' && typeof args.y === 'number') {
            point = { x: Math.round(args.x), y: Math.round(args.y) };
            await b
              .action('pointer', { parameters: { pointerType: 'mouse' } })
              .move({ origin: 'viewport', x: point.x, y: point.y })
              .perform();
            await clickWithOptions(b, button, args.double, args.modifiers);
          } else {
            throw new Error('click needs one of testid, selector, or x+y');
          }
          return { result: { clicked: true }, resolved: { selector, point } };
        });
      },
      { digest: true, sweep: true },
    ),
);

define(
  'type_text',
  'Type text, optionally focusing a target first (testid/selector). clear empties it first; submit presses Enter after.',
  {
    text: z.string(),
    testid: z.string().optional(),
    selector: z.string().optional(),
    clear: z.boolean().optional(),
    submit: z.boolean().optional(),
  },
  (args) =>
    run(
      'type_text',
      args,
      async () => {
        const b = session.browser;
        const selector = targetSelector(args);
        return session.exclusive(async () => {
          let method: 'keys' | 'paste' = 'keys';
          let typed = true;
          let verified: boolean | null = null;
          let reason: string | undefined;
          let truncated: boolean | undefined;
          const applyReactSafe = async () => {
            method = 'paste';
            // Errors here (script threw, driver timeout) are left to
            // propagate to run()'s catch, same as every other tool - a
            // swallowed exception used to surface as a false `typed: true`.
            const outcome = await evalInApp<{ ok: boolean; verified?: boolean; truncated?: boolean; reason?: string }>(
              b,
              REACT_SAFE_SET_VALUE_SCRIPT,
              [selector, args.text, !!args.clear],
            );
            if (!outcome.ok) {
              typed = false;
              reason = outcome.reason;
              verified = null;
            } else {
              verified = outcome.verified ?? null;
              truncated = outcome.truncated ?? undefined;
            }
          };
          if (selector) {
            const el = b.$(selector);
            await el.waitForDisplayed({ timeout: 5000 });
            if (isDarwinTarget()) {
              // On this route el.setValue() lands the DOM value without
              // React seeing it (#531) - go straight to the React-safe
              // script rather than trusting a landed-value check.
              await applyReactSafe();
            } else {
              if (args.clear) await el.clearValue();
              await el.setValue(args.text);
              // WebKitWebDriver can coalesce rapid repeated keystrokes ("8000"
              // arriving as "800"), silently corrupting input. Verify the value
              // landed; on mismatch fall back to the React-safe script.
              const landed = (await el.getValue().catch(() => null)) as string | null;
              if (typeof landed === 'string') {
                verified = landed === args.text;
                if (!verified) await applyReactSafe();
              }
            }
          } else {
            await b.keys(args.text.split(''));
          }
          let submitted = false;
          if (args.submit) {
            // Never submit on a write that failed (e.g. a disabled/readOnly
            // target): pressing Enter would act on whatever already had
            // focus, not on the text this call was supposed to deliver.
            if (typed) {
              await b.keys(['Enter']);
              if (isDarwinTarget()) {
                // Same native-key path #531 found broken for Escape - the
                // Enter this submit relies on gets the same synthetic fallback.
                await evalInApp(b, SYNTHETIC_KEY_SCRIPT, ['Enter']);
              }
              submitted = true;
            }
          }
          return {
            result: {
              typed,
              method,
              verified,
              reason,
              truncated,
              submitted: args.submit ? submitted : undefined,
            },
            resolved: { selector },
          };
        });
      },
      { digest: true, sweep: true },
    ),
);

define('press_keys', 'Press a sequence of WebDriver keys (e.g. ["Control","z"]).', {
  keys: z.array(z.string()),
}, (args) =>
  run(
    'press_keys',
    args,
    async () => {
      await session.exclusive(() => session.browser.keys(args.keys));
      // #531 characterized exactly one shape: a lone ['Escape'] call
      // produces no observable effect natively on this route. Scoped this
      // narrowly - a multi-key sequence containing 'Escape' (e.g.
      // ['Shift','Escape'] or ['a','Escape','b']) was never verified and
      // firing a modifier-less synthetic Escape for it would be a guess.
      const singleEscape = args.keys.length === 1 && args.keys[0] === 'Escape';
      let syntheticEscape = false;
      // isDarwinTarget() is a platform predicate, not a route one in
      // general - but createSession() (../createSession.ts) only ever
      // builds a MacSession (the tauri-wd route) for darwin, so there is no
      // other darwin route to conflate here. Reported via `syntheticEscape`
      // regardless, so a call is never silently doubled without a trace.
      if (isDarwinTarget() && singleEscape) {
        await session.exclusive(() => evalInApp(session.browser, SYNTHETIC_KEY_SCRIPT, ['Escape']));
        syntheticEscape = true;
      }
      const caveats: string[] = [];
      if (isDarwinTarget() && args.keys.includes('Escape') && !singleEscape) {
        caveats.push(
          'Only a lone ["Escape"] call gets the verified synthetic fallback (#531) - this ' +
            'sequence relies on the native WebDriver path, whose delivery here is unconfirmed.',
        );
      }
      // darwin: keys are synthetic KeyboardEvents in the webview, so Meta chords
      // never reach the native menu - a "passed" Cmd shortcut here proves only
      // the DOM handler. No silent remap to Control: the app may bind both, and
      // rewriting would corrupt repro records (#421).
      if (isDarwinTarget() && args.keys.some((k) => k === 'Meta' || k === 'Command')) {
        caveats.push(
          'Meta is synthetic-only on macOS: native menu accelerators will not fire. ' +
            'Use the in-app Control binding, or Tier 2 os_keys for a real Cmd chord.',
        );
      }
      const result: Record<string, unknown> = { pressed: args.keys };
      if (syntheticEscape) result.syntheticEscape = true;
      if (caveats.length) result.caveat = caveats.join(' ');
      return { result };
    },
    { digest: true, sweep: true },
  ),
);

define(
  'hover',
  'Move the pointer over an element by data-testid or CSS selector.',
  { testid: z.string().optional(), selector: z.string().optional() },
  (args) =>
    run(
      'hover',
      args,
      async () => {
        const selector = await session.exclusive(() => hoverElement(session.browser, args));
        return { result: { hovered: true }, resolved: { selector } };
      },
      { sweep: true },
    ),
);

define(
  'drag',
  'Drag from an element or viewport point to another element or viewport point.',
  {
    fromTestid: z.string().optional(),
    fromSelector: z.string().optional(),
    fromX: z.number().optional(),
    fromY: z.number().optional(),
    toTestid: z.string().optional(),
    toSelector: z.string().optional(),
    toX: z.number().optional(),
    toY: z.number().optional(),
    button: z.enum(['left', 'right', 'middle']).optional(),
    durationMs: z.number().int().min(0).max(10000).optional(),
  },
  (args) =>
    run(
      'drag',
      args,
      async () => ({
        result: { dragged: true },
        resolved: await session.exclusive(() => dragPointer(session.browser, args)),
      }),
      { digest: true, sweep: true },
    ),
);

define(
  'scroll',
  'Scroll an element into view, or scroll an element/window by a pixel delta.',
  {
    testid: z.string().optional(),
    selector: z.string().optional(),
    deltaX: z.number().optional(),
    deltaY: z.number().optional(),
    block: z.enum(['start', 'center', 'end', 'nearest']).optional(),
    inline: z.enum(['start', 'center', 'end', 'nearest']).optional(),
  },
  (args) =>
    run(
      'scroll',
      args,
      async () => {
        const selector = await session.exclusive(() => scrollTarget(session.browser, args));
        return { result: { scrolled: true }, resolved: { selector } };
      },
      { sweep: true },
    ),
);

define(
  'select_option',
  'Select a value from a native select or Stalagmight Select identified by data-testid.',
  { testid: z.string().optional(), selector: z.string().optional(), value: z.string() },
  (args) =>
    run(
      'select_option',
      args,
      async () => ({
        result: { selected: args.value },
        resolved: await session.exclusive(() => selectOption(session.browser, args)),
      }),
      { digest: true, sweep: true },
    ),
);

define(
  'element_state',
  'Query matching element count and actionable DOM state. unique=false identifies duplicate runtime test IDs.',
  { testid: z.string().optional(), selector: z.string().optional() },
  (args) =>
    run('element_state', args, async () => ({
      result: capFolded(await session.exclusive(() => queryElementState(session.browser, args))),
    })),
);

define(
  'file_dialog_answer',
  'Trigger a real UI control and atomically answer the native open/save dialog it opens.',
  {
    kind: z.enum(['open', 'save']),
    triggerTestid: z.string().min(1),
    path: z.string().optional(),
    paths: z.array(z.string()).min(1).optional(),
    cancel: z.boolean().optional(),
  },
  (args) =>
    run('file_dialog_answer', args, async () => {
      const choices = Number(args.path !== undefined) + Number(args.paths !== undefined) + Number(args.cancel === true);
      if (choices !== 1) throw new Error('file_dialog_answer needs exactly one of path, paths, or cancel=true');
      if (args.kind === 'save' && args.paths) throw new Error('save dialog answer cannot contain multiple paths');
      const value = args.cancel ? null : (args.paths ?? args.path ?? null);
      const resolved = await session.exclusive(() =>
        answerFileDialog(session.browser, args.triggerTestid, args.kind, value),
      );
      return { result: { consumed: true, kind: args.kind, triggerTestid: args.triggerTestid }, resolved };
    },
    // Answering a native dialog commits an open/save - decisive in a repro, and
    // frequently the step a report most needs to show. It skips the oracle
    // sweep only because the app is mid-dialog, not because it is observation.
    { mutates: true }),
);

// ------------------------------------------------------------ escape hatches

define('eval', 'Run an async function body in the webview (page context) and return its value.', {
  script: z.string(),
  args: z.array(z.unknown()).optional(),
}, (args) =>
  run(
    'eval',
    args,
    async () => ({
      result: capFolded(await session.exclusive(() => evalInApp(session.browser, args.script, args.args ?? []))),
    }),
    { sweep: true },
  ),
);

define('ipc', 'Invoke a Tauri command from the webview (e.g. get_recent_logs_since, check_project_integrity).', {
  command: z.string(),
  args: z.record(z.string(), z.unknown()).optional(),
}, (args) =>
  run(
    'ipc',
    args,
    async () => ({
      result: capFolded(await session.exclusive(() => invokeInApp(session.browser, args.command, args.args))),
    }),
    { sweep: true },
  ),
);

define(
  'store_action',
  'Call an action on a Zustand store (generic seeding/driving primitive).',
  { store: z.enum(STORE_NAMES), action: z.string(), args: z.array(z.unknown()).optional() },
  (args) =>
    run(
      'store_action',
      args,
      async () => ({
        result: await session.exclusive(() =>
          callStoreAction(session.browser, args.store as StoreName, args.action, args.args ?? []),
        ),
      }),
      { digest: true, sweep: true },
    ),
);

// ---------------------------------------------------------------------- map

define('map_view', 'Read or set the MapLibre view (center/zoom/bearing).', {
  set: z
    .object({ center: z.array(z.number()).optional(), zoom: z.number().optional(), bearing: z.number().optional() })
    .optional(),
}, (args) =>
  run('map_view', args, async () => ({
    result: await session.exclusive(() =>
      evalInApp<{ ok: boolean; view?: unknown }>(
        session.browser,
        `
        const map = window.__stalagmight_map__;
        if (!map) return { ok: false };
        const set = args[0];
        if (set) map.jumpTo({ ...(set.center ? { center: set.center } : {}), ...(set.zoom != null ? { zoom: set.zoom } : {}), ...(set.bearing != null ? { bearing: set.bearing } : {}) });
        return { ok: true, view: { center: map.getCenter && map.getCenter(), zoom: map.getZoom && map.getZoom(), bearing: map.getBearing && map.getBearing() } };
      `,
        [args.set ?? null],
      ),
    ),
    // Read OR set: only the setting form moves the map, so this is a per-call
    // fact rather than a property of the tool. A bare read is observation and
    // does not belong in a list of steps to reproduce.
  }), { mutates: args.set != null }),
);

define('map_click', 'Click a visible geographic point through the active map interaction path.', {
  lngLat: z.array(z.number()).length(2),
  button: z.enum(['left', 'right']).optional(),
}, (args) =>
  run(
    'map_click',
    args,
    async () =>
      session.exclusive(async () => {
        const b = session.browser;
        const button = args.button ?? 'left';
        const resolved = await mapClick(b, args.lngLat as [number, number], button);
        return {
          result: { clicked: true, interaction: resolved.interaction, interactionVerified: true },
          resolved: {
            lngLat: args.lngLat,
            point: { x: resolved.x, y: resolved.y },
            interaction: resolved.interaction,
            interactionSequence: resolved.interactionSequence,
          },
        };
      }),
    { digest: true, sweep: true },
  ),
);

define(
  'map_feature_click',
  'Resolve a visible custom-WebGL shot or station by domain identity, prefer it across overlaps, and prove the map processed this click.',
  {
    featureType: z.enum(['shot', 'station']),
    sectionId: z.number().int(),
    shotId: z.number().int().optional(),
    stationIndex: z.number().int().optional(),
    button: z.enum(['left', 'right']).optional(),
    verifySelection: z.boolean().optional(),
  },
  (args) =>
    run(
      'map_feature_click',
      args,
      async () =>
        session.exclusive(async () => {
          const resolved = await mapFeatureClick(session.browser, args);
          return {
            result: {
              clicked: true,
              featureType: resolved.featureType,
              sectionId: resolved.sectionId,
              shotId: resolved.shotId,
              stationIndex: resolved.stationIndex,
              interactionVerified: resolved.interactionVerified,
              selectionVerified: resolved.selectionVerified,
            },
            resolved,
          };
        }),
      { digest: true, sweep: true },
    ),
);

// ----------------------------------------------------------------- seeding

define(
  'seed',
  'Seed a project: open a fixture (.stalx), open a fractal fixture, or create a basic empty project. Fixtures resolve under e2e/fixtures/.',
  {
    mode: z.enum(['fixture', 'fractal', 'basic']),
    name: z.string().optional(),
    projectName: z.string().optional(),
  },
  (args) =>
    run(
      'seed',
      args,
      async () => {
        if (args.mode === 'basic') {
          const res = await session.exclusive(() =>
            callStoreAction(session.browser, 'project', 'createProject', [args.projectName ?? 'harness-project']),
          );
          return { result: { mode: 'basic', result: res } };
        }
        const fixture = args.name ?? (args.mode === 'fractal' ? 'fractal_plant_iter3.stalx' : DEFAULT_FIXTURE);
        const p = materializeFixture(fixture, `${session.sessionDir}/fixtures`);
        const res = await session.exclusive(() =>
          callStoreAction(session.browser, 'project', 'loadProject', [p]),
        );
        return { result: { mode: args.mode, fixture, result: res }, resolved: { path: p } };
      },
      { digest: true, sweep: true },
    ),
);

// -------------------------------------------------------------- escape hatch: driver script

define(
  'driver_script',
  'Run a Node-context async function body with browser/ipc/evalInApp/readStore/callAction/log in scope, under the session mutex, for tight stress loops (one MCP call per action is ~100x too slow). Returns the value (JSON-capped).',
  { script: z.string(), timeoutMs: z.number().optional() },
  (args) =>
    run(
      'driver_script',
      args,
      async () => {
        const timeoutMs = args.timeoutMs ?? 60000;
        const value = await session.exclusive(async () => {
          // A promise cannot be killed: on timeout the script body keeps
          // running even though the mutex is released. Fence every helper
          // (and the browser proxy) so a timed-out script cannot keep driving
          // the app concurrently with later tool calls.
          let expired = false;
          const fence = () => {
            if (expired) throw new Error('driver_script timed out; its helpers are disabled');
          };
          const helpers = {
            browser: new Proxy(session.browser, {
              get(target, prop) {
                fence();
                const v = Reflect.get(target, prop) as unknown;
                return typeof v === 'function'
                  ? (v as (...a: unknown[]) => unknown).bind(target)
                  : v;
              },
            }),
            ipc: (c: string, a?: Record<string, unknown>) => {
              fence();
              return invokeInApp(session.browser, c, a);
            },
            evalInApp: (b: string, a?: unknown[]) => {
              fence();
              return evalInApp(session.browser, b, a ?? []);
            },
            readStore: (s: StoreName, p?: string) => {
              fence();
              return readStore(session.browser, s, p ?? null);
            },
            callAction: (s: StoreName, act: string, a?: unknown[]) => {
              fence();
              return callStoreAction(session.browser, s, act, a ?? []);
            },
            log: (m: unknown) => {
              if (session.hasRecorder) session.recorder.event('driver-log', { message: String(m) });
            },
          };
          const fn = new Function(
            'helpers',
            `return (async () => { const { browser, ipc, evalInApp, readStore, callAction, log } = helpers; ${args.script} })();`,
          );
          const body = fn(helpers) as Promise<unknown>;
          const raced = await withTimeout(body, timeoutMs);
          if (raced === TIMED_OUT) {
            expired = true;
            if (session.hasRecorder) session.recorder.event('driver-script-timeout', { timeoutMs });
            // A promise cannot be killed, and the fence only stops NEW helper
            // calls - an already-awaiting WebDriver command, or a script
            // driving an unproxied element/action handle, keeps running. Hold
            // the mutex for a bounded grace so it can settle; if the body is
            // STILL running after that, poison the session (state 'crashed')
            // so no later tool call can interleave with it on the same
            // WebDriver session - only app_restart recovers.
            const settled = await withTimeout(
              body.catch(() => undefined),
              10000,
            );
            if (settled === TIMED_OUT) {
              session.state = 'crashed';
              if (session.hasRecorder) {
                session.recorder.event('crash-detected', { reason: 'driver-script-wedged', timeoutMs });
              }
              throw new Error(
                `driver_script timed out after ${timeoutMs}ms and is still running; ` +
                  'session disabled to prevent concurrent WebDriver traffic - call app_restart',
              );
            }
            throw new Error(
              `driver_script timed out after ${timeoutMs}ms (body settled during the grace window; helpers disabled)`,
            );
          }
          return raced;
        });
        return { result: capFolded(value) };
      },
      { sweep: true },
    ),
);

// ------------------------------------- Tier 2: grant-gated OS input (darwin)
//
// Real trusted input (CGEvent) and real-pixel window capture - everything the
// synthetic WebDriver tier structurally cannot do. Registered only on darwin.
// os_grant is deliberately NEVER put on any permission allowlist, so invoking
// it always produces a real harness prompt: the human, not the model, holds
// the grant (valid until this server process exits). Every os_* result carries
// {frontmost_ok, permissions}; frontmost_ok false means the evidence is
// untrustworthy (discard it), not that the app misbehaved.

function defineDarwin<S extends z.ZodRawShape>(
  name: string,
  description: string,
  schema: S,
  handler: (args: z.infer<z.ZodObject<S>>) => Promise<ToolResult>,
): void {
  if (!isDarwinTarget()) return;
  define(name, description, schema, handler);
}

defineDarwin(
  'os_grant',
  'Grant OS-level input for this server process (real cursor + keyboard). Ask the user in chat first; this tool is deliberately un-allowlisted so calling it prompts them for real.',
  {},
  (args) =>
    run('os_grant', args, async () => {
      // Preflight FIRST: a grant that reports failure must not leave the gate
      // open. Recorded like any other action - it is the consent event every
      // later os_* step depends on, so a repro must show when it happened.
      const permissions = await osPreflight();
      osGrant();
      return {
        result: { granted: true, note: 'valid until this MCP server exits', permissions },
      };
    },
    // Not a change to app state, but the comment above is the contract: every
    // later os_* step depends on this consent, so the repro has to show it.
    // The sweep flag cannot express that - granting is not worth an oracle
    // pass - so it is declared.
    { mutates: true }),
);

defineDarwin(
  'os_click',
  'OS-level trusted click (moves the real cursor). Target a testid or screen-point coordinates.',
  {
    testid: z.string().optional(),
    x: z.number().optional(),
    y: z.number().optional(),
    button: z.enum(['left', 'right']).optional(),
    double: z.boolean().optional(),
    force: z.boolean().optional(),
  },
  (args) =>
    run(
      'os_click',
      args,
      async () => ({ result: await osClick(session, args) }),
      { digest: true, sweep: true },
    ),
);

defineDarwin(
  'os_keys',
  'OS-level trusted key chord (real modifiers - Cmd+S fires the actual menu accelerator). Spec grammar: see e2e/agent/os/README.md, e.g. "cmd+s", "cmd+shift+z", "escape".',
  { spec: z.string().min(1), force: z.boolean().optional() },
  (args) =>
    run(
      'os_keys',
      args,
      async () => ({ result: await osKeys(session, args.spec, undefined, { force: args.force }) }),
      { digest: true, sweep: true },
    ),
);

defineDarwin(
  'os_screenshot',
  'Real-pixel window capture via screencapture (occlusion-immune, needs Screen Recording). The WebGL map is actually visible here, unlike the DOM-only `screenshot`.',
  // The name is a LABEL, not a path: it is interpolated into a filename under
  // the session dir, so anything path-ish would let a caller write a PNG (and
  // mkdir) anywhere the user can write. Constrained at the schema boundary.
  { name: z.string().regex(/^[\w.-]{1,64}$/).optional() },
  async (args) => {
    try {
      // Observation, despite the os_ prefix: capturing real pixels changes
      // nothing, so it is not a step in a repro. Declared rather than left to
      // the default, because the prefix makes the opposite look plausible.
      const { base64, meta } = await record('os_screenshot', args, async (seq) => {
        const file = path.join(
          session.recorder.dir,
          'screenshots',
          `${String(seq).padStart(4, '0')}-${path.basename(args.name ?? 'os-shot')}.png`,
        );
        const shot = await osScreenshot(session, file);
        const b64 = fs.readFileSync(shot.path).toString('base64');
        return {
          result: { base64: b64, path: shot.path, meta: shot },
          artifacts: { screenshot: shot.path },
        };
      });
      return {
        content: [
          { type: 'image', data: base64, mimeType: 'image/png' },
          { type: 'text', text: JSON.stringify(meta) },
        ],
      };
    } catch (err) {
      return errorResult(asMessage(err));
    }
  },
);

// ------------------------------------------------------------- oracles + scenarios

define(
  'check_problems',
  'Run the full oracle sweep: crash markers, error/warn logs, error boundary, project integrity, leftover temp files, memory growth, and a responsiveness probe. Use after scenarios and before ending a session.',
  {},
  async () => {
    try {
      const result = await record('check_problems', {}, async () => {
        const sweep = await runSweep(session, oracleState, {
          full: true,
          recorder: session.hasRecorder ? session.recorder : undefined,
        });
        return {
          result: {
            ok: sweep.worst === null,
            worst: sweep.worst,
            findings: sweep.findings,
            newFindings: sweep.newFindings,
          },
        };
      });
      return textResult(result);
    } catch (err) {
      return errorResult(asMessage(err));
    }
  },
);

define(
  'run_scenario',
  `Run a named stress scenario against the current session, then a full sweep. Scenarios: ${listScenarios().join(', ')}.`,
  { name: z.string(), seed: z.number().optional(), iterations: z.number().optional() },
  async (args) => {
    try {
      // record() puts the scenario run in actions.jsonl so replays and repro
      // reports include it (a scenario is a mutating action, not observation).
      const metrics = await record('run_scenario', args, async () => ({
        result: await runScenario(args.name, session, {
          seed: args.seed,
          iterations: args.iterations,
          log: (m) => {
            if (session.hasRecorder) session.recorder.event('scenario-log', { scenario: args.name, message: m });
          },
        }),
      }), { mutates: true });
      const sweep = await runSweep(session, oracleState, {
        full: true,
        recorder: session.hasRecorder ? session.recorder : undefined,
      });
      // The findings quote the app's error log, which quotes the path of a
      // corpus copy it failed to import (#576 HFI-13): runSweep hands them over
      // folded, and textResult folds the whole answer again.
      return textResult({
        scenario: args.name,
        metrics,
        ok: sweep.worst === null || sweep.worst === 'warning',
        worst: sweep.worst,
        findings: sweep.findings,
      });
    } catch (err) {
      return errorResult(asMessage(err));
    }
  },
);

// -------------------------------------------------------------- helpers

const TIMED_OUT = Symbol('timed-out');
async function withTimeout<T>(p: Promise<T>, ms: number): Promise<T | typeof TIMED_OUT> {
  let timer: NodeJS.Timeout | undefined;
  const timeout = new Promise<typeof TIMED_OUT>((resolve) => {
    timer = setTimeout(() => resolve(TIMED_OUT), ms);
  });
  try {
    return await Promise.race([p, timeout]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

async function clickWithOptions(
  b: AgentSession['browser'],
  button: 'left' | 'right' | 'middle',
  double?: boolean,
  modifiers?: Array<'Control' | 'Shift' | 'Alt'>,
): Promise<void> {
  const mods = modifiers ?? [];
  if (mods.length) {
    const key = b.action('key');
    for (const m of mods) key.down(m);
    await key.perform();
  }
  try {
    const pointer = b
      .action('pointer', { parameters: { pointerType: 'mouse' } })
      .down({ button })
      .up({ button });
    if (double) pointer.down({ button }).up({ button });
    await pointer.perform();
  } finally {
    // Always release modifiers, or a failed click leaves e.g. Control held
    // down for every subsequent action in the session.
    if (mods.length) {
      const up = b.action('key');
      for (const m of mods) up.up(m);
      await up.perform().catch(() => undefined);
    }
  }
}

// ---------------------------------------------------------------------- boot

async function main(): Promise<void> {
  const transport = new StdioServerTransport();
  await server.connect(transport);
  process.stderr.write('[agent-mcp] server connected on stdio\n');

  const shutdown = async () => {
    try {
      if (session.state !== 'idle') await session.stop('server-shutdown');
    } finally {
      process.exit(0);
    }
  };
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);
  process.stdin.on('close', shutdown);
}

main().catch((err) => {
  process.stderr.write(`[agent-mcp] fatal: ${err instanceof Error ? err.stack : String(err)}\n`);
  process.exit(1);
});
