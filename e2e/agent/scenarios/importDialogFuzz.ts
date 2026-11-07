/**
 * import-dialog-fuzz: a seeded random walk over EVERY import flow.
 *
 * It generates survey files in the five formats the app imports - mNemo `.dmp`,
 * Ariane `.tml`/`.tmlu`, Compass `.dat`/`.mak`, Walls `.srv`/`.wpj`, GPX - hands
 * them to the real app through the real entry points, and then clicks the import
 * UI at random - selection dialog, zero-datum pair, the placement wizard, the
 * shot-table grid, the map placement, the four options screens, GPX placement,
 * orphan placement, the receipt - asserting after every move that the app is
 * still coherent, and after every FLOW that the import kept its contract.
 *
 * ## Formats, and why the flow shape follows from the extension
 *
 * `.dmp` is the ONLY additive import. Every other extension routes to
 * `new-project-for-import` whether or not a project is open
 * (`src/utils/openOrImport.ts`), and with a project open it goes through
 * `import-as-new-project` first, whose confirm CLOSES the project and re-enters
 * the router. So a non-DMP flow always creates a project in the middle of
 * itself, which is why those flows are the ones that reach the deferred initial
 * save, the TML residue choice, and `f1Skipped:projectSwapped`.
 *
 * ## Three input sources
 *
 * A generated spec (valid-biased, or corrupted with probability
 * `STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P`), a tracked `.dmp` fixture, or an entry
 * from a PRIVATE corpus of real survey files when one is configured
 * (`STALAGMIGHT_FUZZ_CORPUS_DIR`). Generated files reach the parsers' edges;
 * real files reach the shapes nobody would think to generate. A run with no
 * corpus counts `corpus:absent` and is exactly as valid, just narrower.
 *
 * NOTHING THIS RUN PUBLISHES NAMES A CORPUS FILE. A corpus entry is
 * identified everywhere by `corpusFileId` - a 5-hex fold of its relative path -
 * because a survey filename can name a cave, a landowner or an unreleased
 * project, and a metric key, a log line, a failure payload and a recorded trace
 * are all things that end up attached to an issue. The real path stays inside
 * `lib/fixtureCorpus.ts`, which states the full rule; the `corpusFile` trace
 * step carries the id, the step that OPENS the copy carries
 * `fuzzTrace.corpusEntryRel` rather than its basename (#576 HFI-13), and
 * `fuzzTrace.ts` refuses to write or read anything else. What the app echoes
 * back is folded by `fixtureCorpus.redactCorpusText` wherever it is printed.
 * The session's own working files are NOT publishable and are not covered: the
 * copy under `fuzz-files/` keeps its real names, and the sweep's `findings/`
 * and the app's logs quote them.
 *
 * ## The shape of a run
 *
 *   EPISODE (= one iteration)
 *     open a fresh copy of the fixture with NO inherited undo history
 *     turn auto-save off
 *     K = randInt(1, STALAGMIGHT_FUZZ_IMPORT_FLOWS_PER_EPISODE) FLOWS, with NO
 *       reload between them - state accumulates on purpose, because a second
 *       import into a project the first one changed is where the overlap, merge
 *       and renumbering paths live
 *     teardown: close, assert nothing survived, delete this episode's files
 *
 *   FLOW (= one file, from an entry point to whatever ends it)
 *     draw a starter at `idle`, then walk until the screen comes back to `idle`
 *     or the step budget runs out (`STALAGMIGHT_FUZZ_IMPORT_MAX_FLOW_STEPS`,
 *     default 140 - exhausting the cap is a METRIC, not a failure)
 *
 * ## Why `screen:orphan-placement` was zero, and what fixed it
 *
 * FIXED - kept here because the surface stayed dark for 242 cells and the
 * reason was not a coverage gap. `App.tsx`'s DMP `onComplete` asks for orphan
 * placements only `if (newSectionIds?.length)`, and on any flow that used MAP
 * PLACEMENT that array used to be empty by the time it was read:
 *
 *   - `importPlacedSegmentNow` imports the placed segment and pushes its ids
 *     into `importedSectionIdsRef` (`DmpNewImportDialog.tsx`);
 *   - when the queue empties, `handlePlacementComplete` calls
 *     `openPlacementDialog([], ...)`, which re-opens `dmp-import-new` and
 *     therefore REMOUNTS the dialog;
 *   - `SavedManualPlacingState` carries twelve fields and neither
 *     `importedSectionIds` nor `importedReceipts`, and both refs initialise to
 *     `[]` on the new instance;
 *   - the restored instance finishes and calls `onComplete([], <empty receipt>)`.
 *
 * So the one import that reliably CREATES an orphan - a segment dropped on the
 * map, merged to nothing, in a component with no datum - is the one import that
 * can never report one. Verified by hand against a live app: two DMP files whose
 * sections share geometry, the first placed far from anything, the second
 * imported after it. The dialog did not open, and asking the backend the same
 * question `App.tsx` asks, with the same ids, returned ONE high-confidence
 * suggestion (4 matched shots, score 1.42, direction reverse). The data was
 * there; the frontend did not ask.
 *
 * Two fixes closed it, one from each side of a parallel effort. Master carries
 * the run's ids and receipts across the remount in `SavedManualPlacingState`
 * (6667269b, 2c2e6c72 - #487 residue, and what closes #523), and this branch's
 * `startPlacementMode` turns on the stations a placement needs, without which
 * every placement was cancelled before its first click (#518). The same
 * hand-driven sequence then raised the dialog - "Connect Orphaned Sections,
 * Section 1 of 2", one suggestion at High confidence, 5 shots matched - and the
 * screenshot and log are kept under `.agent-artifacts/orphan-proof-20260811/`.
 *
 * The scenario never worked around either defect. `unanchoredComponents:*` still
 * measures the precondition, so "no orphan component existed" stays
 * distinguishable from "the geometry did not match", and the ORPHAN PLACEMENT
 * check above now holds both of the dialog's answers to account.
 *
 * ## No `wiz-no-suggest` screen
 *
 * When the suggestion pool runs dry the wizard goes straight to
 * `manual_placing` (reuse a skipped suggestion, attach to an existing section,
 * place on the map by hand, or discard, with a map placement and a shot
 * table), so the walk meets that as `wiz-manual`. The "no suggestions" frame
 * the digest used to classify is no longer rendered (#576 NF-W8D4-1).
 *
 * ## What is asserted
 *
 * Per STEP:
 *   S1 the screen is one this build's table names (`unknown` FAILS)
 *   S2 no React error boundary
 *   S3 no project invariant breach (`fuzzOracleCore.ts`'s checker)
 *   S4 no error dialog from a move that has no business raising one
 *   S5 WEDGE: a non-idle screen with zero actionable controls
 *   plus `check_project_integrity` after any step that pushed an undo entry -
 *   `save_blocked`, or a non-finite-geometry message, is a hard failure
 *
 * Seven inline expected-behaviour checks ride along with the moves that provoke
 * them, because each one is only meaningful against the state immediately
 * before its own move:
 *   FROZEN DELETE    an all-frozen selection must survive, and must say so
 *   INCOMPLETE SHOTS a wizard commit the incomplete-shot guard refused must
 *                    import nothing, push no undo entry, and leave the wizard up
 *   ORPHAN PLACEMENT declining must not touch the project at all; accepting may
 *                    only JOIN components, and if it did, it must be undoable
 *   OVERLAP MODES    the two unconditional overlap treatments must always be
 *                    offered, and changing the treatment - which writes the
 *                    GLOBAL CONFIG mid-import - must not move the undo stack
 *   IMPORT GATE      a second file routed into a screen an import owns must be
 *                    refused (#411's `importFlowOwnsScreen`)
 *   EXPORT CHORD     Ctrl+E must be inert while a dialog owns the screen -
 *                    judged on the dialog the DISPATCHER saw, not the digest
 *   CLOSE            closing the project at idle must leave nothing behind
 *
 * The `RACE_TARGETS` generators still play (clicking a confirm the digest saw
 * disabled is a real thing to do to a wizard), but what they produce is COUNTED
 * rather than asserted: see the `raceMove` branch for why neither end of that
 * race is a contract this harness can hold the app to.
 *
 * Per FLOW (`lib/importFlowOracles.ts`): F1 undo roundtrip, F2 residue,
 * F3 receipt-vs-project accounting, F4 receipt rendering, F5 temp files.
 *
 * Per EPISODE: E1 teardown leaves no project, dialog, placement, context or
 * generated file; E2 the next episode's fixture opens integrity-clean; E3 DOM
 * growth is recorded as a metric.
 *
 * ## Progress forcing
 *
 * A random walk over a wizard does not terminate: `toggle-alternatives` is
 * always legal and never advances anything. So a screen sat on for 8 steps gets
 * its advancing move played and one sat on for 16 gets abandoned - see
 * `importDialogModel.ts`, which also owns the ESCAPE and WEDGE oracles.
 *
 * ## Impossible moves are skipped, never asserted
 *
 * Every generator returns `null` when its control is not there, and the bridge
 * reports `absent` / `not-actionable` / `option-absent` rather than throwing.
 * That is what keeps a ddmin-cut trace RUNNABLE: a shrunk trace reaches states
 * the original run never had, and half its moves become impossible there.
 *
 * ## Record, replay, shrink
 *
 * Every executed move is recorded as a trace step (`lib/fuzzTrace.ts`, v2). A
 * failing run drops the trace beside the session artifacts and names it in the
 * failure message; `pnpm agent:scenario import-dialog-fuzz --trace FILE` replays
 * it through the same bridge and the same oracles, and `pnpm agent:fuzz-shrink`
 * minimizes it. Replay is a MODE of this scenario, never a second scenario.
 *
 * CLEANUP IS PART OF THE CONTRACT, not tidiness. `fuzz-shrink` reuses ONE live
 * app session for every ddmin candidate, so a candidate that leaves a project
 * open, a dialog up or a generated file on disk poisons the verdict of every
 * candidate after it - and ddmin would then minimize toward the leak rather than
 * toward the bug. That is why E1 ASSERTS the teardown rather than merely
 * performing it.
 *
 * A trace never carries an absolute path. Generated files travel as their SPEC
 * plus a session-relative `rel`; the one move that needs a writable directory
 * (the new-project save path) records a PLACEHOLDER that this file substitutes
 * at dispatch time. Both are the same rule `fuzzTrace.ts` states: a trace is a
 * file from wherever the reader got it, and it does not get to choose where this
 * process writes.
 *
 * ## Knobs (all read at CALL time, so a replay's restored env reaches them)
 *
 *   STALAGMIGHT_FUZZ_FIXTURE                    which `.stalx` to open
 *   STALAGMIGHT_FUZZ_IMPORT_FORMAT              csv of families, default all
 *                                               (dmp,tml,compass,walls,gpx)
 *   STALAGMIGHT_FUZZ_IMPORT_FLOWS_PER_EPISODE   default 3
 *   STALAGMIGHT_FUZZ_IMPORT_MAX_FLOW_STEPS      default 140
 *   STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P         default 0.2
 *   STALAGMIGHT_FUZZ_IMPORT_SUGGESTIVE_P        default 0.5 (well-formed .dmp)
 *   STALAGMIGHT_FUZZ_IMPORT_INTERJECT_P         default 0.08
 *   STALAGMIGHT_FUZZ_IMPORT_AUX_INTERJECT_P     default 0.04 (wave-3 stream)
 *   STALAGMIGHT_FUZZ_IMPORT_SETTLE_MS           default 12000
 *   STALAGMIGHT_FUZZ_IMPORT_GPS_PASSABLE_P      default 0.6, clamped to [0.5, 1]
 *   STALAGMIGHT_FUZZ_IMPORT_NO_PROJECT_P        default 0.3; 0 = fixture always
 *   STALAGMIGHT_FUZZ_IMPORT_FORCE_STATIONS      default on; 0 = fixture's own
 *   STALAGMIGHT_FUZZ_CORPUS_DIR                 optional corpus root
 *   STALAGMIGHT_FUZZ_CORPUS_DRAW_P              default 0.25; 1 = corpus only
 *   STALAGMIGHT_FUZZ_TRACE / _REPLAY            record / replay a trace
 *
 * ## Findings
 *
 * This is the RECORD, not a to-do list, and it follows `undoRedoFuzz.ts`'s
 * conventions: every repro line is kept verbatim and stays runnable, and when a
 * finding is fixed its entry names the commit and becomes that fix's regression
 * check. Do NOT add a classifier or a tolerance to turn a red green - this file
 * has neither, and that is the whole discipline it inherits.
 *
 * WAVE 3 MOVED THE TRAJECTORIES, and the entries below say so rather than
 * implying otherwise. Findings 1 and 3 were recorded on the DMP-only walk; the
 * five-format draw and the auxiliary interjection stream change which moves a
 * seed makes, so those repro lines no longer replay the exact sequence that
 * found the bug. They are still run as regression checks and are still expected
 * green - and the SHRUNK five-step sequence in finding 1 is the durable form of
 * that check, because it is a trace rather than a seed. The auxiliary stream is
 * deliberately a SECOND stream (`AUX_INTERJECT_STREAM_SALT`) so that the
 * original interjection stream's own draws are bit-identical; that bounds the
 * change to inserted moves rather than a re-partitioned distribution.
 *
 * 1. FIXED (95155c51), issue #476. THE IMPORT WIZARD BECAME AN INVISIBLE
 *    MODAL, PERMANENTLY. This is now that fix's regression check - a red here
 *    again is a NEW bug.
 *
 *        pnpm agent:scenario import-dialog-fuzz --seed 7 --iterations 8
 *
 *    Found on this scenario's first real run; `pnpm agent:fuzz-shrink` cut the
 *    124-step trace to FIVE steps (83 replays, 1-minimal, 0 divergent):
 *
 *        1. generate a VALID .dmp                (no malformed input involved)
 *        2. import it                            (router or toolbar, either)
 *        3. continue                             (the default whole-file selection)
 *        4. skip zero datum
 *        5. accept the first suggestion          (confirm-button)
 *
 *    `activeDialog` stayed `'dmp-import-new'` with the wizard mounted under a
 *    `display:none` ancestor - the digest measured `display=flex
 *    visibility=visible 0x0`, which is what a `display:none` PARENT produces on
 *    a descendant. `DialogWrapper` sets it in one place, its `hidden` prop, fed
 *    by `autoStartManualPlacementPending`.
 *
 *    Mechanism: the flag is set at the `manual_placing` transition and was
 *    cleared in exactly ONE place, inside `handleStartSegmentPlacement`, behind
 *    two early returns - the first of which is `refuseIncompleteShots()`. The
 *    auto-start effect has no dependency that changes on a refusal, so it never
 *    retried. The incompleteness is FILE-BORNE: the parser clamps a negative
 *    raw length to exactly 0.0 (`parsers/dmp.rs:370`), the shot survives
 *    `isImportableDmpShot`, and `editableShotStatus.ts:66` marks `length <= 0`
 *    missing. Fixed by moving ownership of the hidden window into the effect
 *    that opens it, and taking the payload guard off a transition that submits
 *    nothing.
 *
 *    A NOTE ON THIS FILE'S FIRST DIAGNOSIS, because it was wrong and the way it
 *    was wrong is reusable. It blamed the effect's cleanup starving the
 *    `setTimeout(0)`, and ruled the refusal out on `toasts: []` in the payload.
 *    That evidence was worthless: the settle loop treats "wizard mounted but
 *    hidden" as busy, so on this very wedge it burns the whole settle cap
 *    (12 s, or 60 s in the verification run) before the digest is taken, and a
 *    warning toast dwells 7 s. The toast had always expired. An oracle that
 *    waits for quiescence cannot then use the ABSENCE of a transient as
 *    evidence.
 *
 * 2. FIXED (3595b151), issue #477. A COMPLETED DMP IMPORT LEFT `importContext`
 *    SET.
 *
 *    `completeImportToSingleView` states the contract - "This is also where the
 *    import flow ENDS, so it is where `importContext` is dropped" - and every
 *    other importer routes through it, as do both failure paths via
 *    `surfaceImportFailure`. The DMP wizard's `onComplete` in `App.tsx`
 *    hand-rolled the same exit sequence and omitted that line. The sibling
 *    branch settles that it was an oversight: `OrphanPlacementDialog.finish()`
 *    clears it explicitly and points at #435 for the duplication. Fixed by
 *    routing the wizard's completion through the shared helper.
 *
 *    NOT VERIFIED BY THIS SCENARIO, and the entry says so rather than implying
 *    otherwise. #477 was established by READING the code. The red that first
 *    drew attention to it was F2 asserting "a flow whose undo depth grew must
 *    end with a null context", and that premise is false: the DMP wizard commits
 *    EARLY and mid-flow, so a user can land sections and then walk back and
 *    cancel, and a retained context on the cancel path is by design
 *    (`routeOpenedFile` resets it at the next flow's entry). Re-checked with the
 *    fix reverted, the corrected oracle does not fire - measured: no flow on
 *    that seed presented a receipt in either surface. The assertion is gone and
 *    the value is counted instead (`residualImportContext:*`); see
 *    `checkResidue` for the full argument.
 *
 * 3. FIXED (caaa1847), issue #478. A CLEARED DATE MADE THE WIZARD SEND
 *    `date: ""`, AND TAURI REJECTED THE WHOLE `import_sections` CALL.
 *
 *        pnpm agent:scenario import-dialog-fuzz --seed 7 --iterations 8
 *
 *    By hand: import a `.dmp`, continue, skip the zero datum, CLEAR the date
 *    field on the section being placed, and complete the placement. The call
 *    fails at Tauri's argument layer with `invalid args \`sections\` for command
 *    \`import_sections\`: premature end of input`, so the placement does not
 *    import and the wizard reopens showing serde's message.
 *
 *    `ImportSectionConfig.date` is `Option<NaiveDate>`
 *    (`src-tauri/src/models/section.rs:253`), and chrono parses `""` as
 *    TOO_SHORT, whose Display is literally `premature end of input`.
 *    `buildBatchImport` normalised `surveyor` and not `date`, one line apart.
 *
 *    HOW THIS ONE WAS FOUND is the part worth keeping. It was first recorded
 *    here as a log line the oracle sweep reported at severity `error` - which
 *    does not fail a run - so the scenario was GREEN while the app was telling
 *    it that a command had been rejected. The gap was the oracle's, not the
 *    app's: S6 now asserts the IPC contract directly, and the distinction it
 *    draws is in `checkIpcContract`. A command that RAN and returned
 *    `success: false` is the app reporting a problem properly, and this
 *    scenario provokes plenty of those on purpose; a command whose ARGUMENTS
 *    could not be deserialized never ran at all, and that is a wire-format
 *    disagreement, always a defect, whatever the command or the content. That
 *    is what makes it an oracle rather than a classifier for this bug.
 *
 *    The root cause was measured rather than guessed, because the message
 *    points nowhere near it. `e2e/agent/ipc-payload-probe.ts` (kept, and the
 *    evidence behind the issue) ruled out the three things it suggests: payload
 *    SIZE (32 MiB of one string argument arrives intact), CHARACTER CLASS (NUL,
 *    SOH, BS, TAB, LF, CR, ESC, DEL, U+200B, U+202E and U+FFFD all arrive; lone
 *    surrogates fail with a DIFFERENT message), and SERIALIZATION refusals
 *    (BigInt and cyclic structures fail in the webview with their own errors,
 *    never as a truncated body). `date: ""` is the only probed input that
 *    reproduces it.
 *
 *    A note for whoever next wants the failing PAYLOAD rather than the failing
 *    line: you cannot get it by patching `invoke`. Tauri ships
 *    `window.__TAURI_INTERNALS__.invoke` NON-CONFIGURABLE and non-writable, and
 *    the global object itself resists replacement, so a spy wrapped around it
 *    silently records nothing - which looks exactly like a spy that saw
 *    nothing. That was tried, measured (zero rejections recorded on a run whose
 *    log had two) and removed; the log ring is the source S6 uses.
 *
 * 4. FIXED (64f6a525), issue #479. A COMPASS `.DAT` WITH AN IMPOSSIBLE
 *    `SURVEY DATE:` PANICKED THE BACKEND, AND THE IMPORT PROMISE NEVER SETTLED.
 *
 *    NOT FOUND BY A RUN, and the entry says so rather than implying otherwise.
 *    It was found while transcribing the Compass grammar for this wave's
 *    writers - `NaiveDate::from_ymd_opt(..).unwrap()` in the vendored survey
 *    parser, with `digit1.parse::<u32>().unwrap()` beside it - and then
 *    measured against the real parser. `parse_compass` is a synchronous
 *    `#[tauri::command]`, so the panic unwinds out of the command and the
 *    frontend's promise never resolves: the dialog sits on its spinner with no
 *    error and no way back except restarting the app.
 *
 *    It is now this scenario's coverage rather than only a Rust test: the
 *    `dat:impossible-date` and `dat:date-field-overflow` catalog entries emit
 *    exactly those six shapes, so a malformed-heavy Compass run walks the fix.
 *
 *        STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P=0.6 pnpm agent:scenario \
 *          import-dialog-fuzz --seed 3 --iterations 6
 *
 * ## Observations that are NOT findings
 *
 * `closeProject` does not clear `activeDialog`. It nulls the project and the
 * derived state (`clearOpenProjectState`) and never touches the dialog, so
 * every import dialog stays mounted over a project that is gone - the ones
 * gated on `dialogData.<x>File` because that survives the close, and
 * `new-project-for-import` / `orphan-placement` because they are gated on
 * `activeDialog` alone.
 *
 * This scenario does NOT report it, and the reason is the discipline rather
 * than the severity: NOTHING IN THE UI CAN REACH IT. No control binds
 * `handleCloseProject` while a dialog is open, both keyboard dispatchers return
 * early for every non-Escape key in that state, and the mode carousel and the
 * toolbar sit behind the modal backdrop. Driving the raw store action there
 * reaches a state no user can, and a bug found in one of those is a fabricated
 * bug - the same argument this file already makes for `uiStore.confirmPlacement()`.
 * So the interjection plays `closeProject` at `idle` only, where it IS
 * reachable, and asserts the contract there. If a future change gives the user
 * a way to close a project mid-import, this becomes a finding on the spot.
 */
