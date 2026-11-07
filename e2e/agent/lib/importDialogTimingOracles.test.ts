// @vitest-environment jsdom
/**
 * #576 HFI2-F3 / F4 / F5: three import-fuzzer oracles that judged the wrong
 * moment or the wrong number, each a false FAIL on the harness host.
 *
 * - F4: the settle loop did not wait for a running orphan-placement Accept, so
 *   the next move's "undo before" was read mid-chain and OVERLAP MODES blamed
 *   the picker for the Accept's own undo pushes.
 * - F3: IMPORT GATE looked for the refusal toast in the digest taken after the
 *   settle - up to 12 s later when a wizard calculation was running, and the
 *   toast lives about 9 s. The refusal is now read from the store on the tick
 *   the route returns.
 * - F5: FROZEN DELETE compared RENDERED rows of a virtualized grid, which grow
 *   when the grid first measures its box. It now compares the grid's data row
 *   count (`aria-rowcount`).
 *
 * The bridge's in-page code runs here as shipped: `routeFile` through
 * `evalInApp` against jsdom, `HELPERS` evaluated as the page evaluates it, and
 * the screen digest itself. Everything on screen is synthetic.
 */
import { afterEach, describe, expect, it } from 'vitest';
import type { Browser } from 'webdriverio';

import type { ScenarioContext } from '../scenarios/framework';
import { HELPERS, routeFile } from '../scenarios/importDialogBridge';
import { checkFrozenDelete, ImportOracleError, importGateRefusalSeen, toastKeysRaised, type ToastLike } from './importFlowOracles';
import { computeImportScreenDigest, IMPORT_SCREEN_GLOBAL } from './importScreenDigest';
import { evalInApp } from './page';

/** A synthetic toast key; the oracle takes the real one as an argument. */
const GATE = 'test-import-gate';

type TestWindow = Window & { __E2E_TEST__?: Record<string, unknown> };
const testWindow = (): TestWindow => window as TestWindow;

afterEach(() => {
  delete testWindow().__E2E_TEST__;
  delete (globalThis as Record<string, unknown>)[IMPORT_SCREEN_GLOBAL];
  document.body.innerHTML = '';
});

/** Runs an eval body in this jsdom, exactly as WebDriver runs it in the webview. */
const ctx = {
  eval: (body: string, args: unknown[] = []) =>
    evalInApp(
      {
        executeAsync: (runner: (fnArgs: unknown[], done: (r: unknown) => void) => void, fnArgs: unknown[]) =>
          new Promise((resolve) => runner(fnArgs, resolve)),
      } as unknown as Browser,
      body,
      args,
    ),
} as unknown as ScenarioContext;

/** The bridge's in-page helpers, compiled against this document (digest injected, as the run does). */
function bridgeHelpers(): { busy: () => boolean; settle: (capMs: number) => Promise<boolean> } {
  (globalThis as Record<string, unknown>)[IMPORT_SCREEN_GLOBAL] = { digest: computeImportScreenDigest };
  return new Function(`${HELPERS}\nreturn { busy, settle };`)() as {
    busy: () => boolean;
    settle: (capMs: number) => Promise<boolean>;
  };
}

/** OrphanPlacementDialog's buttons; every one is `disabled={processing}`. */
function mountOrphanPlacement(processing: boolean, withSuggestion = true): void {
  const off = processing ? ' disabled' : '';
  document.body.innerHTML =
    '<div data-testid="orphan-placement-dialog">' +
    (withSuggestion
      ? `<button data-testid="orphan-accept"${off}>Accept</button><button data-testid="orphan-skip"${off}>Skip</button>`
      : '') +
    `<button data-testid="orphan-accept-all"${off}>Accept all</button>` +
    `<button data-testid="orphan-close"${off}>Skip all</button>` +
    '</div>';
}

describe('HFI2-F4: the settle waits for a running orphan-placement Accept', () => {
  it('reads an Accept in flight as busy, with or without a suggestion on screen', () => {
    const verdicts: Record<string, boolean> = {};
    mountOrphanPlacement(true);
    verdicts.accepting = bridgeHelpers().busy();
    mountOrphanPlacement(true, false);
    verdicts.acceptingAll = bridgeHelpers().busy();
    mountOrphanPlacement(false);
    verdicts.idle = bridgeHelpers().busy();

    expect(verdicts).toEqual({ accepting: true, acceptingAll: true, idle: false });
  });

  it('settles once the Accept ends, not while it runs', async () => {
    mountOrphanPlacement(true);
    const { settle } = bridgeHelpers();
    const endsAt = 400;
    setTimeout(() => mountOrphanPlacement(false), endsAt);

    const started = Date.now();
    const settled = await settle(5000);

    expect({ settled, waitedForTheAccept: Date.now() - started >= endsAt }).toEqual({
      settled: true,
      waitedForTheAccept: true,
    });
  });
});

