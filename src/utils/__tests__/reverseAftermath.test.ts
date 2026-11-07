/**
 * Positional-ref mirroring after a section reverse (#24).
 *
 * The hazard these pin: a reverse renumbers station `i` to `N - i`, so a
 * positional `StationRef` stays IN RANGE and silently names a different physical
 * station - the distance ramp silently re-roots from the far end while still
 * looking correct. So every assertion here compares against a mirrored index,
 * and every fixture uses a station index that differs from its mirror.
 *
 * The GAS PLAN's pins are no longer among them (#310): they hold backend
 * anchors and are re-resolved around every mutation instead, which is
 * reverse-proof by construction. Their coverage lives in the store
 * (`gasPlanAnchors.test.ts`), against the real IPC round trip.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import {
  applyTransform,
  composeReverseTransforms,
  isIdentityTransform,
  mirrorRef,
  singleReverseTransform,
  type ReverseTransform,
} from '../reverseAftermath';
import { applyReverseAftermath } from '../reverseAftermath';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { useUIStore } from '../../store/uiStore';
import type { Section } from '../../types/project';

const REVERSED = 7;
const UNTOUCHED = 9;
/** 7 shots -> 8 stations, so station 2 mirrors to 5. Deliberately not 4 (its own mirror). */
const counts = new Map([[REVERSED, singleReverseTransform(7)]]);

const ref = (section_id: number, station_index: number) => ({ section_id, station_index });

describe('mirrorRef', () => {
  it('maps i to N - i inside a reversed section', () => {
    expect(mirrorRef(ref(REVERSED, 2), counts)).toEqual(ref(REVERSED, 5));
    expect(mirrorRef(ref(REVERSED, 0), counts)).toEqual(ref(REVERSED, 7));
  });

  it('leaves a section this batch did not reverse alone', () => {
    expect(mirrorRef(ref(UNTOUCHED, 2), counts)).toEqual(ref(UNTOUCHED, 2));
  });

  it('is an involution for an in-range index', () => {
    const once = mirrorRef(ref(REVERSED, 2), counts);
    expect(mirrorRef(once, counts)).toEqual(ref(REVERSED, 2));
  });

  it('clamps a ref that outlived the shots it was dropped on', () => {
    // The distance origin survives shot deletion (its only reset is a project
    // switch), so station_index can sit past the end of a now-shorter section.
    // The raw mirror would go NEGATIVE, and station_index is a u32 on the Rust
    // side - serde would reject the whole call and the overlay would fail
    // permanently on one poisoned ref.
    expect(mirrorRef(ref(REVERSED, 12), counts)).toEqual(ref(REVERSED, 0));
    expect(mirrorRef(ref(REVERSED, 8), counts)).toEqual(ref(REVERSED, 0));
    // Exactly at the end still mirrors normally - the clamp must not eat it.
    expect(mirrorRef(ref(REVERSED, 7), counts)).toEqual(ref(REVERSED, 0));
    expect(mirrorRef(ref(REVERSED, 6), counts)).toEqual(ref(REVERSED, 1));
  });

  it('leaves an out-of-range ref exactly where it is for an identity transform', () => {
    // The clamp is the reason `mirrorRef` skips an identity entry outright
    // instead of running `applyTransform`: `i -> i` still clamps, so a pin that
    // already sat past the end would be dragged onto the last station by an
    // operation that renumbered NOTHING.
    const identity = new Map([[REVERSED, { shift: 0, sign: 1 as const, maxIndex: 7 }]]);
    expect(mirrorRef(ref(REVERSED, 12), identity)).toEqual(ref(REVERSED, 12));
    expect(applyTransform(ref(REVERSED, 12), { shift: 0, sign: 1, maxIndex: 7 })).toEqual(
      ref(REVERSED, 7)
    );
  });
});

/**
 * Which sections a reverse (or the undo/redo of one) renumbered, and against
 * which shot count, comes straight off `CommandResult.reversed_sections` (#24) -
 * the geometry cannot answer it, because a reverse keeps every world coordinate
 * and a section that was reversed AND moved in the same action matches neither
 * arrangement afterwards.
 *
 * From `undo_to`/`redo_to` that field is an ORDERED PER-STEP LOG with repeats
 * intact, so the entries have to be COMPOSED into one affine map and applied
 * once. Two folds that look obviously right are both wrong:
 *   - cancelling repeats (the old parity `Set`) assumes the count never changed
 *     between the two reverses, which a `CreateShot` in the range breaks;
 *   - replaying `i -> c - i` per entry clamps per step, so an out-of-range pin
 *     goes `p -> 0 -> c` across a pair that between them renumbered nothing.
 */
