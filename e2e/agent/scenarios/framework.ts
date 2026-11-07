/**
 * Stress-scenario framework. A Scenario is a deterministic (seeded-PRNG) loop
 * of app interactions run through a ScenarioContext. The context exposes the
 * same primitives as the MCP tools (eval/ipc/store reads+actions/keys) but as
 * direct Node calls, so a scenario can do thousands of interactions without the
 * per-call MCP round-trip. Every context primitive goes through the session
 * command mutex, so a scenario never interleaves with another WebDriver caller.
 *
 * A scenario FAILS by throwing (a thrown step aborts the run). Invariant
 * breaches should throw with a clear message. Timing/memory observations are
 * recorded as metrics (warnings), not failures; the oracle full-sweep that the
 * runner's callers perform afterwards turns real problems into findings.
 */
import type { Browser } from 'webdriverio';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { invokeInApp, evalInApp } from '../lib/page';
import { readStore, callStoreAction, type StoreName } from '../lib/stores';
import { materializeFixture } from '../lib/fixtures';

/** Minimal session surface a scenario needs; AppSession satisfies it. */
export interface ScenarioTarget {
  readonly browser: Browser;
  readonly sessionDir?: string;
  exclusive<T>(fn: () => Promise<T>): Promise<T>;
}

export interface ScenarioContext {
  readonly browser: Browser;
  exclusive<T>(fn: () => Promise<T>): Promise<T>;
  eval<T = unknown>(body: string, args?: unknown[]): Promise<T>;
  ipc<T = unknown>(command: string, args?: Record<string, unknown>): Promise<T>;
  readStore(store: StoreName, path?: string | null): Promise<unknown>;
  callAction(
    store: StoreName,
    action: string,
    args?: unknown[],
  ): Promise<{ ok: boolean; result?: unknown; error?: string }>;
  keys(keys: string[]): Promise<void>;
  /**
   * The PRNG seed this run was built with (the effective one - a falsy seed
   * becomes 1). Exposed so a failing scenario can print the exact
   * `run_scenario(name, seed, iterations)` that reproduces it.
   */
  readonly seed: number;
  /**
   * The session's artifact directory, when the runner has one. Scenarios drop
   * failure traces here (`e2e/agent/lib/fuzzTrace.ts`); null when a caller
   * constructed a context without a session dir, in which case a trace is only
   * written if `STALAGMIGHT_FUZZ_TRACE` names a path.
   */
  readonly artifactsDir: string | null;
  /** Uniform in [0, 1). Deterministic for a given seed. */
  rng(): number;
  /** Integer in [min, max] inclusive. */
  randInt(min: number, max: number): number;
  pick<T>(items: readonly T[]): T;
  log(message: string): void;
  metric(key: string, delta?: number): void;
  setMetric(key: string, value: number): void;
  sleep(ms: number): Promise<void>;
  fixturePath(name: string): string;
  readonly metrics: Record<string, number>;
}

export interface Scenario {
  name: string;
  description: string;
  defaultIterations: number;
  run(ctx: ScenarioContext, opts: { iterations: number }): Promise<void>;
}

export interface ScenarioMetrics {
  scenario: string;
  seed: number;
  iterations: number;
  durationMs: number;
  metrics: Record<string, number>;
}

/**
 * mulberry32: tiny, fast, deterministic PRNG.
 *
 * Exported so a scenario can derive an INDEPENDENT stream from the same seed
 * (e.g. `mulberry32(seed ^ 0x9e3779b9)`) for decisions it wants to add without
 * shifting the primary draw sequence every existing repro depends on.
 */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export function makeContext(
  target: ScenarioTarget,
  seed: number,
  onLog?: (message: string) => void,
): ScenarioContext & { metrics: Record<string, number> } {
  // A number whatever the caller read it from (#576 W51): `fuzz-shrink` and a
  // generated replay spec take the seed out of a trace or an actions.jsonl
  // that a forged copy can fill with text, and `ctx.seed` prints in progress
  // and reproduce lines.
  const effectiveSeed = Number(seed) || 1;
  const rand = mulberry32(effectiveSeed);
  const metrics: Record<string, number> = {};
  const fixtureDir = target.sessionDir
    ? path.join(target.sessionDir, 'fixtures')
    : fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-scenario-fixtures-'));
  return {
    get browser() {
      return target.browser;
    },
    metrics,
    exclusive: (fn) => target.exclusive(fn),
    eval: (body, args = []) => target.exclusive(() => evalInApp(target.browser, body, args)),
    ipc: (command, args) => target.exclusive(() => invokeInApp(target.browser, command, args)),
    readStore: (store, path = null) => target.exclusive(() => readStore(target.browser, store, path)),
    callAction: (store, action, args = []) =>
      target.exclusive(() => callStoreAction(target.browser, store, action, args)),
    keys: (keys) => target.exclusive(() => target.browser.keys(keys)),
    seed: effectiveSeed,
    artifactsDir: target.sessionDir ?? null,
    rng: () => rand(),
    randInt: (min, max) => min + Math.floor(rand() * (max - min + 1)),
    pick: (items) => items[Math.floor(rand() * items.length)] as (typeof items)[number],
    log: (message) => onLog?.(message),
    metric: (key, delta = 1) => {
      metrics[key] = (metrics[key] ?? 0) + delta;
    },
    setMetric: (key, value) => {
      metrics[key] = value;
    },
    sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
    fixturePath: (name) => materializeFixture(name, fixtureDir),
  };
}
