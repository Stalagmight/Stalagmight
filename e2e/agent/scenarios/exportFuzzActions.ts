/**
 * The draw and the driving half of `export-roundtrip-fuzz`.
 *
 * `lib/exportOracles.ts` judges; this file decides what to do and does it.
 * Same split (and the same allowlist argument) as
 * `importDialogGenerators.ts` + `importDialogBridge.ts`, collapsed into one
 * module because the export surface is one form with sixteen controls rather
 * than eleven screens.
 *
 * ## The allowlist is the security boundary, again
 *
 * Every testid a move may name is a LITERAL in `EXPORT_TESTIDS` below. On a
 * replay that id comes out of a trace file, which `readTrace` checks only for
 * schema shape - so without the set a crafted trace could click
 * `delete-project-confirm` or a SpeleoDB push inside a privileged webview with
 * the `__E2E_TEST__` hooks live. The set is small enough to read in one screen,
 * which is the point.
 *
 * ## Why this file has its own in-page helpers
 *
 * `importDialogBridge.ts`'s `HELPERS` are welded to the import screen digest
 * (they throw if it is not injected) and its settle marker watches four wizard
 * busy flags that no export path raises. Reusing them would make this scenario
 * depend on the import model for no benefit; the helpers here are the same
 * click/visibility/settle idea over the markers an export actually has.
 *
 * ## Impossible moves are skipped and counted, never asserted
 *
 * Every driver returns a reason instead of throwing when its control is not
 * there. A shrunk trace reaches states the original run never had - Export
 * already left, no project open, the panel unmounted - and half its moves become
 * impossible there. That is what keeps a ddmin-cut trace RUNNABLE.
 *
 * ## Entering and leaving the Export workspace
 *
 * Export is a workspace with a recorded way back, not a mode switch: a panel
 * move enters through a drawn `ExportEntry` (the menu row, Ctrl+E, or
 * `enterExport`) from a drawn mode, and leaves through a drawn `ExportExit`
 * (the toolbar close, the panel close, the menu row, Ctrl+E, Escape) - so every
 * panel export also exercises the code that decides where the user lands, and
 * X10 judges it. A drawn affordance whose control is absent falls back to the
 * store action (`enterExport` / `closeExport`) and is counted, never asserted.
 */
import type { ScenarioContext } from './framework';
import {
  DATA_FORMATS,
  DRAWN_EXPORT_EXITS,
  EXPORT_ENTRIES,
  IMAGE_FORMATS,
  naturalExtension,
  PATH_SHAPES,
  RETURNABLE_MODES,
  type ExportEntry,
  type ExportExit,
  type ExportFormat,
  type ExportRoute,
  type ModeSnapshot,
  type OptionSnapshot,
  type PathShape,
  type ReturnableMode,
  type ToastSnapshot,
} from '../lib/exportOracles';

// ---------------------------------------------------------------------------
// Knobs. Read at CALL time so a replay's restored environment reaches them.
// ---------------------------------------------------------------------------

/**
 * The format axis. `STALAGMIGHT_FUZZ_EXPORT_FORMATS` is a csv list; an unknown
 * name is a hard error rather than a silent drop, because a typo'd axis value
 * in a matrix cell would otherwise run the full default set and be reported as
 * having covered something it did not.
 */
export function enabledFormats(): ExportFormat[] {
  const raw = (process.env.STALAGMIGHT_FUZZ_EXPORT_FORMATS ?? '').trim();
  if (raw === '') return [...ALL_FORMATS];
  const wanted = raw
    .split(',')
    .map((entry) => entry.trim())
    .filter(Boolean);
  const out: ExportFormat[] = [];
  for (const name of wanted) {
    const found = ALL_FORMATS.find((format) => format === name);
    if (found === undefined) {
      throw new Error(
        `STALAGMIGHT_FUZZ_EXPORT_FORMATS names "${name}", which is not a format this scenario ` +
          `exports; known: ${ALL_FORMATS.join(',')}`,
      );
    }
    if (!out.includes(found)) out.push(found);
  }
  return out;
}

/**
 * Ceiling for the DPI draw. Default 600, the highest the resolution Select
 * offers (`ExportControls.tsx`).
 *
 * There IS no backend ceiling - `export_png` bounds the derived pixel
 * dimensions at 16384 (`export/image.rs:405`), not the DPI - so a fuzzer that
 * drew freely would mostly measure "the survey times the DPI overflowed",
 * which is a legitimate refusal and says nothing about anything else. Raise the
 * knob to go looking for that ceiling deliberately.
 */
export function maxDpi(): number {
  const raw = Number(process.env.STALAGMIGHT_FUZZ_EXPORT_MAX_DPI);
  return Number.isFinite(raw) && raw > 0 ? Math.floor(raw) : 600;
}

/** `0` turns the X5/X6 import chains off; anything else leaves them on. */
export function roundtripEnabled(): boolean {
  return (process.env.STALAGMIGHT_FUZZ_EXPORT_ROUNDTRIP ?? '') !== '0';
}

/** How long an in-page settle waits before giving up. */
export function settleCapMs(): number {
  const raw = Number(process.env.STALAGMIGHT_FUZZ_EXPORT_SETTLE_MS);
  return Number.isFinite(raw) && raw > 0 ? raw : 20000;
}

// ---------------------------------------------------------------------------
// The draw
// ---------------------------------------------------------------------------

const ALL_FORMATS: readonly ExportFormat[] = [
  'csv',
  'json',
  'tml',
  'tmlu',
  'png',
  'svg',
  'pdf',
  'preview',
  'diveProfile',
];

/**
 * Draw weights.
 *
 * Text formats are cheap (a serialize and a write) and are where the escaping,
 * the extension rules and the round trip live, so they get most of the budget.
 * PNG and PDF each rasterize the whole survey, so one of those costs more than
 * a dozen CSVs; they are drawn often enough to reach the differential and no
 * more. `diveProfile` is weighted low because it is usually skipped (it needs a
 * computed route), and a skipped move that consumed a draw is wasted budget.
 */