import * as fs from 'fs';
import * as path from 'path';

import { mulberry32, type Scenario, type ScenarioContext } from './framework';
import { DEFAULT_FIXTURE, disableAutoSave, loadFixtureWithoutUndoHistory } from './common';
import { ORACLE_GLOBAL, oracleInjectionSource } from '../lib/fuzzOracleCore';
import { mapClick } from '../lib/controls';
import { isCorpusEntryRel, planReplay, traceHasWellFormedFlows, TraceRecorder, type TraceStep } from '../lib/fuzzTrace';
import { fuzzFixture } from '../lib/fuzzKnobs';
import {
  describeRunError,
  failureText,
  isWebDriverStall,
  MoveLog,
  openReplay,
  RERUN_WITH,
  throwFailure,
  writeRunTrace,
} from '../lib/fuzzHarness';
import {
  materializeSurveyFile,
  randomSurveyFileSpec,
  selfCheckClaim,
  type GeneratedFileSpec,
  type SurveyFileFormat,
} from '../lib/surveyFileSpecs';
import {
  CORPUS_DIR_ENV,
  corpusEntryFor,
  corpusLookup,
  describeCorpus,
  discoverCorpus,
  foldThenCut,
  materializeCorpusEntry,
  redactCorpusText,
  registerCorpusRedactions as registerProcessRedactions,
  type CorpusEntry,
} from '../lib/fixtureCorpus';
import type { ImportScreenDigest } from '../lib/importScreenDigest';
import { LogCursor } from '../lib/logs';
import { valueShape } from '../lib/shareableFailure';
import { shareableTestId } from '../../../scripts/lib/shareable-selector.mjs';
import {
  checkAccounting,
  checkFrozenDelete,
  checkIpcContract,
  checkPushed,
  checkReceipt,
  checkResidue,
  checkRoundtripState,
  checkTempFiles,
  exportChordVerdict,
  ImportOracleError,
  importGateRefusalSeen,
  leftoverTempNames,
  captureReceipt,
  nextErrorGrace,
  routeHandoff,
  s4WorkStarted,
  type CountedProject,
  type FileHandoff,
  type FlowBounds,
  type FlowShape,
  type ReceiptClaim,
} from '../lib/importFlowOracles';
import {
  ADVANCE_MOVE,
  ESCAPE_BUDGET,
  ESCAPE_PATH,
  IMPORT_GUARDED_SCREENS,
  isEscaped,
  NON_FLOW_SCREENS,
  OVERLAP_MODES,
  progressMode,
  WEDGE_EXEMPT_SCREENS,
  type ScreenId,
} from './importDialogModel';
import {
  answerFileDialogInPage,
  answerKeyboardFileDialog,
  dismissFirstRunModal,
  injectScreenDigest,
  observeScreen,
  routeFile,
  runUiStep,
  type Move,
} from './importDialogBridge';
import {
  CorpusEntryPaths,
  FileSeq,
  moveFromTraceStep,
  moveTraceStep,
  sourceTraceStep,
} from './importDialogTrace';
import {
  AUX_INTERJECT_STREAM_SALT,
  auxInterjectProbability,
  buildAuxInterjection,
  buildInterjection,
  corpusDrawProbability,
  drawAuxInterjection,
  drawInterjection,
  enabledFamilies,
  enabledFormats,
  interjectProbability,
  INTERJECT_STREAM_SALT,
  PROJECT_DIR_PLACEHOLDER,
  weightedFor,
  type FileSource,
  type FuzzMove,
  type GenEnv,
} from './importDialogGenerators';

const SCENARIO = 'import-dialog-fuzz';
/** Session-relative homes for what a run writes. Both are wiped per episode. */
const FILES_REL = 'fuzz-files';
const PROJECTS_REL = 'fuzz-projects';
/** Attempts to leave `idle` before a flow gives up and is counted as skipped. */
const START_ATTEMPTS = 4;
/** Failure messages print this many trailing moves. */
const HISTORY_TAIL = 25;
/** Specs the writer/parser drift detector pushes through `import_dmp`. */
const SELF_CHECK_SPECS = 6;
/**
 * `TOAST_KEYS.IMPORT_RECEIPT` (`src/utils/toastKeys.ts`). A clean receipt is a
 * toast rather than the modal, and both surfaces prove the completion path ran.
 */
const RECEIPT_TOAST_KEY = 'import-receipt';
/**
 * `TOAST_KEYS.IMPORT_IN_PROGRESS` (`src/utils/toastKeys.ts`). What
 * `routeOpenedFile` raises when `importFlowOwnsScreen` refuses a second file.
 */
const IMPORT_GATE_TOAST_KEY = 'import-in-progress';
/** Times an `idle` screen is re-observed before a flow is called finished. */
const IDLE_CONFIRMATIONS = 3;
/** Grace between those observations. One IPC round trip is tens of ms. */
const IDLE_CONFIRM_MS = 200;
/**
 * Default share of episodes that run with NO project open.
 *
 * The split matters: with nothing loaded a `.dmp` routes to
 * `new-project-for-import` instead of the selection dialog, which is the only
 * way to reach `importContext='new-project'` and its deferred initial save.
 *
 * It is now a knob (`STALAGMIGHT_FUZZ_IMPORT_NO_PROJECT_P`) because the other
 * end matters too, and could not be asked for: the DMP wizard's suggester scores
 * an import against the sections ALREADY in the project, so a campaign aimed at
 * the suggestion surface wants the fixture open every time. Zero is a legal
 * value and means exactly that. Every seed's trajectory depends on it, so a
 * repro line has to carry it - which the `STALAGMIGHT_FUZZ_` prefix rule in
 * `fuzzTrace.ts` makes automatic.
 */
const NO_PROJECT_EPISODE_P = 0.3;

function noProjectEpisodeProbability(): number {
  const raw = Number(process.env.STALAGMIGHT_FUZZ_IMPORT_NO_PROJECT_P);
  return Number.isFinite(raw) && raw >= 0 && raw <= 1 ? raw : NO_PROJECT_EPISODE_P;
}

/**
 * Should each episode make the survey's stations visible before it starts?
 *
 * Default ON. `0` restores the old behaviour - the fixture's own saved display
 * settings - which is what a run WOULD want if it were fuzzing the display
 * toggles themselves rather than the import flows. See `ensureStationsVisible`
 * for what a hidden-station fixture does to the placement surface.
 */
function forceStationsVisible(): boolean {
  return process.env.STALAGMIGHT_FUZZ_IMPORT_FORCE_STATIONS !== '0';
}

// ---------------------------------------------------------------------------
// Knobs. Read at CALL time, never frozen at import: a replay restores the
// recording's environment AFTER this module is loaded (see scenario-run.ts).
// The fixture and the trace replay/record paths are the shared ones in
// `lib/fuzzKnobs.ts`.
// ---------------------------------------------------------------------------

