/**
 * The webview half of `import-dialog-fuzz`: ONE fused `executeAsync` per UI move
 * - perform it, let the screen settle, compute the digest, return a verdict.
 *
 * Same argument as `undoRedoFuzzBridge.ts`: a UI step is "look, choose, click",
 * and the "look" half is ~40 DOM queries plus a `get_project_state`. One
 * WebDriver round trip per query would make the bridge, not the app, the cost.
 *
 * ## The allowlists are the security boundary
 *
 * `runUiStep` reaches a control by TESTID, and on a replay that testid comes out
 * of a trace file, which `readTrace` checks only for its schema version and step
 * shapes. Exactly the argument `REPLAYABLE_MUTATIONS` makes for store actions,
 * applied to the DOM: without an allowlist a crafted trace clicks
 * `delete-project-confirm`, `speleodb-push`, or a Save-As trigger inside a
 * privileged webview with the `__E2E_TEST__` hooks live.
 *
 * So every id a move may name is either a LITERAL in one of the sets below or
 * matches one of the ANCHORED, BOUNDED patterns. `lib/importDialogActions.test.ts`
 * pins both directions: every id a generator can emit is allowed, and the named
 * destructive ids are not.
 *
 * ## Synthetic events, not `el.click()`
 *
 * The grid's cells and gutters listen on `onMouseDown` / `onContextMenu`, not
 * `onClick`, so `HTMLElement.click()` reaches buttons and nothing else. Every
 * click here dispatches the full pointer/mouse sequence, which covers both.
 * Activation behaviour still runs for a dispatched `click`, so checkboxes and
 * radios toggle and React's change event fires exactly once.
 *
 * ## Settling
 *
 * A click that starts an import returns long before the screen it produces
 * exists. After every move the page polls a CHEAP marker (no IPC) until the four
 * wizard busy markers are gone and the marker has been stable for two reads, or
 * `STALAGMIGHT_FUZZ_IMPORT_SETTLE_MS` elapses. The hash-settling window on
 * `continue-button` is deliberately NOT waited for - that window is what the
 * `race-continue` generator exists to hit.
 */
import type { ScenarioContext } from './framework';
import {
  IMPORT_SCREEN_GLOBAL,
  importScreenInjectionSource,
  type ImportScreenDigest,
  type ImportScreenSpec,
} from '../lib/importScreenDigest';
import { toastKeysRaised } from '../lib/importFlowOracles';
import { valueShape } from '../lib/shareableFailure';
import { shareableTestId } from '../../../scripts/lib/shareable-selector.mjs';
import { SELECT_TESTIDS, TESTID_INVENTORY } from './importDialogModel';

/**
 * A UI move. Mirrors the `ui` / `input` trace steps one-to-one, so recording is
 * a copy and replaying is a dispatch.
 */
export type Move =
  | { kind: 'ui'; testid: string; double?: boolean; label: string }
  | { kind: 'input'; testid: string; value: string; mode: 'type' | 'select' | 'check'; label: string };

/**
 * Chords a move may press, spelled as the pseudo-testid `key:<chord>` on a `ui`
 * step so the whole vocabulary stays inside the trace schema wave 1 landed.
 *
 * The spelling is the app's own canonical form (`src/utils/shortcuts.ts`
 * `normalizeKeyName`): modifiers first, single letters uppercase. `Ctrl+E`
 * therefore dispatches `{ key: 'e', ctrlKey: true }`, which is what
 * `eventToShortcut` folds back into `Ctrl+E`.
 *
 * `Ctrl+O` / `Ctrl+I` are deliberately NOT here. They raise a NATIVE picker,
 * and a picker with no armed answer falls through to the real GTK chooser and
 * hangs the webview (`src/utils/fileDialogs.ts` - an unarmed queue returns
 * `queued:false`). They are driven by `answerKeyboardFileDialog` below, which
 * arms the answer FIRST and is recorded as a `fileDialog` step.
 */
export const PRESSABLE_KEYS: ReadonlySet<string> = new Set(['Escape', 'Ctrl+E']);

/**
 * Chords whose picker the scenario may arm and press.
 *
 * Separate from `PRESSABLE_KEYS` on purpose: a bare press of one of these is
 * the hang described above, so the two vocabularies must not be interchangeable
 * even by accident.
 */
export const ARMABLE_KEYS: ReadonlySet<string> = new Set(['Ctrl+O', 'Ctrl+I']);

/**
 * `{ key, ctrlKey, ... }` for a chord in the canonical spelling.
 *
 * Exported so `lib/importDialogActions.test.ts` can pin it against the app's
 * own `eventToShortcut` rather than against this file's idea of it.
 */
export function keyEventInit(chord: string): { key: string; ctrl: boolean; alt: boolean; shift: boolean } {
  const parts = chord.split('+');
  const key = parts[parts.length - 1] as string;
  return {
    // `eventToShortcut` uppercases a single letter, and a real browser reports
    // the LOWERCASE letter for an unshifted key press.
    key: /^[A-Z]$/.test(key) ? key.toLowerCase() : key,
    ctrl: parts.includes('Ctrl'),
    alt: parts.includes('Alt'),
    shift: parts.includes('Shift'),
  };
}