const FORMAT_WEIGHT: Record<ExportFormat, number> = {
  csv: 6,
  json: 5,
  tml: 7,
  tmlu: 5,
  svg: 4,
  png: 2,
  pdf: 2,
  preview: 3,
  diveProfile: 1,
};

/**
 * Which routes can reach a format at all.
 *
 * Drawn from the format's LEGAL set rather than independently, for the reason
 * `importDialogGenerators.ts` draws from `weightedFor(screen)`: an independent
 * draw would spend most of its iterations on combinations that can only be
 * skipped, and a fuzzer that mostly skips is a fuzzer with a smaller budget
 * than its iteration count claims.
 *
 *  - `preview` has no store route: there is no `projectStore` action for it,
 *    and calling `preview_svg_export` raw would bypass `withThemeDepthRamp` -
 *    the scenario would then be hand-rolling the depth ramp instead of testing
 *    the pipeline the app uses. Driving the mounted `ExportPreview` IS that
 *    pipeline.
 *  - `diveProfile` has no UI route from here: `ProfileChart` lives in the
 *    Plan-mode bottom dock, which Export mode hides entirely
 *    (`SingleSystemView.tsx` skips the Dock when `appMode === 'export'`).
 *
 * There is no `modal` route any more: the Export dialog is retired and the
 * workspace panel is the one UI surface, so `preview` is panel-only.
 */
const ROUTES_FOR: Record<ExportFormat, readonly ExportRoute[]> = {
  csv: ['panel', 'store'],
  json: ['panel', 'store'],
  tml: ['panel', 'store'],
  tmlu: ['panel', 'store'],
  png: ['panel', 'store'],
  svg: ['panel', 'store'],
  pdf: ['panel', 'store'],
  preview: ['panel'],
  diveProfile: ['store'],
};

/** Image options as the app's own type spells them. */
export interface ImageOptionDraw {
  format: 'png' | 'svg' | 'pdf';
  dpi: number;
  branching_threshold: number;
  show_penetration_labels: boolean;
  color_mode: 'none' | 'section' | 'depth' | 'distance';
  show_station_labels: boolean;
  show_station_names: boolean;
  show_section_names: boolean;
  show_station_comments: boolean;
  show_shot_comments: boolean;
  show_shot_data: boolean;
}

/**
 * How a PANEL move gets into the Export workspace and out again.
 *
 * Drawn per move, and carried in the trace's `export` step, so a replay enters
 * from the same mode through the same affordance and leaves the same way.
 */
export interface PanelPath {
  /** The mode the move is in when it enters Export - and must return to (X10). */
  from: ReturnableMode;
  entry: ExportEntry;
  exit: ExportExit;
}

export interface ExportDraw {
  format: ExportFormat;
  route: ExportRoute;
  shape: PathShape;
  /** Present for every draw; only consulted for the image formats. */
  imageOptions: ImageOptionDraw;
  /** Per-move sequence number, so generated basenames never collide. */
  seq: number;
  /** The panel route's entry/exit; null on the store route. */
  panel: PanelPath | null;
}

/**
 * The path an old trace's `panel` step takes when it carries none.
 *
 * A trace recorded before the workspace had a recorded way back entered with
 * `setAppMode('export')` from Survey and left with `setAppMode('survey')`. The
 * closest thing that still exists is the store pair, from Survey, so that is
 * what such a step replays with - deterministically, and without inventing a
 * menu or keyboard path the recording never took.
 */
export const LEGACY_PANEL_PATH: PanelPath = { from: 'survey', entry: 'enterExport', exit: 'closeExport' };

/**
 * Read a trace step's panel path back, or refuse it.
 *
 * `options` is untyped trace data, so every field is checked against this
 * build's vocabulary - the same rule the testid allowlist applies to clicks.
 */
export function panelPathFromTrace(value: unknown): PanelPath {
  if (value === undefined || value === null) return LEGACY_PANEL_PATH;
  if (typeof value !== 'object') throw new Error(`trace export step carries a panel path of type ${typeof value}`);
  const raw = value as { from?: unknown; entry?: unknown; exit?: unknown };
  const from = RETURNABLE_MODES.find((mode) => mode === raw.from);
  const entry = EXPORT_ENTRIES.find((name) => name === raw.entry);
  const exit = [...DRAWN_EXPORT_EXITS, 'closeExport' as const].find((name) => name === raw.exit);
  if (from === undefined || entry === undefined || exit === undefined) {
    throw new Error(
      `trace export step names panel path ${JSON.stringify(value)}, which this build does not run ` +
        `(from: ${RETURNABLE_MODES.join('|')}; entry: ${EXPORT_ENTRIES.join('|')}; ` +
        `exit: ${DRAWN_EXPORT_EXITS.join('|')}|closeExport)`,
    );
  }
  return { from, entry, exit };
}

/**
 * DPI values worth drawing.
 *
 * `0` and `1` are the backend's own edges: `export_png` bails on 0 before it
 * generates anything (`"DPI must be at least 1"`), and 1 does NOT bail - the
 * derived width and height are `.max(1)`-clamped, so it writes a legal
 * near-1px PNG. Both are reachable through raw IPC; the Select offers neither.
 */
const DPI_VALUES: readonly number[] = [0, 1, 72, 150, 300, 600];
/** `0` and `1` bracket the "everything is thin / everything is thick" ends. */
const THRESHOLD_VALUES: readonly number[] = [0, 1, 2, 99];
const COLOR_MODES: readonly ImageOptionDraw['color_mode'][] = ['none', 'section', 'depth', 'distance'];

