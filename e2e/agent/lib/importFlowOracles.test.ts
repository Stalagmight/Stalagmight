/**
 * The pure half of `import-dialog-fuzz`'s FLOW oracles.
 *
 * Every F-check is a decision over two sets of numbers, and the scenario that
 * feeds them needs a live app, a corpus and twenty minutes - so the branch
 * structure is pinned here instead: what each oracle fails on, and what it
 * deliberately lets past.
 *
 * The case worth naming is F3's. It compares the receipt against the project AS
 * IT WAS WHEN THE RECEIPT WAS MADE, not at the end of the flow, because the walk
 * keeps moving after a receipt appears - an `undo` interjection lands, a second
 * file is imported - and an accounting oracle measured across those is a test of
 * the walk rather than of the receipt. Measured, on the campaign's seed-8 cell:
 * a Walls import whose receipt correctly said "1 section - 7 shots" was reported
 * as a 0/0 delta because an undo interjection two moves later had taken it back.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { describe, expect, it } from 'vitest';

import type { ImportScreenDigest } from './importScreenDigest';
import {
  captureReceipt,
  emptyReceiptCapture,
  ImportOracleError,
  MAX_VISIBLE_WARNINGS,
  checkAccounting,
  checkGateRefusalToast,
  checkIpcContract,
  checkPushed,
  checkReceipt,
  checkResidue,
  checkRoundtripState,
  checkTempFiles,
  exportChordVerdict,
  nextErrorGrace,
  s4WorkStarted,
  ipcArgRejections,
  leftoverTempNames,
  parseReceiptSummary,
  receiptClaimFrom,
  routeHandoff,
  type FlowBounds,
  type ReceiptClaim,
} from './importFlowOracles';

const bounds = (over: Partial<FlowBounds> = {}): FlowBounds => ({
  snapshot: null,
  undoDepth: 0,
  sections: 0,
  shots: 0,
  projectDir: null,
  leftovers: [],
  ...over,
});

const claim = (over: Partial<ReceiptClaim> = {}): ReceiptClaim => ({
  summary: '1 section - 7 shots',
  sections: 1,
  shots: 7,
  shown: 0,
  more: 0,
  hasDetails: false,
  ...over,
});

const digest = (over: Partial<ImportScreenDigest> = {}): ImportScreenDigest => ({
  screen: 'idle',
  dialog: null,
  placement: false,
  parsing: false,
  ctx: null,
  phase: null,
  hasProject: true,
  projectFile: '/tmp/project.stalx',
  importInProgress: false,
  live: {},
  opts: {},
  rows: 0,
  dataRows: 0,
  frozen: [],
  activeRow: null,
  selRows: [],
  toast: [],
  err: null,
  boundary: false,
  inv: [],
  undo: 0,
  sections: 0,
  shots: 0,
  receipt: null,
  dirty: false,
  savePath: null,
  why: '',
  ...over,
});

/** Every oracle reports through one error type, so `guarded` can catch them all. */
const throwsOracleError = (what: () => unknown, matching: RegExp): void => {
  expect(what).toThrow(ImportOracleError);
  expect(what).toThrow(matching);
};

// ---------------------------------------------------------------------------
// The receipt's own text
// ---------------------------------------------------------------------------

describe('parseReceiptSummary', () => {
  it('reads the counts positionally, not by label', () => {
    expect(parseReceiptSummary('1 section - 7 shots')).toEqual({ sections: 1, shots: 7 });
    expect(parseReceiptSummary('2 sections - 12 shots - 3 loops')).toEqual({ sections: 2, shots: 12 });
  });

  it('survives a grouped thousands separator', () => {
    expect(parseReceiptSummary('1 section - 1,234 shots')).toEqual({ sections: 1, shots: 1234 });
    expect(parseReceiptSummary("3 sections - 12'000 shots")).toEqual({ sections: 3, shots: 12000 });
  });

  it('returns nulls rather than a guess when the text is not that shape', () => {
    expect(parseReceiptSummary('imported')).toEqual({ sections: null, shots: null });
    expect(parseReceiptSummary('1 section')).toEqual({ sections: 1, shots: null });
  });
});

