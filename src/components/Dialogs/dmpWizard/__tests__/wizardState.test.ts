/**
 * #576 D4: the DMP wizard's state machine. Each phase carries its own data, so
 * the combinations the old phase-label-plus-flags state could write and never
 * meant (a suggestion while placing, a segment anchor with no segment, both
 * confirmations open, the dialog hidden outside manual placing) are not states
 * here; these pin the transitions the dialog dispatches.
 *
 * All data is synthetic.
 */
import { describe, it, expect } from 'vitest';
import type { CrossSectionSuggestion } from '../../../../types/commands';
import {
  initialWizardState,
  segmentQueue,
  wizardReducer,
  type ManualPlacingPhase,
  type SegmentChoices,
  type WizardAction,
  type WizardState,
} from '../wizardState';

const suggestion = (importSectionIndex: number, score: number): CrossSectionSuggestion => ({
  existingStationIndex: 0,
  importStationIndex: 0,
  score,
  matchedShots: 0,
  existingMatchedShots: 0,
  preferredMergeOffset: 0,
  direction: 'forward',
  isPartial: false,
  confidence: 'high',
  existingSectionName: 'CaveA',
  importSectionIndex,
  importSectionName: `S${importSectionIndex}`,
  targetType: 'existing',
  existingSectionId: 1,
  targetImportIndex: null,
});

const NONE: SegmentChoices = { skipped: new Set(), withSuggestion: new Set() };

const run = (state: WizardState, ...actions: WizardAction[]) => actions.reduce(wizardReducer, state);

const manualOn = (queue: number[][], choices: SegmentChoices = NONE, mode: 'manual' | 'bySegment' = 'bySegment') =>
  run(initialWizardState(undefined, false), { type: 'placeManually', queue: segmentQueue(queue)!, choices, mode });

const asManual = (state: WizardState): ManualPlacingPhase => {
  if (state.phase.kind !== 'manual_placing') throw new Error(`not manual placing: ${state.phase.kind}`);
  return state.phase;
};

describe('DMP wizard state: where an instance starts', () => {
  it('a fresh wizard starts in init, not calculating, no confirmation open', () => {
    expect(initialWizardState(undefined, true)).toEqual({ phase: { kind: 'init', calculating: false }, exit: null });
  });

  it('a reopened one starts on its queue, hidden exactly when it auto-starts', () => {
    const saved = { segmentQueue: [[1, 2], [3]], segmentAnchor: { sectionIndex: 2, stationIndex: 4 } };
    for (const autoStart of [true, false]) {
      const phase = asManual(initialWizardState(saved, autoStart));
      expect(phase).toMatchObject({
        queue: [[1, 2], [3]],
        anchor: { sectionIndex: 2, stationIndex: 4 },
        previewAnchor: { sectionIndex: 2, stationIndex: 4 },
        mode: 'manual',
        fromExisting: { sectionId: null, stationIndex: 0 },
        handoffPending: autoStart,
      });
    }
  });

  it('a reopened one with nothing queued goes straight to the late import', () => {
    const state = initialWizardState({ segmentQueue: [], segmentAnchor: { sectionIndex: 0, stationIndex: 0 } }, true);
    expect(state.phase).toEqual({ kind: 'placing' });
  });
});

describe('DMP wizard state: suggestions', () => {
  it('a picked suggestion moves init to suggesting with its alternatives and offset', () => {
    const top = suggestion(1, 5);
    const alt = suggestion(1, 3);
    const state = run(
      initialWizardState(undefined, false),
      { type: 'calculating', calculating: true },
      { type: 'calculating', calculating: false },
      { type: 'suggest', suggestion: top, alternatives: [alt], mergeOffset: 2 },
    );
    expect(state.phase).toEqual({ kind: 'suggesting', suggestion: top, alternatives: [alt], mergeOffset: 2 });
  });

  it('calculating is a state of init only', () => {
    const suggestingState = run(initialWizardState(undefined, false), {
      type: 'suggest', suggestion: suggestion(1, 1), alternatives: [], mergeOffset: 0,
    });
    expect(wizardReducer(suggestingState, { type: 'calculating', calculating: true })).toBe(suggestingState);
  });

  it('choosing an alternative swaps it with the suggestion on screen, best first, at most four', () => {
    const top = suggestion(1, 9);
    const alts = [8, 7, 6, 5].map((score) => suggestion(1, score));
    const state = run(
      initialWizardState(undefined, false),
      { type: 'suggest', suggestion: top, alternatives: alts, mergeOffset: 0 },
      { type: 'chooseAlternative', alternative: alts[2]!, mergeOffset: 3 },
    );
    expect(state.phase).toEqual({
      kind: 'suggesting',
      suggestion: alts[2],
      alternatives: [top, alts[0], alts[1], alts[3]],
      mergeOffset: 3,
    });
  });

  it('a suggestion cannot land on a wizard that has moved on to placing', () => {
    const placing = manualOn([[1]]);
    expect(wizardReducer(placing, { type: 'suggest', suggestion: suggestion(1, 1), alternatives: [], mergeOffset: 0 })).toBe(placing);
  });
});