export function drawImageOptions(ctx: ScenarioContext, format: 'png' | 'svg' | 'pdf'): ImageOptionDraw {
  const cap = maxDpi();
  const allowed = DPI_VALUES.filter((dpi) => dpi <= cap);
  // Draw order is fixed: a recorded seed replays the same rng sequence.
  const dpi = ctx.pick(allowed.length > 0 ? allowed : [72]); // never empty: 0 <= every positive cap
  const branching_threshold = ctx.pick(THRESHOLD_VALUES);
  const show_penetration_labels = ctx.rng() < 0.5;
  const color_mode = ctx.pick(COLOR_MODES);
  const show_station_labels = ctx.rng() < 0.5;
  const show_section_names = ctx.rng() < 0.5;
  return {
    format,
    dpi,
    branching_threshold,
    show_penetration_labels,
    color_mode,
    show_station_labels,
    // Station names are derived, not drawn, so existing seeds keep their rng
    // sequence; and never together with the IDs, which the UI route cannot
    // reach (the two checkboxes are one choice). The XOR keeps them independent
    // of each of those two draws, so names are fuzzed with and without section
    // names and penetration labels.
    show_station_names: !show_station_labels && show_penetration_labels !== show_section_names,
    show_section_names,
    show_station_comments: ctx.rng() < 0.5,
    show_shot_comments: ctx.rng() < 0.5,
    show_shot_data: ctx.rng() < 0.5,
  };
}

/** One export move. */
export function drawExport(ctx: ScenarioContext, seq: number): ExportDraw {
  const formats = enabledFormats();
  const format = weightedPick(ctx, formats);
  const routes = ROUTES_FOR[format];
  const route = ctx.pick(routes);
  const shape = ctx.pick(PATH_SHAPES);
  const imageFormat: 'png' | 'svg' | 'pdf' =
    format === 'png' || format === 'svg' || format === 'pdf'
      ? format
      : ctx.pick(['png', 'svg', 'pdf'] as const);
  const imageOptions = drawImageOptions(ctx, imageFormat);
  // Drawn LAST, and only for the panel, so the draws above keep their order.
  const panel: PanelPath | null =
    route === 'panel'
      ? { from: ctx.pick(RETURNABLE_MODES), entry: ctx.pick(EXPORT_ENTRIES), exit: ctx.pick(DRAWN_EXPORT_EXITS) }
      : null;
  return { format, route, shape, imageOptions, seq, panel };
}

function weightedPick(ctx: ScenarioContext, formats: readonly ExportFormat[]): ExportFormat {
  const total = formats.reduce((sum, format) => sum + FORMAT_WEIGHT[format], 0);
  let roll = ctx.rng() * total;
  for (const format of formats) {
    roll -= FORMAT_WEIGHT[format];
    if (roll <= 0) return format;
  }
  return formats[formats.length - 1] as ExportFormat;
}

/** Is this a `data`-type export in `ExportControls` terms? */
export function isDataFormat(format: ExportFormat): boolean {
  return DATA_FORMATS.includes(format);
}
/** Is this an `image`-type export? */
export function isImageFormat(format: ExportFormat): boolean {
  return IMAGE_FORMATS.includes(format);
}

/** What the `data-format-select` value must be for a data format. */
export function dataFormatValue(format: ExportFormat): 'csv' | 'json' | 'tml' {
  if (format === 'csv') return 'csv';
  if (format === 'json') return 'json';
  // The Select offers three values; `.tmlu` is reached by TYPING the extension
  // into the save dialog, which is exactly the `upperTmlu` path shape's point.
  return 'tml';
}

// ---------------------------------------------------------------------------
// The allowlist
// ---------------------------------------------------------------------------

/**
 * Every testid an export move may drive. Literals only - no patterns, because
 * the export form has no generated ids.
 *
 * `pnpm lint:test-ids` owns the other direction (every interactive control
 * carries an id); this set owns "a trace may only name these".
 */
export const EXPORT_TESTIDS: ReadonlySet<string> = new Set([
  // entry / exit. The hamburger row is ONE id in both directions: "Export"
  // outside the workspace, "Close export" inside it.
  'app-menu-trigger',
  'toolbar-export-button',
  'export-close-button',
  'export-panel-close',
  'mode-survey',
  // the Export panel's form (`ExportControls` inside `ExportPanel`)
  'export-type-data',
  'export-type-image',
  'data-format-select',
  'image-format-select',
  'resolution-select',
  'branching-threshold-slider',
  'export-labels-options-toggle',
  'export-show-section-names',
  'export-show-station-labels',
  'export-show-station-names',
  'export-show-station-comments',
  'export-show-shot-comments',
  'export-show-shot-data',
  'export-show-labels',
  'export-color-mode',
  'export-button',
  // the TML unanchored-sections notice, which sits between the button and the
  // save dialog whenever any section is unanchored
  'tml-unanchored-warning-continue',
  'tml-unanchored-warning-cancel',
]);

export function isAllowedTestId(testid: string): boolean {
  return EXPORT_TESTIDS.has(testid);
}

/** Why a driver did nothing. Every value is skipped-and-counted. */
export type ExportSkip =
  | 'absent'
  | 'not-actionable'
  | 'option-absent'
  | 'no-change'
  | 'dialog-refused'
  | 'no-dive-route'
  | 'unconsumed';

// ---------------------------------------------------------------------------
// In-page helpers
// ---------------------------------------------------------------------------

/**
 * Shared prelude for every `ctx.eval` below.
 *
 * The settle marker is the export surface's own: the in-flight flag
 * (`export-progress-container`), the preview's loading state, and the identity
 * of whatever dialog is up. Two stable reads in a row with nothing busy, or the
 * cap.
 */
