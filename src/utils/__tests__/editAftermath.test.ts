/**
 * The frontend's positional refs across an edit that moves shots between
 * lines: Split Line Here, Combine Lines, and their undo and redo.
 *
 * No index transform describes those edits, so the distance-gradient origin is
 * anchored by the backend before the edit (`plan_anchors_for_refs`, the shots
 * either side of it) and placed again after it (`plan_refs_for_anchors`), in
 * whichever line now holds those shots. The backend half - that the anchors
 * really do land on the same physical station across a split, a combine and
 * their undo - is pinned in `dive_plan.rs`'s tests. Here: what the frontend
 * does with the answer, and what it clears.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useUIStore } from '../../store/uiStore';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { TOAST_KEYS } from '../toastKeys';
import {
  afterSurveyEdit,
  applyEditAftermath,
  beforeSurveyEdit,
  movedShotsBetweenLines,
} from '../reverseAftermath';

const planAnchorsForRefs = vi.hoisted(() => vi.fn());
const planRefsForAnchors = vi.hoisted(() => vi.fn());

vi.mock('../divePlanApi', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../divePlanApi')>()),
  planAnchorsForRefs,
  planRefsForAnchors,
}));

const MAIN = 3;
const SECOND = 8;
const ref = (line_id: number, station_index: number) => ({ line_id, station_index });
const line = (id: number, shots: number) => ({ id, shots: Array.from({ length: shots }, () => ({})) }) as never;

/** Main of 6 shots before; Main (3) + a new second half (3) after a split at station 3. */
const BEFORE_SPLIT = [line(MAIN, 6)];
const AFTER_SPLIT = [line(MAIN, 3), line(SECOND, 3)];

function armEverything() {
  const ui = useUIStore.getState();
  ui.setSelection({ type: 'station', stationRef: ref(MAIN, 5) });
  ui.setMergeStationA(ref(MAIN, 5));
  ui.setMoveCommentSource({ stationRef: ref(MAIN, 5) } as never);
  ui.setCommentTarget({ stationRef: ref(MAIN, 5) } as never);
  ui.setHoveredStation(ref(MAIN, 5));
  ui.setHoveredShot({ lineId: MAIN, shotId: 4 });
  ui.setHoveredLineId(MAIN);
}

function armed() {
  const ui = useUIStore.getState();
  return [
    ui.selection,
    ui.mergeStationA,
    ui.moveCommentSource,
    ui.commentTarget,
    ui.hoveredStation,
    ui.hoveredShot,
    ui.hoveredLineId,
  ].map((x) => x !== null);
}

const resetToasts = () => useUIStore.setState({ toasts: [] });
const resetNotices = () => useUIStore.getState().toasts.filter((t) => t.key === TOAST_KEYS.DISTANCE_ORIGIN_RESET);

