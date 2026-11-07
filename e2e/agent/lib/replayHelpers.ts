/**
 * Helpers a generated replay spec imports. Replay specs were written for the
 * retired WDIO runner's ambient `browser`/`$` globals (#549; the #466 replay
 * runner replaces it), so these take the browser explicitly and wrap
 * the same page-context primitives the live harness uses. Keeping the generated
 * spec's imports pointed here (not at internal lib layout) decouples committed
 * replays from harness refactors.
 */
import type { Browser } from 'webdriverio';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { invokeInApp, evalInApp } from './page';
import { readStore, callStoreAction, type StoreName } from './stores';
import { runScenario } from '../scenarios';
import {
  dragPointer,
  hoverElement,
  mapClick,
  answerFileDialog,
  scrollTarget,
  selectOption,
  type DragRequest,
  type ElementTarget,
  type ScrollRequest,
  type SelectOptionRequest,
} from './controls';
import { waitForActiveTool, waitForAppReady, waitForMapIdle, waitForUiIdle } from './waiters';
import { mapFeatureClick, type MapFeatureClickRequest } from './mapControls';
import { materializeFixture } from './fixtures';

export { invokeInApp as ipc, evalInApp };
export { callStoreAction as storeAction };
export { answerFileDialog, dragPointer, hoverElement, mapClick, scrollTarget, selectOption };
export { mapFeatureClick };

const replayFixtureDirs = new Set<string>();

/** Give every replay seed a fresh copy that the app may safely migrate or save. */
export function materializeReplayFixture(name: string): string {
  const fixtureDir = mkdtempSync(join(tmpdir(), 'stalagmight-replay-fixture-'));
  replayFixtureDirs.add(fixtureDir);
  return materializeFixture(name, fixtureDir);
}

/** Remove fixture databases and any SQLite sidecars created during a replay. */
export function cleanupReplayFixtures(): void {
  for (const fixtureDir of replayFixtureDirs) rmSync(fixtureDir, { recursive: true, force: true });
  replayFixtureDirs.clear();
}

interface ReplayWaitRequest {
  condition: 'selector' | 'hidden' | 'app-ready' | 'ui-idle' | 'map-idle' | 'active-tool' | 'store-path';
  selector?: string;
  tool?: string;
  store?: StoreName;
  path?: string;
  equals?: unknown;
  timeoutMs?: number;
}

interface ReplayClickRequest {
  selector?: string;
  x?: number;
  y?: number;
  button?: 'left' | 'right' | 'middle';
  double?: boolean;
  modifiers?: Array<'Control' | 'Shift' | 'Alt'>;
}