/** Clickable ids, as literals. */
export const CLICKABLE_TESTIDS: ReadonlySet<string> = new Set([
  // entry points
  'toolbar-import-button',
  'global-open-import-button',
  // dmp-select
  'select-all-button',
  'select-none-button',
  'invert-selection-button',
  'continue-button',
  'cancel-button',
  // tml-config / tml-residue
  'tml-import-cancel',
  'tml-import-confirm',
  'tml-import-residue-keep',
  'tml-import-residue-discard',
  // compass
  'compass-import-cancel',
  'compass-import-confirm',
  // walls
  'walls-import-cancel',
  'walls-import-confirm',
  // gpx
  'gpx-waypoints-toggle',
  'gpx-tracks-toggle',
  'gpx-routes-toggle',
  'gpx-selection-cancel',
  'gpx-continue-button',
  'gpx-import-confirm',
  'gpx-placement-back',
  'gpx-placement-cancel',
  // unsaved-changes prompt. CANCEL ONLY - `unsaved-changes-discard` quits the
  // app on the quit path and is on the forbidden list.
  'unsaved-changes-cancel',
  // export modal (opened by Ctrl+E; its contents are not fuzzed)
  'export-dialog-close',
  // app mode carousel. Only ever clicked at `idle`: everywhere else a modal
  // covers the toolbar, so a synthetic click on it would reach a control no
  // user can reach - see the note on `mode:*` in importDialogGenerators.ts.
  'mode-survey',
  'mode-plan',
  'mode-export',
  // zero-datum-select
  'zero-datum-selection-cancel',
  'zero-datum-selection-back',
  'zero-datum-selection-skip',
  // zero-datum-config
  'zero-datum-config-cancel',
  'zero-datum-config-back',
  'zero-datum-config-confirm',
  // zero-datum-config's own Back-discard confirm (see importDialogModel.ts)
  'zero-datum-config-back-confirm-cancel',
  'zero-datum-config-back-confirm-discard',
  // wizard
  'back-button',
  'skip-button',
  'confirm-button',
  'retry-button',
  'segment-skip-button',
  'segment-place-button',
  'segment-place-on-map-button',
  'toggle-alternatives',
  'shot-editor-expand',
  // wizard confirm overlays. `cancel-remove-button` DELETES the sections the
  // wizard imported (one batched, undoable step) - a real destructive action
  // the walk is allowed to take, since the whole point of the overlay is that
  // both answers are legitimate exits.
  'cancel-confirm-continue',
  'confirm-cancel-button',
  'cancel-remove-button',
  'back-confirm-continue',
  'confirm-back-button',
  // grid context menu
  'shot-editor-context-insert-above',
  'shot-editor-context-insert-below',
  'shot-editor-context-delete-rows',
  'shot-editor-context-clear',
  'shot-editor-context-fill-down',
  'shot-editor-context-copy',
  // CUT is a copy plus a clear, and it is the grid mutation with the most to say
  // about the wizard's frozen rows: `GridContextMenu` renders it
  // unconditionally, so a selection made entirely of overlap-frozen shots can be
  // aimed at it exactly like `delete-rows` can.
  'shot-editor-context-cut',
  'shot-editor-context-paste',
  // new project for import
  'new-project-for-import-cancel',
  'create-and-continue-button',
  // receipt / blocked / as-new-project / error
  'import-receipt-details',
  'import-receipt-continue',
  'import-blocked-stay',
  'import-blocked-switch',
  'import-as-new-project-cancel',
  'import-as-new-project-confirm',
  'error-dialog-close',
  // orphan placement (raised right after a DMP import commits)
  'orphan-accept',
  'orphan-skip',
  'orphan-accept-all',
  'orphan-close',
  // dirty-discard overlay
  'dirty-dialog-keep-editing',
  'dirty-dialog-discard',
  // first-run modal. Declining is the only choice the fuzzer ever makes here:
  // opting IN would point a fuzz run's telemetry at a real endpoint.
  'telemetry-opt-in-no',
]);

/**
 * Clickable id PATTERNS. Anchored at both ends and bounded to at most three
 * digits (or a known column key), so no pattern can widen into an unrelated id
 * however long a trace's string is.
 */
export const CLICKABLE_PATTERNS: readonly RegExp[] = [
  /^zero-datum-section-card-\d{1,3}$/,
  /^alternative-\d{1,3}$/,
  /^shot-editor-gutter-\d{1,3}$/,
  /^shot-editor-row-\d{1,3}$/,
  /^shot-editor-cell-\d{1,3}-[a-z_]{1,20}$/,
];

/** Ids a `type` move may target. */
export const TYPEABLE_TESTIDS: ReadonlySet<string> = new Set([
  'default-surveyor-input',
  'default-date-input',
  'zero-datum-gps-input',
  'section-name-input',
  'section-surveyor-input',
  'section-date-input',
  'section-quality-input',
  'section-color-hex-input',
  'project-name-input',
  'project-save-path-input',
  'project-location-input',
  'shot-editor-cell-editor',
]);

/** Ids a `select` move may target (all custom `Select`s except where noted). */
export const SELECTABLE_TESTIDS: ReadonlySet<string> = new Set([
  'tml-virtual-shot-mode',
  'zero-datum-station-select',
  'merge-station-select',
  'overlap-mode-select',
  'section-direction-select',
  'from-existing-import-section-select',
  'from-existing-import-station-select',
  'from-existing-section-select',
  'from-existing-station-select',
  'manual-import-section-select',
  'manual-import-station-select',
]);

/**
 * Select id PATTERNS. `GpxPlacementDialog` renders one zero-datum picker per
 * track/route section, so the id carries the section index.
 */
export const SELECTABLE_PATTERNS: readonly RegExp[] = [/^gpx-placement-zero-datum-\d{1,3}$/];