describe('the distance-gradient origin across a split or a combine', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useGasPlanStore.getState().reset();
    useUIStore.setState({ distanceGradientOrigin: null, selection: null, multiSelection: [] });
    resetToasts();
    planAnchorsForRefs.mockResolvedValue(['anchor']);
  });

  it('follows its station into the second half of a split', async () => {
    useUIStore.setState({ distanceGradientOrigin: ref(MAIN, 5) });
    planRefsForAnchors.mockResolvedValue([ref(SECOND, 2)]);

    const edit = await beforeSurveyEdit(BEFORE_SPLIT);
    await afterSurveyEdit(edit);
    applyEditAftermath(undefined, AFTER_SPLIT, edit);

    expect(planAnchorsForRefs).toHaveBeenCalledWith([ref(MAIN, 5)]);
    expect(planRefsForAnchors).toHaveBeenCalledWith(['anchor']);
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(SECOND, 2));
    expect(resetNotices()).toHaveLength(0);
  });

  it('is re-rooted by a prepending combine only as far as its station moved', async () => {
    // Combining a 2-shot line onto Main's START renumbers Main's stations by
    // two: station 1 is now station 3. Left alone, the ramp would re-root two
    // stations up the line, silently.
    useUIStore.setState({ distanceGradientOrigin: ref(MAIN, 1) });
    planRefsForAnchors.mockResolvedValue([ref(MAIN, 3)]);

    const edit = await beforeSurveyEdit([line(MAIN, 6), line(SECOND, 2)]);
    await afterSurveyEdit(edit);
    applyEditAftermath(undefined, [line(MAIN, 8)], edit);

    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(MAIN, 3));
  });

  it('is cleared, with a notice, when its station places nowhere', async () => {
    useUIStore.setState({ distanceGradientOrigin: ref(MAIN, 5) });
    planRefsForAnchors.mockResolvedValue([null]);

    const edit = await beforeSurveyEdit(BEFORE_SPLIT);
    await afterSurveyEdit(edit);
    applyEditAftermath(undefined, AFTER_SPLIT, edit);

    expect(useUIStore.getState().distanceGradientOrigin).toBeNull();
    expect(resetNotices()).toHaveLength(1);
    expect(resetNotices()[0]!.kind).toBe('info');
  });

  it('without an anchor, is cleared when the split left it past the end, and kept otherwise', async () => {
    planAnchorsForRefs.mockResolvedValue([null]);

    useUIStore.setState({ distanceGradientOrigin: ref(MAIN, 5) });
    let edit = await beforeSurveyEdit(BEFORE_SPLIT);
    await afterSurveyEdit(edit);
    applyEditAftermath(undefined, AFTER_SPLIT, edit);
    expect(planRefsForAnchors).not.toHaveBeenCalled();
    expect(useUIStore.getState().distanceGradientOrigin).toBeNull();
    expect(resetNotices()).toHaveLength(1);

    resetToasts();
    useUIStore.setState({ distanceGradientOrigin: ref(MAIN, 2) });
    edit = await beforeSurveyEdit(BEFORE_SPLIT);
    await afterSurveyEdit(edit);
    applyEditAftermath(undefined, AFTER_SPLIT, edit);
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(MAIN, 2));
    expect(resetNotices()).toHaveLength(0);
  });

  it('keeps a re-root the user made while the edit was in flight', async () => {
    useUIStore.setState({ distanceGradientOrigin: ref(MAIN, 5) });
    planRefsForAnchors.mockImplementation(async () => {
      useUIStore.getState().setDistanceGradientOrigin(ref(MAIN, 1));
      return [ref(SECOND, 2)];
    });

    const edit = await beforeSurveyEdit(BEFORE_SPLIT);
    await afterSurveyEdit(edit);

    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(MAIN, 1));
    expect(edit.originPlaced).toBe(false);
  });

  it('is not mirrored a second time once its anchor placed it after a reverse', async () => {
    // A reverse of a 7-shot line: the anchor already lands station 2 on 5.
    // Mirroring that again would send it back to 2, the opposite end.
    useUIStore.setState({ distanceGradientOrigin: ref(MAIN, 2) });
    planRefsForAnchors.mockResolvedValue([ref(MAIN, 5)]);

    const edit = await beforeSurveyEdit([line(MAIN, 7)]);
    await afterSurveyEdit(edit);
    applyEditAftermath([{ line_id: MAIN, shot_count: 7 }], [line(MAIN, 7)], edit);

    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(MAIN, 5));
  });

  it('costs no IPC when no origin is set', async () => {
    const edit = await beforeSurveyEdit(BEFORE_SPLIT);
    await afterSurveyEdit(edit);
    expect(planAnchorsForRefs).not.toHaveBeenCalled();
    expect(planRefsForAnchors).not.toHaveBeenCalled();
  });
});

describe('the armed tool state across a split or a combine', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useUIStore.setState({ distanceGradientOrigin: null });
    armEverything();
  });

  it('is cleared when shots moved between lines, the undo or redo of either included', async () => {
    const cases: [never[], never[]][] = [
      [BEFORE_SPLIT, AFTER_SPLIT], // split, or the redo of one
      [AFTER_SPLIT, BEFORE_SPLIT], // the undo of a split, or a combine
      [[line(MAIN, 6)], [line(MAIN, 4), line(SECOND, 2)]], // the undo of a combine
    ];
    for (const [before, after] of cases) {
      armEverything();
      const edit = await beforeSurveyEdit(before);
      applyEditAftermath(undefined, after, edit);
      expect(armed()).toEqual(Array(7).fill(false));
    }
  });

  it('is kept for an edit that only added a line, or changed none', async () => {
    let edit = await beforeSurveyEdit(BEFORE_SPLIT);
    applyEditAftermath(undefined, [line(MAIN, 6), line(SECOND, 2)], edit);
    expect(armed()).toEqual(Array(7).fill(true));

    edit = await beforeSurveyEdit(BEFORE_SPLIT);
    applyEditAftermath(undefined, [line(MAIN, 7)], edit);
    expect(armed()).toEqual(Array(7).fill(true));
  });
});

describe('movedShotsBetweenLines', () => {
  const shape = (entries: [number, number][]) => new Map(entries);

  it('is true when a line vanished', () => {
    expect(movedShotsBetweenLines(shape([[1, 2], [2, 3]]), [line(1, 5)])).toBe(true);
  });

  it('is true when a line appeared and an existing one changed its shot count', () => {
    expect(movedShotsBetweenLines(shape([[1, 5]]), [line(1, 2), line(2, 3)])).toBe(true);
  });

  it('is false for an added line alone, or a shot count change alone', () => {
    expect(movedShotsBetweenLines(shape([[1, 5]]), [line(1, 5), line(2, 3)])).toBe(false);
    expect(movedShotsBetweenLines(shape([[1, 5]]), [line(1, 6)])).toBe(false);
  });
});