const HELPERS = `
  const q = (id) => (/["\\\\]/.test(id) ? null : document.querySelector('[data-testid="' + id + '"]'));
  const isVisible = (el) => {
    if (!el) return false;
    const style = getComputedStyle(el);
    if (style.display === 'none' || style.visibility === 'hidden') return false;
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const isOff = (el) =>
    el.disabled === true || el.hasAttribute('disabled') || el.getAttribute('aria-disabled') === 'true';
  const shown = (id) => isVisible(q(id));
  const ui = () => {
    const store = window.__E2E_TEST__ && window.__E2E_TEST__.uiStore;
    return store ? store.getState() : {};
  };
  const busy = () => shown('export-progress-container') || shown('export-preview-loading');
  const marker = () => {
    const state = ui();
    return [
      state.activeDialog == null ? '-' : state.activeDialog,
      state.appMode == null ? '-' : state.appMode,
      shown('export-panel') ? 'P' : '-',
      shown('export-preview') || shown('export-mode-preview') ? 'V' : '-',
      busy() ? 'B' : '-',
    ].join('|');
  };
  const settle = async (capMs) => {
    const deadline = Date.now() + capMs;
    let previous = null;
    let stable = 0;
    for (;;) {
      await new Promise((resolve) => setTimeout(resolve, 60));
      const now = marker();
      stable = now === previous ? stable + 1 : 0;
      previous = now;
      if (stable >= 2 && !busy()) return true;
      if (Date.now() >= deadline) return false;
    }
  };
  const dispatchClick = (el) => {
    const rect = el.getBoundingClientRect();
    const common = {
      bubbles: true, cancelable: true, composed: true, view: window, button: 0,
      clientX: Math.round(rect.left + rect.width / 2),
      clientY: Math.round(rect.top + rect.height / 2),
    };
    const Pointer = typeof PointerEvent === 'function' ? PointerEvent : MouseEvent;
    el.dispatchEvent(new Pointer('pointerdown', { ...common, buttons: 1, pointerId: 1, pointerType: 'mouse', isPrimary: true }));
    el.dispatchEvent(new MouseEvent('mousedown', { ...common, buttons: 1, detail: 1 }));
    el.dispatchEvent(new Pointer('pointerup', { ...common, buttons: 0, pointerId: 1, pointerType: 'mouse', isPrimary: true }));
    el.dispatchEvent(new MouseEvent('mouseup', { ...common, buttons: 0, detail: 1 }));
    el.dispatchEvent(new MouseEvent('click', { ...common, buttons: 0, detail: 1 }));
  };
  const setRangeValue = (el, value) => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value');
    if (!setter || typeof setter.set !== 'function') return false;
    setter.set.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return true;
  };
  const toastKeys = () => {
    const out = [];
    const nodes = document.querySelectorAll('[data-testid="toast"]');
    for (let i = 0; i < nodes.length; i++) out.push(nodes[i].getAttribute('data-toast-key') || '(unkeyed)');
    return out;
  };
`;

/**
 * The toast stack as one move sees it: identified marks, plus the visible keys.
 *
 * `marks` is `key#id@revision` per STORE toast. The id and the revision are
 * what make this a snapshot rather than a key set: `uiStore.showToast` refreshes
 * a repeat of the same key in place within its dedup window, bumping the
 * revision, so a second failure produces the same key at a new revision and no
 * new key at all. `exportOracles.newToastKeys` diffs two of these; its header
 * states what each half is for and what reading the live key set instead cost.
 */
export async function readToastSnapshot(ctx: ScenarioContext): Promise<ToastSnapshot> {
  return ctx.eval<ToastSnapshot>(
    `
    ${HELPERS}
    const dom = toastKeys();
    const store = ui().toasts || [];
    const marks = [];
    const keys = dom.slice();
    for (let i = 0; i < store.length; i++) {
      const entry = store[i] || {};
      const key = entry.key || '(unkeyed)';
      marks.push(key + '#' + String(entry.id) + '@' + String(entry.revision));
      if (keys.indexOf(key) < 0) keys.push(key);
    }
    return { marks: marks, keys: keys };
  `,
  );
}

/** Live `data-toast-key`s, plus the store's copy (which keeps leaving toasts). */
export async function readToastKeys(ctx: ScenarioContext): Promise<string[]> {
  return [...(await readToastSnapshot(ctx)).keys];
}

/**
 * `appMode` / `geometryMode` / `isMapReadOnly`, plus whether the project has a
 * closure the adjusted view would actually move.
 *
 * That last field is not decoration. `setAppMode` sets
 * `geometryMode: 'closureAdjusted'` on the way into Plan/Export, and MapCanvas
 * then REVERTS it to measured when `hasEligibleClosures(project.loop_closures)`
 * is false - "the adjusted view is meaningless without an eligible closure"
 * (`MapCanvas.tsx:2344-2349`), and the read-only comment two lines below says
 * the same thing from the other side: the map stays read-only in Plan/Export
 * "even on a project with no closures where the display falls back to measured
 * geometry". So the geometry half of the invariant is conditional and the
 * read-only half is not, and an oracle that did not carry this fact would
 * report the app's documented fallback as a defect - which is exactly what
 * happened on this scenario's first run.
 */