describe('receiptClaimFrom', () => {
  it('is null with no receipt on screen', () => {
    expect(receiptClaimFrom(digest())).toBeNull();
  });

  it('carries the rendering counts alongside the parsed summary', () => {
    const got = receiptClaimFrom(
      digest({
        receipt: { summary: '2 sections - 9 shots', shown: 3, more: 4, hasDetails: true, unitConversion: false },
      }),
    );
    expect(got).toEqual({
      summary: '2 sections - 9 shots',
      sections: 2,
      shots: 9,
      shown: 3,
      more: 4,
      hasDetails: true,
    });
  });
});

// ---------------------------------------------------------------------------
// F4 receipt rendering
// ---------------------------------------------------------------------------

describe('checkReceipt (F4)', () => {
  it('passes a full visible list with a toggle over the rest', () => {
    expect(() => checkReceipt(claim({ shown: MAX_VISIBLE_WARNINGS, more: 2, hasDetails: true }))).not.toThrow();
    expect(() => checkReceipt(claim({ shown: 1, more: 0, hasDetails: false }))).not.toThrow();
  });

  it('fails an over-full visible list', () => {
    throwsOracleError(() => checkReceipt(claim({ shown: MAX_VISIBLE_WARNINGS + 1 })), /cap is 3/);
  });

  it('fails hidden warnings with no toggle to reveal them', () => {
    throwsOracleError(() => checkReceipt(claim({ shown: 3, more: 1, hasDetails: false })), /no details toggle/);
  });

  it('fails a toggle offered over a list that is not even full', () => {
    throwsOracleError(() => checkReceipt(claim({ shown: 1, more: 0, hasDetails: true })), /not even full/);
  });
});

// ---------------------------------------------------------------------------
// F3 accounting
// ---------------------------------------------------------------------------

describe('checkAccounting (F3)', () => {
  it('passes when the delta is what the receipt claimed', () => {
    expect(() => checkAccounting(claim(), bounds({ sections: 2, shots: 10 }), { sections: 3, shots: 17 })).not.toThrow();
  });

  it('fails when it is not, and says both numbers', () => {
    throwsOracleError(
      () => checkAccounting(claim(), bounds({ sections: 2, shots: 10 }), { sections: 3, shots: 12 }),
      /receipt:\s+1 section\(s\), 7 shot\(s\)[\s\S]*project:\s+1 section\(s\), 2 shot\(s\)/,
    );
  });

  it('measures AT THE RECEIPT, so a later undo cannot make the receipt wrong', () => {
    // The seed-8 shape: the import landed 1/7 and the receipt said so; an
    // `undo` interjection two moves later took it back. The end of the flow
    // reads 0/0, and F3 must not care - that is the walk's doing, not the
    // receipt's. The caller passes the counts read from the receipt's own
    // digest, which is what makes this true.
    const atReceipt = { sections: 1, shots: 7 };
    expect(() => checkAccounting(claim(), bounds({ sections: 0, shots: 0 }), atReceipt)).not.toThrow();
  });

  it('still fails a receipt that disagreed with the project the moment it was made', () => {
    throwsOracleError(
      () => checkAccounting(claim(), bounds({ sections: 0, shots: 0 }), { sections: 0, shots: 0 }),
      /F3 ACCOUNTING/,
    );
  });

  it('checks nothing when the summary did not parse', () => {
    expect(() => checkAccounting(claim({ sections: null }), bounds(), { sections: 9, shots: 9 })).not.toThrow();
    expect(() => checkAccounting(claim({ shots: null }), bounds(), { sections: 9, shots: 9 })).not.toThrow();
  });

  it('treats the no-project sentinel as no measurement, and as zero before', () => {
    // -1 is "no project open", not a count. Measured with one, there is nothing
    // to compare; measured BEFORE, the flow created the project and it started
    // empty - subtracting the sentinel would make every count one too many.
    expect(() => checkAccounting(claim(), bounds(), { sections: -1, shots: -1 })).not.toThrow();
    expect(() => checkAccounting(claim(), bounds({ sections: -1, shots: -1 }), { sections: 1, shots: 7 })).not.toThrow();
  });
});

// ---------------------------------------------------------------------------
// F2 residue
// ---------------------------------------------------------------------------