/** Ids a `check` move may target (checkboxes and the segment-mode radios). */
export const CHECKABLE_TESTIDS: ReadonlySet<string> = new Set([
  'section-reverse-checkbox',
  'segment-mode-suggestion-input',
  'segment-mode-from-existing-input',
  'segment-mode-manual-input',
  'segment-mode-discard-input',
  // tml-config
  'tml-include-excluded',
  'tml-comments-on-shots',
  // compass
  'compass-apply-corrections',
  'compass-comments-on-stations',
  'compass-include-excluded',
  // walls
  'walls-select-all',
  'walls-comments-on-stations',
  'walls-include-splays',
  // gpx-select
  'gpx-import-waypoints',
  'gpx-import-tracks',
  'gpx-import-routes',
  'gpx-depth-inverted',
]);

export const CHECKABLE_PATTERNS: readonly RegExp[] = [
  /^section-checkbox-\d{1,3}$/,
  /^walls-survey-\d{1,3}$/,
];

function allowed(testid: string, literals: ReadonlySet<string>, patterns: readonly RegExp[] = []): boolean {
  if (literals.has(testid)) return true;
  return patterns.some((pattern) => pattern.test(testid));
}

/** Is this move one this build will dispatch at all? Checked in NODE. */
export function isDispatchable(move: Move): boolean {
  if (move.kind === 'ui') {
    if (move.testid.startsWith('key:')) return PRESSABLE_KEYS.has(move.testid.slice('key:'.length));
    return allowed(move.testid, CLICKABLE_TESTIDS, CLICKABLE_PATTERNS);
  }
  if (move.mode === 'type') return allowed(move.testid, TYPEABLE_TESTIDS);
  if (move.mode === 'select') return allowed(move.testid, SELECTABLE_TESTIDS, SELECTABLE_PATTERNS);
  return allowed(move.testid, CHECKABLE_TESTIDS, CHECKABLE_PATTERNS);
}

/** Why a move did nothing. Every value is skipped-and-counted, never asserted. */
export type SkipReason =
  | 'absent'
  | 'not-actionable'
  | 'option-absent'
  | 'no-change'
  | 'no-menu-anchor'
  | 'not-typeable';

export interface StepOutcome {
  /** Did the move reach the app at all? */
  performed: boolean;
  /** Null when performed; otherwise why not. */
  reason: SkipReason | null;
  /**
   * The target's disabled state read IMMEDIATELY BEFORE the dispatch gate, or
   * null when there was no element to read (absent, or a `key:` chord).
   *
   * The digest's `live` map cannot answer this. It is taken one round trip
   * earlier, and the gap is exactly where the wizards' async enables land - the
   * Walls dialog holds Import disabled behind `!hashChecked` and flips it when a
   * hash + duplicate-check round trip returns, which the settle marker does not
   * track by design (closing that window would retire the race generators). So
   * "the digest said disabled" and "it was disabled when we clicked" are
   * different facts, and only this one describes what the app was asked to do.
   */
  offAtDispatch: boolean | null;
  /**
   * `uiStore.activeDialog` read IMMEDIATELY BEFORE a `key:` chord is dispatched;
   * null for every other move kind, and for a chord pressed with no dialog up.
   *
   * `useKeyboardShortcuts` gates on `useUIStore.getState().activeDialog` inside
   * the keydown handler, so this is the value the app's own gate consulted. The
   * digest's `screen` is a round trip older, and an import that completes in
   * that gap closes its dialog - which is how EXPORT CHORD came to blame the
   * shortcut dispatcher for a chord it swallowed correctly.
   */
  dialogAtDispatch: string | null;
  /**
   * The OTHER two gates the same keydown handler consults, read on the same
   * tick; null for every move kind that is not a `key:` chord.
   *
   * `activeDialog` is not the whole gate and treating it as such left a hole.
   * `useKeyboardShortcuts` returns early for a non-Escape key while a dialog is
   * open, and then - for a configurable action like `export` - returns again on
   * `placementModeActive && !PLACEMENT_MODE_ALLOWED_SHORTCUTS.has(action)` and
   * on `dataTableFullScreen && !FULLSCREEN_TABLE_ALLOWED_SHORTCUTS.has(action)`.
   * `export` is in neither allowlist, so all three gates swallow Ctrl+E, and an
   * oracle that only knew about the first would score a regression on the other
   * two as a legal chord at idle.
   *
   * Map placement is the live case: the dialog is CLOSED and placement is on,
   * which is exactly the state the import flow spends its map clicks in.
   */
  placementAtDispatch: boolean | null;
  fullscreenAtDispatch: boolean | null;
  /** Value read back after a `type`/`select` (what the control actually took). */
  valueAfter: string | null;
  digest: ImportScreenDigest;
  /** In-page attribution: action, settle, digest. */
  timing: { actionMs: number; settleMs: number; digestMs: number };
}

const SCREEN = `globalThis.${IMPORT_SCREEN_GLOBAL}`;

/** Read at CALL time so a replay's restored environment still reaches it. */
function settleCapMs(): number {
  const raw = Number(process.env.STALAGMIGHT_FUZZ_IMPORT_SETTLE_MS);
  return Number.isFinite(raw) && raw > 0 ? raw : 12000;
}

function screenSpec(): ImportScreenSpec {
  return { inventory: TESTID_INVENTORY, selects: SELECT_TESTIDS };
}

/** Install the screen digest for this run. The fuzz oracle must be in first. */
export async function injectScreenDigest(ctx: ScenarioContext): Promise<void> {
  await ctx.eval<boolean>(`${importScreenInjectionSource()}\nreturn true;`);
}