async function settlesWithin(promise: Promise<unknown>, timeoutMs: number): Promise<boolean> {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      promise.then(() => true, () => true),
      new Promise<false>((resolve) => { timer = setTimeout(() => resolve(false), timeoutMs); }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

/** Replay a click without dropping its button, double-click, or modifier semantics. */
export async function replayClick(browser: Browser, request: ReplayClickRequest): Promise<void> {
  const button = request.button ?? 'left';
  const simple = button === 'left' && !request.double && !request.modifiers?.length;
  if (request.selector) {
    const element = browser.$(request.selector);
    await element.waitForClickable({ timeout: 5000 });
    if (simple) {
      await element.click();
      return;
    }
    await element.moveTo();
  } else if (typeof request.x === 'number' && typeof request.y === 'number') {
    await browser
      .action('pointer', { parameters: { pointerType: 'mouse' } })
      .move({ origin: 'viewport', x: Math.round(request.x), y: Math.round(request.y) })
      .perform();
  } else {
    throw new Error('replayed click has no target');
  }

  const modifiers = request.modifiers ?? [];
  if (modifiers.length) {
    const keys = browser.action('key');
    for (const modifier of modifiers) keys.down(modifier);
    await keys.perform();
  }
  try {
    const pointer = browser.action('pointer', { parameters: { pointerType: 'mouse' } });
    pointer.down({ button }).up({ button });
    if (request.double) pointer.down({ button }).up({ button });
    await pointer.perform();
  } finally {
    if (modifiers.length) {
      const keys = browser.action('key');
      for (const modifier of modifiers) keys.up(modifier);
      await keys.perform().catch(() => undefined);
    }
  }
}

/** Preserve the semantic waiter recorded by the live harness. */
export async function waitForReplay(browser: Browser, request: ReplayWaitRequest): Promise<void> {
  const timeout = request.timeoutMs ?? 10000;
  switch (request.condition) {
    case 'selector': {
      if (!request.selector) throw new Error('replayed selector wait has no selector');
      await browser.$(request.selector).waitForDisplayed({ timeout });
      return;
    }
    case 'hidden': {
      if (!request.selector) throw new Error('replayed hidden wait has no selector');
      await browser.$(request.selector).waitForDisplayed({ timeout, reverse: true });
      return;
    }
    case 'app-ready':
      await waitForAppReady(browser, timeout);
      return;
    case 'ui-idle':
      await waitForUiIdle(browser, 0, timeout);
      return;
    case 'map-idle':
      await waitForMapIdle(browser, 0, timeout);
      return;
    case 'active-tool':
      if (!request.tool) throw new Error('replayed active-tool wait has no tool');
      await waitForActiveTool(browser, request.tool, timeout);
      return;
    case 'store-path': {
      if (!request.store || !request.path) throw new Error('replayed store-path wait is incomplete');
      await browser.waitUntil(
        async () => {
          const value = await readStore(browser, request.store!, request.path!);
          return JSON.stringify((value as { value?: unknown }).value) === JSON.stringify(request.equals);
        },
        { timeout, timeoutMsg: 'replayed store-path did not match' },
      );
    }
  }
}

export type { DragRequest, ElementTarget, MapFeatureClickRequest, ScrollRequest, SelectOptionRequest };

/** Click at a canvas-relative pixel (for map/canvas features with no DOM node). */
export async function clickAt(
  browser: Browser,
  x: number,
  y: number,
  button: 'left' | 'right' | 'middle' = 'left',
): Promise<void> {
  await browser
    .action('pointer', { parameters: { pointerType: 'mouse' } })
    .move({ x: Math.round(x), y: Math.round(y) })
    .down({ button })
    .up({ button })
    .perform();
}

/** Bounded pause between replayed actions (recorded inter-action gaps, capped). */
export async function pause(browser: Browser, ms: number): Promise<void> {
  await browser.pause(Math.max(0, Math.min(ms, 2000)));
}

/**
 * Re-run a recorded driver_script body with the same helpers in scope it had
 * live (browser/ipc/evalInApp/readStore/callAction/log). The spec runs
 * single-threaded, so no mutex is needed here. Bounded by `timeoutMs` exactly
 * like the live tool, so a script that timed out during recording (an infinite
 * or slow loop) cannot hang the replay indefinitely.
 */
export async function driverScript(
  browser: Browser,
  script: string,
  timeoutMs = 60000,
): Promise<unknown> {
  let expired = false;
  const fence = () => {
    if (expired) throw new Error('replayed driver_script timed out; its helpers are disabled');
  };
  const helpers = {
    browser: new Proxy(browser, {
      get(target, prop) {
        fence();
        const value = Reflect.get(target, prop) as unknown;
        return typeof value === 'function'
          ? (...args: unknown[]) => {
              fence();
              return Reflect.apply(value as (...callArgs: unknown[]) => unknown, target, args);
            }
          : value;
      },
    }),
    ipc: (c: string, a?: Record<string, unknown>) => {
      fence();
      return invokeInApp(browser, c, a);
    },
    evalInApp: (b: string, a?: unknown[]) => {
      fence();
      return evalInApp(browser, b, a ?? []);
    },
    readStore: (s: StoreName, p?: string) => {
      fence();
      return readStore(browser, s, p ?? null);
    },
    callAction: (s: StoreName, act: string, a?: unknown[]) => {
      fence();
      return callStoreAction(browser, s, act, a ?? []);
    },
    log: (_m: unknown) => undefined,
  };
  const fn = new Function(
    'helpers',
    `return (async () => { const { browser, ipc, evalInApp, readStore, callAction, log } = helpers; ${script} })();`,
  );
  let timer: NodeJS.Timeout | undefined;
  const body = fn(helpers) as Promise<unknown>;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => {
        expired = true;
        reject(new Error(`replayed driver_script timed out after ${timeoutMs}ms`));
      },
      timeoutMs,
    );
  });
  try {
    return await Promise.race([body, timeout]);
  } catch (error) {
    if (!expired) throw error;
    // A replay timeout is terminal for its dedicated WebDriver session. This
    // also quarantines element/action handles captured before the timeout.
    const quarantined = typeof browser.deleteSession === 'function'
      ? settlesWithin(browser.deleteSession(), 1000)
      : Promise.resolve(true);
    const [settled] = await Promise.all([settlesWithin(body, 10000), quarantined]);
    if (!settled) {
      throw new Error(
        `replayed driver_script timed out after ${timeoutMs}ms and is still running; WebDriver session closed`,
      );
    }
    throw new Error(
      `replayed driver_script timed out after ${timeoutMs}ms (body settled during the grace window; helpers disabled)`,
    );
  } finally {
    if (timer) clearTimeout(timer);
  }
}

/**
 * Re-run a recorded stress scenario with the recorded seed/iterations. The
 * scenarios are seeded-PRNG deterministic, so the replayed interaction stream
 * matches the recorded one.
 */
export async function runScenarioReplay(
  browser: Browser,
  name: string,
  seed?: number,
  iterations?: number,
): Promise<void> {
  await runScenario(name, { browser, exclusive: (fn) => fn() }, { seed, iterations });
}