describe('checkResidue (F2)', () => {
  it('passes a screen with nothing of the flow left on it', () => {
    expect(() => checkResidue(digest(), 'flow 1')).not.toThrow();
  });

  it('reports every kind of leftover it names', () => {
    throwsOracleError(() => checkResidue(digest({ dialog: 'walls-import' }), 'flow 1'), /activeDialog/);
    throwsOracleError(() => checkResidue(digest({ placement: true }), 'flow 1'), /placementMode/);
    throwsOracleError(() => checkResidue(digest({ importInProgress: true }), 'flow 1'), /isImportInProgress/);
    throwsOracleError(() => checkResidue(digest({ phase: 'suggesting' }), 'flow 1'), /dmpImportWizardPhase/);
  });

  it('does NOT report a retained importContext', () => {
    // Counted by the scenario, never asserted: the oracle's own header cites the
    // two source comments that make retention the contract on the cancel path.
    expect(() => checkResidue(digest({ ctx: 'new-project' }), 'flow 1')).not.toThrow();
  });
});

// ---------------------------------------------------------------------------
// F1 undo roundtrip
// ---------------------------------------------------------------------------

describe('checkPushed (F1)', () => {
  it('returns what the flow pushed', () => {
    expect(checkPushed('imported', bounds({ undoDepth: 2 }), bounds({ undoDepth: 5 }), 'flow 1')).toBe(3);
    expect(checkPushed('cancelled', bounds({ undoDepth: 2 }), bounds({ undoDepth: 2 }), 'flow 1')).toBe(0);
  });

  it('fails an import that landed with no undo entry behind it', () => {
    throwsOracleError(
      () => checkPushed('imported', bounds({ undoDepth: 4 }), bounds({ undoDepth: 4, sections: 1 }), 'flow 1'),
      /imported sections but pushed no undo entry/,
    );
  });

  it('fails a stack that SHRANK', () => {
    throwsOracleError(
      () => checkPushed('imported', bounds({ undoDepth: 4 }), bounds({ undoDepth: 1 }), 'flow 1'),
      /SHRANK the undo stack by 3/,
    );
  });

  it('fails a cancel that pushed, and a cancel that mutated silently', () => {
    throwsOracleError(
      () => checkPushed('cancelled', bounds({ undoDepth: 0 }), bounds({ undoDepth: 1 }), 'flow 1'),
      /cancelled but pushed 1 undo entr/,
    );
    throwsOracleError(
      () =>
        checkPushed(
          'cancelled',
          bounds({ snapshot: '{"a":1}' }),
          bounds({ snapshot: '{"a":2}' }),
          'flow 1',
        ),
      /cancelled but changed the project/,
    );
  });
});

describe('checkRoundtripState (F1)', () => {
  it('passes identical states, and any comparison with nothing to compare', () => {
    expect(() => checkRoundtripState('pre-flow', '{"a":1}', '{"a":1}', 'flow 1')).not.toThrow();
    expect(() => checkRoundtripState('pre-flow', null, '{"a":1}', 'flow 1')).not.toThrow();
    expect(() => checkRoundtripState('pre-flow', '{"a":1}', null, 'flow 1')).not.toThrow();
  });

  it('fails a walk that did not land back where it started', () => {
    throwsOracleError(
      () => checkRoundtripState('pre-flow', '{"a":1}', '{"a":2}', 'undoing flow 1'),
      /did not restore the pre-flow state/,
    );
  });
});

// ---------------------------------------------------------------------------
// F5 temp files, and S6
// ---------------------------------------------------------------------------

describe('checkTempFiles (F5)', () => {
  it('reports only files the flow itself left', () => {
    const before = bounds({ projectDir: '/p', leftovers: ['old.stalx.tmp'] });
    expect(() => checkTempFiles(before, { ...before }, 'flow 1')).not.toThrow();
    throwsOracleError(
      () => checkTempFiles(before, bounds({ projectDir: '/p', leftovers: ['old.stalx.tmp', 'new.stalx.tmp'] }), 'flow 1'),
      /left 1 staging file\(s\)[\s\S]*new\.stalx\.tmp/,
    );
  });

  it('checks nothing when the flow ended with no project on disk', () => {
    expect(() => checkTempFiles(bounds(), bounds({ leftovers: ['x.stalx.tmp'] }), 'flow 1')).not.toThrow();
  });
});

