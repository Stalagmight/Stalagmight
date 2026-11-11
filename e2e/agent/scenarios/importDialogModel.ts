/**
 * The import fuzzer's MODEL of the import flows: what screens exist, what
 * controls each one has, how to get OFF each one, and when the walk has to stop
 * exploring and start leaving.
 *
 * Nothing here talks to the app. It is the part a reader can check against
 * `src/components/Dialogs/` by eye, and the part `lib/importDialogActions.test.ts`
 * pins against the bridge's allowlists.
 *
 * ## Progress forcing, and why it is not a timeout
 *
 * A random walk over a wizard does not terminate on its own: `toggle-alternatives`
 * and `section-name-input` are always there, always legal, and never advance
 * anything. Left alone, a flow spends its whole step budget on one screen and
 * the run reports coverage it never had.
 *
 * So each screen names the ONE move that advances it and the sequence that
 * abandons the flow from it. After `FORCE_ADVANCE_AT` steps on the same screen
 * the walk stops drawing and plays the advancing move; after `FORCE_ESCAPE_AT`
 * it plays the escape. That keeps the budget spent on flows rather than on one
 * screen, and it makes "stuck" mean something precise:
 *
 *   ESCAPE oracle - an escape that has not reached `idle` with no placement
 *   within `ESCAPE_BUDGET` forced steps is a WEDGE, and the scenario throws.
 *   A user who cannot leave a dialog has no other recourse, so neither does this.
 *
 *   WEDGE oracle - a non-idle screen with ZERO actionable testids in the whole
 *   inventory is a dead end by construction, and the scenario throws on the spot
 *   rather than spending the escape budget proving it.
 *
 * Both are absolute. There is no tolerance and no known-bug list: if either
 * fires, either the app wedged or this model is wrong about the screen, and both
 * are worth stopping for.
 */
import type { ScreenId } from '../lib/importScreenDigest';

export type { ScreenId };

/**
 * Screens that are NOT part of an import flow, though they are not `idle`.
 *
 * Only the export modal, which a Ctrl+E interjection can open from `idle`.
 * A flow is measured from the moment the walk leaves `idle` to the moment it
 * comes back, and opening an unrelated modal would otherwise start a "flow"
 * with no file in it - which the residue and accounting oracles would then
 * measure as an import that imported nothing.
 */
export const NON_FLOW_SCREENS: ReadonlySet<ScreenId> = new Set<ScreenId>(['export-modal']);

/**
 * Screens `routeOpenedFile`'s `importFlowOwnsScreen` guard REFUSES a second
 * file on.
 *
 * A transcription of `IMPORT_FLOW_DIALOGS` plus the placement clause
 * (`src/utils/openOrImport.ts`), mapped from `activeDialog` values onto the
 * screens this table names. The three import-adjacent dialogs that source
 * deliberately leaves out - `import-receipt`, `import-blocked` and
 * `import-as-new-project` - are correspondingly absent here, and their comment
 * explains why: the first is post-save, and the other two are pre-flow prompts
 * that `closeDialog()` and re-enter the router themselves.
 */
export const IMPORT_GUARDED_SCREENS: ReadonlySet<ScreenId> = new Set<ScreenId>([
  'new-project',
  'dmp-select',
  'zero-datum-select',
  'zero-datum-config',
  'wiz-init',
  'wiz-suggest',
  'wiz-no-suggest',
  'wiz-manual',
  'wiz-placing',
  'wiz-done',
  'wiz-error',
  'wiz-blank',
  'wiz-confirm',
  'map-placement',
  'tml-config',
  'tml-residue',
  'compass',
  'walls',
  'gpx-select',
  'gpx-place',
  'orphan-placement',
]);