export async function readModeSnapshot(ctx: ScenarioContext): Promise<ModeSnapshot> {
  return ctx.eval<ModeSnapshot>(
    `
    ${HELPERS}
    const state = ui();
    const appMode = state.appMode == null ? 'survey' : state.appMode;
    const geometryMode = state.geometryMode == null ? 'measured' : state.geometryMode;
    // isMapReadOnly (src/store/uiStore.ts) is a free function over the state,
    // not a store field, so there is nothing to read - it is applied here rather
    // than reimplemented in Node, where a drift would be invisible. Survey and
    // Data are the two editable workspaces; Plan and Export are read-only.
    const mapReadOnly = geometryMode === 'closureAdjusted' || !(appMode === 'survey' || appMode === 'data');
    // CLOSURE_STRATEGY_ELIGIBILITY (src/utils/closures.ts:14-20). Copied for the
    // reason every cross-tier constant here is copied; the full table is spelled
    // out rather than an inclusion list so a NEW strategy shows up as undefined
    // (falsy) instead of silently inheriting somebody's default.
    const eligible = { Batch: true, LeastSquaresWeighted: true, Move: false, ProportionalByLength: false, EqualDistribution: false };
    const projectStore = window.__E2E_TEST__ && window.__E2E_TEST__.projectStore;
    const project = projectStore ? projectStore.getState().project : null;
    const closures = project && Array.isArray(project.loop_closures) ? project.loop_closures : [];
    const hasEligibleClosures = closures.some((c) => c && eligible[c.strategy] === true);
    return {
      appMode: appMode,
      geometryMode: geometryMode,
      mapReadOnly: mapReadOnly,
      hasEligibleClosures: hasEligibleClosures,
    };
  `,
  );
}

/** The panel's option set, straight from `exportStore`. */
export async function readPanelOptions(ctx: ScenarioContext): Promise<OptionSnapshot> {
  return ctx.eval<OptionSnapshot>(
    `
    const store = window.__E2E_TEST__ && window.__E2E_TEST__.exportStore;
    if (!store) throw new Error('exportStore is not exposed; start an E2E build');
    const state = store.getState();
    const image = state.imageOptions || {};
    return {
      exportType: String(state.exportType),
      dataFormat: String(state.dataFormat),
      imageFormat: String(image.format),
      dpi: Number(image.dpi),
      branchingThreshold: Number(image.branching_threshold),
    };
  `,
  );
}

export interface ClickOutcome {
  performed: boolean;
  reason: ExportSkip | null;
}

/** Click one allowlisted control, then settle. */
export async function clickControl(ctx: ScenarioContext, testid: string): Promise<ClickOutcome> {
  if (!isAllowedTestId(testid)) {
    throw new Error(
      `refusing to click "${testid}": not in export-roundtrip-fuzz's allowlist. A recorded trace ` +
        `can only replay the controls this scenario drives.`,
    );
  }
  return ctx.eval<ClickOutcome>(
    `
    const testid = args[0];
    const capMs = args[1];
    ${HELPERS}
    const el = q(testid);
    if (!el) return { performed: false, reason: 'absent' };
    if (!isVisible(el) || isOff(el)) return { performed: false, reason: 'not-actionable' };
    dispatchClick(el);
    await settle(capMs);
    return { performed: true, reason: null };
  `,
    [testid, settleCapMs()],
  );
}

/** Set a custom `Select` by clicking its trigger and then its portaled option. */
export async function selectValue(
  ctx: ScenarioContext,
  testid: string,
  value: string,
): Promise<ClickOutcome> {
  if (!isAllowedTestId(testid)) {
    throw new Error(`refusing to drive select "${testid}": not in export-roundtrip-fuzz's allowlist`);
  }
  return ctx.eval<ClickOutcome>(
    `
    const testid = args[0];
    const value = args[1];
    const capMs = args[2];
    ${HELPERS}
    const trigger = q(testid);
    if (!trigger) return { performed: false, reason: 'absent' };
    if (!isVisible(trigger) || isOff(trigger)) return { performed: false, reason: 'not-actionable' };
    if (trigger.tagName.toLowerCase() === 'select') {
      const before = trigger.value;
      trigger.value = value;
      if (trigger.value !== value) { trigger.value = before; return { performed: false, reason: 'option-absent' }; }
      trigger.dispatchEvent(new Event('change', { bubbles: true }));
      await settle(capMs);
      return { performed: true, reason: null };
    }
    dispatchClick(trigger);
    await new Promise((resolve) => setTimeout(resolve, 40));
    const option = q(testid + '-option-' + encodeURIComponent(value));
    if (!option || !isVisible(option) || isOff(option)) {
      const escape = { key: 'Escape', code: 'Escape', bubbles: true, cancelable: true, composed: true };
      (document.activeElement || document.body).dispatchEvent(new KeyboardEvent('keydown', escape));
      return { performed: false, reason: 'option-absent' };
    }
    dispatchClick(option);
    await settle(capMs);
    return { performed: true, reason: null };
  `,
    [testid, value, settleCapMs()],
  );
}

/** Set a checkbox to a value (a no-op when it is already there). */
export async function setCheckbox(
  ctx: ScenarioContext,
  testid: string,
  want: boolean,
): Promise<ClickOutcome> {
  if (!isAllowedTestId(testid)) {
    throw new Error(`refusing to drive checkbox "${testid}": not in export-roundtrip-fuzz's allowlist`);
  }
  return ctx.eval<ClickOutcome>(
    `
    const testid = args[0];
    const want = args[1];
    const capMs = args[2];
    ${HELPERS}
    const el = q(testid);
    if (!el) return { performed: false, reason: 'absent' };
    if (!isVisible(el) || isOff(el)) return { performed: false, reason: 'not-actionable' };
    if (el.checked === want) return { performed: false, reason: 'no-change' };
    dispatchClick(el);
    await settle(capMs);
    return { performed: true, reason: null };
  `,
    [testid, want, settleCapMs()],
  );
}

/** Set the branching-threshold range input. */
export async function setThreshold(ctx: ScenarioContext, value: number): Promise<ClickOutcome> {
  return ctx.eval<ClickOutcome>(
    `
    const value = args[0];
    const capMs = args[1];
    ${HELPERS}
    const el = q('branching-threshold-slider');
    if (!el) return { performed: false, reason: 'absent' };
    if (!isVisible(el) || isOff(el)) return { performed: false, reason: 'not-actionable' };
    if (!setRangeValue(el, String(value))) return { performed: false, reason: 'not-actionable' };
    await settle(capMs);
    // The input CLAMPS to [min, max]; a draw outside that range is honoured as
    // far as the control allows and the caller reads back what stuck.
    return { performed: true, reason: null };
  `,
    [value, settleCapMs()],
  );
}