describe('leftoverTempNames', () => {
  it('lists the staging names beside a project, and nothing else', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'import-flow-oracles-'));
    try {
      fs.writeFileSync(path.join(dir, 'p.stalx'), '');
      fs.writeFileSync(path.join(dir, 'p.stalx.tmp'), '');
      fs.writeFileSync(path.join(dir, 'notes.txt'), '');
      const got = leftoverTempNames(path.join(dir, 'p.stalx'));
      expect(got.dir).toBe(dir);
      expect(got.names).toEqual(['p.stalx.tmp']);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  it('is empty for no project, and for a directory that is not there', () => {
    expect(leftoverTempNames(null)).toEqual({ dir: null, names: [] });
    expect(leftoverTempNames('/nope/does/not/exist/p.stalx').names).toEqual([]);
  });
});

describe('checkIpcContract (S6)', () => {
  const REJECTION =
    'ERROR invalid args `payload` for command `import_sections`: invalid type: null, expected a sequence';

  it('finds an argument-deserialization rejection in the log ring', () => {
    expect(ipcArgRejections(['INFO opened project', REJECTION])).toEqual([REJECTION]);
    throwsOracleError(() => checkIpcContract([REJECTION], 'walls import'), /S6 IPC CONTRACT/);
  });

  it('ignores a command that RAN and reported a problem properly', () => {
    const reported = ['WARN import_sections returned success=false: no valid sections found'];
    expect(ipcArgRejections(reported)).toEqual([]);
    expect(() => checkIpcContract(reported, 'walls import')).not.toThrow();
  });
});

describe('captureReceipt (F3 capture ordering)', () => {
  const digestWith = (
    summary: string | null,
    sections: number,
    shots: number,
    projectFile: string | null = '/tmp/a.stalx',
  ): ImportScreenDigest =>
    ({
      receipt: summary === null ? null : { summary, shown: 0, more: 0, hasDetails: false, unitConversion: false },
      sections,
      shots,
      projectFile,
    }) as unknown as ImportScreenDigest;

  it('takes the counts on the SAME step as the claim', () => {
    // The whole point. Reading them at the end of the flow compares the receipt
    // against a project that later moved - a second import, an undo
    // interjection - and F3 then reports a receipt that was telling the truth.
    const captured = captureReceipt(emptyReceiptCapture(), digestWith('2 sections - 9 shots', 12, 40), false);
    expect(captured.claim?.sections).toBe(2);
    expect(captured.atReceipt).toEqual({ sections: 12, shots: 40 });
  });

  it('FIRST wins: a second receipt in one flow does not overwrite the first', () => {
    const first = captureReceipt(emptyReceiptCapture(), digestWith('2 sections - 9 shots', 12, 40), false);
    const second = captureReceipt(first, digestWith('7 sections - 70 shots', 99, 500), true);
    expect(second).toBe(first);
    expect(second.claim?.sections).toBe(2);
    expect(second.atReceipt).toEqual({ sections: 12, shots: 40 });
    expect(second.historyTouchedBeforeReceipt).toBe(false);
  });

  it('records whether history had already been touched when the receipt appeared', () => {
    // If an undo or a save ran first, the counts are not the ones the import
    // produced, and F3 skips rather than guesses.
    const touched = captureReceipt(emptyReceiptCapture(), digestWith('1 section - 3 shots', 5, 8), true);
    expect(touched.historyTouchedBeforeReceipt).toBe(true);
    const clean = captureReceipt(emptyReceiptCapture(), digestWith('1 section - 3 shots', 5, 8), false);
    expect(clean.historyTouchedBeforeReceipt).toBe(false);
  });

  it('is inert on a step with no receipt on screen', () => {
    const start = emptyReceiptCapture();
    expect(captureReceipt(start, digestWith(null, 3, 3), true)).toBe(start);
  });

  it('takes the PROJECT on the same step too, so the swap skip can ask about it', () => {
    // F3's project-swap skip asked `before.projectFile !== after.projectFile` -
    // the END of the flow. A second file later in the same flow swapping
    // projects then suppressed an oracle whose claim and counts were both
    // already fixed on the original project. Same mistake `atReceipt` exists to
    // avoid, one field over, so it is captured at the same instant.
    const captured = captureReceipt(emptyReceiptCapture(), digestWith('1 section - 3 shots', 5, 8, '/tmp/first.stalx'), false);
    expect(captured.projectFileAtReceipt).toBe('/tmp/first.stalx');

    // The swap arrives afterwards: FIRST WINS keeps the receipt's own project,
    // so the skip compares `before` against `/tmp/first.stalx` and F3 runs.
    const afterSwap = captureReceipt(captured, digestWith('9 sections - 90 shots', 99, 99, '/tmp/second.stalx'), true);
    expect(afterSwap).toBe(captured);
    expect(afterSwap.projectFileAtReceipt).toBe('/tmp/first.stalx');
  });

  it('carries a null project file rather than inventing one', () => {
    // No project open, or one never saved. `before.projectFile !== null` is what
    // gates the skip, so a null here must not read as a swap.
    const captured = captureReceipt(emptyReceiptCapture(), digestWith('1 section - 3 shots', 5, 8, null), false);
    expect(captured.projectFileAtReceipt).toBeNull();
    expect(emptyReceiptCapture().projectFileAtReceipt).toBeNull();
  });
});