/**
 * In-page helpers shared by every eval body below. Kept as one string rather
 * than four so the settle rule cannot drift between the move path and the
 * observe path.
 */
export const HELPERS = `
  const SCREEN = ${SCREEN};
  if (!SCREEN) throw new Error('the import screen digest is not injected');
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
  /**
   * Cheap stability probe: no IPC, no full digest.
   *
   * The wizard BUSY markers, the TML import's progress marker, plus the wizard
   * being MOUNTED-BUT-HIDDEN.
   *
   * \`tml-import-progress\` is here because the walk does not act while it is
   * up. \`tml-import-cancel\` stays live during an import (#576 RV4-FE-MISC1: it
   * asks the backend to stop it, \`ImportCancelButton\`), but that mid-import
   * cancel is not fuzzed: the walk waits for the import to settle. A progress
   * marker that OUTLASTS the settle cap is a hang, which is what the WEDGE oracle
   * is for.
   *
   * That last one is \`DialogWrapper\`'s \`hidden\` prop, which
   * \`DmpNewImportDialog\` sets while \`autoStartManualPlacementPending\` is true:
   * for a frame or two the dialog is \`display:none\` and \`placementMode\` is not
   * set yet, so the screen is neither a wizard phase nor a placement. Without it
   * here the digest catches that gap and the S1 oracle reports an unknown screen
   * for a transition the app is entitled to make. If it ever OUTLASTS the settle
   * cap it is a genuine wedge, and S1 then says so.
   *
   * Every marker in this list is a WAIT, never a suppression: one that outlasts
   * the cap still reaches the digest, still gets classified, and is still judged
   * by the escape and wedge oracles.
   *
   * The dmp-selection hash-settling window is deliberately NOT waited for -
   * that window is what \`race-continue\` exists to hit.
   */
  const busy = () =>
    shown('wizard-calculating') ||
    shown('wizard-importing-zero-datum') ||
    shown('wizard-placing') ||
    shown('tml-import-progress') ||
    (!!q('dmp-new-import-dialog') && !shown('dmp-new-import-dialog')) ||
    orphanAcceptRunning();
  /**
   * An orphan-placement Accept (or Accept all) in flight (#576 HFI2-F4). It
   * awaits mergeStations and then the overlap treatment's setShotKind /
   * updateShot, each an undo entry, and holds every button of the dialog
   * \`disabled\` (\`processing\`) until the chain ends. A settle that returned
   * mid-chain gave the next move an undo depth still climbing, and OVERLAP
   * MODES blamed the picker for the Accept's pushes. Read off the disabled
   * attribute, not visibility: a hidden dialog's chain is still running.
   */
  function orphanAcceptRunning() {
    const ids = ['orphan-accept', 'orphan-accept-all', 'orphan-close'];
    for (let i = 0; i < ids.length; i++) {
      const el = q(ids[i]);
      if (el && el.disabled === true) return true;
    }
    return false;
  }
  const marker = () => {
    const ui = window.__E2E_TEST__ && window.__E2E_TEST__.uiStore
      ? window.__E2E_TEST__.uiStore.getState()
      : { activeDialog: null, placementMode: null };
    return [
      ui.activeDialog === null || ui.activeDialog === undefined ? '-' : ui.activeDialog,
      ui.placementMode ? 'P' : '-',
      document.querySelectorAll('[data-testid^="shot-editor-row-"]').length,
      document.querySelectorAll('[data-testid^="section-checkbox-"]').length,
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
  const dispatchClick = (el, right, double) => {
    const rect = el.getBoundingClientRect();
    const common = {
      bubbles: true,
      cancelable: true,
      composed: true,
      view: window,
      button: right ? 2 : 0,
      clientX: Math.round(rect.left + rect.width / 2),
      clientY: Math.round(rect.top + rect.height / 2),
    };
    const Pointer = typeof PointerEvent === 'function' ? PointerEvent : MouseEvent;
    el.dispatchEvent(new Pointer('pointerdown', { ...common, buttons: right ? 2 : 1, pointerId: 1, pointerType: 'mouse', isPrimary: true }));
    el.dispatchEvent(new MouseEvent('mousedown', { ...common, buttons: right ? 2 : 1, detail: 1 }));
    el.dispatchEvent(new Pointer('pointerup', { ...common, buttons: 0, pointerId: 1, pointerType: 'mouse', isPrimary: true }));
    el.dispatchEvent(new MouseEvent('mouseup', { ...common, buttons: 0, detail: 1 }));
    if (right) {
      el.dispatchEvent(new MouseEvent('contextmenu', { ...common, buttons: 0, detail: 0 }));
      return;
    }
    el.dispatchEvent(new MouseEvent('click', { ...common, buttons: 0, detail: 1 }));
    if (double) el.dispatchEvent(new MouseEvent('dblclick', { ...common, buttons: 0, detail: 2 }));
  };
  const setReactValue = (el, value) => {
    const proto = el.tagName.toLowerCase() === 'textarea'
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(proto, 'value');
    if (!setter || typeof setter.set !== 'function') return false;
    el.focus();
    setter.set.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return true;
  };
`;

/**
 * One fused UI step.
 *
 * The dispatch gate is checked in NODE first (see the module header): a move
 * naming something outside the allowlists is not a failed move, it is not a
 * move, so it throws rather than returning a skip.
 */