/** Screens on which the walk is inside a flow (i.e. a flow is in progress). */
export const IN_FLOW_SCREENS: ReadonlySet<ScreenId> = new Set<ScreenId>([
  'dmp-select',
  'tml-config',
  'tml-residue',
  'compass',
  'walls',
  'gpx-select',
  'gpx-place',
  'zero-datum-select',
  'zero-datum-config',
  'wiz-init',
  'wiz-suggest',
  'wiz-no-suggest',
  'wiz-manual',
  'wiz-placing',
  'wiz-done',
  'wiz-error',
  'wiz-blank',
  'wiz-confirm',
  'map-placement',
  'new-project',
  'receipt',
  'blocked',
  'as-new-project',
  'orphan-placement',
  'unsaved-changes',
  'error',
  'first-run',
]);

/**
 * Every testid the generators may target, plus the prefix patterns the digest
 * expands against the live DOM.
 *
 * Verified against the components, one by one - `DmpSelectionDialog.tsx`,
 * `ZeroDatumSelectionDialog.tsx`, `ZeroDatumConfigDialog.tsx`,
 * `DmpNewImportDialog.tsx` (+ `WizardScrollBody`/`WizardMapPanel`),
 * `TmlImportDialog.tsx` (+ its inline residue body), `CompassImportDialog.tsx`,
 * `WallsImportDialog.tsx`, `GpxSelectionDialog.tsx`, `GpxPlacementDialog.tsx`,
 * `NewProjectForImportDialog.tsx`, `ImportReceiptDialog.tsx`,
 * `ImportBlockedDialog.tsx`, `ImportAsNewProjectDialog.tsx`,
 * `OrphanPlacementDialog.tsx`, `ErrorDialog.tsx`, `ExportDialog.tsx`,
 * `SectionMetadataForm.tsx`, `ShotTableEditor.tsx` + `SpreadsheetGrid/`,
 * `ModeCarousel.tsx` and `Toolbar.tsx`/`GlobalView.tsx`.
 * A testid that is not in a component is a control the fuzzer silently never
 * reaches, which is the failure mode this list exists to prevent.
 *
 * ## Two controls are deliberately ABSENT
 *
 * `project-save-path-browse-button` opens a native SAVE picker through
 * `saveDialog(...)` with NO `triggerTestId` (`NewProjectForImportDialog.tsx`).
 * `takeTestFileDialogAnswer` matches an armed answer on
 * `activeFileDialogTriggerTestId`, which only a CLICK on a named trigger sets,
 * so an answer cannot be aimed at it - the picker would fall through to the
 * real GTK chooser and hang the webview. The path is typed instead
 * (`project-save-path-input`), which is the same code path minus the picker.
 *
 * `tml-import-cancel` is in the list but there is no mid-import cancel to
 * fuzz: it is `disabled={isImporting}` and `handleCancel` early-returns while
 * an import is running (`TmlImportDialog.tsx`), so a TML import cannot be
 * abandoned once started. The bridge's settle loop waits for
 * `tml-import-progress` to clear for exactly that reason - there is nothing
 * actionable during that window, and a window that OUTLASTS the settle cap is
 * a hang, which the WEDGE oracle then reports.
 */