/**
 * S4's single question, which decides BOTH the same-step exemption and the
 * one-move grace window: did this move hand work to something that can fail?
 *
 * Every no-work case below is a move that is eligible by shape and was
 * performed, so every rule that looked at shape alone said yes to it - and each
 * handed no file to anything. As an exemption that means the step first showing
 * an unrelated error dialog is not blamed for it; as grace it means the NEXT
 * step is not blamed either. Both were false negatives, which is why the two
 * answers now come from one predicate: on shape these look like work, on
 * outcome they are not, and any rule that reads shape reintroduces the bug.
 */
describe('s4WorkStarted', () => {
  const started = { started: true, reason: null };

  it('is work when the entry point actually accepted the file', () => {
    expect(s4WorkStarted({ eligible: true, performed: true, cancel: false, handoff: 'accepted' })).toEqual(started);
  });

  it('is work for an ERROR_PRODUCING confirm, which dispatches a real command', () => {
    // Not a file entry point, so no handoff to read - the dispatch is the work.
    expect(s4WorkStarted({ eligible: true, performed: true, cancel: false, handoff: null })).toEqual(started);
  });

  it('is NOT work on a SWALLOWED picker: the chord dispatched and nothing opened', () => {
    // Ctrl+O with a dialog already up. `useKeyboardShortcuts` returns early for
    // every non-Escape key, so the picker is never armed and the answer is
    // never consumed - but the bridge still reports `performed: true`, because
    // the keydown really was dispatched.
    expect(s4WorkStarted({ eligible: true, performed: true, cancel: false, handoff: 'unconsumed' })).toEqual({
      started: false,
      reason: 'unconsumed',
    });
  });

  it('is NOT work on a GATE-REFUSED route', () => {
    // `routeOpenedFile` raises the import-gate toast and returns before parsing
    // whenever a dialog is up, placement is on, or an import's file is still
    // parsing (`routeHandoff`). The harness sets
    // `performed = true` unconditionally on a route, so only the handoff can
    // tell this from an import that started.
    expect(s4WorkStarted({ eligible: true, performed: true, cancel: false, handoff: 'gate-refused' })).toEqual({
      started: false,
      reason: 'gate-refused',
    });
  });

  it('is NOT work on a picker cancel, whatever else is true of it', () => {
    expect(s4WorkStarted({ eligible: true, performed: true, cancel: true, handoff: null })).toEqual({
      started: false,
      reason: 'cancel',
    });
  });

  it('is NOT work when the move never happened', () => {
    // The exemption did not even check this one: a file move whose control was
    // absent was exempted for an error it could not have caused.
    expect(s4WorkStarted({ eligible: true, performed: false, cancel: false, handoff: 'accepted' })).toEqual({
      started: false,
      reason: 'not-performed',
    });
  });

  it('is NOT work for a move kind that could never start any', () => {
    expect(s4WorkStarted({ eligible: false, performed: true, cancel: false, handoff: 'accepted' })).toEqual({
      started: false,
      reason: 'not-eligible',
    });
  });

  it('THE REGRESSION: nothing short of an accepted handoff counts as work', () => {
    // The property rather than the examples. Both S4 answers read this one
    // boolean, so this is simultaneously "cannot be exempted" and "cannot grant
    // grace" for every combination that did not land a file.
    for (const handoff of ['unconsumed', 'gate-refused'] as const) {
      for (const cancel of [false, true]) {
        for (const performed of [false, true]) {
          expect(s4WorkStarted({ eligible: true, performed, cancel, handoff }).started).toBe(false);
        }
      }
    }
  });
});