export async function runUiStep(ctx: ScenarioContext, move: Move): Promise<StepOutcome> {
  if (!isDispatchable(move)) {
    throw new Error(
      `refusing to drive ${move.kind} on "${shareableTestId(move.testid)}": not in the import fuzzer's allowlist. ` +
        `A recorded trace can only replay the controls the generators produce.`,
    );
  }
  return ctx.eval<StepOutcome>(
    `
    const req = args[0];
    const spec = args[1];
    const capMs = args[2];
    ${HELPERS}

    let performed = false;
    let reason = null;
    let valueAfter = null;
    // Set by every branch that has an element to read, immediately before that
    // branch's gate. See StepOutcome.offAtDispatch for why the digest's own
    // reading is not a substitute.
    let offAtDispatch = null;
    let dialogAtDispatch = null;
    let placementAtDispatch = null;
    let fullscreenAtDispatch = null;
    const actionStarted = Date.now();

    if (req.kind === 'ui' && req.testid.indexOf('key:') === 0) {
      // args[3] is the event init Node resolved from the chord's canonical
      // spelling ("Escape", "Ctrl+E"), so that string is never parsed twice.
      const chord = args[3];
      const target = document.activeElement || document.body;
      const init = {
        key: chord.key,
        code: chord.key,
        ctrlKey: chord.ctrl,
        altKey: chord.alt,
        shiftKey: chord.shift,
        bubbles: true,
        cancelable: true,
        composed: true,
      };
      // Read here, not from the digest: this is the value the app's own gate
      // consults, one statement later, on the same tick.
      const ui = window.__E2E_TEST__ && window.__E2E_TEST__.uiStore ? window.__E2E_TEST__.uiStore.getState() : null;
      dialogAtDispatch = ui && ui.activeDialog !== undefined && ui.activeDialog !== null ? ui.activeDialog : null;
      // The same tick, for the same reason: all three are read inside the
      // handler one statement from now, and only these readings describe what
      // the app's gate actually saw.
      placementAtDispatch = !!(ui && ui.placementMode);
      fullscreenAtDispatch = !!(ui && ui.dataTableFullScreen);
      target.dispatchEvent(new KeyboardEvent('keydown', init));
      target.dispatchEvent(new KeyboardEvent('keyup', init));
      performed = true;
    } else if (req.kind === 'ui') {
      // A grid context-menu item only exists once the menu is open, and the menu
      // opens on a RIGHT-CLICK of the active row's gutter. Opening it here keeps
      // the trace vocabulary to plain clicks (the schema has no button field)
      // while still exercising the real path: the preceding cell click is what
      // chose the row, so the anchor is the move before this one, not a guess.
      let el = q(req.testid);
      if (!el && req.testid.indexOf('shot-editor-context-') === 0) {
        const activeRow = document.querySelector('[data-testid^="shot-editor-row-"][data-active="true"]')
          || document.querySelector('[data-testid^="shot-editor-row-"]');
        if (!activeRow) {
          reason = 'no-menu-anchor';
        } else {
          const index = (activeRow.getAttribute('data-testid') || '').slice('shot-editor-row-'.length);
          const gutter = q('shot-editor-gutter-' + index);
          if (!gutter || !isVisible(gutter)) reason = 'no-menu-anchor';
          else {
            dispatchClick(gutter, true, false);
            await new Promise((resolve) => setTimeout(resolve, 30));
            el = q(req.testid);
          }
        }
      }
      offAtDispatch = el ? isOff(el) : null;
      if (reason === null) {
        if (!el) reason = 'absent';
        else if (!isVisible(el) || offAtDispatch) reason = 'not-actionable';
        else {
          dispatchClick(el, false, req.double === true);
          performed = true;
        }
      }
    } else if (req.mode === 'type') {
      const el = q(req.testid);
      offAtDispatch = el ? isOff(el) : null;
      if (!el) reason = 'absent';
      else if (!isVisible(el) || offAtDispatch) reason = 'not-actionable';
      else if (!setReactValue(el, req.value)) reason = 'not-typeable';
      else {
        // The grid's cell editor is a textarea that commits on BLUR
        // (SpreadsheetGrid's onBlur -> commitEdit). Every other input here is a
        // controlled React field that has already committed on change, so the
        // blur is scoped to the one control that needs it.
        if (req.testid === 'shot-editor-cell-editor') el.blur();
        performed = true;
        valueAfter = String(el.value);
      }
    } else if (req.mode === 'select') {
      const trigger = q(req.testid);
      offAtDispatch = trigger ? isOff(trigger) : null;
      if (!trigger) reason = 'absent';
      else if (!isVisible(trigger) || offAtDispatch) reason = 'not-actionable';
      else if (trigger.tagName.toLowerCase() === 'select') {
        const before = trigger.value;
        trigger.value = req.value;
        if (trigger.value !== req.value) {
          trigger.value = before;
          reason = 'option-absent';
        } else {
          trigger.dispatchEvent(new Event('change', { bubbles: true }));
          performed = true;
          valueAfter = String(trigger.value);
        }
      } else {
        // Custom Select: click the trigger, then its PORTALED option. An option
        // the current state does not offer is skipped-and-counted, and the
        // listbox is closed again so the next move is not shadowed by it.
        dispatchClick(trigger, false, false);
        await new Promise((resolve) => setTimeout(resolve, 30));
        const optionId = req.testid + '-option-' + encodeURIComponent(req.value);
        const option = q(optionId);
        if (!option || !isVisible(option) || isOff(option)) {
          reason = 'option-absent';
          const escape = { key: 'Escape', code: 'Escape', bubbles: true, cancelable: true, composed: true };
          (document.activeElement || document.body).dispatchEvent(new KeyboardEvent('keydown', escape));
        } else {
          dispatchClick(option, false, false);
          performed = true;
          valueAfter = req.value;
        }
      }
    } else {
      const el = q(req.testid);
      offAtDispatch = el ? isOff(el) : null;
      if (!el) reason = 'absent';
      else if (!isVisible(el) || offAtDispatch) reason = 'not-actionable';
      else {
        const want = req.value === 'true';
        if (el.checked === want) reason = 'no-change';
        else {
          dispatchClick(el, false, false);
          performed = true;
          valueAfter = String(el.checked);
        }
      }
    }

    const actionMs = Date.now() - actionStarted;
    const settleStarted = Date.now();
    if (performed) await settle(capMs);
    const settleMs = Date.now() - settleStarted;
    const digestStarted = Date.now();
    const digest = await SCREEN.digest(spec);
    const digestMs = Date.now() - digestStarted;

    return {
      performed: performed,
      reason: reason,
      valueAfter: valueAfter,
      offAtDispatch: offAtDispatch,
      dialogAtDispatch: dialogAtDispatch,
      placementAtDispatch: placementAtDispatch,
      fullscreenAtDispatch: fullscreenAtDispatch,
      digest: digest,
      timing: { actionMs: actionMs, settleMs: settleMs, digestMs: digestMs },
    };
  `,
    [
      move,
      screenSpec(),
      settleCapMs(),
      keyEventInit(move.kind === 'ui' && move.testid.startsWith('key:') ? move.testid.slice(4) : 'Escape'),
    ],
  );
}