export const TESTID_INVENTORY: readonly string[] = [
  // --- entry points ---------------------------------------------------------
  'toolbar-import-button',
  'global-open-import-button',
  // --- first-run modal (rendered OUTSIDE activeDialog; covers everything) ----
  'telemetry-opt-in-dialog',
  'telemetry-opt-in-no',
  // --- dmp-select -----------------------------------------------------------
  'dmp-selection-dialog',
  'select-all-button',
  'select-none-button',
  'invert-selection-button',
  'default-surveyor-input',
  'default-date-input',
  'continue-button',
  'cancel-button',
  'section-checkbox-*',
  // --- tml-config -----------------------------------------------------------
  'tml-import-dialog',
  'tml-include-excluded',
  'tml-comments-on-shots',
  'tml-virtual-shot-mode',
  // Present only while the import runs; the settle loop waits for it to clear,
  // so a step that still sees it is a TML import that outlasted the cap.
  'tml-import-progress',
  'tml-import-cancel',
  'tml-import-confirm',
  // --- tml-residue ----------------------------------------------------------
  'tml-import-residue-dialog',
  'tml-import-residue-keep',
  'tml-import-residue-discard',
  // --- compass --------------------------------------------------------------
  'compass-import-dialog',
  'compass-apply-corrections',
  'compass-comments-on-stations',
  'compass-include-excluded',
  'compass-import-cancel',
  'compass-import-confirm',
  // --- walls ----------------------------------------------------------------
  'walls-import-dialog',
  'walls-select-all',
  // Sparse: only surveys whose `.srv` resolved render a row, and the index is
  // into the FULL survey list (`WallsImportDialog.tsx`).
  'walls-survey-*',
  'walls-comments-on-stations',
  'walls-include-splays',
  'walls-import-cancel',
  'walls-import-confirm',
  // --- gpx-select -----------------------------------------------------------
  'gpx-selection-dialog',
  'gpx-import-waypoints',
  'gpx-import-tracks',
  'gpx-import-routes',
  'gpx-depth-inverted',
  'gpx-waypoints-toggle',
  'gpx-tracks-toggle',
  'gpx-routes-toggle',
  'gpx-selection-cancel',
  // The footer's primary is one OR the other: `gpx-import-confirm` when only
  // waypoints are selected (nothing to place), `gpx-continue-button` otherwise.
  'gpx-continue-button',
  'gpx-import-confirm',
  // --- gpx-place ------------------------------------------------------------
  'gpx-placement-dialog',
  'gpx-placement-zero-datum-*',
  'gpx-placement-back',
  'gpx-placement-cancel',
  // --- unsaved-changes prompt -----------------------------------------------
  //
  // CANCEL ONLY, deliberately. `unsaved-changes-discard` is on the forbidden
  // list `lib/importDialogActions.test.ts` pins, and `handleDiscard` is why: on
  // the quit path it calls `performQuit()`, so a crafted trace naming it would
  // end the session, and on the others it throws away work with no undo entry.
  // Cancelling abandons whatever raised the prompt, which is all the walk needs.
  'unsaved-changes-dialog',
  'unsaved-changes-cancel',
  // --- export modal (Ctrl+E; contents are not fuzzed this wave) --------------
  'export-dialog',
  'export-dialog-close',
  // --- app mode carousel ----------------------------------------------------
  'mode-survey',
  'mode-plan',
  'mode-export',
  // --- zero-datum-select ----------------------------------------------------
  'zero-datum-selection-dialog',
  'zero-datum-selection-cancel',
  'zero-datum-selection-back',
  'zero-datum-selection-skip',
  'zero-datum-section-card-*',
  // --- zero-datum-config ----------------------------------------------------
  'zero-datum-config-dialog',
  'zero-datum-station-select',
  'zero-datum-gps-input',
  'zero-datum-config-cancel',
  'zero-datum-config-back',
  'zero-datum-config-confirm',
  // Back's own small discard confirm (#F9/dirty guard) - self-contained, not
  // DialogWrapper's `dirty-dialog-*` overlay, so it needs its own ids here.
  'zero-datum-config-back-confirm',
  'zero-datum-config-back-confirm-cancel',
  'zero-datum-config-back-confirm-discard',
  // --- wizard ---------------------------------------------------------------
  'dmp-new-import-dialog',
  'back-button',
  'skip-button',
  'confirm-button',
  'retry-button',
  'segment-skip-button',
  'segment-place-button',
  'segment-place-on-map-button',
  'toggle-alternatives',
  'alternative-*',
  'merge-station-select',
  'overlap-mode-select',
  'segment-mode-suggestion-input',
  'segment-mode-from-existing-input',
  'segment-mode-manual-input',
  'segment-mode-discard-input',
  'from-existing-import-section-select',
  'from-existing-import-station-select',
  'from-existing-section-select',
  'from-existing-station-select',
  'manual-import-section-select',
  'manual-import-station-select',
  // --- wizard confirm overlays ---------------------------------------------
  // Three actions once the wizard has imported anything (#394): continue
  // editing, close KEEPING what is in the project (`confirm-cancel-button`,
  // which kept its id and its meaning), or remove it first
  // (`cancel-remove-button`). Back offers no keep - the step it returns to
  // re-imports from scratch - so `confirm-back-button` always withdraws.
  'cancel-confirm-dialog',
  'cancel-confirm-continue',
  'confirm-cancel-button',
  'cancel-remove-button',
  'back-confirm-dialog',
  'back-confirm-continue',
  'confirm-back-button',
  // --- shared section metadata (all three editing screens) ------------------
  'section-name-input',
  'section-surveyor-input',
  'section-date-input',
  'section-quality-input',
  'section-reverse-checkbox',
  'section-direction-select',
  'section-color-hex-input',
  // --- shot table -----------------------------------------------------------
  'shot-table-container',
  'shot-editor-expand',
  'shot-editor-cell-editor',
  'shot-editor-row-*',
  'shot-editor-gutter-*',
  'shot-editor-cell-*',
  'shot-frozen-*',
  'shot-editor-context-menu',
  'shot-editor-context-insert-above',
  'shot-editor-context-insert-below',
  'shot-editor-context-delete-rows',
  'shot-editor-context-clear',
  'shot-editor-context-fill-down',
  'shot-editor-context-copy',
  'shot-editor-context-cut',
  'shot-editor-context-paste',
  // --- new project for import ----------------------------------------------
  'new-project-for-import-dialog',
  'project-name-input',
  'project-save-path-input',
  'project-location-input',
  'new-project-for-import-cancel',
  'create-and-continue-button',
  // --- receipt / blocked / as-new-project / error ---------------------------
  'import-receipt-dialog',
  'import-receipt-details',
  'import-receipt-continue',
  'import-blocked-dialog',
  'import-blocked-stay',
  'import-blocked-switch',
  'import-as-new-project-dialog',
  'import-as-new-project-cancel',
  'import-as-new-project-confirm',
  'error-dialog',
  'error-dialog-close',
  // --- orphan placement (raised AFTER a DMP import commits) -----------------
  'orphan-placement-dialog',
  'orphan-accept',
  'orphan-skip',
  'orphan-accept-all',
  'orphan-close',
  // the repeated-shots table (#681): a set-all button per choice, a choice per row
  'overlap-set-all-*',
  'overlap-choice-*',
  // --- dirty-discard overlay ------------------------------------------------
  'dirty-dialog-keep-editing',
  'dirty-dialog-discard',
];