/**
 * A routed file's handoff, read off what `routeOpenedFile`'s gates consulted on
 * the tick it was called (`RouteOutcome`). The router returns void, so these
 * readings are the only account of whether it refused.
 *
 * #576 W67-NF3: the one-import-at-a-time gate also refuses while an import's
 * file is still parsing (`uiStore.importParse`), with no dialog up and
 * placement off. The rule read only the other two, so a route the parse gate
 * refused was scored `accepted`: S4 exempted its step and re-opened grace.
 */
describe('routeHandoff', () => {
  const idle = { dialogAtDispatch: null, placementAtDispatch: false, parseAtDispatch: false };

  it('is accepted when none of the gates was on', () => {
    expect(routeHandoff(idle)).toBe('accepted');
  });

  it('is gate-refused when a dialog was up, or placement was on', () => {
    expect(routeHandoff({ ...idle, dialogAtDispatch: 'tml-import' })).toBe('gate-refused');
    expect(routeHandoff({ ...idle, dialogAtDispatch: 'settings' })).toBe('gate-refused');
    expect(routeHandoff({ ...idle, placementAtDispatch: true })).toBe('gate-refused');
  });

  it('THE REGRESSION (W67-NF3): is gate-refused while an import file is still parsing', () => {
    // Nothing on screen yet - no dialog, no placement - and the router still
    // refuses: `importFlowOwnsScreen` returns true on `importParse !== null`.
    const handoff = routeHandoff({ ...idle, parseAtDispatch: true });
    expect(handoff).toBe('gate-refused');
    expect(s4WorkStarted({ eligible: true, performed: true, cancel: false, handoff })).toEqual({
      started: false,
      reason: 'gate-refused',
    });
  });

  it('is accepted only when every gate was off', () => {
    for (const dialogAtDispatch of [null, 'dmp-import-new']) {
      for (const placementAtDispatch of [false, true]) {
        for (const parseAtDispatch of [false, true]) {
          const anyOn = dialogAtDispatch !== null || placementAtDispatch || parseAtDispatch;
          expect(routeHandoff({ dialogAtDispatch, placementAtDispatch, parseAtDispatch })).toBe(
            anyOn ? 'gate-refused' : 'accepted',
          );
        }
      }
    }
  });
});

/**
 * #576 W71-NF9: IMPORT GATE's refusal half. Every refusal of `routeOpenedFile`
 * raises `TOAST_KEYS.IMPORT_IN_PROGRESS` ("Finish or cancel the current import
 * first."), but the screen half of IMPORT GATE judges only routes drawn on a
 * screen an import owns (`IMPORT_GUARDED_SCREENS`), and the parse gate refuses
 * with no such screen up. A parse-gate refusal was never checked for its toast.
 */