describe('DMP wizard state: manual placing', () => {
  it('opens a segment on a skipped section, on reusing its suggestion, with the handoff pending', () => {
    const choices: SegmentChoices = { skipped: new Set([3]), withSuggestion: new Set([3]) };
    expect(asManual(manualOn([[2, 3]], choices))).toMatchObject({
      anchor: { sectionIndex: 3, stationIndex: 0 },
      previewAnchor: { sectionIndex: 3, stationIndex: 0 },
      mode: 'suggestion',
      handoffPending: true,
    });
    // The early import's way in opens on map placement whatever the segment holds.
    expect(asManual(manualOn([[2, 3]], choices, 'manual')).mode).toBe('manual');
  });

  it('a placed segment opens the next one fresh, and the last one goes to the late import', () => {
    let state = manualOn([[1], [2]]);
    state = run(
      state,
      { type: 'setMode', mode: 'fromExisting' },
      { type: 'setFromExistingSection', sectionId: 9 },
      { type: 'setFromExistingStation', stationIndex: 4 },
      { type: 'handoffSettled' },
      { type: 'segmentDone', choices: { skipped: new Set(), withSuggestion: new Set([2]) } },
    );
    expect(asManual(state)).toMatchObject({
      queue: [[2]],
      anchor: { sectionIndex: 2, stationIndex: 0 },
      mode: 'suggestion',
      fromExisting: { sectionId: null, stationIndex: 0 },
      handoffPending: false,
    });
    expect(wizardReducer(state, { type: 'segmentDone', choices: NONE }).phase).toEqual({ kind: 'placing' });
  });

  it('skipping rotates the queue; the only segment left stays', () => {
    const rotated = run(manualOn([[1], [2], [3]]), { type: 'segmentSkipped', choices: NONE });
    expect(asManual(rotated).queue).toEqual([[2], [3], [1]]);
    expect(asManual(rotated).anchor).toEqual({ sectionIndex: 2, stationIndex: 0 });
    const single = manualOn([[1]]);
    expect(wizardReducer(single, { type: 'segmentSkipped', choices: NONE })).toBe(single);
  });

  it('a hover previews, closing the dropdown reverts to the committed anchor', () => {
    const state = run(manualOn([[1, 2]]), { type: 'commitAnchor', anchor: { sectionIndex: 2, stationIndex: 3 } });
    expect(asManual(run(state, { type: 'hoverAnchorSection', sectionIndex: 1 })).previewAnchor).toEqual({ sectionIndex: 1, stationIndex: 0 });
    // A station hover stays on the COMMITTED section.
    expect(asManual(run(state, { type: 'hoverAnchorSection', sectionIndex: 1 }, { type: 'hoverAnchorStation', stationIndex: 5 })).previewAnchor)
      .toEqual({ sectionIndex: 2, stationIndex: 5 });
    expect(asManual(run(state, { type: 'hoverAnchorStation', stationIndex: 5 }, { type: 'hoverAnchorStation', stationIndex: null })).previewAnchor)
      .toEqual({ sectionIndex: 2, stationIndex: 3 });
  });

  it('rows inserted before the anchor renumber it with them, on its own section only', () => {
    const state = run(manualOn([[1]]), { type: 'commitAnchor', anchor: { sectionIndex: 1, stationIndex: 3 } });
    const inserted = run(state, { type: 'rowsChanged', sectionIndex: 1, at: 1, delta: 2, survivorsAfter: 7 });
    expect(asManual(inserted).anchor).toEqual({ sectionIndex: 1, stationIndex: 5 });
    expect(asManual(inserted).previewAnchor).toEqual({ sectionIndex: 1, stationIndex: 5 });
    expect(wizardReducer(state, { type: 'rowsChanged', sectionIndex: 4, at: 1, delta: 2, survivorsAfter: 7 })).toBe(state);
  });

  it('the hidden handoff window exists only in manual placing', () => {
    const init = initialWizardState(undefined, false);
    expect(wizardReducer(init, { type: 'handoffSettled' })).toBe(init);
    const settled = run(manualOn([[1]]), { type: 'handoffSettled' });
    expect(asManual(settled).handoffPending).toBe(false);
    expect(wizardReducer(settled, { type: 'handoffSettled' })).toBe(settled);
  });
});

describe('DMP wizard state: the exits', () => {
  it('one confirmation at a time, with the count it opened with', () => {
    const state = run(initialWizardState(undefined, false), { type: 'openExit', exit: { kind: 'cancel', count: 2 } });
    expect(state.exit).toEqual({ kind: 'cancel', count: 2 });
    expect(run(state, { type: 'openExit', exit: { kind: 'back', count: 1 } }).exit).toEqual({ kind: 'back', count: 1 });
    expect(run(state, { type: 'closeExit' }).exit).toBeNull();
    // A confirmation overlays the phase; it does not change it.
    expect(run(state, { type: 'closeExit' }).phase).toEqual(state.phase);
  });
});