/** Selects whose option values the digest should report when it can read them. */
export const SELECT_TESTIDS: readonly string[] = [
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
];

/** `overlap-mode-select`'s option values are static (see DmpNewImportDialog). */
export const OVERLAP_MODES: readonly string[] = ['interpolate', 'import_data', 'existing_data', 'keep_both'];

/**
 * The orphan dialog's per-shot choices (#681): the option values of every
 * `overlap-choice-N` (see `OVERLAP_CHOICES` in src/utils/overlapChoices.ts).
 */
export const ORPHAN_OVERLAP_CHOICES: readonly string[] = ['keep_existing', 'use_new', 'average'];

/** The orphan dialog's set-all buttons, one per choice (#681). */
const ORPHAN_OVERLAP_SET_ALL: readonly string[] = [
  'overlap-set-all-keep-existing',
  'overlap-set-all-use-new',
  'overlap-set-all-average',
];

const ORPHAN_OVERLAP_CHOICE_PREFIX = 'overlap-choice-';

/** Whether `testid` is one of the orphan dialog's repeated-shot controls. */
export function isOrphanRepeatControl(testid: string): boolean {
  return testid.startsWith(ORPHAN_OVERLAP_CHOICE_PREFIX) || ORPHAN_OVERLAP_SET_ALL.includes(testid);
}