export interface SaveDialogOutcome {
  /** Was the export trigger clicked at all? */
  performed: boolean;
  reason: ExportSkip | null;
  /** Did the app's save picker consume the queued answer? */
  consumed: boolean;
  /** Did the TML unanchored-sections notice appear instead? */
  sawUnanchoredNotice: boolean;
  /** `activeDialog` right after the move settled. */
  activeDialog: string | null;
  toastKeys: string[];
}

/**
 * Click a trigger that opens the SAVE picker, with `path` queued as its answer.
 *
 * The arming is the production contract, copied from `answerFileDialogInPage`:
 * the entry goes in `armed:false` and a capture-phase, once-only click listener
 * on the trigger sets `armed` and `activeFileDialogTriggerTestId`. Arming
 * eagerly would let an unrelated picker take the answer.
 *
 * AN UNCONSUMED ANSWER IS ALWAYS REMOVED AGAIN, in a `finally`.
 * `takeTestFileDialogAnswer` refuses to queue a second entry, so a leaked one
 * would break every later export in the session - and under `fuzz-shrink`,
 * which reuses one live session across ddmin candidates, it would poison every
 * candidate after it.
 *
 * `sawUnanchoredNotice` is how the TML path is handled without a second code
 * path: `handleExport` RETURNS before opening the picker when any section is
 * unanchored, so the answer is simply never consumed, and the caller re-arms it
 * on `tml-unanchored-warning-continue` - which is the same function called with
 * `skipTmlAnchorCheck`.
 */
export async function clickAndAnswerSave(
  ctx: ScenarioContext,
  triggerTestId: string,
  path: string | null,
): Promise<SaveDialogOutcome> {
  if (!isAllowedTestId(triggerTestId)) {
    throw new Error(`refusing to arm a save picker on "${triggerTestId}": not in the allowlist`);
  }
  return ctx.eval<SaveDialogOutcome>(
    `
    const trigger = args[0];
    const value = args[1];
    const capMs = args[2];
    ${HELPERS}

    const testApi = window.__E2E_TEST__;
    const queue = testApi && testApi.fileDialogAnswers && testApi.fileDialogAnswers.save;
    if (!queue) throw new Error('file dialog hooks are unavailable; start an E2E build');
    if (queue.length > 0) throw new Error('a save dialog answer is already pending');

    const el = q(trigger);
    if (!el) return { performed: false, reason: 'absent', consumed: false, sawUnanchoredNotice: false, activeDialog: null, toastKeys: [] };
    if (!isVisible(el) || isOff(el)) {
      return { performed: false, reason: 'not-actionable', consumed: false, sawUnanchoredNotice: false, activeDialog: null, toastKeys: [] };
    }

    const id = 'export-fuzz-' + Date.now() + '-' + Math.floor(Math.random() * 1e9);
    const entry = { id: id, value: value, triggerTestId: trigger, armed: false };
    queue.push(entry);
    el.addEventListener('click', () => {
      entry.armed = true;
      testApi.activeFileDialogTriggerTestId = trigger;
    }, { capture: true, once: true });

    let consumed = false;
    let sawNotice = false;
    try {
      dispatchClick(el);
      const deadline = Date.now() + capMs;
      for (;;) {
        const seen = testApi.fileDialogConsumptions && testApi.fileDialogConsumptions[id];
        if (seen === 'save') { consumed = true; break; }
        // handleExport returns BEFORE the picker when the TML notice fires, so
        // waiting for the whole cap there would burn the budget on every
        // unanchored project.
        if (shown('tml-unanchored-warning-dialog')) { sawNotice = true; break; }
        if (Date.now() >= deadline) break;
        await new Promise((resolve) => setTimeout(resolve, 40));
      }
    } finally {
      const index = queue.findIndex((candidate) => candidate.id === id);
      if (index >= 0) queue.splice(index, 1);
      if (testApi.activeFileDialogTriggerTestId === trigger) delete testApi.activeFileDialogTriggerTestId;
      if (testApi.fileDialogConsumptions) delete testApi.fileDialogConsumptions[id];
    }
    await settle(capMs);

    const state = ui();
    return {
      performed: true,
      reason: consumed ? null : 'unconsumed',
      consumed: consumed,
      sawUnanchoredNotice: sawNotice,
      activeDialog: state.activeDialog == null ? null : String(state.activeDialog),
      toastKeys: toastKeys(),
    };
  `,
    [triggerTestId, path, settleCapMs()],
  );
}

export interface ActionOutcome {
  ok: boolean;
  /** The rejection message, or the CommandResult's error. Never named `error`. */
  err: string | null;
}

/**
 * Call a `projectStore` action and report a rejection as DATA.
 *
 * `lib/stores.ts`'s `callStoreAction` cannot be used for anything that rejects,
 * and the reason is in `lib/page.ts`'s own header: the WebDriver bridge coerces
 * a resolved value carrying an `error` key into a SCRIPT ERROR. `callStoreAction`
 * resolves exactly `{ ok: false, error: <message> }` on a rejection, so a
 * refused export - `"Cannot export over the open project file"`, which this
 * scenario provokes on purpose - came back as a `WebDriverError` that killed the
 * run instead of as the failure the oracle was waiting to judge. Measured: the
 * first `--iterations 10` run died with that message and no scenario frame.
 *
 * So the shape here is `{ ok, err }` and `success`, with no `error` and no
 * `message` key at any level. Everything else about the call is the same.
 */