function flowsPerEpisode(): number {
  const raw = Number(process.env.STALAGMIGHT_FUZZ_IMPORT_FLOWS_PER_EPISODE);
  return Number.isInteger(raw) && raw > 0 ? raw : 3;
}
function maxFlowSteps(): number {
  const raw = Number(process.env.STALAGMIGHT_FUZZ_IMPORT_MAX_FLOW_STEPS);
  return Number.isInteger(raw) && raw > 0 ? raw : 140;
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/**
 * Clicks that CAN start fallible work, and so are eligible for S4's exemption.
 *
 * Eligibility only - `s4WorkStarted` decides whether a given move actually did,
 * and a move that started nothing is neither exempted nor granted grace.
 *
 * Every one of them re-enters the open/import ROUTER (or creates a project),
 * and a malformed file or an unwritable path there is a reported error, not a
 * defect. Nothing else on the flow has any business producing one.
 *
 * The four import confirms are here for the same reason a `route` is: they hand
 * a file the parser accepted to the command that actually imports it, and a
 * file whose CONTENT the importer then refuses is a reported error.
 */
const ERROR_PRODUCING: ReadonlySet<string> = new Set([
  'create-and-continue-button',
  'import-blocked-switch',
  'import-as-new-project-confirm',
  'retry-button',
  'tml-import-confirm',
  'tml-import-residue-discard',
  'compass-import-confirm',
  'walls-import-confirm',
  'gpx-import-confirm',
  'gpx-continue-button',
]);

function mayRaiseError(move: FuzzMove): boolean {
  if (move.kind === 'openFile' || move.kind === 'cancelDialog' || move.kind === 'keyFile') return true;
  if (move.kind === 'ui') return ERROR_PRODUCING.has(move.testid);
  return false;
}

/**
 * Is this move a picker CANCEL - eligible by shape, but resolving with no path?
 *
 * The only no-work case the move's own kind can settle. Everything else needs
 * the OUTCOME, which is what `FileHandoff` carries; `s4WorkStarted` puts the
 * two together.
 */
function isPickerCancel(move: FuzzMove): boolean {
  return move.kind === 'cancelDialog';
}

/**
 * Confirms that are DISABLED while a duplicate-hash probe is in flight.
 *
 * The window is short and real, and each screen's `race-*` generator aims at
 * exactly these. What comes back is COUNTED rather than asserted - see the
 * `raceMove` branch, which explains why neither end of the race is a contract
 * reachable from here.
 */
const RACE_TARGETS: ReadonlySet<string> = new Set([
  'continue-button',
  'tml-import-confirm',
  'compass-import-confirm',
  'walls-import-confirm',
  'gpx-continue-button',
  'gpx-import-confirm',
]);

/**
 * Wizard commits `refuseIncompleteShots()` guards.
 *
 * All three open with `if (refuseIncompleteShots()) return;`
 * (`DmpNewImportDialog.tsx`), so a row the grid left half-filled blocks every
 * way forward until it is fixed. The guard is the only one in the wizard whose
 * refusal the fuzzer could reach and never checked.
 */
const INCOMPLETE_GUARDED_MOVES: ReadonlySet<string> = new Set([
  'confirm-button',
  'skip-button',
  'segment-place-button',
  // The guard's FOURTH call site, and the one that was missed:
  // `handlePlaceOnMapClick` opens with the same `if (refuseIncompleteShots())
  // return;` before it hands off to `handleStartSegmentPlacement`. It is also
  // the busiest - it is what `ADVANCE_MOVE['wiz-manual']` reaches for first - so
  // leaving it out meant the refusal that stops most flows was the one nothing
  // watched.
  'segment-place-on-map-button',
]);

/** `TOAST_KEYS.DMP_WIZARD_INCOMPLETE` - the key, which is i18n-independent. */
const TOAST_INCOMPLETE = 'dmp-wizard-incomplete';

/**
 * The orphan dialog's two kinds of button, and what each one owes the project.
 *
 * Written from the dialog as OBSERVED, not from its source: a hand-driven run
 * (`.agent-artifacts/orphan-proof-20260811/`) raised it with two suggestions
 * queued - "Connect Orphaned Sections / Section 1 of 2", one suggested
 * connection with a merge-station picker and an overlap mode, and four enabled
 * buttons. Accepting joins the orphan to a newly imported section; skipping and
 * closing only move through the queue.
 */
const ORPHAN_ACCEPTS: ReadonlySet<string> = new Set(['orphan-accept', 'orphan-accept-all']);
const ORPHAN_DECLINES: ReadonlySet<string> = new Set(['orphan-skip', 'orphan-close']);

/**
 * Overlap treatments NO guard in the wizard disables.
 *
 * `isTargetDoublyAnchored` greys out `interpolate` and `import_data` only; the
 * other two are rendered unconditionally, so a build that stopped offering one
 * of them would leave a user with an overlap and no treatment to apply to it.
 */
const OVERLAP_MODES_ALWAYS_LEGAL: ReadonlySet<string> = new Set(['existing_data', 'keep_both']);

function actionableCount(digest: ImportScreenDigest): number {
  let count = 0;
  for (const state of Object.values(digest.live)) if (state === 2) count += 1;
  return count;
}

/** Bounds a flow is measured between. */
interface Bounds extends FlowBounds {
  /** The project's own file path, so a flow that SWAPPED projects is visible. */
  projectFile: string | null;
}

// ---------------------------------------------------------------------------

/**
 * A refused map click as its `mapSkip:*` metric key names it (#576 W51): a code
 * for each refusal `lib/controls.ts` `mapClick` words itself, and `other` for
 * any other error. The key prints in the runner's PASS line and the matrix
 * report, and an error's own text - a page script's message, or a map position,
 * which is where the cave is - is for neither.
 */
export const mapSkipCode = (err: unknown): string => {
  if (!(err instanceof Error)) return 'unknown';
  if (err.message === 'map not available') return 'map-not-available';
  if (err.message.startsWith('map click failed: map point is outside the visible viewport')) return 'outside-viewport';
  if (err.message.startsWith('map click failed: ')) return 'click-refused';
  if (err.message === 'map interaction hooks are unavailable') return 'no-interaction-hooks';
  if (/^map \w+ handler did not process the click/.test(err.message)) return 'not-processed';
  return 'other';
};

export const importDialogFuzz: Scenario = {
  name: SCENARIO,
  description:
    'Generates DMP/Ariane/Compass/Walls/GPX files and walks the real import UI at random, asserting ' +
    'per-step coherence and a per-flow undo roundtrip, residue, accounting, receipt and temp-file contract.',
  defaultIterations: 3,

  async run(ctx: ScenarioContext, { iterations }): Promise<void> {
    const fixture = fuzzFixture();
    const sessionDir = ctx.artifactsDir;
    if (!sessionDir) {
      throw new Error(`${SCENARIO} needs a session artifacts dir to write its generated files into`);
    }
    const filesDir = path.join(sessionDir, FILES_REL);
    const projectsDir = path.join(sessionDir, PROJECTS_REL);

    // ---- input sources ---------------------------------------------------
    //
    // The format axis and the private corpus are both resolved ONCE: discovery
    // walks a directory, and a generator runs once per step. Neither is ever
    // named in a log line - `describeCorpus` prints counts per format, and a
    // corpus file is identified downstream by `corpusFileId`.
    const formats = enabledFormats();
    // ONE walk, shared by every format: `discoverCorpus` reads a directory tree
    // that can hold thousands of files, and this runs at the start of every
    // session (and of every ddmin candidate under `fuzz-shrink`).
    const inventory = discoverCorpus();
    const corpus: Partial<Record<SurveyFileFormat, readonly CorpusEntry[]>> = {};
    let corpusUsable = 0;
    for (const format of formats) {
      const entries = inventory.byFormat[format] ?? [];
      corpus[format] = entries;
      corpusUsable += entries.length;
    }
    ctx.log(`formats: ${enabledFamilies().join(',')} (${formats.join(',')})`);
    ctx.log(describeCorpus(inventory));
    ctx.setMetric('corpusEntriesUsable', corpusUsable);
    if (inventory.tooDeep > 0) {
      // The inventory above is a LOWER bound: this many directories sat past the
      // discovery walk's depth limit and were never opened. A metric as well as
      // the log line, because the log scrolls and the metric block is what a
      // run's summary and an issue carry - and a corpus the fuzzer can only half
      // see is a coverage claim wrong by an unknown amount. It names a count,
      // never a path, like everything else corpus-shaped that leaves this run.
      ctx.setMetric('corpusDirsTooDeep', inventory.tooDeep);
    }
    if (corpusUsable === 0 && corpusDrawProbability() >= 1) {
      // The run ASKED for corpus-only and there is nothing to draw: every file
      // will be synthetic, which is the opposite of what the knob requested. A
      // metric as well as a log line, because the log scrolls and the metric
      // block is what a campaign summary carries.
      ctx.setMetric('corpusDrawUnsatisfied', 1);
      ctx.log(
        `${CORPUS_DIR_ENV} draw share is 1 but no entry matches the enabled formats ` +
          `(${enabledFormats().join(',')}) - every file this run imports will be generated`,
      );
    }
    if (corpusUsable === 0) {
      // Not a failure and never one: a corpus is optional by design, and a run
      // without one is exactly as valid, just narrower. Counted so a run that
      // MEANT to use one can tell that it did not.
      ctx.metric('corpus:absent');
      // Only the UNCONFIGURED case gets told to set the knob. A corpus that is
      // set and yielded nothing has a different problem, and `describeCorpus`
      // has already printed which one (a depth bound, a skip, no importable
      // format) on the line above - repeating "set this variable" there sends
      // the reader to check something that is already correct.
      if (!inventory.configured) ctx.log(`corpus: set ${CORPUS_DIR_ENV} to widen this run's inputs`);
    }

    // `openReplay` refuses a trace recorded by another scenario or on another
    // fixture: the recorded steps carry ids and rels resolved against THAT
    // project, so replaying them elsewhere drives moves at things that do not
    // exist there - an invented failure, or a green run, either way not the
    // recorded bug.
    const { replayPath, traceOut, replay } = openReplay(SCENARIO, fixture, RERUN_WITH);

    const recorder = new TraceRecorder();
    /**
     * The run's move history and the move RUNNING RIGHT NOW (`lib/fuzzHarness.ts`):
     * `execute` arms a move before it runs and disarms it after, so a failure
     * names a move that never returned instead of the one before it.
     */
    const moveLog = new MoveLog();
    /**
     * Substrings printed output must never contain, and what to say instead.
     *
     * `describeSource` keeps a corpus file's NAME out of every label this run
     * writes, and the trace names an opened entry by `corpusEntryRel`, but the
     * app puts the name back: an import that fails quotes the path it was
     * given, and the copy under `fuzz-files/corpus-N/` keeps the original
     * basename because a `.mak` names its sibling `.dat` files and a `.wpj`
     * names its `.srv` tree - renaming them would break the very files the
     * corpus exists to exercise. So the file names are real, and a failure
     * message that echoes one would carry it into the issue the failure gets
     * attached to. Filled in as entries are materialized; applied by `fail`,
     * and - because the list is the PROCESS's (`fixtureCorpus.redactCorpusText`)
     * - by the runners that print the oracle sweep's findings too (#576 HFI-13).
     */

    /**
     * Register the redactions for one materialized corpus entry.
     *
     * Shared by BOTH materialization sites, and that is the whole point: the
     * generator path registered them and the REPLAY path did not, so replaying a
     * corpus trace put real basenames - which the app echoes back in its own
     * messages - straight into `fail()` output, the trace and the printed
     * detail. One helper is what stops the next call site from forgetting.
     *
     * `names` is every segment the copy created AT ANY DEPTH - a Walls
     * project's `.PATH` subfolder can name a cave system exactly as well as a
     * file can, and a single-level readdir here used to miss all of them.
     */
    const registerCorpusRedactions = (id: string, destination: string, names: readonly string[]): void => {
      registerProcessRedactions(id, destination, names);
    };

    const redact = redactCorpusText;
    /** Where this run's corpus entry tokens point, on a replay - see `CorpusEntryPaths`. */
    const corpusEntries = new CorpusEntryPaths();
    /** Numbers the walk's input files and new projects - see `FileSeq`. */
    const files = new FileSeq();
    let lastUndoDepth = -1;
    /**
     * Episodes that ran with NO project open, recorded so a replay can reproduce
     * them.
     *
     * The decision is a per-episode coin toss and it changes which import flow
     * the same file steps drive - with nothing loaded a `.dmp` routes through
     * `new-project-for-import` instead of the selection dialog. Restoring
     * `STALAGMIGHT_FUZZ_IMPORT_NO_PROJECT_P` restores the ODDS, and a replay
     * does not re-draw them, so the odds alone cannot reproduce the choice.
     */
    const headlessEpisodes: number[] = [];

    const dumpTrace = (failure: { message: string } | null): string | null =>
      writeRunTrace(
        ctx,
        recorder,
        {
          scenario: SCENARIO,
          iterations,
          fixture,
          // v3 = the five-format vocabulary: corpus sources, the armed
          // keyboard picker, and the auxiliary interjection stream.
          stream: 'import-v3',
          noProjectEpisodes: headlessEpisodes,
        },
        failure,
        traceOut,
      );

    /**
     * Every error `fail` has thrown. A throw that is NOT one of these died
     * outside the oracles - a stall, a harness error - and the run's catch
     * routes it through `fail` so it gets the reproduce line, the move history
     * and the trace a failure is owed (#576 HFI-9).
     */
    const raisedByFail = new WeakSet<object>();
    /**
     * The step a dispatch that has NOT RETURNED would have recorded, or null.
     * Set immediately before each dispatching await and cleared the moment it
     * returns, so a stall leaves exactly the move it stalled on - which goes
     * into the trace as its last step, because a trace that stopped one move
     * short would replay green.
     */
    let inFlightStep: TraceStep | null = null;
    /** Set when the run died on a stall: the page is not answering, so skip in-page cleanup. */
    let pageStalled = false;

    /**
     * Annotated on the CONST so TypeScript treats a `fail(...)` call as
     * never-returning (a caller ending in one needs no dead `return` after it).
     */
    const fail: (headline: string, detail?: string) => never = (headline, detail = '') => {
      // The tail ends on the move that had not RETURNED, if any: without that it
      // would end on the move BEFORE the one that failed, which reads as an
      // attribution and is one.
      const tail = moveLog.tail(HISTORY_TAIL);
      // REDACTED, because this text is what goes into an issue: the app quotes
      // the path it was handed, and a corpus copy keeps its original file name.
      const message = redact(
        failureText(
          headline,
          [
            `  reproduce: pnpm agent:scenario ${SCENARIO} --seed ${ctx.seed} --iterations ${iterations}` +
              `${fixture === DEFAULT_FIXTURE ? '' : ` with STALAGMIGHT_FUZZ_FIXTURE=${fixture}`}` +
              `${replayPath ? ` (REPLAY of ${replayPath})` : ''}`,
            `  fixture=${fixture} flowsPerEpisode=${flowsPerEpisode()} maxFlowSteps=${maxFlowSteps()}`,
          ],
          detail,
          'move(s)',
          tail,
        ),
      );
      try {
        throwFailure(message, dumpTrace);
      } catch (thrown) {
        if (typeof thrown === 'object' && thrown !== null) raisedByFail.add(thrown);
        throw thrown;
      }
    };

    // ---- paths -----------------------------------------------------------

    /**
     * A session-relative `rel` as an absolute path, refusing anything that
     * leaves the run's own directory.
     *
     * A tracked fixture is named by `fixtures/<basename>` and resolved through
     * `ctx.fixturePath`, which materializes a COPY into exactly that directory.
     * So one rel means the same file on any machine, without the trace carrying
     * a fixture (which the repo forbids) or an absolute path. `path.basename` is
     * the containment rule on that branch.
     *
     * `readTrace` already applies the same rule to every file-bearing step, so on
     * the replay path this is belt and braces - but the GENERATOR path never goes
     * through `readTrace`, and a containment rule that only one of two callers
     * enforces is a convention rather than a boundary.
     */
    const resolveRel = (rel: string): string => {
      if (rel.startsWith('fixtures/')) return ctx.fixturePath(path.basename(rel));
      const root = path.resolve(sessionDir);
      const target = path.resolve(root, rel);
      if (target !== root && !target.startsWith(root + path.sep)) {
        throw new Error(`generated file path ${valueShape(rel)} escapes ${root}`);
      }
      return target;
    };

    const materialize = (spec: GeneratedFileSpec, rel: string): string => {
      const target = resolveRel(rel);
      const named: GeneratedFileSpec = { ...spec, name: path.basename(target) };
      materializeSurveyFile(path.dirname(target), named);
      return target;
    };

    /**
     * Absolute path of a source's file, or null when it is not on disk.
     *
     * A CORPUS source is the one that can legitimately come back null on a
     * machine that is not the recorder's: the trace names the file by id, and a
     * replayer with no corpus (or a different one) simply does not have it. That
     * is skipped-and-counted like any other impossible move - never a failure,
     * because a corpus is optional by design.
     */
    const sourceOnDisk = (source: FileSource): string | null => {
      if (source.origin === 'gen') {
        // Counted here rather than at draw time: `drawFileSource` is called for
        // candidates the 8-attempt loop may discard, and a generator that
        // recorded a metric would count files that were never written.
        if (source.shape !== undefined) ctx.metric(`file:shape:${source.shape}`);
        return materialize(source.spec, source.rel);
      }
      if (source.origin === 'fixture') return ctx.fixturePath(source.name);
      if (source.origin === 'corpus') {
        const entry = corpusEntryFor(source.corpusRel);
        if (entry === null) {
          ctx.metric('corpus:absent');
          return null;
        }
        const destination = resolveRel(source.dirRel);
        const copied = materializeCorpusEntry(entry, destination);
        if (copied.skipped) {
          // WHICH bound, because they mean different things: `bytes`/`files`
          // is a directory too big to copy per iteration, `depth` is a tree
          // shape nobody expected, and `io` is a copy that failed and was
          // undone. A half-copied project would look like a corrupt one from
          // inside the app, which is why none of them copies anything.
          ctx.metric(`corpus:notMaterialized:${copied.reason ?? 'unknown'}`);
          return null;
        }
        ctx.metric('corpus:materialized');
        ctx.metric('corpus:copiedBytes', copied.bytes);
        ctx.metric('corpus:copiedFiles', copied.copied);
        registerCorpusRedactions(source.id, destination, copied.names);
        return copied.path;
      }
      // A replayed corpus entry is a TOKEN, bound when its `corpusFile` step
      // materialized the copy; unbound (the step was shrunk away, or this
      // machine lacks the entry) it is a missing file like any other.
      const entry = corpusEntries.resolve(source.rel);
      if (entry !== undefined) return entry;
      const target = resolveRel(source.rel);
      return fs.existsSync(target) ? target : null;
    };

    /**
     * Record the step that PUT a file on disk, before the step that opens it.
     *
     * A generated file travels as its spec; a corpus file travels as its
     * `corpusFileId` and nothing else - never its name, which is the rule
     * `lib/fixtureCorpus.ts` states and `fuzzTrace.ts` enforces. Both are
     * resolved against a directory the REPLAYER chooses.
     */
    const recordFileSource = (source: FileSource, record: boolean): void => {
      if (!record) return;
      const step = sourceTraceStep(source);
      if (step !== null) recorder.record(step);
    };

    /**
     * A run must never write a `.stalx` outside its own session directory.
     *
     * `NewProjectForImportDialog` defaults the save path to the user's
     * Documents folder and the project NAME to the imported file's basename, so
     * a `create-and-continue-button` clicked before the path was redirected
     * creates a project there - and on a corpus run that file is NAMED AFTER A
     * PRIVATE SURVEY, in a directory nothing here ever cleans up. Measured: a
     * Walls run left `~/Documents/stalagmight/fuzz_3.stalx` behind, and the app
     * auto-renamed around the previous run's copy of it.
     *
     * So the create click is REPLACED by the move that fixes the path, and the
     * walk gets to it on its next step. The substitution is recorded, so a
     * replay is faithful; and it is done here rather than in a generator
     * because progress forcing plays the same click from the model's table
     * without asking a generator at all.
     */
    const enforceSavePath = (move: FuzzMove, digest: ImportScreenDigest): FuzzMove => {
      if (move.kind !== 'ui' || move.testid !== 'create-and-continue-button') return move;
      if (digest.savePath !== null && digest.savePath.startsWith(projectsDir)) return move;
      ctx.metric('savePathRedirected');
      return {
        kind: 'input',
        testid: 'project-save-path-input',
        value: PROJECT_DIR_PLACEHOLDER,
        mode: 'type',
        label: 'redirect the save path into the session directory',
      };
    };

    /**
     * Substitute the real projects directory for the placeholder a generator
     * emits. Applied at DISPATCH, never before recording, so the trace keeps
     * the placeholder and a replayed file cannot pick where a `.stalx` lands.
     */
    const dispatchable = (move: FuzzMove): FuzzMove => {
      if (move.kind !== 'input' || move.value !== PROJECT_DIR_PLACEHOLDER) return move;
      fs.mkdirSync(projectsDir, { recursive: true });
      return { ...move, value: path.join(projectsDir, `import-${ctx.seed}-${files.value}.stalx`) };
    };

    /** The map's current centre, so a placement click lands on the survey. */
    const mapCentre = async (): Promise<[number, number] | null> =>
      ctx.eval<[number, number] | null>(`
        const map = window.__stalagmight_map__;
        if (!map || typeof map.getCenter !== 'function') return null;
        const centre = map.getCenter();
        return [centre.lng, centre.lat];
      `);

    // ---- move execution --------------------------------------------------

    /**
     * Per-call WALL TIME, summed and at its worst.
     *
     * The maxima are the load-bearing half. Every in-page call this scenario
     * makes goes through WebDriver's `execute/async`, which
     * `lib/sessionShared.ts` gives 30 s and no more - and a fused UI step is
     * action + settle (up to `STALAGMIGHT_FUZZ_IMPORT_SETTLE_MS`, 12 s) + a
     * digest that invokes `get_project_state` on whatever the project has grown
     * to. On a corpus run that is a real number, so how close the worst call came
     * to the ceiling is something a campaign should be able to read off its own
     * metrics rather than reconstruct from a `WebDriverError: operation was
     * aborted due to timeout` with no attribution in it.
     */
    let bridgeStepMsMax = 0;
    let bridgeDigestMsMax = 0;
    const noteBridgeTiming = (timing: { actionMs: number; settleMs: number; digestMs: number }): void => {
      ctx.metric('bridgeActionMs', timing.actionMs);
      ctx.metric('bridgeSettleMs', timing.settleMs);
      ctx.metric('bridgeDigestMs', timing.digestMs);
      const step = timing.actionMs + timing.settleMs + timing.digestMs;
      if (step > bridgeStepMsMax) {
        bridgeStepMsMax = step;
        ctx.setMetric('bridgeStepMsMax', step);
      }
      if (timing.digestMs > bridgeDigestMsMax) {
        bridgeDigestMsMax = timing.digestMs;
        ctx.setMetric('bridgeDigestMsMax', timing.digestMs);
      }
    };

    interface Executed {
      digest: ImportScreenDigest;
      performed: boolean;
      skip: string | null;
      /**
       * The target's disabled state as the BRIDGE read it, one round trip after
       * the digest that chose the move. Null for every move that is not a click
       * or an edit on a live element. See `StepOutcome.offAtDispatch`.
       */
      offAtDispatch: boolean | null;
      /**
       * The dialog the app's own gate saw: for a `key:` chord, what
       * `useKeyboardShortcuts` read; for a `route`, what `importFlowOwnsScreen`
       * read. Null for every other move, and whenever no dialog was up.
       */
      dialogAtDispatch: string | null;
      /**
       * `placementMode !== null` at the same instant: on a `route`, what
       * `importFlowOwnsScreen` read; on a `key:` chord, one of the three gates
       * `useKeyboardShortcuts` reads. Null on every other move.
       */
      placementAtDispatch: boolean | null;
      /** `dataTableFullScreen` at the same instant. `key:` chords only. */
      fullscreenAtDispatch: boolean | null;
      /**
       * On a `route`, the toasts it raised, read when it returned
       * (`RouteOutcome.toastsRaised`, #576 HFI2-F3). Null on every other move.
       */
      toastsRaisedAtDispatch: string[] | null;
      /**
       * What a file entry point did with the path. Null for every other move.
       *
       * The distinction S4's grace window turns on - see `errorGraceVerdict`.
       * A move can be `performed` and still have handed nothing over: a chord
       * the dispatcher swallowed, a trigger that opened no picker, a route the
       * import gate refused.
       */
      handoff: FileHandoff | null;
    }

    /** Record what a performed move did - the one shape `moveTraceStep` builds. */
    const recordMove = (move: FuzzMove): void => {
      const step = moveTraceStep(move);
      if (step !== null) recorder.record(step);
    };

    /**
     * One dispatching await, with `step` as the in-flight step while it runs.
     * A throw leaves `inFlightStep` set - see its comment - and propagates.
     */
    const dispatch = async <T>(step: TraceStep | null, call: () => Promise<T>): Promise<T> => {
      inFlightStep = step;
      const out = await call();
      inFlightStep = null;
      return out;
    };

    /** A corpus draw, or a replayed corpus entry token. */
    const fromCorpus = (source: FileSource): boolean =>
      source.origin === 'corpus' || (source.origin === 'disk' && isCorpusEntryRel(source.rel));

    /** Run one move, record it if it happened, and report the screen after. */
    const execute = async (move: FuzzMove, record: boolean): Promise<Executed> => {
      moveLog.startingMove(move.label);
      let performed = false;
      let skip: string | null = null;
      let offAtDispatch: boolean | null = null;
      let dialogAtDispatch: string | null = null;
      let placementAtDispatch: boolean | null = null;
      let fullscreenAtDispatch: boolean | null = null;
      let toastsRaisedAtDispatch: string[] | null = null;
      let handoff: FileHandoff | null = null;
      let digest: ImportScreenDigest;

      if (move.kind === 'ui') {
        const outcome = await dispatch(record ? moveTraceStep(move) : null, () => runUiStep(ctx, move as Move));
        performed = outcome.performed;
        skip = outcome.reason;
        offAtDispatch = outcome.offAtDispatch;
        dialogAtDispatch = outcome.dialogAtDispatch;
        placementAtDispatch = outcome.placementAtDispatch;
        fullscreenAtDispatch = outcome.fullscreenAtDispatch;
        digest = outcome.digest;
        noteBridgeTiming(outcome.timing);
        if (record && performed) recordMove(move);
      } else if (move.kind === 'input') {
        // The dispatched value may carry a substituted path; the RECORDED one
        // never does - see `dispatchable`.
        const outcome = await dispatch(record ? moveTraceStep(move) : null, () =>
          runUiStep(ctx, dispatchable(move) as Move),
        );
        performed = outcome.performed;
        skip = outcome.reason;
        offAtDispatch = outcome.offAtDispatch;
        dialogAtDispatch = outcome.dialogAtDispatch;
        digest = outcome.digest;
        noteBridgeTiming(outcome.timing);
        if (record && performed) recordMove(move);
      } else if (move.kind === 'openFile') {
        const live = move;
        const absolute = sourceOnDisk(live.source);
        if (absolute === null) {
          // A REPLAY whose `genFile` step ddmin cut away, or a corpus entry
          // this machine does not have.
          skip = fromCorpus(live.source) ? 'corpus-missing' : 'file-missing';
          digest = await observeScreen(ctx, false);
        } else {
          recordFileSource(live.source, record);
          if (live.via === 'route') {
            const routed = await dispatch(record ? moveTraceStep(move) : null, () => routeFile(ctx, absolute));
            if (!routed.routed) fail('the E2E routeOpenedFile hook is missing; rebuild with VITE_E2E=1');
            performed = true;
            dialogAtDispatch = routed.dialogAtDispatch;
            placementAtDispatch = routed.placementAtDispatch;
            toastsRaisedAtDispatch = routed.toastsRaised;
            // `routeOpenedFile` returns void, so acceptance is read off the
            // values its guards consulted on the same tick: a dialog, placement,
            // or an import's file still parsing (`routeHandoff`). The E2E hook
            // never passes opts, so no re-entry exception applies.
            handoff = routeHandoff(routed);
            if (record) recordMove(move);
            digest = await observeScreen(ctx);
          } else {
            const answered = await dispatch(record ? moveTraceStep(move) : null, () =>
              answerFileDialogInPage(ctx, live.trigger, 'open', absolute),
            );
            performed = answered.performed;
            skip = answered.reason;
            digest = answered.digest;
            handoff = answered.consumed ? 'accepted' : 'unconsumed';
            if (performed && !answered.consumed) ctx.metric('dialogAnswerUnconsumed');
            if (record && performed) recordMove(move);
          }
        }
      } else if (move.kind === 'keyFile') {
        const live = move;
        const absolute = sourceOnDisk(live.source);
        if (absolute === null) {
          skip = fromCorpus(live.source) ? 'corpus-missing' : 'file-missing';
          digest = await observeScreen(ctx, false);
        } else {
          recordFileSource(live.source, record);
          const answered = await dispatch(record ? moveTraceStep(move) : null, () =>
            answerKeyboardFileDialog(ctx, live.chord, absolute),
          );
          performed = answered.performed;
          skip = answered.reason;
          digest = answered.digest;
          // A chord the dispatcher swallowed (it returns early whenever a
          // dialog is open) leaves the answer unconsumed. That is the app's
          // documented gate, so it is counted rather than asserted.
          handoff = answered.consumed ? 'accepted' : 'unconsumed';
          ctx.metric(answered.consumed ? 'keyPickerConsumed' : 'keyPickerSwallowed');
          if (record && performed) recordMove(move);
        }
      } else if (move.kind === 'closeProject') {
        const res = await dispatch(record ? moveTraceStep(move) : null, () =>
          ctx.callAction('project', 'closeProject', []),
        );
        performed = res.ok;
        if (!res.ok) skip = 'close-refused';
        if (record && performed) recordMove(move);
        digest = await observeScreen(ctx);
      } else if (move.kind === 'cancelDialog') {
        const live = move;
        const answered = await dispatch(record ? moveTraceStep(move) : null, () =>
          answerFileDialogInPage(ctx, live.trigger, 'open', null),
        );
        performed = answered.performed;
        skip = answered.reason;
        digest = answered.digest;
        if (record && performed) recordMove(move);
      } else if (move.kind === 'mapClick') {
        const live = move;
        // A relative draw is resolved HERE and recorded ABSOLUTE: replay must
        // click the same geographic point, not the same offset from whatever
        // camera the replay happens to have.
        const centre = live.relative === true ? await mapCentre() : [0, 0];
        const at: [number, number] =
          centre === null
            ? live.lngLat
            : [live.lngLat[0] + (centre[0] as number), live.lngLat[1] + (centre[1] as number)];
        try {
          await dispatch(record ? { kind: 'mapClick', lngLat: at, label: move.label } : null, () =>
            ctx.exclusive(() => mapClick(ctx.browser, at)),
          );
          performed = true;
          if (record) recorder.record({ kind: 'mapClick', lngLat: at, label: move.label });
        } catch (err) {
          // A STALL is not an impossible click: the page stopped answering, and
          // swallowing it here would let the walk carry on into the next
          // timeout with the attribution lost.
          if (isWebDriverStall(err)) throw err;
          // Outside the viewport, or no map on this screen. Both are ordinary
          // impossible moves - skipped and counted, exactly like an absent
          // testid - because a shrunk trace reaches states with a different
          // camera and would otherwise die at the first recorded click.
          skip = 'map-unavailable';
          // A code, not the error's text (#576 W51; it was the text folded
          // and cut, W33-NF3): a metric key reaches the runner's PASS line and
          // the matrix report, and neither folds it.
          ctx.metric(`mapSkip:${mapSkipCode(err)}`);
        }
        digest = await observeScreen(ctx);
      } else if (move.kind === 'undo' || move.kind === 'redo') {
        const which = move.kind;
        const res = await dispatch(record ? moveTraceStep(move) : null, () => ctx.callAction('project', which, []));
        performed = res.ok;
        if (!res.ok) skip = `${which}-refused`;
        if (record && performed) recordMove(move);
        digest = await observeScreen(ctx);
      } else {
        fs.mkdirSync(projectsDir, { recursive: true });
        const target = path.join(projectsDir, `flow-save-${ctx.seed}.stalx`);
        const res = await dispatch(record ? moveTraceStep(move) : null, () =>
          ctx.callAction('project', 'saveProject', [target]),
        );
        performed = res.ok;
        if (!res.ok) skip = 'save-refused';
        if (record && performed) recordMove(move);
        digest = await observeScreen(ctx);
      }

      moveLog.finishedMove();
      moveLog.note(`${move.label}${performed ? '' : ` -> SKIPPED (${skip ?? 'unknown'})`} [${digest.screen}]`);
      ctx.metric(performed ? 'movesPerformed' : `skip:${skip ?? 'unknown'}`);
      return {
        digest,
        performed,
        skip,
        offAtDispatch,
        dialogAtDispatch,
        placementAtDispatch,
        fullscreenAtDispatch,
        toastsRaisedAtDispatch,
        handoff,
      };
    };

    // ---- per-step oracles ------------------------------------------------

    /**
     * Moves for which a NEWLY raised error dialog is still the previous exempt
     * move's, not this one's.
     *
     * An exempt move hands a file to a parser and returns; the parse resolves on
     * its own schedule, and the error dialog it raises can land after the move's
     * settle loop has already reported a stable, dialog-free screen - the settle
     * marker cannot see a promise. S4 then blames whatever move happened to be
     * next. Measured, on the corpus campaign's seed-70 cell: `import via
     * toolbar-import-button` (a `.dmp` with no valid sections) came back at
     * `idle`, and "Import Failed / No valid sections found in DMP file" was
     * reported against the mode-switch interjection that followed it.
     *
     * One move of grace, because that is the whole observed lag and every extra
     * move is real coverage given away: S4's other half - an error dialog left
     * UP exempting every later move - is already conceded by the
     * `screenBefore !== 'error'` clause, and this is the same concession for the
     * instant the dialog arrives.
     *
     * The one longer lag the app itself announces is held instead of guessed: an
     * import's file still parsing after a move (`digest.parsing`) keeps the
     * window open until the parse ends (`nextErrorGrace`, #576 W62-NF3).
     */
    const ERROR_GRACE_MOVES = 1;
    let errorGrace = 0;

    const stepOracles = async (
      move: FuzzMove,
      digest: ImportScreenDigest,
      screenBefore: ScreenId,
      performed: boolean,
      handoff: FileHandoff | null,
    ): Promise<void> => {
      if (digest.boundary) {
        fail('S2 ERROR BOUNDARY: the React tree crashed', `  after: ${move.label}`);
      }
      if (digest.screen === 'unknown') {
        fail(
          // The dialog name is IN THE HEADLINE, not in the detail, because
          // `failureSignature` builds a shrink's target from the first line: a
          // generic "unknown screen" headline matches every unknown screen, and
          // ddmin then accepts a candidate that fails on a different one.
          // A store value (`activeDialog: string | null`): it prints as a test
          // id does, the words of the app's source and other text as its
          // length (#576 W51).
          `S1 UNKNOWN SCREEN "${shareableTestId(digest.dialog)}": a dialog is up that this build's ` +
            'screen table does not name',
          `  activeDialog=${JSON.stringify(digest.dialog)} after: ${move.label}\n` +
            `  dom: ${digest.why}\n` +
            // Toast KEYS, not text: they are i18n-independent, and a refusal
            // toast is usually the whole explanation for a screen that stopped
            // moving (the wizard's guards refuse silently apart from these).
            `  toasts: ${JSON.stringify(digest.toast)}`,
        );
      }
      if (digest.inv.length > 0) {
        fail(
          'S3 INVARIANT: the project is not well-formed',
          [`  after: ${move.label}`, ...digest.inv.map((line) => `  INVARIANT: ${line}`)].join('\n'),
        );
      }
      // RAISED one, which is not the same as "was on screen while one was up".
      // A move that legitimately raises the error dialog (a malformed file
      // routed in) leaves it there, and every move after it - an undo, an
      // Escape, a click that missed - would otherwise be blamed for it.
      // Measured, on the seed-2 matrix cell: `import via router` produced "No
      // valid sections found in DMP file", which is the app reporting a problem
      // properly, and the NEXT interjection was reported as the defect.
      // ONE decision, two uses - see `s4WorkStarted`. A move that handed work to
      // something that can fail is not blamed for an error on its own step AND
      // opens the window for that result to land on the next one; a move that
      // handed nothing over gets neither. Deciding those separately, one on move
      // shape and one on outcome, is what let the same false negative in three
      // times.
      const work = s4WorkStarted({
        eligible: mayRaiseError(move),
        performed,
        cancel: isPickerCancel(move),
        handoff,
      });
      if (digest.screen === 'error' && screenBefore !== 'error' && !work.started) {
        if (errorGrace > 0) {
          // The previous move's parse landing late - see ERROR_GRACE_MOVES.
          ctx.metric('s4LateErrorCarried');
        } else {
          fail(
            'S4 UNEXPECTED ERROR DIALOG: a move with no business failing raised one',
            // The digest carries the whole text; folded, THEN cut (#576 W33-NF3).
            `  after: ${move.label}\n  error: ${digest.err === null ? '(no text)' : foldThenCut(digest.err, 300)}\n` +
              `  started no fallible work: ${work.reason}`,
          );
        }
      }
      // Counted AFTER the check, so a move's own error does not spend the grace
      // it is granting. Both sides counted, and only for moves whose KIND could
      // have started work, so a campaign sees the split rather than inferring it
      // from an absence: an `s4NoWork:*` that dominates would mean the entry
      // points are mostly not landing, which is worth knowing on its own.
      if (work.started) ctx.metric('s4WorkStarted');
      else if (work.reason !== 'not-eligible') ctx.metric(`s4NoWork:${work.reason}`);
      if (!work.started && digest.parsing) ctx.metric('s4GraceHeldByParse');
      errorGrace = nextErrorGrace({
        previous: errorGrace,
        started: work.started,
        parsing: digest.parsing,
        window: ERROR_GRACE_MOVES,
      });
      if (digest.screen !== 'idle' && !WEDGE_EXEMPT_SCREENS.has(digest.screen) && actionableCount(digest) === 0) {
        fail(
          `S5 WEDGE on "${digest.screen}": no actionable control anywhere in the inventory`,
          `  screen=${digest.screen} activeDialog=${JSON.stringify(digest.dialog)} after: ${move.label}`,
        );
      }
      ctx.metric(`screen:${digest.screen}`);

      // S6 IPC CONTRACT. `checkIpcContract` documents the distinction it draws.
      // Through `guarded`, like every other oracle that lives in the oracle
      // module: its own `fail` throws a bare ImportOracleError, which would lose
      // the reproduce line, the move history and the dropped trace.
      const fresh = await ctx.exclusive(() => logs.read('error'));
      guarded(() => checkIpcContract(fresh.newEntries, move.label));

      // An import commits through `import_sections`, which pushes exactly one
      // undo entry per call - so a rise in depth is the cheap signal that
      // something landed, and the moment to pay for the structural check.
      if (digest.undo >= 0 && lastUndoDepth >= 0 && digest.undo > lastUndoDepth) {
        ctx.metric('undoEntriesPushed', digest.undo - lastUndoDepth);
        const verdict = (await ctx.ipc('check_project_integrity')) as {
          save_blocked?: boolean;
          messages?: string[];
        } | null;
        const messages = verdict?.messages ?? [];
        const nonFinite = messages.filter((line) => line.includes('non-finite'));
        if (verdict?.save_blocked === true || nonFinite.length > 0) {
          fail(
            'INTEGRITY: an import left the project in a state the app refuses to save',
            [
              `  after: ${move.label}`,
              `  save_blocked=${verdict?.save_blocked === true}`,
              ...messages.slice(0, 8).map((line) => `  ${line}`),
            ].join('\n'),
          );
        }
      }
      if (digest.undo >= 0) lastUndoDepth = digest.undo;
    };

    /**
     * Wait until `idle` is REST, not just a gap between two macrotasks.
     *
     * Completing a map placement runs `void handlePlacementComplete()` - an
     * async chain that imports the segment and then REOPENS the wizard. Between
     * the store update that ends the placement and the reopen, `activeDialog` is
     * null and `placementMode` is null, so the screen reads `idle` while the
     * import is still in flight. A flow that ended there measured its residue
     * mid-completion and reported a context the app was about to clear.
     *
     * So an idle screen is re-observed a few times before it is believed. The
     * settle loop already waits for a stable marker; this adds the "and it is
     * still idle a moment later" half, which is what distinguishes finished from
     * between-two-ticks. Anything that comes back non-idle hands the walk back
     * its screen and the flow continues.
     */
    const settleToRest = async (from: ImportScreenDigest): Promise<ImportScreenDigest> => {
      let digest = from;
      for (let attempt = 0; attempt < IDLE_CONFIRMATIONS; attempt += 1) {
        if (digest.screen !== 'idle' || digest.placement) return digest;
        await ctx.sleep(IDLE_CONFIRM_MS);
        digest = await observeScreen(ctx);
      }
      return digest;
    };

    /**
     * Run a FLOW oracle through `fail`.
     *
     * The oracles in `lib/importFlowOracles.ts` throw on their own, which is
     * right - they are the contract - but a raw throw loses the three things
     * that make a failure actionable: the `reproduce:` line, the tail of moves
     * that produced it, and the dropped trace. Every F-check goes through here
     * so a flow failure reads exactly like a step failure.
     */
    const guarded = <T>(what: () => T): T => {
      try {
        return what();
      } catch (err) {
        if (err instanceof ImportOracleError) fail(err.message);
        throw err;
      }
    };

    /**
     * Play a screen's escape path until the walk is back at `idle`.
     *
     * The budget is absolute. A user who cannot leave a dialog has no other
     * recourse, so neither does this: an escape that has not landed inside
     * `ESCAPE_BUDGET` forced moves FAILS the run.
     */
    const escapeFrom = async (from: ImportScreenDigest): Promise<ImportScreenDigest> => {
      let digest = from;
      for (let attempt = 0; attempt < ESCAPE_BUDGET; attempt += 1) {
        if (isEscaped(digest.screen, digest.placement)) return digest;
        const route = ESCAPE_PATH[digest.screen] ?? ['key:Escape'];
        const testid =
          route.find((candidate) => candidate.startsWith('key:') || digest.live[candidate] === 2) ?? 'key:Escape';
        const move: FuzzMove = { kind: 'ui', testid, label: `ESCAPE ${digest.screen} via ${testid}` };
        ctx.metric('escapeMoves');
        const screenBefore = digest.screen;
        const outcome = await execute(move, true);
        digest = outcome.digest;
        await stepOracles(move, digest, screenBefore, outcome.performed, outcome.handoff);
      }
      if (isEscaped(digest.screen, digest.placement)) return digest;
      fail(
        `ESCAPE: could not leave "${digest.screen}" in ${ESCAPE_BUDGET} forced move(s)`,
        `  activeDialog=${JSON.stringify(digest.dialog)} placement=${digest.placement}\n` +
          `  path tried: ${JSON.stringify(ESCAPE_PATH[digest.screen] ?? [])}`,
      );
    };

    // ---- flow bookkeeping ------------------------------------------------

    /**
     * The whole project state, canonicalized IN PAGE - the single most expensive
     * call this scenario makes, and the one with the most room to grow: it
     * invokes `get_project_state` and then walks the result. Timed and sized for
     * the same reason `noteBridgeTiming` keeps maxima.
     */
    let canonicalMsMax = 0;
    let canonicalBytesMax = 0;
    const canonicalNow = async (has: boolean): Promise<string | null> => {
      if (!has) return null;
      const startedAt = Date.now();
      const canonical = await ctx.eval<string | null>(`
        const O = globalThis.${ORACLE_GLOBAL};
        if (!O) throw new Error('fuzz oracle is not injected');
        const core = window.__TAURI__ && window.__TAURI__.core;
        const state = await core.invoke('get_project_state');
        return state === null || state === undefined ? null : O.canonical(state, false);
      `);
      const elapsed = Date.now() - startedAt;
      ctx.metric('canonicalMs', elapsed);
      if (elapsed > canonicalMsMax) {
        canonicalMsMax = elapsed;
        ctx.setMetric('canonicalMsMax', elapsed);
      }
      if (canonical !== null && canonical.length > canonicalBytesMax) {
        canonicalBytesMax = canonical.length;
        ctx.setMetric('canonicalBytesMax', canonical.length);
      }
      return canonical;
    };

    const boundsNow = async (digest: ImportScreenDigest): Promise<Bounds> => {
      // Off the DIGEST, not a round trip of its own: bounds are compared against
      // counts read in that same digest, and a separately-fetched path could
      // describe a different instant than the numbers beside it.
      const file = digest.projectFile;
      const leftovers = leftoverTempNames(file);
      return {
        snapshot: await canonicalNow(digest.hasProject),
        undoDepth: digest.undo,
        sections: digest.sections,
        shots: digest.shots,
        projectDir: leftovers.dir,
        leftovers: leftovers.names,
        projectFile: file,
      };
    };

    // ---- move sources ----------------------------------------------------

    const interjectRng = mulberry32((ctx.seed ^ INTERJECT_STREAM_SALT) >>> 0);
    const auxRng = mulberry32((ctx.seed ^ AUX_INTERJECT_STREAM_SALT) >>> 0);

    const genEnv = (digest: ImportScreenDigest): GenEnv => ({
      ctx,
      digest,
      filesRel: FILES_REL,
      fileSeq: files.value,
      corpus,
    });

    const drawMove = (digest: ImportScreenDigest, mode: 'draw' | 'advance'): FuzzMove | null => {
      if (mode === 'advance') {
        const route = ADVANCE_MOVE[digest.screen] ?? [];
        // A `*` entry is a PREFIX the model could not spell concretely (the
        // zero-datum section cards carry an index). Expanded against the live
        // digest and resolved to the LOWEST actionable index, so a forced
        // advance is as deterministic as a literal one.
        const resolve = (candidate: string): string | undefined => {
          if (!candidate.endsWith('*')) return digest.live[candidate] === 2 ? candidate : undefined;
          const prefix = candidate.slice(0, -1);
          const matches = Object.keys(digest.live)
            .filter((id) => id.startsWith(prefix) && digest.live[id] === 2 && /^\d+$/.test(id.slice(prefix.length)))
            .sort((a, b) => Number(a.slice(prefix.length)) - Number(b.slice(prefix.length)));
          return matches[0];
        };
        let testid: string | undefined;
        for (const candidate of route) {
          testid = resolve(candidate);
          if (testid !== undefined) break;
        }
        if (testid !== undefined) {
          ctx.metric('forcedAdvance');
          // Per screen as well as in total. The aggregate said 135 forced
          // advances and 435 unavailable ones over the corpus campaign and
          // could not say WHERE, which is what hid two screens whose advancing
          // control is disabled by default: `zero-datum-config`'s confirm needs
          // a GPS that parses, and `wiz-manual`'s needed a placement mode the
          // walk had not chosen yet. Both read as "the walk is slow" until the
          // counter was split.
          ctx.metric(`forcedAdvance:${digest.screen}`);
          return { kind: 'ui', testid, label: `FORCED advance on ${digest.screen}` };
        }
        ctx.metric('forcedAdvanceUnavailable');
        ctx.metric(`forcedAdvanceUnavailable:${digest.screen}`);
      }
      // Interjections come off their OWN stream, so adding one later does not
      // shift the trajectory every existing repro line depends on. Wave 3's
      // additions get a SECOND stream for the same reason, rather than being
      // folded into `drawInterjection`'s partition.
      if (interjectRng() < interjectProbability()) {
        const kind = drawInterjection(interjectRng);
        const move = buildInterjection(kind, genEnv(digest));
        if (move !== null) {
          ctx.metric(`interject:${kind}`);
          files.noteMove(move);
          return move;
        }
      }
      if (auxRng() < auxInterjectProbability()) {
        const kind = drawAuxInterjection(auxRng);
        const move = buildAuxInterjection(kind, genEnv(digest));
        if (move !== null) {
          ctx.metric(`interject:${kind}`);
          files.noteMove(move);
          return move;
        }
        ctx.metric(`interjectSkipped:${kind}`);
      }
      const pool = weightedFor(digest.screen);
      if (pool.length === 0) {
        ctx.metric(`noGenerators:${digest.screen}`);
        return null;
      }
      // Up to 8 draws before giving up: on a screen whose controls are mostly
      // absent most generators legitimately return null, and one draw would
      // report a screen as generator-less when it is not.
      for (let attempt = 0; attempt < 8; attempt += 1) {
        const generator = pool[Math.floor(ctx.rng() * pool.length)];
        if (!generator) continue;
        const move = generator.build(genEnv(digest));
        if (move === null) continue;
        ctx.metric(`gen:${generator.name}`);
        // A generator legal on ONE screen is already attributed by its name; one
        // legal on several is not, and the shot-table and metadata families are
        // shared by `zero-datum-config`, `wiz-suggest` and `wiz-manual`. The
        // corpus campaign could only apportion their 226 firings by the ratio of
        // screen samples, which is an estimate presented as a measurement. This
        // is the measurement.
        if (generator.screens.length > 1) ctx.metric(`gen@${digest.screen}:${generator.name}`);
        files.noteMove(move);
        return move;
      }
      ctx.metric(`noMove:${digest.screen}`);
      return null;
    };

    // ---- injection + generator self-check ---------------------------------

    await ctx.eval<boolean>(`${oracleInjectionSource()}\nreturn true;`);
    await injectScreenDigest(ctx);
    // A fresh profile opens behind the first-run telemetry modal, which is
    // rendered outside `activeDialog` and covers the toolbar. Clear it once here
    // rather than spending a flow on it; the model still knows the screen, so a
    // reappearance mid-run is handled rather than mistaken for `idle`.
    if (await dismissFirstRunModal(ctx)) ctx.setMetric('dismissedFirstRunModal', 1);
    const logs = new LogCursor(ctx.browser);
    await ctx.exclusive(() => logs.checkpoint());

    /**
     * WRITER/PARSER DRIFT DETECTOR.
     *
     * `surveyFileWriters.ts` encodes what the Rust parser decodes. If the two
     * ever disagree, every generated file is rejected at the door and the whole
     * scenario passes GREEN having exercised nothing but the error dialog. So
     * valid-biased specs are written and pushed through the REAL `import_dmp`
     * command before any fuzzing starts, and a disagreement is the scenario's
     * own failure rather than a quiet loss of coverage.
     */
    {
      const probeDir = path.join(filesDir, 'selfcheck');
      const probeRng = mulberry32((ctx.seed ^ 0x5bd1e995) >>> 0);
      let probes = 0;
      // EVERY format this run may draw, not just the first: a drift in one
      // writer would otherwise hide behind the others passing.
      for (const format of formats) {
        for (let i = 0; i < SELF_CHECK_SPECS; i += 1) {
          const drawn = randomSurveyFileSpec(probeRng, 'valid', format);
          const spec: GeneratedFileSpec = { ...drawn, name: `selfcheck-${format}-${i}.${format}` };
          const { path: written } = materializeSurveyFile(probeDir, spec);
          const claim = selfCheckClaim(spec);
          const parsed = await ctx.ipc(claim.command, { path: written });
          const found = claim.read(parsed);
          const withinBounds = found >= claim.min && (claim.max === null || found <= claim.max);
          if (!withinBounds) {
            throw new Error(
              `GENERATOR SELF-CHECK failed on ${written}: the writer emitted a spec claiming ` +
                `${claim.min}${claim.max === null ? '+' : claim.max === claim.min ? '' : `..${claim.max}`} ` +
                `${claim.what}(s) and ${claim.command} read ${found}. ` +
                'The writer has drifted from the Rust parser; every generated file of this format would be ' +
                'rejected at the door and this run would pass green having exercised nothing.',
            );
          }
          probes += 1;
        }
      }
      ctx.setMetric('selfCheckSpecs', probes);
      fs.rmSync(probeDir, { recursive: true, force: true });
    }

    /**
     * Put the app and the disk back to a usable state, whatever happened.
     *
     * Best-effort and unconditional, and it is what makes `fuzz-shrink` sound.
     * The shrinker reuses ONE live session for every ddmin candidate, and a
     * candidate that FAILS never reaches its episode teardown - so without this
     * it leaves the project open, a dialog up and its generated `.dmp` files on
     * disk, and the next candidate inherits all of it. That is not a tidiness
     * problem: it makes later candidates fail (or pass) for reasons their own
     * trace does not contain, and ddmin then minimizes toward the leak instead
     * of toward the bug. Measured, not theorised - the first shrink of the
     * wizard-hidden finding cut 124 steps to 1, and that survivor passed
     * cleanly when replayed on its own.
     */
    const forceClean = async (): Promise<void> => {
      // After a STALL every in-page call would wait out the same budget again,
      // only to fail; the session is torn down after a failure anyway.
      if (!pageStalled) {
        await ctx.callAction('ui', 'cancelPlacementMode', []).catch(() => undefined);
        await ctx.callAction('ui', 'closeDialog', []).catch(() => undefined);
        await ctx.callAction('ui', 'setImportContext', [null]).catch(() => undefined);
        await ctx.callAction('project', 'closeProject', []).catch(() => undefined);
      }
      for (const dir of [filesDir, projectsDir]) {
        try {
          fs.rmSync(dir, { recursive: true, force: true });
        } catch {
          // A file the app still holds open is not worth failing a cleanup for.
        }
      }
    };

    // ---- the run ----------------------------------------------------------

    const replaySteps = replay ? replay.steps : null;
    /**
     * Which episodes the RECORDING ran headless, or null when the trace predates
     * the field. See `FuzzTrace.noProjectEpisodes`.
     */
    const replayHeadless = replay?.noProjectEpisodes ? new Set(replay.noProjectEpisodes) : null;

    /**
     * The recording's own episode and flow structure, rebuilt from its boundary
     * steps.
     *
     * A replay used to be ONE pass over the whole step list: `replayAt` is a
     * closure variable, so episode 0's single flow drained everything and every
     * later episode replayed nothing. Two things broke on that. A later
     * episode's headless choice could never reach the steps recorded under it -
     * a `.dmp` that originally entered `new-project-for-import` replayed against
     * an open project and silently took the selection flow. And F3 had no flow
     * boundaries to measure a receipt against, so it compared the first
     * receipt's claim to the delta of every import in the trace.
     *
     * `episode`/`flow` markers give both back. A trace WITHOUT them - anything
     * recorded before this - folds into a single episode carrying every step,
     * which is exactly what the old replay did, so those traces keep replaying
     * the way they always have.
     */
    const replayPlan = replay ? planReplay(replay) : [];
    /** Does this trace carry the boundaries F3 needs to measure a flow? */
    const replayHasBoundaries = replay ? traceHasWellFormedFlows(replay) : false;
    if (replay && replayHeadless === null && noProjectEpisodeProbability() > 0) {
      // The recording COULD have gone headless and this trace cannot say whether
      // it did, so the replay may walk a different import flow than the one that
      // failed. Loud, and only when the restored odds make it possible.
      ctx.metric('replay:noProjectUnknown');
      ctx.log(
        'REPLAY: this trace predates `noProjectEpisodes` and the restored ' +
          `STALAGMIGHT_FUZZ_IMPORT_NO_PROJECT_P is ${noProjectEpisodeProbability()} - ` +
          'assuming the fixture stayed open, which may not be the flow that was recorded',
      );
    }
    let replayAt = 0;

    /**
     * Make the survey's stations visible, because the placement surface needs
     * them.
     *
     * `MapCanvas` runs an effect that cancels an active placement the moment
     * `showStations` is false - `handlePlacementCancel()` fires the placement's
     * own `onCancel`, which in the DMP wizard's case re-opens the wizard. A
     * fixture whose SAVED display settings hide stations therefore turns the
     * whole manual-placement surface into a loop: the wizard auto-starts a
     * placement, the effect cancels it before a click can land, the wizard comes
     * back, and the Place-on-Map button it offers does nothing.
     *
     * Measured on `naharon.stalx`, whose `display_settings` carry
     * `showStations=false`: 955 place-on-map clicks across one campaign phase
     * produced ONE sampled `map-placement` frame, and every inert click came
     * back with the placement-hint toast up and the wizard visible again.
     *
     * Toggled here rather than worked around, because a hidden-station project
     * is a legitimate state a user can save and this is the state the SURFACE
     * needs, not a bug being hidden: the app behaviour that makes the button
     * inert is reported separately and is not patched by this scenario.
     */
    const ensureStationsVisible = async (): Promise<boolean> => {
      const read = async (): Promise<boolean> =>
        ctx.eval<boolean>(
          'return !!(window.__E2E_TEST__ && window.__E2E_TEST__.uiStore ' +
            '&& window.__E2E_TEST__.uiStore.getState().showStations);',
        );
      if (await read()) return true;
      // `toggleDisplay` is a toggle, not a setter, so this is read-then-flip.
      await ctx.callAction('ui', 'toggleDisplay', ['showStations']).catch(() => undefined);
      return read();
    };

    /**
     * How many merge-connected components carry NO zero datum.
     *
     * A transcription of the first two steps of
     * `Project::suggest_orphan_placements` - union-find over `station_merges`,
     * then drop every component that holds a datum - computed in the page off
     * `get_project_state`, which is the same payload the digest reads. It is the
     * PRECONDITION for the orphan dialog, and nothing else the harness records
     * can distinguish "no orphan component existed" from "one existed and the
     * geometry did not match".
     */
    const countUnanchoredComponents = async (): Promise<number> =>
      ctx.eval<number>(`
        const core = window.__TAURI__ && window.__TAURI__.core;
        if (!core || typeof core.invoke !== 'function') return -1;
        const state = await core.invoke('get_project_state');
        const project = (state && state.project) || null;
        if (!project || !Array.isArray(project.sections)) return -1;
        const parent = new Map();
        const find = (x) => { while (parent.get(x) !== x) { parent.set(x, parent.get(parent.get(x))); x = parent.get(x); } return x; };
        for (const section of project.sections) parent.set(section.id, section.id);
        const union = (a, b) => {
          if (!parent.has(a) || !parent.has(b)) return;
          const ra = find(a); const rb = find(b);
          if (ra !== rb) parent.set(ra, rb);
        };
        for (const merge of (project.station_merges || [])) {
          union(merge.station_a && merge.station_a.section_id, merge.station_b && merge.station_b.section_id);
        }
        // \`ZeroDatumWire.station_ref\` - the field name matters, and getting it
        // wrong is silent: every component then reads as unanchored.
        const anchored = new Set();
        for (const datum of (project.zero_datums || [])) {
          const ref = datum && datum.station_ref;
          const id = ref && ref.section_id;
          if (id !== undefined && parent.has(id)) anchored.add(find(id));
        }
        if (anchored.size === 0 && (project.zero_datums || []).length > 0) return -2;
        const roots = new Set();
        for (const section of project.sections) roots.add(find(section.id));
        let count = 0;
        for (const root of roots) if (!anchored.has(root)) count += 1;
        return count;
      `);

    /** The episode loop. Extracted so `forceClean` can wrap it in a finally. */
    const runEpisodes = async (): Promise<void> => {
    const episodeCount = replaySteps ? Math.max(1, replayPlan.length) : iterations;
    for (let episode = 0; episode < episodeCount; episode += 1) {
      await loadFixtureWithoutUndoHistory(ctx, fixture);
      if (!(await disableAutoSave(ctx))) {
        throw new Error('could not turn auto-save off; a timer firing mid-run would change unmodelled state');
      }
      if (forceStationsVisible() && !(await ensureStationsVisible())) {
        throw new Error(
          'could not make stations visible; every map placement would be cancelled the instant it started ' +
            '(MapCanvas cancels placement mode whenever showStations is false), and the wizard would offer an ' +
            'inert Place-on-Map button for the whole run',
        );
      }
      // E2: the fixture this episode starts from must itself be sound, or every
      // integrity verdict inside it is measured from a broken floor.
      const opening = (await ctx.ipc('check_project_integrity')) as {
        save_blocked?: boolean;
        messages?: string[];
      } | null;
      if (opening?.save_blocked === true) {
        throw new Error(
          `E2 INTEGRITY: ${fixture} opened SAVE-BLOCKED at the start of episode ${episode + 1}: ` +
            JSON.stringify(opening.messages ?? []),
        );
      }
      const domBefore = await ctx.eval<number>('return document.querySelectorAll("*").length;');

      // Some episodes run with NO project open, which is a different import
      // flow, not a smaller one: a `.dmp` with nothing loaded routes to
      // `new-project-for-import` instead of the selection dialog, so it is the
      // only way to reach `importContext='new-project'` and the DEFERRED
      // initial save that context selects. The fixture is still opened first,
      // so E2's integrity check on it runs either way.
      // On a REPLAY the recording's own choice is authoritative; a fresh run
      // draws it. `replayHeadless` is null for a trace written before the field
      // existed, which is warned about once at startup and treated as
      // fixture-open - what every replay did before this.
      const headless = replaySteps
        ? (replayPlan[episode]?.headless ?? false)
        : ctx.rng() < noProjectEpisodeProbability();
      if (!replaySteps && headless) headlessEpisodes.push(episode);
      // The boundary a replay rebuilds the episode structure from. Recorded
      // BEFORE the close below, so the marker precedes every step it scopes.
      if (!replaySteps) {
        recorder.record({
          kind: 'episode',
          index: episode,
          headless,
          label: `episode ${episode + 1}${headless ? ' (no project)' : ''}`,
        });
      }
      if (headless) {
        await ctx.callAction('project', 'closeProject', []).catch(() => undefined);
        ctx.metric('episodeWithoutProject');
      }

      let digest = await observeScreen(ctx, false);
      lastUndoDepth = digest.undo;

      // A replay is ONE pass over the recorded steps; the recording's own flow
      // boundaries are not replayed, they are re-derived from the screen.
      // On a replay, one flow per RECORDED flow - which is what gives F3 the
      // boundaries it measures a receipt against.
      const episodeFlows = replaySteps ? (replayPlan[episode]?.flows ?? [[]]) : null;
      const flows = episodeFlows ? episodeFlows.length : ctx.randInt(1, flowsPerEpisode());
      for (let flow = 0; flow < flows; flow += 1) {
        // The steps THIS flow replays. Per-flow rather than one global cursor:
        // that cursor is what let episode 0 drain the whole trace.
        const flowSteps = episodeFlows ? (episodeFlows[flow] ?? []) : null;
        replayAt = 0;
        if (!replaySteps) recorder.record({ kind: 'flow', index: flow, label: `flow ${flow + 1}` });
        const before = await boundsNow(digest);
        let receipt: ReceiptClaim | null = null;
        /**
         * The project as it was WHEN THE RECEIPT WAS PRESENTED - read off the
         * receipt's own digest, so it costs no extra IPC and cannot describe a
         * different moment than the claim it will be compared against.
         *
         * F3 needs this rather than the end-of-flow bounds because the walk goes
         * on after a receipt appears: the interjection stream plays `undo`, the
         * next file gets imported, the flow ends somewhere else entirely. A
         * delta measured across those is a measurement of the WALK.
         */
        let atReceipt: CountedProject | null = null;
        /** The project the receipt is ABOUT - see `ReceiptCapture`. */
        let projectFileAtReceipt: string | null = null;
        /**
         * Had an undo/redo/save/close already landed when the receipt appeared?
         *
         * Then `before` describes a project the interjection has since rewritten,
         * and the subtraction has no meaning in EITHER direction - so the flow is
         * counted rather than measured, exactly as F1 does with `historyTouched`.
         */
        let historyTouchedBeforeReceipt = false;
        /**
         * Did the completion path present a receipt, in EITHER of its surfaces?
         * The modal carries a decision; a clean receipt is a toast. Both come
         * from `presentImportReceipt`, so either one proves the flow finished
         * through the path that owes the `importContext` clear.
         */
        let sawReceipt = false;
        /**
         * Was the receipt TOAST already on screen when this flow began?
         *
         * `TOAST_KEYS.IMPORT_RECEIPT` dwells for seconds, and a flow starts the
         * moment the previous one reaches `idle` - so the previous flow's
         * receipt is routinely still up. Counting it would classify the next
         * flow as `imported`, and F1 would then demand an undo entry from a
         * flow that did nothing. Measured, on the seed-7 five-format run: a
         * Walls import's receipt toast outlived its flow and failed the NEXT
         * one, which had only opened and cancelled the as-new-project dialog.
         *
         * When it is already up, this flow's evidence is the receipt MODAL and
         * the undo depth, both of which are unambiguous.
         */
        const receiptToastAtStart = digest.toast.includes(RECEIPT_TOAST_KEY);
        let sawCommit = false;
        let historyTouched = false;
        let leftIdle = false;
        let steps = 0;
        let stepsOnScreen = 0;
        let previousScreen: ScreenId = digest.screen;
        let startAttempts = 0;
        const budget = maxFlowSteps();

        /** Everything both walks do after a move lands. */
        const afterMove = async (move: FuzzMove, outcome: Executed): Promise<void> => {
          const screenBefore = digest.screen;
          digest = outcome.digest;
          await stepOracles(move, digest, screenBefore, outcome.performed, outcome.handoff);
          if (move.kind === 'undo' || move.kind === 'redo' || move.kind === 'save') historyTouched = true;
          if (move.kind === 'closeProject') historyTouched = true;
          // The export modal is not an import screen, so opening it must not
          // make an idle walk look like a flow that imported nothing - see
          // NON_FLOW_SCREENS.
          if (digest.screen !== 'idle' && !NON_FLOW_SCREENS.has(digest.screen)) leftIdle = true;
          {
            // FIRST WINS, and the counts are read on the SAME step as the claim -
            // see `captureReceipt`, which owns that rule and is tested for it.
            const grown = captureReceipt(
              { claim: receipt, atReceipt, projectFileAtReceipt, historyTouchedBeforeReceipt },
              digest,
              historyTouched,
            );
            if (grown.claim !== null && receipt === null) {
              receipt = grown.claim;
              atReceipt = grown.atReceipt;
              projectFileAtReceipt = grown.projectFileAtReceipt;
              historyTouchedBeforeReceipt = grown.historyTouchedBeforeReceipt;
              guarded(() => checkReceipt(receipt as ReceiptClaim));
            }
          }
          if (digest.receipt !== null) sawReceipt = true;
          if (!receiptToastAtStart && digest.toast.includes(RECEIPT_TOAST_KEY)) sawReceipt = true;
          // `-1` is the digest's "no project open" sentinel, so a flow that
          // CREATES a project would otherwise read the new project's empty undo
          // stack (depth 0) as a commit. An import pushes an entry; creating a
          // project does not.
          if (digest.undo > Math.max(0, before.undoDepth)) sawCommit = true;
        };

        /**
         * Run one move and everything that judges it. SHARED by both walks.
         *
         * It used to be the live walk's own code, sitting after the draw, and a
         * replay `continue`d before reaching any of it - so every INLINE check
         * here (FROZEN DELETE, INCOMPLETE SHOTS, OVERLAP MODES, ORPHAN PLACEMENT,
         * IMPORT GATE, EXPORT CHORD, CLOSE) was live-only, and a recorded red from
         * one of them replayed GREEN. That is the reproduction blindness we already
         * closed for F3, one layer over: an oracle a replay cannot run is an oracle
         * record/replay/shrink cannot minimize.
         *
         * Every check in here is a function of the PRE-move screen, the move, and
         * the outcome, all three of which a replay has - `before` is the digest the
         * replay samples ahead of the step, exactly as the live walk uses the digest
         * it drew against. Nothing needed generator context in the end, including
         * the race counters: `raceMove` reads `before.live[testid]`, not the
         * generator that chose it.
         */
        const runMove = async (move: FuzzMove, before: ImportScreenDigest, record: boolean): Promise<void> => {
          // Snapshots taken BEFORE the move, for the three inline checks: each
          // is only meaningful against the state its own move acted on.
          const screenBefore = before.screen;
          // The two store values `importFlowOwnsScreen` consults, as the digest
          // that CLASSIFIED this move saw them - see the IMPORT GATE branch.
          const dialogBefore = before.dialog;
          const placementBefore = before.placement;
          const raceMove = move.kind === 'ui' && RACE_TARGETS.has(move.testid) && before.live[move.testid] === 1;
          const exportChord = move.kind === 'ui' && move.testid === 'key:Ctrl+E';
          const closeMove = move.kind === 'closeProject';
          const frozenDelete =
            move.kind === 'ui' &&
            move.testid === 'shot-editor-context-delete-rows' &&
            before.selRows.length > 0 &&
            before.selRows.every((row) => before.frozen.includes(row));
          const interjectRoute =
            move.kind === 'openFile' && move.label.startsWith('INTERJECT:') && IMPORT_GUARDED_SCREENS.has(screenBefore);
          // A wizard move the incomplete-shot guard can refuse. All three call
          // `refuseIncompleteShots()` before they do anything else.
          const wizardCommit =
            move.kind === 'ui' && INCOMPLETE_GUARDED_MOVES.has(move.testid) && screenBefore.startsWith('wiz-');
          const incompleteBefore = before.toast.includes(TOAST_INCOMPLETE);
          const overlapMode =
            move.kind === 'input' && move.mode === 'select' && move.testid === 'overlap-mode-select'
              ? move.value
              : null;
          const sectionsBefore = before.sections;
          const undoBefore = before.undo;
          const orphanMove =
            move.kind === 'ui' &&
            screenBefore === 'orphan-placement' &&
            (ORPHAN_ACCEPTS.has(move.testid) || ORPHAN_DECLINES.has(move.testid))
              ? move.testid
              : null;
          // Only paid for on an orphan move: it is an extra `get_project_state`.
          const unanchoredBefore = orphanMove !== null && before.hasProject ? await countUnanchoredComponents() : -1;

          const outcome = await execute(move, record);
          steps += 1;
          await afterMove(move, outcome);

          if (raceMove) {
            // COUNTED, never asserted - and the reason is the same one that took
            // the screen-equality check out of the IMPORT GATE branch below.
            //
            // The generator picks a confirm the DIGEST saw disabled. The bridge
            // then refuses a disabled element at dispatch with byte-identical
            // predicates, so a race move that was performed is a move whose
            // target was ENABLED when it was clicked, and clicking an enabled
            // confirm is not a defect - it is the app having finished the async
            // work the digest caught it in the middle of (`WallsImportDialog`
            // holds Import behind `!hashChecked` while a hash and a
            // duplicate-check round trip run). Asserting on it reported that
            // legal click as "a DISABLED control advanced the flow" on four
            // campaign cells; `raceContinue` never once fired for another reason.
            //
            // And the other branch cannot be asserted either. When the target IS
            // still disabled the bridge does not click it, so nothing the app did
            // can be attributed to the move: any screen change belongs to work
            // that was already in flight, which is precisely the shape that made
            // this scenario abandon screen-equality once already.
            //
            // Nor is there a third dispatch that reaches the contract. MEASURED
            // on this app under WebKitGTK, on a disabled button carrying a click
            // listener: a real WebDriver coordinate click ran the listener ZERO
            // times, and `dispatchEvent` ran it ONCE. The browser itself already
            // enforces "a disabled control must not act" and never delivers the
            // click, so the only dispatch that reaches the app at all is the one
            // that bypasses that gate - and asserting on THAT is testing the
            // harness against a button no user could press.
            if (outcome.offAtDispatch === true) {
              ctx.metric('raceRefused');
              if (digest.screen !== screenBefore) ctx.metric('raceRefusedScreenMoved');
            } else if (outcome.performed) {
              ctx.metric('raceEnabledBeforeClick');
              if (digest.screen !== screenBefore) ctx.metric('raceEnabledAdvanced');
            } else {
              // Gone from the DOM between the digest and the dispatch - the
              // dialog closed under the race. Same window, other end.
              ctx.metric(`raceVanished:${outcome.skip ?? 'unknown'}`);
            }
          }
          if (exportChord && outcome.performed) {
            ctx.metric(`exportChord:${screenBefore}`);
            // The app's own gate, in full. `useKeyboardShortcuts` reads THREE
            // pieces of store state inside the keydown handler, and Ctrl+E is
            // swallowed by any of them:
            //
            //   activeDialog        -> returns early for every non-Escape key
            //   placementModeActive -> `export` is not in PLACEMENT_MODE_ALLOWED_SHORTCUTS
            //   dataTableFullScreen -> nor in FULLSCREEN_TABLE_ALLOWED_SHORTCUTS
            //
            // Reading only the first was a hole with a live case behind it: on
            // MAP PLACEMENT the dialog is CLOSED and placement is on, which is
            // where this scenario spends its map clicks. A regression letting
            // the chord through there would have been recorded as
            // `exportChordAfterDialogClosed` - a counter - instead of failing.
            // An export modal opened behind any of the three sits over a flow
            // that still owns the state under it.
            //
            // The handler's two remaining early returns - `defaultPrevented`,
            // and focus sitting in an input or contenteditable - are deliberately
            // NOT read. Both are conditions under which the app swallows MORE
            // chords, never fewer, so neither can turn a legal export modal into
            // a failure here; and a chord they swallow opens nothing to assert
            // on. They are properties of the event and its target rather than of
            // app state, which is also why the store cannot answer for them.
            //
            // Judged at dispatch - the values that gate read - and not
            // on `screenBefore`, which is a round trip older. An import that
            // completes in that gap closes its dialog, and the chord then lands
            // at idle and is entitled to open the modal. Measured, on the corpus
            // campaign's seed-117 retry: `walls import` left the screen on
            // `walls` with the import still running, the next move was the Ctrl+E
            // interjection, and the failure blamed the dispatcher for a chord it
            // had swallowed correctly - the dialog was simply gone by then.
            //
            // A chord dispatched over the export modal ITSELF opens nothing: the
            // modal after it is the one that was up - see `exportChordVerdict`.
            const verdict = exportChordVerdict(outcome, screenBefore, digest.screen);
            if (verdict.kind === 'opened-behind-gate') {
              fail(
                `EXPORT CHORD: Ctrl+E opened the export modal behind ${verdict.gates.join(' + ')}`,
                `  screen ${screenBefore} -> ${digest.screen}; that gate was on when the keydown was ` +
                  'dispatched, so the shortcut dispatcher must swallow the chord',
              );
            } else if (verdict.kind === 'already-open') {
              ctx.metric('exportChordOverOpenModal');
            } else if (verdict.kind === 'after-dialog-closed') {
              ctx.metric('exportChordAfterDialogClosed');
            }
          }
          if (frozenDelete && outcome.performed) {
            ctx.metric('frozenDeleteRefused');
            // On the grid's DATA rows, not the rendered window (#576 HFI2-F5).
            guarded(() => checkFrozenDelete(before, digest));
          }
          // INCOMPLETE SHOTS. `handleConfirm`, `handleSkip` and
          // `handleSegmentPlace` all open with `if (refuseIncompleteShots()) return;`
          // (`DmpNewImportDialog.tsx`), which raises `dmp-wizard-incomplete` and
          // does nothing else - so a row the grid's Clear left half-filled blocks
          // the whole wizard until it is fixed.
          //
          // Armed on the TOAST rather than on a pre-move fact, because the digest
          // cannot see what `findIncompleteSections` sees; and on a toast this
          // move RAISED, because the key is deduped and one from two moves ago
          // would otherwise blame whatever came next. A confirm that was allowed
          // through never raises it, so the arming cannot fire on the happy path.
          // DIAGNOSTIC, not an assertion: a commit played while the warning was
          // ALREADY up. `showToast` dedups on the key, so a second refusal
          // re-raises nothing and `incompleteRefused` below cannot see it - which
          // is how a wizard stuck behind one cleared cell reads as a quiet walk
          // rather than as a flow that can no longer commit anything.
          if (wizardCommit && outcome.performed && incompleteBefore) {
            ctx.metric(`wizardCommitBlocked:${move.testid}`);
          }
          // A place-on-map that did NOT start a placement. Every other outcome of
          // that button is a placement (`startPlacementMode`) or a re-opened
          // wizard, so this counts the two early returns in
          // `handlePlaceOnMapClick` / `handleStartSegmentPlacement`: the
          // incomplete-shot guard, and a missing segment or anchor.
          if (
            move.kind === 'ui' &&
            move.testid === 'segment-place-on-map-button' &&
            outcome.performed &&
            !digest.placement &&
            digest.screen === screenBefore
          ) {
            // WHY it was inert, from the toasts the click left behind. The
            // placement-hint toast is the LAST statement of the success path in
            // `startSegmentPlacement`, so seeing it means the placement really
            // did start and something cancelled it before the next observation -
            // a very different fault from the two silent early returns, which
            // leave no toast at all. That distinction is what identified the
            // `showStations` cancel loop, and it is worth keeping.
            ctx.metric(
              digest.toast.includes('dmp-manual-placement-hint')
                ? 'placeOnMapInert:startedThenCancelled'
                : digest.toast.includes(TOAST_INCOMPLETE)
                  ? 'placeOnMapInert:incompleteShots'
                  : 'placeOnMapInert:silentEarlyReturn',
            );
          }
          if (wizardCommit && outcome.performed && !incompleteBefore && digest.toast.includes(TOAST_INCOMPLETE)) {
            ctx.metric('incompleteRefused');
            if (sectionsBefore >= 0 && digest.sections !== sectionsBefore) {
              fail(
                'INCOMPLETE SHOTS: a refused wizard commit imported sections anyway',
                `  after: ${move.label}\n  sections ${sectionsBefore} -> ${digest.sections}`,
              );
            }
            if (undoBefore >= 0 && digest.undo > undoBefore) {
              fail(
                'INCOMPLETE SHOTS: a refused wizard commit pushed an undo entry',
                `  after: ${move.label}\n  undo depth ${undoBefore} -> ${digest.undo}`,
              );
            }
            // The guard returns before any transition, so the wizard is still the
            // thing on screen - and a refusal that closed it would leave the user
            // with a warning about a section they can no longer reach.
            if (!digest.screen.startsWith('wiz-')) {
              fail(
                'INCOMPLETE SHOTS: the refusal closed the wizard',
                `  after: ${move.label}\n  screen ${screenBefore} -> ${digest.screen}`,
              );
            }
          }
          // ORPHAN PLACEMENT. The dialog offers to connect a section that was
          // left floating - no zero datum, merged to nothing - to something the
          // import just landed. Both of its answers are accountable.
          if (orphanMove !== null && outcome.performed) {
            ctx.metric(`orphanChoice:${orphanMove}`);
            const unanchoredAfter = digest.hasProject ? await countUnanchoredComponents() : -1;
            // Neither answer creates or destroys a section. Accepting MERGES two
            // that already exist; declining moves through the queue.
            if (sectionsBefore >= 0 && digest.sections !== sectionsBefore) {
              fail(
                `ORPHAN PLACEMENT: "${orphanMove}" changed the section count`,
                `  after: ${move.label}\n  sections ${sectionsBefore} -> ${digest.sections}; ` +
                  'the dialog connects sections that already exist, it does not import any',
              );
            }
            if (ORPHAN_DECLINES.has(orphanMove)) {
              // Pure navigation. Skip advances the queue and Skip All closes it;
              // neither may touch the project, or a user who declined every
              // suggestion would still have been charged for one.
              if (undoBefore >= 0 && digest.undo !== undoBefore) {
                fail(
                  `ORPHAN PLACEMENT: declining with "${orphanMove}" moved the undo stack`,
                  `  after: ${move.label}\n  undo depth ${undoBefore} -> ${digest.undo}`,
                );
              }
              if (unanchoredBefore >= 0 && unanchoredAfter >= 0 && unanchoredAfter !== unanchoredBefore) {
                fail(
                  `ORPHAN PLACEMENT: declining with "${orphanMove}" changed the survey's shape`,
                  `  after: ${move.label}\n  unanchored components ${unanchoredBefore} -> ${unanchoredAfter}`,
                );
              }
            } else {
              // Accepting is a merge, so it may only ever JOIN components - the
              // count can fall (the orphan found a home) or hold (this queue
              // entry was refused), never rise. A rise would mean the connection
              // broke something apart, which no merge can do.
              if (unanchoredBefore >= 0 && unanchoredAfter > unanchoredBefore) {
                fail(
                  `ORPHAN PLACEMENT: accepting with "${orphanMove}" left MORE unanchored components`,
                  `  after: ${move.label}\n  unanchored components ${unanchoredBefore} -> ${unanchoredAfter}; ` +
                    'a merge can only join components, never split them',
                );
              }
              if (unanchoredBefore >= 0 && unanchoredAfter >= 0 && unanchoredAfter < unanchoredBefore) {
                // It landed, so it is a project mutation and must be undoable -
                // the same rule F1 holds every import to.
                ctx.metric('orphanAccepted');
                if (undoBefore >= 0 && digest.undo <= undoBefore) {
                  fail(
                    'ORPHAN PLACEMENT: an accepted connection pushed no undo entry',
                    `  after: ${move.label}\n  unanchored ${unanchoredBefore} -> ${unanchoredAfter} ` +
                      `but undo depth ${undoBefore} -> ${digest.undo}`,
                  );
                }
              }
            }
          }
          if (overlapMode !== null) {
            // The doubly-anchored guard disables `interpolate` and `import_data`
            // when the target has positional constraints on BOTH sides of the
            // overlap (`isTargetDoublyAnchored`), and the bridge will not click a
            // disabled option - so a refusal here is the guard being reached, and
            // worth counting rather than asserting.
            if (!outcome.performed && outcome.skip === 'option-absent') {
              // A replayed move's value is a trace's text: only the picker's own
              // option values name a metric (#576 W51).
              ctx.metric(`overlapModeRefused:${OVERLAP_MODES.includes(overlapMode) ? overlapMode : 'other'}`);
              // But the guard must never disable ALL of them. `existing_data` and
              // `keep_both` are unconditional in the source, and a build where
              // one of them stopped being offered would leave a user with an
              // overlap they cannot choose a treatment for.
              if (OVERLAP_MODES_ALWAYS_LEGAL.has(overlapMode) && digest.live['overlap-mode-select'] === 2) {
                fail(
                  'OVERLAP MODES: an unconditional overlap treatment was not offered',
                  `  after: ${move.label}\n  the picker is actionable but refused "${overlapMode}", ` +
                    'which no guard in the wizard disables',
                );
              }
            }
            // The picker WRITES THE GLOBAL CONFIG (`handleOverlapModeChange` ->
            // `updateConfig`) in the middle of an import. That is a preference,
            // not a project mutation, so it must not reach the undo stack - if it
            // ever did, every undo count F1 makes about the flow around it would
            // be off by the number of times the user changed their mind.
            if (outcome.performed && undoBefore >= 0 && digest.undo !== undoBefore) {
              fail(
                'OVERLAP MODES: changing the treatment moved the undo stack',
                `  after: ${move.label}\n  undo depth ${undoBefore} -> ${digest.undo}; ` +
                  'the overlap mode is config, not project state',
              );
            }
          }
          if (closeMove && outcome.performed) {
            ctx.metric('closeProjectInterjections');
            // Only ever played at `idle` (see `buildAuxInterjection`), so this
            // is E1's contract applied mid-run: closing must leave no project,
            // no dialog and no placement behind it.
            const residue = [
              digest.hasProject ? 'a project is still open' : null,
              digest.dialog !== null ? `activeDialog=${JSON.stringify(digest.dialog)}` : null,
              digest.placement ? 'placementMode is still set' : null,
            ].filter((line): line is string => line !== null);
            if (residue.length > 0) {
              fail('CLOSE: closing the project at idle left state behind', residue.map((line) => `  ${line}`).join('\n'));
            }
          }
          if (interjectRoute && outcome.performed) {
            // Was the screen still the one the move was CLASSIFIED on when the
            // router actually looked?
            //
            // `importFlowOwnsScreen` reads `activeDialog` and `placementMode`
            // inside `routeOpenedFile`; `IMPORT_GUARDED_SCREENS` maps the same
            // two onto this scenario's screen table, but off a digest taken a
            // round trip earlier. An import that completes in that gap closes
            // its dialog, and the router is then right to accept the file.
            // Measured, on the corpus campaign's seed-117 rerun: a `walls
            // import` raced against its own async enable left the digest on
            // `walls`, the import finished, and the route that followed was
            // reported as a broken guard for landing on `as-new-project`.
            //
            // Comparing the two readings costs nothing and needs no second copy
            // of the app's dialog set: same `activeDialog`, same placement, and
            // the classification the digest made still holds.
            const sameScreenAtDispatch =
              outcome.dialogAtDispatch === dialogBefore && outcome.placementAtDispatch === placementBefore;
            ctx.metric(sameScreenAtDispatch ? 'interjectRouteGuarded' : 'importGateSkipped:screenMovedBeforeDispatch');
            // The REFUSAL TOAST, not the screen, is the guard's observable
            // effect - and the screen is not a sound test of it. Measured, on
            // the seed-7 five-format run: `create-and-continue-button` starts
            // an async chain (create the project, then open the format's own
            // dialog) that the settle loop cannot wait for, because
            // `activeDialog` does not change until it lands. So the screen can
            // move from `new-project` to `tml-config` three moves later, for a
            // reason that has nothing to do with the routed file, and a
            // screen-equality check reports the guard as broken when it worked.
            // Read when the route RETURNED (#576 HFI2-F3): the settle behind
            // `digest` can outlast the toast while a wizard calculation runs.
            if (
              sameScreenAtDispatch &&
              !importGateRefusalSeen(outcome.toastsRaisedAtDispatch, digest.toast, IMPORT_GATE_TOAST_KEY)
            ) {
              fail(
                'IMPORT GATE: a second file was routed into a screen an import already owned',
                `  screen ${screenBefore} -> ${digest.screen}; routeOpenedFile must refuse and SAY so (#411)\n` +
                  `  activeDialog at dispatch=${JSON.stringify(outcome.dialogAtDispatch)} ` +
                  `placement=${outcome.placementAtDispatch}\n` +
                  `  toasts raised by the route: ${JSON.stringify(outcome.toastsRaisedAtDispatch)}; ` +
                  `after settle: ${JSON.stringify(digest.toast)}`,
              );
            }
          }
        };

        for (;;) {
          if (replaySteps) {
            const source = flowSteps ?? replaySteps;
            if (replayAt >= source.length) break;
            const step = source[replayAt] as TraceStep;
            replayAt += 1;
            if (step.kind === 'genFile') {
              materialize(step.spec as GeneratedFileSpec, step.rel);
              ctx.metric('replay:genFile');
              continue;
            }
            if (step.kind === 'corpusFile') {
              // The replayer's OWN corpus, or none. A trace carries a reference
              // and no content, so resolving one means scanning the corpus THIS
              // machine configured; a machine without that file replays
              // everything else and the move that would have opened it is
              // skipped and counted. That is a narrower replay, not a failed one.
              //
              // THE HASH IS PREFERRED AND THERE IS NO FALLBACK TO THE ID. Both
              // resolve the same file when the replayer has it, and when the
              // hash misses, the id is precisely the thing that might match a
              // DIFFERENT file - 20 bits over a few thousand entries - which
              // would materialize somebody else's survey and open it under the
              // recorded file's name. A miss is a skip; a wrong file is a lie.
              const lookup = corpusLookup(step.corpusHash ?? step.corpusId);
              if (lookup.entry === null) {
                // `ambiguous` is a v3-shaped step (id only) landing on a corpus
                // whose crc20 collides. Counted apart from a plain absence
                // because it says something about the corpus, not the trace.
                ctx.metric(lookup.outcome === 'ambiguous' ? 'corpus:ambiguousId' : 'corpus:absent');
                continue;
              }
              const destination = resolveRel(step.rel);
              const copied = materializeCorpusEntry(lookup.entry, destination);
              ctx.metric(
                copied.skipped ? `corpus:notMaterialized:${copied.reason ?? 'unknown'}` : 'replay:corpusFile',
              );
              // A REPLAY needs the redactions as much as a recording does - the
              // app echoes the basenames it opened, and a replayed failure is
              // exactly the message somebody attaches to an issue.
              if (!copied.skipped) {
                registerCorpusRedactions(lookup.entry.id, destination, copied.names);
                corpusEntries.bind(step.rel, copied.path);
              }
              continue;
            }
            // Counted as the live walk counts a drawn one - see `FileSeq`.
            const move = moveFromTraceStep(step, files, SCENARIO);
            if (move === null) {
              ctx.metric(`replay:unsupported:${step.kind}`);
              continue;
            }
            // OBSERVE BEFORE ACTING, on a replay only.
            //
            // The walk otherwise sees the screen only AFTER each move, so a
            // dialog that opens in the async tail of move N and is dismissed by
            // move N+1 is never sampled. Measured on a recorded F3 red: the
            // recording's steps 29-30 are `zero-datum-config-confirm` then
            // `import-receipt-continue`, and on replay the post-confirm digest
            // read `wiz-done` (the modal had not opened yet) while the next step
            // dispatched successfully against `import-receipt-continue` and left
            // `idle`. The receipt existed only between the two moves. F3 reads
            // its claim off that modal, so it had nothing to compare and a
            // recorded F3 failure replayed green.
            //
            // A live walk does not need this - it keeps drawing moves and meets
            // the dialog on a later step - and must not have it: the race
            // generators depend on dispatching against the screen the digest
            // already classified.
            const beforeStep = await observeScreen(ctx, false);
            {
              const seen = captureReceipt(
                { claim: receipt, atReceipt, projectFileAtReceipt, historyTouchedBeforeReceipt },
                beforeStep,
                historyTouched,
              );
              if (seen.claim !== null && receipt === null) {
                receipt = seen.claim;
                atReceipt = seen.atReceipt;
                projectFileAtReceipt = seen.projectFileAtReceipt;
                historyTouchedBeforeReceipt = seen.historyTouchedBeforeReceipt;
                ctx.metric('replay:receiptSeenBetweenSteps');
                guarded(() => checkReceipt(receipt as ReceiptClaim));
              }
              if (beforeStep.receipt !== null) sawReceipt = true;
            }
            // The SAME judgement the live walk runs - see `runMove`. `beforeStep`
            // is this walk's pre-move digest, which is what every inline check is
            // written against.
            await runMove(move, beforeStep, false);
            continue;
          }

          if (steps >= budget) {
            ctx.metric('flowBudgetExhausted');
            break;
          }
          if (digest.screen === 'idle') {
            if (leftIdle) break;
            if (startAttempts >= START_ATTEMPTS) {
              ctx.metric('flowNeverStarted');
              break;
            }
            startAttempts += 1;
          }

          stepsOnScreen = digest.screen === previousScreen ? stepsOnScreen + 1 : 0;
          previousScreen = digest.screen;
          const mode = digest.screen === 'idle' ? 'draw' : progressMode(stepsOnScreen);
          if (mode === 'escape') {
            digest = await escapeFrom(digest);
            stepsOnScreen = 0;
            previousScreen = digest.screen;
            break;
          }

          const drawn = drawMove(digest, mode);
          const move = drawn === null ? null : enforceSavePath(drawn, digest);
          if (move === null) {
            // Nothing to draw here. At idle there is nothing to leave either, so
            // the flow is simply not startable this time round.
            if (digest.screen === 'idle') {
              ctx.metric('idleHadNoMove');
              break;
            }
            digest = await escapeFrom(digest);
            break;
          }

          await runMove(move, digest, true);
        }

        if (!leftIdle) {
          ctx.metric('flowSkipped');
          continue;
        }

        // The flow is over. Make sure it really is before measuring it - both
        // that nothing is still on screen, and that `idle` is rest rather than
        // a gap in an async completion.
        digest = await settleToRest(digest);
        if (!isEscaped(digest.screen, digest.placement)) {
          digest = await escapeFrom(digest);
          digest = await settleToRest(digest);
        }
        const after = await boundsNow(digest);
        ctx.metric('flows');

        const what = `flow ${flow + 1} of episode ${episode + 1}`;
        const shape: FlowShape = sawCommit || sawReceipt ? 'imported' : 'cancelled';
        ctx.metric(`flowShape:${shape}`);
        // The ORPHAN precondition, measured rather than assumed.
        //
        // `Project::suggest_orphan_placements` offers a placement only for a
        // merge-connected component that has NO zero datum and NO newly-imported
        // member - so unless a flow LEAVES one behind, the next flow's import
        // cannot raise the dialog whatever its geometry does. Counting it here
        // separates "the orphan surface is unreachable" from "the geometry never
        // matched", which are different bugs with different fixes and read
        // identically from `screen:orphan-placement` alone.
        if (digest.hasProject) {
          const unanchored = await countUnanchoredComponents();
          // -1 no project state, -2 the datum field name stopped resolving. Both
          // are reported rather than folded into a bucket, because a probe that
          // silently reads every component as unanchored is worse than none.
          ctx.metric(
            unanchored < 0
              ? `unanchoredComponents:probeFailed${unanchored}`
              : `unanchoredComponents:${unanchored === 0 ? 'none' : unanchored === 1 ? 'one' : 'many'}`,
          );
        }
        guarded(() => checkResidue(digest, what));
        guarded(() => checkTempFiles(before, after, what));
        // Counted, never asserted: `checkResidue` documents why this scenario
        // cannot tell a completion from an abandonment, and #477 - the real
        // completion-path defect - was established by reading the code.
        if (digest.ctx !== null) ctx.metric(`residualImportContext:${digest.ctx}`);
        if (sawReceipt) ctx.metric('flowPresentedReceipt');
        // F3 measures the receipt against the delta the import made, so it needs
        // both measurements to describe the same project. A flow that started
        // with a project and ended with a DIFFERENT one - `import-as-new-project`
        // confirms by closing the current project and re-entering the router -
        // has bounds taken from two unrelated projects, and the difference
        // between them is not an accounting error, it is a subtraction with no
        // meaning. A flow that CREATED the project it ended with is still
        // measured: `before` is then simply empty.
        //
        // The measured end is `atReceipt`, not `after` - see `checkAccounting`.
        if (receipt && atReceipt) {
          if (replaySteps && !replayHasBoundaries) {
            // A LEGACY trace cannot say where a flow began. It replays as one
            // pass over the whole step list, so a recording that imported three
            // times replays as one flow and F3 compares the FIRST receipt's
            // claim against the delta of all three. Measured on such a trace:
            // "1 section - 6 shots" against a project holding 5 sections and 34
            // shots. Same objection as the two skips below - a subtraction
            // between things that are not each other's pair.
            //
            // NARROW, and deliberately so. Skipping every replay would mean a
            // trace whose recorded failure WAS an F3 red replays green, and
            // record/replay/shrink could never reproduce or minimize the very
            // failure class the campaign's seed-8 red belonged to. A trace with
            // `flow` markers has real boundaries, so F3 runs on it exactly as it
            // ran on the recording.
            ctx.metric('f3Skipped:replayNoBoundaries');
            // Asked AT THE RECEIPT, like everything else F3 measures. Comparing
            // `after.projectFile` asked it at the end of the flow, where a
            // SECOND file swapping projects suppressed an oracle whose claim and
            // counts were both fixed, on the original project, moves earlier -
            // the same mistake `atReceipt` exists to avoid, one field over.
          } else if (before.projectFile !== null && before.projectFile !== projectFileAtReceipt) {
            ctx.metric('f3Skipped:projectSwapped');
          } else if (historyTouchedBeforeReceipt) {
            ctx.metric('f3Skipped:historyTouched');
          } else {
            guarded(() => checkAccounting(receipt as ReceiptClaim, before, atReceipt as CountedProject));
          }
        }

        // F1 needs a flow whose only history effect was the import itself, and
        // whose project did not change identity underneath it. An undo/redo/save
        // interjection is a deliberate part of the walk and a real thing to do;
        // a flow that created or swapped the project (importContext
        // 'new-project', or `import-as-new-project`) writes an initial `.stalx`
        // with `save_project`'s healing mutations and no undo entry. Both make
        // "undo exactly what this flow pushed" meaningless, so those flows are
        // COUNTED rather than measured - never tolerated inside the oracle.
        if (historyTouched) {
          ctx.metric('f1Skipped:historyTouched');
        } else if (before.projectFile !== after.projectFile) {
          ctx.metric('f1Skipped:projectSwapped');
        } else if (before.undoDepth < 0 || after.undoDepth < 0) {
          ctx.metric('f1Skipped:noProject');
        } else {
          const pushed = guarded(() => checkPushed(shape, before, after, what));
          ctx.metric(`f1Pushed:${shape}:${pushed}`);
          if (pushed > 0) {
            for (let i = 0; i < pushed; i += 1) await ctx.callAction('project', 'undo', []);
            const unwound = await canonicalNow(true);
            guarded(() => checkRoundtripState('pre-flow', before.snapshot, unwound, `undoing ${what}`));
            for (let i = 0; i < pushed; i += 1) await ctx.callAction('project', 'redo', []);
            const rewound = await canonicalNow(true);
            guarded(() => checkRoundtripState('post-flow', after.snapshot, rewound, `redoing ${what}`));
            ctx.metric('f1Roundtrips');
          }
        }

        digest = await observeScreen(ctx, false);
        lastUndoDepth = digest.undo;
      }

      // ---- E1 teardown ---------------------------------------------------
      await ctx.callAction('project', 'closeProject', []).catch(() => undefined);
      const closed = await observeScreen(ctx, false);
      const residue: string[] = [];
      if (closed.hasProject) residue.push('a project is still open');
      if (closed.dialog !== null) residue.push(`activeDialog=${JSON.stringify(closed.dialog)}`);
      if (closed.placement) residue.push('placementMode is still set');
      // NOT importContext. `closeProject` does not touch it and is not meant
      // to: `routeOpenedFile` resets it at the entry of every import flow, and
      // `checkResidue` cites the two source comments that make that the
      // contract. Counted above per flow, asserted only for a completed import.
      if (residue.length > 0) {
        fail(`E1 TEARDOWN: episode ${episode + 1} did not close cleanly`, residue.map((line) => `  ${line}`).join('\n'));
      }
      for (const dir of [filesDir, projectsDir]) fs.rmSync(dir, { recursive: true, force: true });
      for (const dir of [filesDir, projectsDir]) {
        if (fs.existsSync(dir)) fail(`E1 TEARDOWN: ${dir} survived its own deletion`);
      }

      const domAfter = await ctx.eval<number>('return document.querySelectorAll("*").length;');
      ctx.metric('domNodeDelta', domAfter - domBefore);
      ctx.setMetric('domNodes', domAfter);
      ctx.metric('episodes');
    }
    };

    try {
      await runEpisodes();
    } catch (err) {
      if (typeof err === 'object' && err !== null && raisedByFail.has(err)) throw err;
      // Died OUTSIDE the oracles - a WebDriver call that outran its budget, or
      // a harness error. Same treatment as any failure: the move that had not
      // returned goes into the trace and the history, and `fail` prints the
      // reproduce line and writes the trace (#576 HFI-9).
      const report = describeRunError(err, moveLog.inFlight());
      if (inFlightStep !== null) recorder.record(inFlightStep);
      pageStalled = report.stall;
      fail(report.headline, report.detail);
    } finally {
      await forceClean();
    }

    // SUGGESTER DRIFT. `suggestiveDmpSpec` is calibrated against a THREE-WAY
    // mirror of the Rust suggester's cutoffs - the spec generator, the offline
    // test's copy of the per-shot rule, and the product. The offline test pins
    // the first two to each other and cannot see the third, so the day the
    // product retunes `PlacementSuggestionConfig` the tests stay green while the
    // pool silently comes back empty and every suggestion-shaped counter goes to
    // zero. This is the only place that notices.
    //
    // WARNED, not failed, on the same argument `corpusDirsTooDeep` makes: a
    // suggestion needs sections to score against, and a run whose flows all
    // cancelled early legitimately never opens the wizard. Failing here would
    // red a cell for the walk being unlucky. The metric is what a summary
    // carries, and the log line is what a human reads.
    const suggestiveDrawn = ctx.metrics['file:shape:suggestive'] ?? 0;
    const suggestSamples = ctx.metrics['screen:wiz-suggest'] ?? 0;
    if (suggestiveDrawn > 0 && suggestSamples === 0) {
      ctx.setMetric('suggesterDrift', suggestiveDrawn);
      ctx.log(
        `SUGGESTER DRIFT: ${suggestiveDrawn} suggestion-bearing .dmp file(s) were imported and the wizard ` +
          'never once reached `wiz-suggest`. Either every flow was abandoned before the wizard opened, or ' +
          "`suggestiveDmpSpec`'s calibration no longer clears the product's cutoffs - re-check it against " +
          '`PlacementSuggestionConfig` and the mirror in `lib/surveyFileSpecs.test.ts`.',
      );
    }

    if (traceOut) {
      const written = dumpTrace(null);
      if (written) ctx.log(`trace written to ${written}`);
    }
  },
};

export default importDialogFuzz;