describe('checkGateRefusalToast (IMPORT GATE, refusal half)', () => {
  /** A synthetic toast key; the oracle takes the real one as an argument. */
  const GATE = 'test-import-gate';
  const idle = { dialogAtDispatch: null, placementAtDispatch: false, parseAtDispatch: false };
  const parsing = { ...idle, parseAtDispatch: true };
  const check = (over: Partial<Parameters<typeof checkGateRefusalToast>[0]> = {}) =>
    checkGateRefusalToast({
      handoff: routeHandoff(parsing),
      importGateJudged: false,
      toastsRaised: [],
      digestToasts: [],
      key: GATE,
      gates: parsing,
      ...over,
    });

  it('THE REGRESSION (W71-NF9): fails a parse-gate refusal that raised no toast', () => {
    // No dialog, no placement: nothing on screen for the screen half to judge.
    expect(() => check()).toThrow(ImportOracleError);
    expect(() => check()).toThrow(/IMPORT GATE: a route the gate refused did not say so/);
    expect(() => check()).toThrow(/activeDialog=null placement=false parsing=true/);
  });

  it('passes a parse-gate refusal that raised the toast', () => {
    expect(check({ toastsRaised: [GATE] })).toBe('said');
  });

  it('does not check a route that was not refused, or a move that was not a route', () => {
    expect(check({ handoff: routeHandoff(idle), gates: idle })).toBe('not-refused');
    expect(check({ handoff: 'unconsumed' })).toBe('not-refused');
    expect(check({ handoff: null })).toBe('not-refused');
  });

  it('reads the toast when the route returned, not off the settled digest (HFI2-F3)', () => {
    // Raised by the route, faded by the time a wizard calculation let the digest settle.
    expect(check({ toastsRaised: [GATE], digestToasts: [] })).toBe('said');
    // Up in the digest from an earlier refusal; this route raised nothing.
    expect(() => check({ toastsRaised: [], digestToasts: [GATE] })).toThrow(/did not say so/);
    // Another toast is not the refusal.
    expect(() => check({ toastsRaised: ['test-other'] })).toThrow(/did not say so/);
  });

  it("falls back to the digest only when the route's reading is missing", () => {
    expect(check({ toastsRaised: null, digestToasts: [GATE] })).toBe('said');
    expect(() => check({ toastsRaised: null, digestToasts: [] })).toThrow(/did not say so/);
  });

  it('leaves a route the screen half judged to it, so one refusal is not reported twice', () => {
    // The screen half failed it already, under its own headline, or passed it on the same reading.
    expect(check({ importGateJudged: true })).toBe('import-gate');
    expect(check({ importGateJudged: true, toastsRaised: [GATE] })).toBe('import-gate');
  });

  it('checks every refusal the screen half did not judge, whichever gate refused it', () => {
    // A dialog outside the import flow (the router's second gate), placement,
    // and a guarded screen that moved before dispatch: all refused, all toast.
    for (const gates of [
      { ...idle, dialogAtDispatch: 'settings' },
      { ...idle, dialogAtDispatch: 'dmp-import-new' },
      { ...idle, placementAtDispatch: true },
      parsing,
    ]) {
      const handoff = routeHandoff(gates);
      expect(() => check({ handoff, gates })).toThrow(/did not say so/);
      expect(check({ handoff, gates, toastsRaised: [GATE] })).toBe('said');
    }
  });
});

/**
 * #576 W62-NF3: S4's grace window, held open while an import's file parses.
 *
 * `routeOpenedFile` refuses every second file while a parse is in flight, so the
 * moves that follow a slow parse start nothing - an undo, a mode switch, a route
 * the gate refused - and the one move of grace a started parse opens ran out
 * while the parse was still going. Its error then landed on an unrelated move,
 * and S4 blamed that move.
 */
