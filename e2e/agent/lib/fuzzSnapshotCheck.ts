/**
 * The SLOW half of the undo fuzzers' roundtrip oracle, shared by
 * `undo-redo-fuzz` and `undo-persistence-fuzz`.
 *
 * Both scenarios compare canonical snapshots IN THE PAGE and only fetch the two
 * strings once the page has already reported a mismatch. This is what they do
 * with them: re-compare with numeric tolerance, subtract what a save or a reload
 * is documented to change (only when the caller says the comparison crosses an
 * epoch), and render whatever is left as the failure text.
 *
 * It used to be defined once per scenario, and the two copies drifted: only the
 * `undo-redo-fuzz` one gated the shot-identity delta on a shots path and said
 * so when the uuid set was IDENTICAL, and only it explained a pure
 * `project.loop_closures` reorder. This is that richer copy, and the only one.
 *
 * Pure: no PRNG, no app state, no I/O. Hoisting it out of the scenarios cannot
 * move a single draw, which is what keeps every recorded repro seed valid.
 */
import {
  LOOP_CLOSURES_PATH,
  classifyDiffs,
  describeDiffs,
  describeShotSetDelta,
  diffSnapshots,
  isListPermutationOnly,
  maxNumericDelta,
  shotSetDelta,
  type KnownAsymmetry,
  type SnapshotDiff,
} from './projectSnapshot';

export interface SnapshotCheck {
  equal: boolean;
  /** Equal only once floating-point tolerance was applied. */
  floatNoise: boolean;
  /** Save/reload changes subtracted before reporting the rest (empty = none). */
  attributed: KnownAsymmetry[];
  /** Rendered diff block (empty when equal). */
  text: string;
}

export const EQUAL: SnapshotCheck = {
  equal: true,
  floatNoise: false,
  attributed: [],
  text: '',
};

/**
 * The two caps a caller sets. They stay per scenario because the scenarios
 * document different budgets for them (200 and 400 differences today).
 */
export interface SnapshotCheckLimits {
  /**
   * Differences the slow re-compare collects before it gives up.
   *
   * A cap is necessary (a whole-project divergence would otherwise build a
   * list the size of the project), but a capped list can only ever support a
   * NEGATIVE conclusion. `checkSnapshot` asks for one more than this and
   * refuses to let anything speak for the mismatch - not even the epoch
   * attribution the caller would otherwise RESYNC on - when the extra one
   * comes back.
   */
  diffLimit: number;
  /** Diffs shown in a failure message (more are counted, not printed). */
  shownDiffs: number;
}

/**
 * Re-compare a mismatch the page reported, subtract the documented save/reload
 * families in `attributable`, and render whatever is left.
 *
 * Three outcomes, told apart by the caller:
 *
 *   - `equal: true` - the strings differ only inside the numeric tolerance
 *     (`floatNoise`), or not at all.
 *   - `attributed` non-empty AND `text` starting `  ATTRIBUTED` - every
 *     difference was a documented save heal or reload re-solve, on a COMPLETE
 *     diff. The only shape a caller may resync its model on.
 *   - anything else - a failure, with `text` as its detail.
 *
 * There is no known-open-bug outcome any more. This used to end in a
 * classification `STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS=1` could walk past; every
 * one of those bugs is fixed, so anything that survives the attribution
 * subtraction is a failure.
 */