/** One move on the orphan dialog's repeated-shot table, as the oracle judges it. */
export interface OrphanRepeatStep {
  testid: string;
  /** The option picked on a per-row choice; null for a set-all click. */
  value: string | null;
  performed: boolean;
  skip: string | null;
  /** The control's digest state after the move (2 = actionable). */
  liveAfter: number | undefined;
  /** Undo depth before and after; negative when it could not be read. */
  undoBefore: number;
  undoAfter: number;
}

/**
 * What a repeated-shot move owes (#681), as a failure, or null when it held.
 *
 * - Keep existing is offered on every row whatever the guards: the doubly-anchored
 *   guard disables Use new and Average only, so a row that refuses Keep existing
 *   while actionable leaves the user a repeated shot with no legal choice.
 * - A choice is dialog state, applied only on Accept, so making one must not move
 *   the undo stack.
 *
 * The orphan-dialog twin of the DMP wizard's `overlap-mode-select` checks.
 */
export function orphanRepeatBreach(step: OrphanRepeatStep): { headline: string; detail: string } | null {
  if (
    !step.performed &&
    step.skip === 'option-absent' &&
    step.value === 'keep_existing' &&
    step.liveAfter === 2
  ) {
    return {
      headline: 'OVERLAP CHOICES: Keep existing was not offered on a repeated shot',
      detail: `  ${step.testid} is actionable but refused "keep_existing", which no guard disables`,
    };
  }
  if (step.performed && step.undoBefore >= 0 && step.undoAfter >= 0 && step.undoAfter !== step.undoBefore) {
    return {
      headline: 'OVERLAP CHOICES: choosing a reading moved the undo stack',
      detail:
        `  ${step.testid}${step.value === null ? '' : ` = ${step.value}`}: undo depth ` +
        `${step.undoBefore} -> ${step.undoAfter}; a choice is applied only on Accept`,
    };
  }
  return null;
}

/** `tml-virtual-shot-mode`'s option values are static (see TmlImportDialog). */
export const VIRTUAL_SHOT_MODES: readonly string[] = ['Auto', 'AllJumps', 'AllReferences'];

/**
 * How a screen is made to progress when the walk has stalled on it, in order.
 *
 * A LIST rather than one id, because two of these screens render their
 * advancing control conditionally: `GpxSelectionDialog` shows
 * `gpx-import-confirm` when only waypoints are selected and
 * `gpx-continue-button` otherwise, and they are opposite branches of one
 * ternary. The walk plays the first entry that is actionable.
 *
 * An entry ending in `*` is a PREFIX PATTERN, expanded against the live digest
 * exactly as `TESTID_INVENTORY`'s patterns are: `zero-datum-select` advances by
 * picking a section CARD, and the card ids carry an index this table cannot
 * know. The shell plays the lowest-indexed actionable member.
 */