/** An app whose router does `onRoute` to the toast stack. */
function appRouting(toasts: ToastLike[], onRoute: (toasts: ToastLike[]) => ToastLike[]): { toasts: () => ToastLike[] } {
  let state = { activeDialog: 'dmp-import-new', placementMode: null, toasts };
  testWindow().__E2E_TEST__ = {
    uiStore: { getState: () => state },
    routeOpenedFile: async () => {
      state = { ...state, toasts: onRoute(state.toasts) };
    },
  };
  return { toasts: () => state.toasts };
}

describe('HFI2-F3: the import-gate refusal is read when the route returns', () => {
  it('reports the refusal toast the route raised, though it is gone by the digest', async () => {
    const app = appRouting([], (toasts) => [...toasts, { id: 7, revision: 0, key: GATE, leaving: false }]);

    const outcome = await routeFile(ctx, '/tmp/second.dmp');
    // ~9 s later the toast has faded; a wizard calculation held the settle 12 s.
    const digestToasts = app.toasts().filter((t) => t.id !== 7).map((t) => t.key);

    expect(outcome.toastsRaised).toEqual([GATE]);
    expect(importGateRefusalSeen(outcome.toastsRaised, digestToasts, GATE)).toBe(true);
  });

  it('counts a refresh of a toast already up, and not a toast the route did not raise', async () => {
    const stale: ToastLike = { id: 3, revision: 0, key: GATE, leaving: false };
    appRouting([stale], (toasts) => toasts.map((t) => (t.id === 3 ? { ...t, revision: 1 } : t)));
    const refreshed = await routeFile(ctx, '/tmp/second.dmp');
    appRouting([stale], (toasts) => toasts);
    const accepted = await routeFile(ctx, '/tmp/second.dmp');

    expect({ refreshed: refreshed.toastsRaised, accepted: accepted.toastsRaised }).toEqual({
      refreshed: [GATE],
      accepted: [],
    });
    // The stale toast in the digest is not this route's refusal.
    expect(importGateRefusalSeen(accepted.toastsRaised, [GATE], GATE)).toBe(false);
    // A run without the route's reading falls back to the digest.
    expect(importGateRefusalSeen(null, [GATE], GATE)).toBe(true);
  });

  it('keeps the toast diff injectable: self-contained, and leaving toasts ignored', () => {
    const injected = new Function(`return ${toastKeysRaised.toString()};`)() as typeof toastKeysRaised;
    const before: ToastLike[] = [{ id: 1, revision: 0, key: 'a', leaving: false }];
    const after: ToastLike[] = [
      { id: 1, revision: 0, key: 'a', leaving: false },
      { id: 2, revision: 0, key: 'b', leaving: true },
      { id: 3, revision: 0, key: GATE, leaving: false },
    ];
    expect(injected(before, after)).toEqual([GATE]);
  });
});

/** A shot grid with `data` rows of which `rendered` are in the DOM (virtualized). */
function mountGrid(data: number, rendered: number): void {
  const rows = Array.from({ length: rendered }, (_, i) => `<div data-testid="shot-editor-row-${i}"></div>`).join('');
  document.body.innerHTML =
    `<div data-testid="shot-table"><div role="grid" aria-rowcount="${data + 1}">${rows}</div></div>`;
  testWindow().__E2E_TEST__ = {
    uiStore: { getState: () => ({ activeDialog: null, placementMode: null, dialogData: {} }) },
    projectStore: { getState: () => ({ project: null }) },
  };
}

const digestNow = () => computeImportScreenDigest({ inventory: [], selects: [] });

describe('HFI2-F5: FROZEN DELETE compares the grid data rows, not the rendered ones', () => {
  it('reads the data row count off the grid, whatever the window renders', async () => {
    mountGrid(24, 15); // before the grid measured its box (DEFAULT_GRID_HEIGHT)
    const before = await digestNow();
    mountGrid(24, 24); // measured: more rows rendered, none deleted
    const after = await digestNow();

    expect({ before: [before.rows, before.dataRows], after: [after.rows, after.dataRows] }).toEqual({
      before: [15, 24],
      after: [24, 24],
    });
    expect(() => checkFrozenDelete(before, { ...after, toast: ['dmp-wizard-frozen-delete'] })).not.toThrow();
  });

  it('still fails a delete that removed rows, and a refusal that said nothing', async () => {
    mountGrid(24, 15);
    const before = await digestNow();
    mountGrid(23, 15);
    const deleted = await digestNow();

    expect(() => checkFrozenDelete(before, { ...deleted, toast: ['dmp-wizard-frozen-delete'] })).toThrow(
      /FROZEN DELETE: an all-frozen selection was deleted anyway[\s\S]*data rows 24 -> 23/,
    );
    expect(() => checkFrozenDelete(before, { ...before, toast: [] })).toThrow(ImportOracleError);
    expect(() => checkFrozenDelete(before, { ...before, toast: [] })).toThrow(/the refusal was SILENT/);
  });
});