describe('nextErrorGrace', () => {
  const WINDOW = 1; // importDialogFuzz's ERROR_GRACE_MOVES

  interface Move {
    /** `s4WorkStarted(...).started`. */
    started: boolean;
    /** `digest.parsing` after the move. */
    parsing: boolean;
    /** Did a NEW error dialog come up on this move? */
    errorRaised: boolean;
  }

  /**
   * S4 over a run, as `stepOracles` applies it: a newly raised error dialog on a
   * move that started nothing is blamed on that move unless grace is left, and
   * the grace is updated after the check. Answers the blamed moves' indexes.
   */
  function blamedMoves(moves: readonly Move[]): number[] {
    let grace = 0;
    const blamed: number[] = [];
    moves.forEach((move, index) => {
      if (move.errorRaised && !move.started && grace === 0) blamed.push(index);
      grace = nextErrorGrace({ previous: grace, started: move.started, parsing: move.parsing, window: WINDOW });
    });
    return blamed;
  }

  const unrelated = (parsing: boolean, errorRaised = false): Move => ({ started: false, parsing, errorRaised });

  it('a move that started work opens the window', () => {
    expect(nextErrorGrace({ previous: 0, started: true, parsing: false, window: WINDOW })).toBe(WINDOW);
    expect(nextErrorGrace({ previous: 0, started: true, parsing: true, window: WINDOW })).toBe(WINDOW);
  });

  it('with no parse in flight, the window closes one move at a time, as before', () => {
    expect(nextErrorGrace({ previous: 1, started: false, parsing: false, window: WINDOW })).toBe(0);
    expect(nextErrorGrace({ previous: 0, started: false, parsing: false, window: WINDOW })).toBe(0);
    expect(nextErrorGrace({ previous: 3, started: false, parsing: false, window: 3 })).toBe(2);
  });

  it('a parse in flight holds the window open, move after move', () => {
    let grace = WINDOW;
    for (let move = 0; move < 5; move++) {
      grace = nextErrorGrace({ previous: grace, started: false, parsing: true, window: WINDOW });
      expect(grace).toBe(WINDOW);
    }
    // ...including a parse the run never saw start (a picker answer consumed
    // after its own move had been scored).
    expect(nextErrorGrace({ previous: 0, started: false, parsing: true, window: WINDOW })).toBe(WINDOW);
  });

  it('THE REGRESSION: a parse spanning several moves is not blamed on the move it lands on', () => {
    const run: Move[] = [
      { started: true, parsing: true, errorRaised: false }, // the file picked; its parse is slow
      unrelated(true), // an undo
      unrelated(true), // a route the gate refused
      unrelated(true), // a mode switch
      unrelated(false, true), // the parse fails: its error lands on an Escape
    ];
    expect(blamedMoves(run)).toEqual([]);
  });

  it('once the parse has ended, the window closes again, so a later error is still blamed', () => {
    const run: Move[] = [
      { started: true, parsing: true, errorRaised: false },
      unrelated(true),
      unrelated(false), // the parse ended on this move, with no error
      unrelated(false),
      unrelated(false, true), // an error out of nowhere
    ];
    expect(blamedMoves(run)).toEqual([4]);
  });
});

/**
 * #576 W27 (fuzzers): EXPORT CHORD failed six farm runs on a chord that opened
 * nothing. Two Ctrl+E interjections in a row: the first, at idle, opened the
 * export modal legally; the second was dispatched OVER that modal
 * (`activeDialog="export"`), the screen stayed export-modal, and the oracle read
 * "a gate was on and the modal is up" as a leak. Replays: app-idf-corpus seed
 * 10014 (64 iterations, SYN-naharon + corpus), app-idf-gen seed 10027 (78
 * iterations, SYN-naharon).
 */
describe('exportChordVerdict (EXPORT CHORD)', () => {
  const noGate = { dialogAtDispatch: null, placementAtDispatch: false, fullscreenAtDispatch: false };

  it('THE REGRESSION: a chord over the export modal itself opened nothing', () => {
    const overModal = { ...noGate, dialogAtDispatch: 'export' };
    expect(exportChordVerdict(overModal, 'export-modal', 'export-modal')).toEqual({ kind: 'already-open' });
  });

  it('still fails a modal opened behind an import dialog', () => {
    const overFlow = { ...noGate, dialogAtDispatch: 'new-project-for-import' };
    expect(exportChordVerdict(overFlow, 'new-project', 'export-modal')).toEqual({
      kind: 'opened-behind-gate',
      gates: ['activeDialog="new-project-for-import"'],
    });
  });

  it('still fails a modal opened during placement or over a full-screen table', () => {
    expect(exportChordVerdict({ ...noGate, placementAtDispatch: true }, 'wiz-manual', 'export-modal')).toEqual({
      kind: 'opened-behind-gate',
      gates: ['placementMode active'],
    });
    expect(exportChordVerdict({ ...noGate, fullscreenAtDispatch: true }, 'idle', 'export-modal')).toEqual({
      kind: 'opened-behind-gate',
      gates: ['dataTableFullScreen'],
    });
  });

  it('counts a chord that landed after the dialog closed, and ignores a legal one', () => {
    expect(exportChordVerdict(noGate, 'walls', 'export-modal')).toEqual({ kind: 'after-dialog-closed' });
    expect(exportChordVerdict(noGate, 'idle', 'export-modal')).toEqual({ kind: 'none' });
    expect(exportChordVerdict({ ...noGate, dialogAtDispatch: 'error' }, 'error', 'error')).toEqual({ kind: 'none' });
  });
});