export const ADVANCE_MOVE: Readonly<Partial<Record<ScreenId, readonly string[]>>> = {
  'dmp-select': ['continue-button'],
  // The confirm is disabled for a file with no shots at all, which a malformed
  // spec produces; leaving is then the only progress available.
  'tml-config': ['tml-import-confirm', 'tml-import-cancel'],
  'tml-residue': ['tml-import-residue-keep'],
  compass: ['compass-import-confirm'],
  walls: ['walls-import-confirm', 'walls-import-cancel'],
  'gpx-select': ['gpx-continue-button', 'gpx-import-confirm', 'gpx-selection-cancel'],
  'gpx-place': ['gpx-import-confirm'],
  // PICK A SECTION first, and only skip if there is none to pick.
  //
  // Skipping is not advancing this screen, it is declining its whole purpose:
  // behind a picked card sits `zero-datum-config`, which owns a station select,
  // a GPS parser, a shot table and the confirm that starts the wizard WITH an
  // anchor. Routing every stalled visit to Skip meant the forced advance walked
  // around that surface every time. Measured on the naharon campaign: 27 samples
  // on this screen, 7 picks against 8 skips, and the anchored wizard entry
  // behind it almost never reached.
  //
  // The `*` is a PREFIX the shell expands against the digest - the cards are
  // `zero-datum-section-card-N` and the model cannot know N.
  'zero-datum-select': ['zero-datum-section-card-*', 'zero-datum-selection-skip'],
  'zero-datum-config': ['zero-datum-config-confirm'],
  'wiz-suggest': ['confirm-button'],
  // No entry for `wiz-no-suggest`: the bridge's settle predicate waits through
  // that frame now (see `busy()` in importDialogBridge.ts), so the walk arrives
  // at `wiz-manual` instead - which is where the placement options actually are.
  // A frame that OUTLASTS the settle cap still reaches the digest, is still
  // classified, and is still escapable through the entry below; it just has no
  // advancing move, which is honest, because the app is mid-transition and there
  // is nothing on it to press.
  // PLACE ON MAP first, then the footer's Place, then Skip.
  //
  // `segment-place-button` alone could not advance this screen. It is
  // `disabled={!isSegmentPlacementValid}`, and that memo returns FALSE for the
  // default mode `'manual'` - "must use Place on Map" is written into the
  // branch - so on entry the footer primary is always off and the forced
  // advance always missed. Measured: `gen:man:place` fired ZERO times over 130
  // cells and 94 steps on this screen, and the walk left by burning to the
  // escape budget instead. `segment-place-on-map-button` is the control the
  // default mode actually offers, is never disabled, and starts the real map
  // placement - which `map:click` and `map:escape` then finish.
  'wiz-manual': ['segment-place-on-map-button', 'segment-place-button', 'segment-skip-button'],
  'wiz-done': ['cancel-button'],
  'wiz-error': ['retry-button'],
  'wiz-confirm': ['cancel-confirm-continue'],
  'new-project': ['create-and-continue-button'],
  receipt: ['import-receipt-continue'],
  blocked: ['import-blocked-stay'],
  'as-new-project': ['import-as-new-project-cancel'],
  'unsaved-changes': ['unsaved-changes-cancel'],
  'orphan-placement': ['orphan-close'],
  'export-modal': ['export-dialog-close'],
  error: ['error-dialog-close'],
  'first-run': ['telemetry-opt-in-no'],
};

/**
 * How to abandon the flow from each screen, in order. The bridge plays the first
 * entry that is ACTIONABLE and skips the rest; `Escape` is spelled as the
 * pseudo-testid `key:Escape`.
 *
 * `wiz-confirm` deliberately escapes by CONFIRMING the cancel
 * (`confirm-cancel-button`), not by continuing: the walk only reaches that
 * overlay on its way out, so continuing would put it straight back in the flow
 * it is trying to leave and burn the whole escape budget on a loop.
 */