/** Settle, then digest. Used after the moves Node performs (files, dialogs). */
export async function observeScreen(ctx: ScenarioContext, waitForSettle = true): Promise<ImportScreenDigest> {
  return ctx.eval<ImportScreenDigest>(
    `
    const spec = args[0];
    const capMs = args[1];
    const wait = args[2];
    ${HELPERS}
    if (wait) await settle(capMs);
    return await SCREEN.digest(spec);
  `,
    [screenSpec(), settleCapMs(), waitForSettle],
  );
}

export interface DialogOutcome {
  /** Did the trigger get clicked at all? */
  performed: boolean;
  reason: SkipReason | null;
  /** Did the app's picker consume the answer within the settle budget? */
  consumed: boolean;
  /**
   * What the door's gates read, IMMEDIATELY BEFORE the click or the keystroke:
   * `uiStore.importParse !== null` (`pickAndRoute`'s parse gate, #576 W62-NF1),
   * `activeDialog`, `placementMode !== null` and `dataTableFullScreen` (the
   * gates in front of the door). `pickAndRoute` reads the first one tick later,
   * before its first await. False and null when nothing was dispatched.
   */
  parseAtDispatch: boolean;
  dialogAtDispatch: string | null;
  placementAtDispatch: boolean;
  fullscreenAtDispatch: boolean;
  /**
   * Keys of the toasts the click or the keystroke raised or refreshed, read the
   * moment it returned (`RouteOutcome.toastsRaised`'s reading): `pickAndRoute`
   * raises its refusal before its first await, and the consumption wait after
   * it can outlast the toast. Null when the store has no toast stack, or when
   * nothing was dispatched.
   */
  toastsRaised: string[] | null;
  digest: ImportScreenDigest;
}

/**
 * In-page: the gates a picker door reads and the toast stack, for
 * `answerFileDialogInPage` and `answerKeyboardFileDialog` (`DialogOutcome`).
 */
const PICKER_DOOR = `
  const toastKeysRaised = ${toastKeysRaised.toString()};
  const doorStore = () => (window.__E2E_TEST__ && window.__E2E_TEST__.uiStore ? window.__E2E_TEST__.uiStore.getState() : null);
  const doorToasts = () => {
    const ui = doorStore();
    return ui && Array.isArray(ui.toasts) ? ui.toasts : null;
  };
  const doorGates = () => {
    const ui = doorStore();
    return {
      parseAtDispatch: !!(ui && ui.importParse),
      dialogAtDispatch: ui && ui.activeDialog !== undefined && ui.activeDialog !== null ? ui.activeDialog : null,
      placementAtDispatch: !!(ui && ui.placementMode),
      fullscreenAtDispatch: !!(ui && ui.dataTableFullScreen),
    };
  };
  const noDoorGates = { parseAtDispatch: false, dialogAtDispatch: null, placementAtDispatch: false, fullscreenAtDispatch: false };
`;

/**
 * Answer a native file dialog, entirely IN PAGE.
 *
 * `lib/controls.ts`'s `answerFileDialog` does the same arming but clicks through
 * WebDriver, whose actionability model is stricter than this bridge's - it also
 * requires the element to be un-obscured at its centre point. That is a second,
 * different definition of "can I click this", and the first time this scenario
 * ran it produced a bare `still not clickable after 5000ms` because a first-run
 * modal was covering the toolbar: no screen, no digest, nothing to shrink. Every
 * click in this fuzzer therefore goes through ONE model - the digest's - and an
 * unclickable trigger is a skip with a reason like every other impossible move.
 *
 * The arming is faithful to the production path: the entry goes in `armed:false`
 * and a capture-phase, once-only click listener on the trigger sets `armed` and
 * `activeFileDialogTriggerTestId`, exactly as `answerFileDialog` does. Arming
 * eagerly would let an unrelated picker consume the answer.
 *
 * An UNCONSUMED answer is always removed again. `takeTestFileDialogAnswer`
 * refuses to queue a second entry, so a leaked one would break every later
 * import in the session - and under `fuzz-shrink`, which reuses one live session
 * across ddmin candidates, it would poison every candidate after it.
 */