export async function callProjectAction(
  ctx: ScenarioContext,
  action: string,
  actionArgs: readonly unknown[],
): Promise<ActionOutcome> {
  return ctx.eval<ActionOutcome>(
    `
    const action = args[0];
    const rest = args[1];
    const store = window.__E2E_TEST__ && window.__E2E_TEST__.projectStore;
    if (!store) return { ok: false, err: 'projectStore is not exposed; start an E2E build' };
    const fn = store.getState()[action];
    if (typeof fn !== 'function') return { ok: false, err: 'no such projectStore action: ' + action };
    try {
      const result = await fn(...rest);
      // A CommandResult-shaped answer reports its own failure in-band; unwrap it
      // here so the caller has ONE thing to check.
      if (result && typeof result === 'object' && result.success === false) {
        return { ok: false, err: result.error == null ? 'command reported failure' : String(result.error) };
      }
      return { ok: true, err: null };
    } catch (e) {
      return { ok: false, err: e && e.message ? String(e.message) : String(e) };
    }
  `,
    [action, actionArgs],
  );
}

/**
 * Set `appMode` through the store action. Harness setup and teardown only - the
 * panel route ENTERS and LEAVES Export through `enterExportVia` /
 * `leaveExportVia`, which is what X10 judges.
 */
export async function setAppMode(
  ctx: ScenarioContext,
  mode: 'survey' | 'data' | 'plan' | 'export',
): Promise<boolean> {
  const res = await ctx.callAction('ui', 'setAppMode', [mode]);
  await settleOnce(ctx);
  return res.ok;
}

/** What the store says about the workspace and the way back out of it. */
export interface ExportReturnState {
  appMode: string;
  /** `uiStore.exportReturnMode`; null when nothing is recorded. */
  exportReturnMode: string | null;
  /** False on a build whose uiStore has no `exportReturnMode` at all. */
  hasReturnField: boolean;
  /** `uiStore.activeDialog`, so a leave can clear a stray dialog first. */
  activeDialog: string | null;
}

export async function readExportReturn(ctx: ScenarioContext): Promise<ExportReturnState> {
  return ctx.eval<ExportReturnState>(
    `
    ${HELPERS}
    const state = ui();
    const has = Object.prototype.hasOwnProperty.call(state, 'exportReturnMode');
    const recorded = has ? state.exportReturnMode : null;
    return {
      appMode: state.appMode == null ? 'survey' : String(state.appMode),
      exportReturnMode: recorded == null ? null : String(recorded),
      hasReturnField: has,
      activeDialog: state.activeDialog == null ? null : String(state.activeDialog),
    };
  `,
  );
}

/** One entry or exit, as it landed. */
export interface AffordanceOutcome {
  /** Did the affordance's control exist and get pressed? */
  performed: boolean;
  reason: ExportSkip | null;
  /** `appMode` once the move settled. */
  appMode: string;
  /** Key presses spent - Escape may need more than one; 0 for a click. */
  presses: number;
  /** A text field held focus and was blurred before the chord. */
  blurred: boolean;
}

/**
 * Escape presses an exit may spend before X10 judges it.
 *
 * Escape in Export first clears transient state - an open dropdown, a
 * selection, measure endpoints - and only leaves once there is none. This
 * scenario raises none of those on purpose, so one press is the expectation;
 * three covers an open listbox plus a selection plus the press that leaves, and
 * a fourth would be a wedge rather than a queue.
 */
export const ESCAPE_EXIT_BUDGET = 3;

/**
 * Drive one affordance in page.
 *
 *   click  - one allowlisted control
 *   menu   - open the hamburger (`app-menu-trigger`), then its Export row; a
 *            row that is not there closes the menu again by its own trigger
 *            rather than by Escape, because Escape in Export would LEAVE it
 *   key    - a chord on the focused element, as the shortcut handler reads it;
 *            an Escape repeats while still in Export, up to `budget`
 */
type AffordanceRequest =
  | { kind: 'click'; testid: string }
  | { kind: 'menu' }
  | { kind: 'key'; key: string; ctrl: boolean; budget: number };

async function runAffordance(ctx: ScenarioContext, req: AffordanceRequest): Promise<AffordanceOutcome> {
  if (req.kind === 'click' && !isAllowedTestId(req.testid)) {
    throw new Error(`refusing to click "${req.testid}": not in export-roundtrip-fuzz's allowlist`);
  }
  return ctx.eval<AffordanceOutcome>(
    `
    const req = args[0];
    const capMs = args[1];
    ${HELPERS}
    const mode = () => { const s = ui(); return s.appMode == null ? 'survey' : String(s.appMode); };
    const done = (performed, reason, presses, blurred) =>
      ({ performed: performed, reason: reason, appMode: mode(), presses: presses, blurred: blurred });
    if (req.kind === 'click') {
      const el = q(req.testid);
      if (!el) return done(false, 'absent', 0, false);
      if (!isVisible(el) || isOff(el)) return done(false, 'not-actionable', 0, false);
      dispatchClick(el);
      await settle(capMs);
      return done(true, null, 0, false);
    }
    if (req.kind === 'menu') {
      const trigger = q('app-menu-trigger');
      if (!trigger) return done(false, 'absent', 0, false);
      if (!isVisible(trigger) || isOff(trigger)) return done(false, 'not-actionable', 0, false);
      if (!shown('app-menu')) {
        dispatchClick(trigger);
        await new Promise((resolve) => setTimeout(resolve, 60));
      }
      const row = q('toolbar-export-button');
      if (!row || !isVisible(row) || isOff(row)) {
        if (shown('app-menu')) dispatchClick(trigger);
        await settle(capMs);
        return done(false, row ? 'not-actionable' : 'absent', 0, false);
      }
      dispatchClick(row);
      await settle(capMs);
      return done(true, null, 0, false);
    }
    // A chord typed into a text field is the field's: the shortcut handler
    // ignores every non-Escape key there. Blur it first, the way a user clicks
    // off a field before reaching for the shortcut - the claim under test is
    // what the chord does at idle, not inside an input.
    let blurred = false;
    let presses = 0;
    const startedIn = mode();
    for (;;) {
      const active = document.activeElement;
      if (active && active !== document.body &&
          (active.isContentEditable || /^(input|textarea|select)$/i.test(active.tagName))) {
        active.blur();
        blurred = true;
      }
      const target = document.activeElement || document.body;
      const init = { key: req.key, code: req.key, ctrlKey: req.ctrl, bubbles: true, cancelable: true, composed: true };
      target.dispatchEvent(new KeyboardEvent('keydown', init));
      target.dispatchEvent(new KeyboardEvent('keyup', init));
      presses += 1;
      await settle(capMs);
      // Only Escape repeats, and only while it is still clearing transient
      // state inside the workspace it started in.
      if (req.key !== 'Escape' || presses >= req.budget) break;
      if (startedIn !== 'export' || mode() !== 'export') break;
    }
    return done(true, null, presses, blurred);
  `,
    [req, settleCapMs()],
  );
}