export const ESCAPE_PATH: Readonly<Record<ScreenId, readonly string[]>> = {
  idle: [],
  'dmp-select': ['dirty-dialog-discard', 'cancel-button', 'key:Escape'],
  // `TmlImportDialog` is `isDirty` whenever an option was changed, so leaving
  // raises the discard overlay first - the same shape as `dmp-select`.
  'tml-config': ['dirty-dialog-discard', 'tml-import-cancel', 'key:Escape'],
  // KEEP, never discard: discarding deletes the project the import created,
  // and an escape is the walk trying to LEAVE, not to make that decision. The
  // discard path is reached by its own generator instead.
  'tml-residue': ['tml-import-residue-keep', 'key:Escape'],
  compass: ['compass-import-cancel', 'key:Escape'],
  walls: ['walls-import-cancel', 'key:Escape'],
  'gpx-select': ['gpx-selection-cancel', 'key:Escape'],
  'gpx-place': ['gpx-placement-cancel', 'key:Escape'],
  'export-modal': ['export-dialog-close', 'key:Escape'],
  'zero-datum-select': ['zero-datum-selection-cancel', 'key:Escape'],
  // `ZeroDatumConfigDialog` is `isDirty` whenever GPS text, a metadata field or
  // a shot was touched, so Cancel/Escape may raise the discard overlay first -
  // same shape as `dmp-select`/`tml-config`. `zero-datum-config-back-confirm-*`
  // is deliberately NOT here: Escape dismisses that overlay in place (it does
  // not leave the screen), so the next loop iteration re-evaluates from a
  // clean read of this same route.
  'zero-datum-config': ['dirty-dialog-discard', 'zero-datum-config-cancel', 'key:Escape'],
  'wiz-init': ['cancel-button', 'key:Escape'],
  'wiz-suggest': ['cancel-button', 'key:Escape'],
  'wiz-no-suggest': ['cancel-button', 'key:Escape'],
  'wiz-manual': ['cancel-button', 'key:Escape'],
  'wiz-placing': ['cancel-button', 'key:Escape'],
  'wiz-done': ['cancel-button', 'key:Escape'],
  'wiz-error': ['cancel-button', 'key:Escape'],
  'wiz-blank': ['cancel-button', 'key:Escape'],
  // `confirm-cancel-button` first, and it still means "close, changing nothing
  // in the project" after #394 - the removal is the separate
  // `cancel-remove-button`, which the walk reaches through the generators
  // rather than while trying to leave. Neither this overlay's buttons nor the
  // wizard's own Cancel/Back are actionable while an import is in flight (they
  // carry `disabled`, and the handlers refuse anyway, so a withdrawal can never
  // snapshot a section list that is about to grow) - so an escape attempt
  // during an import falls through to `key:Escape` and retries on the next
  // loop, once the import has settled.
  'wiz-confirm': ['confirm-cancel-button', 'confirm-back-button', 'key:Escape'],
  'map-placement': ['key:Escape'],
  'new-project': ['dirty-dialog-discard', 'new-project-for-import-cancel', 'key:Escape'],
  receipt: ['import-receipt-continue', 'key:Escape'],
  blocked: ['import-blocked-stay', 'key:Escape'],
  'as-new-project': ['import-as-new-project-cancel', 'key:Escape'],
  'unsaved-changes': ['unsaved-changes-cancel', 'key:Escape'],
  'orphan-placement': ['orphan-close', 'key:Escape'],
  error: ['error-dialog-close', 'key:Escape'],
  'first-run': ['telemetry-opt-in-no'],
  unknown: ['key:Escape'],
};

/**
 * Screens whose affordances are NOT testids, and which the WEDGE oracle
 * therefore cannot judge.
 *
 * `map-placement` is driven by clicking the MAP and by Escape; the wizard behind
 * it is `display:none` (DialogWrapper's `hidden` prop) while a placement is
 * running, so every testid in the inventory is legitimately non-actionable. A
 * wedge oracle that counted that as a dead end would fire on the app working
 * exactly as designed. A placement that genuinely cannot be left is still caught
 * - by the ESCAPE oracle, which requires Escape to reach `idle` inside its
 * budget.
 */
export const WEDGE_EXEMPT_SCREENS: ReadonlySet<ScreenId> = new Set<ScreenId>(['map-placement']);

/** Steps on one screen before the walk plays that screen's advancing move. */
export const FORCE_ADVANCE_AT = 8;
/** Steps on one screen before the walk gives up and abandons the flow. */
export const FORCE_ESCAPE_AT = 16;
/** Forced steps an escape gets to reach `idle`. Exceeding it is a FAILURE. */
export const ESCAPE_BUDGET = 6;

/** What the walk should do next, given how long it has sat on one screen. */
export type ProgressMode = 'draw' | 'advance' | 'escape';

export function progressMode(stepsOnScreen: number): ProgressMode {
  if (stepsOnScreen >= FORCE_ESCAPE_AT) return 'escape';
  if (stepsOnScreen >= FORCE_ADVANCE_AT) return 'advance';
  return 'draw';
}

/** Has the escape landed? `idle` with nothing on the map is the only answer. */
export function isEscaped(screen: ScreenId, placement: boolean): boolean {
  return screen === 'idle' && !placement;
}