export async function answerFileDialogInPage(
  ctx: ScenarioContext,
  triggerTestId: string,
  kind: 'open' | 'save',
  value: string | string[] | null,
): Promise<DialogOutcome> {
  return ctx.eval<DialogOutcome>(
    `
    const trigger = args[0];
    const kind = args[1];
    const value = args[2];
    const spec = args[3];
    const capMs = args[4];
    ${HELPERS}
    ${PICKER_DOOR}

    const testApi = window.__E2E_TEST__;
    const queue = testApi && testApi.fileDialogAnswers && testApi.fileDialogAnswers[kind];
    if (!queue) throw new Error('file dialog hooks are unavailable; start an E2E build');
    if (queue.length > 0) throw new Error('a file dialog answer is already pending');

    const el = q(trigger);
    let reason = null;
    if (!el) reason = 'absent';
    else if (!isVisible(el) || isOff(el)) reason = 'not-actionable';

    let consumed = false;
    let gates = noDoorGates;
    let toastsRaised = null;
    if (reason === null) {
      const id = 'fuzz-' + Date.now() + '-' + Math.floor(Math.random() * 1e9);
      const entry = { id: id, value: value, triggerTestId: trigger, armed: false };
      queue.push(entry);
      el.addEventListener('click', () => {
        entry.armed = true;
        testApi.activeFileDialogTriggerTestId = trigger;
      }, { capture: true, once: true });
      try {
        // The door's gates and the toast stack, on the click's own tick (DialogOutcome).
        gates = doorGates();
        const before = doorToasts();
        dispatchClick(el, false, false);
        const after = doorToasts();
        toastsRaised = before && after ? toastKeysRaised(before, after) : null;
        const deadline = Date.now() + capMs;
        for (;;) {
          const seen = testApi.fileDialogConsumptions && testApi.fileDialogConsumptions[id];
          if (seen === kind) { consumed = true; break; }
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
    }

    return {
      performed: reason === null,
      reason: reason,
      consumed: consumed,
      ...gates,
      toastsRaised: toastsRaised,
      digest: await SCREEN.digest(spec),
    };
  `,
    [triggerTestId, kind, value, screenSpec(), settleCapMs()],
  );
}

/**
 * Answer the native picker a KEYBOARD shortcut raises (Ctrl+O / Ctrl+I).
 *
 * Wave 2 excluded these chords because `answerFileDialogInPage` cannot arm
 * them: it attaches a capture-phase click listener to the trigger ELEMENT, and
 * a keystroke clicks nothing, so `activeFileDialogTriggerTestId` is never set
 * and `takeTestFileDialogAnswer` refuses the answer. Then the production path
 * falls through to the real GTK chooser and the webview hangs.
 *
 * It is nonetheless armable, and the mechanism is worth stating because getting
 * it wrong hangs the session rather than failing a step. `pickAndRoute` calls
 * `openDialog(filters, undefined)` - no trigger id - and
 * `takeTestFileDialogAnswer` (`src/utils/fileDialogs.ts:21-44`) then checks
 * exactly two things: that the head entry is `armed`, and that
 * `hooks.activeFileDialogTriggerTestId === entry.triggerTestId`. Its mismatch
 * throw is skipped when the CALLER passes no id. So a SENTINEL trigger id that
 * no element carries, written to both sides before the keystroke, is consumed
 * by the keyboard path and by nothing else.
 *
 * Arming eagerly is safe here for the same reason it is unsafe for a click:
 * there is no element whose listener could arm it late, and the sentinel cannot
 * collide with a real trigger id. The entry is removed again in a `finally`,
 * because `takeTestFileDialogAnswer` refuses a second queued entry and a leaked
 * one would break every later import in the session.
 *
 * A chord the app's dispatcher swallows (every non-Escape key is swallowed
 * while a dialog is open) simply leaves the answer unconsumed - reported as
 * `consumed: false`, which is a metric, not a failure.
 */
export async function answerKeyboardFileDialog(
  ctx: ScenarioContext,
  chord: string,
  value: string | null,
): Promise<DialogOutcome> {
  if (!ARMABLE_KEYS.has(chord)) {
    throw new Error(`refusing to arm a picker for ${valueShape(chord)}: not in the import fuzzer's armable-key set`);
  }
  return ctx.eval<DialogOutcome>(
    `
    const chord = args[0];
    const value = args[1];
    const spec = args[2];
    const capMs = args[3];
    ${HELPERS}
    ${PICKER_DOOR}

    const testApi = window.__E2E_TEST__;
    const queue = testApi && testApi.fileDialogAnswers && testApi.fileDialogAnswers.open;
    if (!queue) throw new Error('file dialog hooks are unavailable; start an E2E build');
    if (queue.length > 0) throw new Error('a file dialog answer is already pending');

    // A sentinel no element carries, so only the keyboard path can take it.
    const trigger = 'fuzz-keyboard-picker';
    const id = 'fuzz-' + Date.now() + '-' + Math.floor(Math.random() * 1e9);
    queue.push({ id: id, value: value, triggerTestId: trigger, armed: true });
    const previousActive = testApi.activeFileDialogTriggerTestId;
    testApi.activeFileDialogTriggerTestId = trigger;

    let consumed = false;
    let gates = noDoorGates;
    let toastsRaised = null;
    try {
      const target = document.activeElement || document.body;
      const init = {
        key: chord.key,
        code: chord.key,
        ctrlKey: chord.ctrl,
        altKey: chord.alt,
        shiftKey: chord.shift,
        bubbles: true,
        cancelable: true,
        composed: true,
      };
      // The gates the dispatcher and pickAndRoute read, and the toast stack, on the keydown's own tick.
      gates = doorGates();
      const before = doorToasts();
      target.dispatchEvent(new KeyboardEvent('keydown', init));
      const after = doorToasts();
      toastsRaised = before && after ? toastKeysRaised(before, after) : null;
      target.dispatchEvent(new KeyboardEvent('keyup', init));
      const deadline = Date.now() + capMs;
      for (;;) {
        const seen = testApi.fileDialogConsumptions && testApi.fileDialogConsumptions[id];
        if (seen === 'open') { consumed = true; break; }
        if (Date.now() >= deadline) break;
        await new Promise((resolve) => setTimeout(resolve, 40));
      }
    } finally {
      const index = queue.findIndex((candidate) => candidate.id === id);
      if (index >= 0) queue.splice(index, 1);
      if (testApi.activeFileDialogTriggerTestId === trigger) {
        if (previousActive === undefined) delete testApi.activeFileDialogTriggerTestId;
        else testApi.activeFileDialogTriggerTestId = previousActive;
      }
      if (testApi.fileDialogConsumptions) delete testApi.fileDialogConsumptions[id];
    }
    await settle(capMs);

    return {
      performed: true,
      reason: null,
      consumed: consumed,
      ...gates,
      toastsRaised: toastsRaised,
      digest: await SCREEN.digest(spec),
    };
  `,
    [keyEventInit(chord), value, screenSpec(), settleCapMs()],
  );
}

