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
 * there. A shrunk trace reaches states the original run never had - the modal
 * closed, no project open, the panel unmounted - and half its moves become
 * impossible there. That is what keeps a ddmin-cut trace RUNNABLE.
 */
import type { ScenarioContext } from './framework';
import {
  DATA_FORMATS,
  IMAGE_FORMATS,
  PATH_SHAPES,
  type ExportFormat,
  type ExportRoute,
  type ModeSnapshot,
  type OptionSnapshot,
  type PathShape,
  type ToastSnapshot,
} from '../lib/exportOracles';
import { detProbe } from '../lib/fuzzDeterminismProbe';

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
 */
const ROUTES_FOR: Record<ExportFormat, readonly ExportRoute[]> = {
  csv: ['modal', 'panel', 'store'],
  json: ['modal', 'panel', 'store'],
  tml: ['modal', 'panel', 'store'],
  tmlu: ['modal', 'panel', 'store'],
  png: ['modal', 'panel', 'store'],
  svg: ['modal', 'panel', 'store'],
  pdf: ['modal', 'panel', 'store'],
  preview: ['modal', 'panel'],
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
  show_section_names: boolean;
  show_station_comments: boolean;
  show_shot_comments: boolean;
  show_shot_data: boolean;
}

export interface ExportDraw {
  format: ExportFormat;
  route: ExportRoute;
  shape: PathShape;
  /** Present for every draw; only consulted for the image formats. */
  imageOptions: ImageOptionDraw;
  /** Per-move sequence number, so generated basenames never collide. */
  seq: number;
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
  return {
    format,
    // `allowed` can never be empty: 0 is <= every positive cap.
    dpi: ctx.pick(allowed.length > 0 ? allowed : [72]),
    branching_threshold: ctx.pick(THRESHOLD_VALUES),
    show_penetration_labels: ctx.rng() < 0.5,
    color_mode: ctx.pick(COLOR_MODES),
    show_station_labels: ctx.rng() < 0.5,
    show_section_names: ctx.rng() < 0.5,
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
  return { format, route, shape, imageOptions: drawImageOptions(ctx, imageFormat), seq };
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
  // entry / exit
  'toolbar-export-button',
  'mode-export',
  'mode-survey',
  // the shared form (identical ids in the modal and the panel)
  'export-type-data',
  'export-type-image',
  'data-format-select',
  'image-format-select',
  'resolution-select',
  'branching-threshold-slider',
  'export-labels-options-toggle',
  'export-show-section-names',
  'export-show-station-labels',
  'export-show-station-comments',
  'export-show-shot-comments',
  'export-show-shot-data',
  'export-show-labels',
  'export-color-mode',
  'export-button',
  'export-cancel',
  // the TML unanchored-sections notice, which sits between the button and the
  // save dialog whenever any section is unanchored
  'tml-unanchored-warning-continue',
  'tml-unanchored-warning-cancel',
  // the Replace File alert (#576 D35): raised after the save dialog when the
  // app's extension fix-up renamed the answer onto a file that exists (or whose
  // existence it could not probe). Answered inside `clickAndAnswerSave`.
  'overwrite-rewritten-replace',
  'overwrite-rewritten-cancel',
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
    // isMapReadOnly (src/store/uiStore.ts:65-67) is a free function over the
    // state, not a store field, so there is nothing to read - it is applied
    // here rather than reimplemented in Node, where a drift would be invisible.
    const mapReadOnly = geometryMode === 'closureAdjusted' || appMode !== 'survey';
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

/**
 * The MODAL's option set, read off the DOM.
 *
 * There is nowhere else to read it: the dialog keeps local `useState`
 * (`ExportDialog.tsx:17-19`), deliberately, so that it always opens on
 * data/CSV. That is exactly what makes X8 worth asserting, and it is also why
 * this reader has to go through the rendered controls.
 */
export async function readModalOptions(ctx: ScenarioContext): Promise<OptionSnapshot | null> {
  return ctx.eval<OptionSnapshot | null>(
    `
    ${HELPERS}
    if (ui().activeDialog !== 'export') return null;
    const dialog = q('export-dialog');
    if (!dialog) return null;
    const scoped = (id) => dialog.querySelector('[data-testid="' + id + '"]');
    const dataChecked = scoped('export-type-data');
    const exportType = dataChecked && dataChecked.checked ? 'data' : 'image';
    const text = (el) => (el ? (el.textContent || '').trim() : '');
    const dataFormat = text(scoped('data-format-select')).toLowerCase().split(' ')[0] || '';
    const imageFormat = text(scoped('image-format-select')).toLowerCase() || '';
    const resolution = text(scoped('resolution-select'));
    const dpiMatch = /(\\d+)\\s*DPI/i.exec(resolution);
    const slider = scoped('branching-threshold-slider');
    return {
      exportType: exportType,
      dataFormat: dataFormat,
      imageFormat: imageFormat,
      dpi: dpiMatch ? Number(dpiMatch[1]) : -1,
      branchingThreshold: slider ? Number(slider.value) : -1,
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
  /**
   * Did the Replace File alert appear after the picker (#576 D35)? It is
   * answered Replace, so the move still exports: every path it can name is the
   * scenario's own scratch file, and leaving it open would block every later
   * move behind a modal (NF-W2FE-2).
   */
  sawOverwritePrompt: boolean;
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
 *
 * THE REPLACE FILE ALERT. A consumed answer whose name the app rewrites (a
 * known extension replaced, an unknown one appended - `domRouteRewritesPath`)
 * goes through `resolveSavePath`, which asks before writing over an existing
 * file at the NEW name and asks anyway when it cannot probe it. The
 * `plnxSibling` and `openProject` shapes are always rewritten, so their moves
 * can stop on that alert; it is answered here (Replace) and reported as
 * `sawOverwritePrompt`.
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
    if (!el) return { performed: false, reason: 'absent', consumed: false, sawUnanchoredNotice: false, sawOverwritePrompt: false, activeDialog: null, toastKeys: [] };
    if (!isVisible(el) || isOff(el)) {
      return { performed: false, reason: 'not-actionable', consumed: false, sawUnanchoredNotice: false, sawOverwritePrompt: false, activeDialog: null, toastKeys: [] };
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

    // The Replace File alert sits between the picker and the write; answer it
    // so the export runs and no modal outlives the move.
    let sawPrompt = false;
    if (consumed && shown('overwrite-rewritten-prompt')) {
      sawPrompt = true;
      const replace = q('overwrite-rewritten-replace');
      if (replace) dispatchClick(replace);
      await settle(capMs);
    }

    const state = ui();
    return {
      performed: true,
      reason: consumed ? null : 'unconsumed',
      consumed: consumed,
      sawUnanchoredNotice: sawNotice,
      sawOverwritePrompt: sawPrompt,
      activeDialog: state.activeDialog == null ? null : String(state.activeDialog),
      toastKeys: toastKeys(),
    };
  `,
    [triggerTestId, path, settleCapMs()],
  );
}

export interface ActionOutcome {
  ok: boolean;
  /** The rejection message, or the refused outcome's error. Never named `error`. */
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
  const outcome = await ctx.eval<ActionOutcome>(
    `
    const action = args[0];
    const rest = args[1];
    const store = window.__E2E_TEST__ && window.__E2E_TEST__.projectStore;
    if (!store) return { ok: false, err: 'projectStore is not exposed; start an E2E build' };
    const fn = store.getState()[action];
    if (typeof fn !== 'function') return { ok: false, err: 'no such projectStore action: ' + action };
    try {
      const result = await fn(...rest);
      // A store mutation's outcome reports its own refusal in-band (\`success: false\`,
      // #576 C4); unwrap it here so the caller has ONE thing to check.
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
  // Determinism mode: the outcome and, for an export, the file (no-op when off).
  detProbe()?.exportCall(action, actionArgs, outcome);
  return outcome;
}

/** Is the modal Export dialog up? */
export async function modalIsOpen(ctx: ScenarioContext): Promise<boolean> {
  return ctx.eval<boolean>(
    `
    ${HELPERS}
    return ui().activeDialog === 'export' && !!q('export-dialog');
  `,
  );
}

/** Open the modal through the store action the toolbar row calls. */
export async function openModal(ctx: ScenarioContext): Promise<boolean> {
  const res = await ctx.callAction('ui', 'openDialog', ['export']);
  if (!res.ok) return false;
  await settleOnce(ctx);
  return modalIsOpen(ctx);
}

/** Close the modal the way its Cancel button does. */
export async function closeModal(ctx: ScenarioContext): Promise<void> {
  const outcome = await clickControl(ctx, 'export-cancel');
  if (!outcome.performed) {
    // Cancel is `disabled={isExporting}`, and a dialog that refuses to close is
    // itself worth knowing about - but not from here: this is teardown, and the
    // scenario's own residue check is what reports it.
    await ctx.callAction('ui', 'closeDialog', []);
  }
  await settleOnce(ctx);
}

/** Enter / leave Export mode through the only control that sets it. */
export async function setAppMode(ctx: ScenarioContext, mode: 'survey' | 'plan' | 'export'): Promise<boolean> {
  const res = await ctx.callAction('ui', 'setAppMode', [mode]);
  await settleOnce(ctx);
  return res.ok;
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