/**
 * Enter the Export workspace through `entry`.
 *
 * The caller reads `appMode` back and decides what a miss means; this only
 * reports whether the affordance was there to press.
 */
export async function enterExportVia(ctx: ScenarioContext, entry: ExportEntry): Promise<AffordanceOutcome> {
  if (entry === 'menu') return runAffordance(ctx, { kind: 'menu' });
  if (entry === 'shortcut') return runAffordance(ctx, { kind: 'key', key: 'e', ctrl: true, budget: 1 });
  return storeAffordance(ctx, 'enterExport');
}

/** Leave it through `exit`. */
export async function leaveExportVia(ctx: ScenarioContext, exit: ExportExit): Promise<AffordanceOutcome> {
  switch (exit) {
    case 'closeButton':
      return runAffordance(ctx, { kind: 'click', testid: 'export-close-button' });
    case 'panelClose':
      return runAffordance(ctx, { kind: 'click', testid: 'export-panel-close' });
    case 'menu':
      return runAffordance(ctx, { kind: 'menu' });
    case 'shortcut':
      return runAffordance(ctx, { kind: 'key', key: 'e', ctrl: true, budget: 1 });
    case 'escape':
      return runAffordance(ctx, { kind: 'key', key: 'Escape', ctrl: false, budget: ESCAPE_EXIT_BUDGET });
    case 'closeExport':
      return storeAffordance(ctx, 'closeExport');
  }
}

/** `enterExport` / `closeExport` straight off the store - the floor and the fallback. */
async function storeAffordance(
  ctx: ScenarioContext,
  action: 'enterExport' | 'closeExport',
): Promise<AffordanceOutcome> {
  const res = await ctx.callAction('ui', action, []);
  await settleOnce(ctx);
  const state = await readExportReturn(ctx);
  return { performed: res.ok, reason: res.ok ? null : 'absent', appMode: state.appMode, presses: 0, blurred: false };
}

/** Wait for the surface to be quiet, without performing anything. */
export async function settleOnce(ctx: ScenarioContext): Promise<boolean> {
  return ctx.eval<boolean>(
    `
    const capMs = args[0];
    ${HELPERS}
    return await settle(capMs);
  `,
    [settleCapMs()],
  );
}

/** Preview state: what `ExportPreview` is showing right now. */
export interface PreviewState {
  mounted: boolean;
  loading: boolean;
  error: string | null;
  svgBytes: number;
}

/**
 * Read the preview back.
 *
 * `ExportPreview` renders one of three things (`ExportPreview.tsx`): a loading
 * marker, `export-preview-error` with the backend's message, or the inline SVG.
 * A preview that is neither loading nor erroring and holds no SVG is the fourth
 * state, and it is the interesting one - `preview_svg_export` returned
 * something the container could not render.
 */
export async function readPreview(ctx: ScenarioContext): Promise<PreviewState> {
  return ctx.eval<PreviewState>(
    `
    ${HELPERS}
    const host = q('export-preview') || q('export-mode-preview');
    if (!host) return { mounted: false, loading: false, error: null, svgBytes: 0 };
    const errorNode = host.querySelector('[data-testid="export-preview-error"]');
    const loadingNode = host.querySelector('[data-testid="export-preview-loading"]');
    const svg = host.querySelector('svg');
    return {
      mounted: true,
      loading: !!loadingNode,
      error: errorNode ? (errorNode.textContent || '').slice(0, 300) : null,
      svgBytes: svg ? svg.outerHTML.length : 0,
    };
  `,
  );
}

/** Does the gas plan currently have a route a dive profile could be built from? */
export async function hasDiveRoute(ctx: ScenarioContext): Promise<boolean> {
  return ctx.eval<boolean>(
    `
    const store = window.__E2E_TEST__ && window.__E2E_TEST__.gasPlanStore;
    if (!store) return false;
    const field = store.getState().reachField;
    if (!field || !Array.isArray(field.route_profile) || field.route_profile.length < 2) return false;
    // Same predicate as buildRouteProfile (src/utils/routeProfile.ts:67-82),
    // which is what ProfileChart gates its export button on.
    return field.route_profile.some((p) => p && p.distance_m > 0);
  `,
  );
}

/** The dive-profile request `ProfileChart.currentRequest()` builds at click time. */
export async function diveProfileRequest(ctx: ScenarioContext): Promise<Record<string, unknown> | null> {
  return ctx.eval<Record<string, unknown> | null>(
    `
    const store = window.__E2E_TEST__ && window.__E2E_TEST__.gasPlanStore;
    if (!store) return null;
    const s = store.getState();
    return {
      origin: s.origin || null,
      endpoint: s.endpoint || null,
      waypoints: s.waypoints || [],
      segments: s.segments || [],
      diver: s.diver,
      cylinders: s.cylinders || [],
      gas_config: s.gasConfig,
      stage_drop_overrides: s.stageDropOverrides || [],
    };
  `,
  );
}