describe('composeReverseTransforms', () => {
  /** Only `id` and `shots.length` are read, so the shots are placeholders. */
  const section = (id: number, shotCount: number) =>
    ({ id, shots: Array.from({ length: shotCount }, () => ({})) }) as unknown as Pick<
      Section,
      'id' | 'shots'
    >;

  const sections = [section(REVERSED, 7), section(UNTOUCHED, 4)];
  const entry = (section_id: number, shot_count: number) => ({ section_id, shot_count });

  /** Run one index through the composed transform for `sectionId`. */
  const move = (
    transforms: Map<number, ReverseTransform>,
    sectionId: number,
    stationIndex: number
  ) => mirrorRef(ref(sectionId, stationIndex), transforms).station_index;

  it('pairs each reported section with the count it was reversed against', () => {
    expect(composeReverseTransforms([entry(REVERSED, 7)], sections)).toEqual(
      new Map([[REVERSED, singleReverseTransform(7)]])
    );
    expect(
      composeReverseTransforms([entry(UNTOUCHED, 4), entry(REVERSED, 7)], sections)
    ).toEqual(
      new Map([
        [REVERSED, singleReverseTransform(7)],
        [UNTOUCHED, singleReverseTransform(4)],
      ])
    );
  });

  it('is empty when the operation renumbered nothing', () => {
    // Absent is the wire truth for every command that reversed nothing.
    expect(composeReverseTransforms(undefined, sections).size).toBe(0);
    expect(composeReverseTransforms([], sections).size).toBe(0);
  });

  it('collapses two equal-count reverses of one section to the identity', () => {
    const composed = composeReverseTransforms(
      [entry(REVERSED, 7), entry(REVERSED, 7)],
      sections
    );
    const transform = composed.get(REVERSED);
    expect(transform).toBeDefined();
    expect(isIdentityTransform(transform as ReverseTransform)).toBe(true);
    expect(move(composed, REVERSED, 2)).toBe(2);
  });

  it('keeps a third listing as a single net flip', () => {
    const composed = composeReverseTransforms(
      [entry(REVERSED, 7), entry(REVERSED, 7), entry(REVERSED, 7)],
      sections
    );
    expect(composed.get(REVERSED)).toEqual(singleReverseTransform(7));
    expect(move(composed, REVERSED, 2)).toBe(5);
  });

  it('composes an interleaved log per section', () => {
    const composed = composeReverseTransforms(
      [entry(REVERSED, 7), entry(UNTOUCHED, 4), entry(REVERSED, 7)],
      sections
    );
    expect(isIdentityTransform(composed.get(REVERSED) as ReverseTransform)).toBe(true);
    expect(composed.get(UNTOUCHED)).toEqual(singleReverseTransform(4));
  });

  it('composes unequal counts into a pure shift, which neither naive fold gets', () => {
    // `[Reverse, CreateShot, Reverse]` undone as one range: the first step
    // mirrors against 4 shots, the second against 3, and the refreshed section
    // has 3. Composed that is `i -> 3 - (4 - i) = i - 1`.
    const composed = composeReverseTransforms(
      [entry(REVERSED, 4), entry(REVERSED, 3)],
      [section(REVERSED, 3)]
    );
    expect(composed.get(REVERSED)).toEqual({ shift: -1, sign: 1, maxIndex: 3 });
    // Cancelling the repeat (the old parity Set) leaves this at 2.
    expect(move(composed, REVERSED, 2)).toBe(1);
    expect(move(composed, REVERSED, 3)).toBe(2);
    // Keeping the repeat but dividing by the post-range count instead would give
    // `3 - 0 = 3`; the composed answer is `-1`, clamped once into range.
    expect(move(composed, REVERSED, 0)).toBe(0);
  });

  it('divides by the count in force at the step, not the post-range one', () => {
    // A single undone reverse whose section then lost a shot later in the range:
    // the entry says 4, the refreshed project says 3. Mirroring against 3 would
    // land station 3 on 0; against 4 it lands on 1.
    const composed = composeReverseTransforms(
      [entry(REVERSED, 4)],
      [section(REVERSED, 3)]
    );
    expect(composed.get(REVERSED)).toEqual({ shift: 4, sign: -1, maxIndex: 3 });
    expect(move(composed, REVERSED, 3)).toBe(1);
  });

  it('drops a section the refreshed project no longer has', () => {
    // Nothing left to mirror into, and no range to clamp against.
    expect(composeReverseTransforms([entry(REVERSED, 7)], []).size).toBe(0);
  });

  it('drops a shot-less section, which reverses to itself', () => {
    // A single station has no other end - mirroring it would only churn the UI
    // state `applyReverseAftermath` clears.
    expect(
      composeReverseTransforms([entry(REVERSED, 7)], [section(REVERSED, 0)]).size
    ).toBe(0);
  });
});

describe('applyReverseAftermath', () => {
  beforeEach(() => {
    useGasPlanStore.getState().reset();
    useUIStore.setState({ distanceGradientOrigin: null });
  });

  it('leaves the distance-gradient origin alone - it is anchor-resolved, not mirrored (#576 D39(g))', () => {
    // Like the plan pins below: the seam re-resolves it from its anchor
    // (`remapDistanceOrigin`) before this runs, so a mirror here would move it twice.
    useUIStore.setState({ distanceGradientOrigin: ref(REVERSED, 2) });
    useUIStore.getState().setSelection({ type: 'station', stationRef: ref(REVERSED, 3) });
    applyReverseAftermath(counts);
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(REVERSED, 2));
    expect(useUIStore.getState().selection).toBeNull();
  });

  it('leaves the GAS PLAN pins alone - they are anchor-resolved, not mirrored (#310)', () => {
    // Mirroring here as well as re-resolving from anchors would apply the shift TWICE, landing
    // the pin back on the station the reverse moved it off. The seam re-resolves them
    // (`remapPlanPins`) before this runs; see `gasPlanAnchors.test.ts` for that half.
    useGasPlanStore.setState({ origin: ref(REVERSED, 2), endpoint: ref(REVERSED, 1) });
    applyReverseAftermath(counts);
    const after = useGasPlanStore.getState();
    expect(after.origin).toEqual(ref(REVERSED, 2));
    expect(after.endpoint).toEqual(ref(REVERSED, 1));
  });

  it('does nothing at all when the range renumbered nothing', () => {
    // The four UI clears below it are destructive, and a cancelled-out range left the user's
    // selection and half-staged merge perfectly valid.
    useUIStore.setState({ distanceGradientOrigin: ref(REVERSED, 2) });
    useUIStore.getState().setSelection({ type: 'station', stationRef: ref(REVERSED, 3) });
    applyReverseAftermath(new Map([[REVERSED, { shift: 0, sign: 1 as const, maxIndex: 7 }]]));
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(REVERSED, 2));
    expect(useUIStore.getState().selection).not.toBeNull();
  });
});