/**
 * Dismiss the first-run telemetry modal if it is up.
 *
 * It is rendered OUTSIDE `activeDialog` and covers the whole window, so until it
 * is gone every click lands on its backdrop. Declining is the only choice this
 * fuzzer ever makes: opting in would point a fuzz run's telemetry at a real
 * endpoint. Returns true when it was there and is now gone.
 */
export async function dismissFirstRunModal(ctx: ScenarioContext): Promise<boolean> {
  return ctx.eval<boolean>(
    `
    const spec = args[0];
    const capMs = args[1];
    ${HELPERS}
    if (!shown('telemetry-opt-in-dialog')) return false;
    const decline = q('telemetry-opt-in-no');
    if (!decline) throw new Error('the first-run telemetry modal is up with no telemetry-opt-in-no to decline it');
    dispatchClick(decline, false, false);
    await settle(capMs);
    if (shown('telemetry-opt-in-dialog')) {
      throw new Error('the first-run telemetry modal survived telemetry-opt-in-no');
    }
    return true;
  `,
    [screenSpec(), settleCapMs()],
  );
}

/**
 * Hand a path straight to the open/import ROUTER.
 *
 * The hook it calls is E2E-gated in `src/main.tsx`; see the comment there for
 * why the router needs one (its own `importFlowOwnsScreen` guard is unreachable
 * from every driver-visible entry point, which is exactly the guard this
 * scenario's mid-flow interjection exists to test).
 */
/** What `routeOpenedFile`'s guard was looking at when it was called. */
export interface RouteOutcome {
  /** Did the E2E hook exist and get called at all? */
  routed: boolean;
  /**
   * `uiStore.activeDialog` and `placementMode`, read on the same tick as the
   * call - which is when `importFlowOwnsScreen` reads them.
   *
   * The pre-move digest is a round trip older, and an import that completes in
   * the gap closes its dialog: the router then legally accepts the file, and an
   * oracle judging by the digest calls that a broken guard. Comparing these two
   * against the digest's own values is how the caller tells "the guard should
   * have fired" from "the screen moved under us".
   */
  dialogAtDispatch: string | null;
  placementAtDispatch: boolean;
  /**
   * `uiStore.importParse !== null`, read on the same tick: an import's file was
   * still parsing, which `importFlowOwnsScreen` also refuses on (#576
   * Q-W27-1-D1) with no dialog up and placement off. Classified by
   * `routeHandoff` (#576 W67-NF3).
   */
  parseAtDispatch: boolean;
  /**
   * Keys of the toasts the route raised or refreshed, read from
   * `uiStore.toasts` on the tick it returned (#576 HFI2-F3); null when the
   * store has no toast stack. The router's refusal IS its toast, and a toast
   * judged off the settled digest could have faded first.
   */
  toastsRaised: string[] | null;
}

export async function routeFile(ctx: ScenarioContext, absolutePath: string): Promise<RouteOutcome> {
  return ctx.eval<RouteOutcome>(
    `
    const toastKeysRaised = ${toastKeysRaised.toString()};
    const route = window.__E2E_TEST__ && window.__E2E_TEST__.routeOpenedFile;
    const store = window.__E2E_TEST__ && window.__E2E_TEST__.uiStore;
    const ui = store ? store.getState() : null;
    const toasts = () => {
      const now = store ? store.getState() : null;
      return now && Array.isArray(now.toasts) ? now.toasts : null;
    };
    const out = {
      routed: false,
      dialogAtDispatch: ui && ui.activeDialog !== undefined && ui.activeDialog !== null ? ui.activeDialog : null,
      placementAtDispatch: !!(ui && ui.placementMode),
      parseAtDispatch: !!(ui && ui.importParse),
      toastsRaised: null,
    };
    if (typeof route !== 'function') return out;
    const before = toasts();
    await route(args[0], 'Import Failed');
    out.routed = true;
    const after = toasts();
    out.toastsRaised = before && after ? toastKeysRaised(before, after) : null;
    return out;
  `,
    [absolutePath],
  );
}