export function checkSnapshot(
  expected: string,
  actual: string,
  attributable: readonly KnownAsymmetry[],
  limits: SnapshotCheckLimits,
): SnapshotCheck {
  const { diffLimit, shownDiffs } = limits;
  if (expected === actual) return EQUAL;
  // Geometry is re-derived rather than copied on every undo/redo, so a
  // coordinate whose true value is 0 can come back as different femtometre dust.
  //
  // ONE MORE than we report, so `truncated` can tell "exactly `diffLimit`
  // differences" from "at least that many, and the rest were never looked at".
  // The distinction decides whether the attribution may speak for this mismatch
  // at all: the walk visits root keys in sorted order, so a wide `project.*`
  // difference can spend the whole budget before `stations` or `stats` is
  // examined once, and "every path was a documented save/reload change" would
  // then be a statement about a prefix rather than about the mismatch.
  const all = diffSnapshots(expected, actual, diffLimit + 1);
  if (all.length === 0) return { ...EQUAL, floatNoise: true };
  const truncated = all.length > diffLimit;
  if (truncated) all.length = diffLimit;

  // SUBTRACT the paths a save or a reload is documented to change before
  // classifying the rest. Without this a real bug found after a save reads as a
  // pile of gauge-anchor noise with the actual divergence buried in it.
  let diffs = all;
  let attributed: KnownAsymmetry[] = [];
  let absorbed: SnapshotDiff[] = [];
  if (attributable.length > 0) {
    const split = classifyDiffs(all, attributable);
    if (split.known.length > 0) {
      diffs = split.unknown;
      attributed = split.asymmetries;
      absorbed = split.known.map((hit) => hit.diff);
    }
  }
  if (diffs.length === 0) {
    // The ATTRIBUTED shape is what the caller RESYNCS on, so it may only be
    // returned for a complete list. Every path we saw was attributable, but on
    // a truncated diff the ones past the cap were never looked at.
    if (truncated) {
      return {
        equal: false,
        floatNoise: false,
        attributed,
        text:
          `  every path in the first ${diffLimit} was a documented save/reload change, but the\n` +
          '  diff was CAPPED there - so this is a statement about a PREFIX, not about the mismatch.',
      };
    }
    // The absorbed MAGNITUDE goes on the record. The re-derived-geometry
    // families are bounded by shape, not by size (see RELOAD_RESOLVED_GEOMETRY),
    // which is only defensible while the number is visible.
    const worst = maxNumericDelta(absorbed);
    return {
      equal: false,
      floatNoise: false,
      attributed,
      text: attributed
        .map((entry) => `  ATTRIBUTED [${entry.id}]: ${entry.description}`)
        .concat(worst === null ? [] : [`  largest absorbed numeric change: ${worst.toExponential(3)}`])
        .join('\n'),
    };
  }

  const lines = [describeDiffs(diffs.slice(0, shownDiffs))];
  if (diffs.length > shownDiffs) lines.push(`  ...and ${diffs.length - shownDiffs} more differing path(s)`);
  if (truncated) {
    lines.push(
      `  THE DIFF WAS CAPPED at ${diffLimit} path(s): everything above is a PREFIX and nothing`,
      '  past it was examined, so no attribution can speak for this mismatch and none is allowed to.',
    );
  }
  // A shot list that differs field-by-field cannot say whether the same shots
  // moved or a DIFFERENT shot was removed. uuid survives a save/reload, so it
  // answers that - and the answer is the difference between drift and data loss.
  //
  // [#576 RV7a-5] Gated on a REPORTED shots path, for both undo fuzzers alike
  // (this one check serves undoRedoFuzz and undoPersistenceFuzz). The persistence
  // fuzzer's own copy used to print the delta whenever the uuid set changed; the
  // gate is kept on purpose: when the walk was capped before any shots path, or
  // every shots difference was attributed and subtracted, the delta would speak
  // about differences this report neither shows nor stands behind. The run fails
  // either way; the gate decides only what the text claims. Pinned by
  // fuzzSnapshotCheck.test.ts ('gates the shot-identity report on a reported
  // shots path').
  if (diffs.some((diff) => /^project\.sections\[\d+\]\.shots\b/.test(diff.path))) {
    const deltas = shotSetDelta(expected, actual);
    if (deltas.length > 0) {
      lines.push('  SHOT IDENTITY DELTA (by uuid, which survives save/reload):');
      lines.push(describeShotSetDelta(deltas));
    } else {
      lines.push('  (shot identity by uuid is IDENTICAL on both sides - the shots moved, none was lost)');
    }
  }
  for (const entry of attributed) {
    lines.push(`  (plus attributable change(s) [${entry.id}], subtracted before this diff)`);
  }

  // A shifted list reads as dozens of unrelated value diffs at every index past
  // the move, so say so in one sentence. DIAGNOSTIC ONLY: the run fails either
  // way. This used to be finding 4's classifier (undo of RemoveLoopClosure
  // pushed instead of reinserting); the classifier is deleted with the bug, and
  // what is kept is the READING of a failure that ever looks like it again.
  // Still gated on `!truncated`, because a capped diff cannot support that
  // sentence either - it would be describing a prefix.
  if (!truncated && attributed.length === 0 && isListPermutationOnly(expected, actual, LOOP_CLOSURES_PATH)) {
    lines.push('  (every difference above is explained by project.loop_closures being REORDERED:');
    lines.push('   same closures, same ids, same metrics, different positions)');
  }

  return {
    equal: false,
    floatNoise: false,
    attributed,
    text: lines.join('\n'),
  };
}
