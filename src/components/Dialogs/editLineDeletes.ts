/**
 * Where a set of shots sits in the line it is being deleted from.
 *
 * Deleting from either END of a line is a plain truncation: the remaining
 * shots keep their geometry and the line just gets shorter. Deleting from the
 * MIDDLE is a different operation - the passage either splits in two or the
 * downstream stations shift - which is why that path asks the user first.
 *
 * Kept pure and React-free so the rule is unit-testable, and because the same
 * question gets asked of one shot and of a whole selection: a run of shots at the
 * end is exactly as safe to truncate as the single last one, and answering it per
 * shot in a loop cannot work - each call would judge the others as still present.
 */

export type DeleteRunKind = 'prefix' | 'suffix' | 'all' | 'mid' | 'none';

/**
 * Classify a delete against the shots that currently exist, in order.
 *
 * `order` is the line's surviving shots as identity keys; `targets` are the
 * ones being deleted. A selection that is not contiguous is `mid` even if it
 * touches an end - deleting shots 0 and 5 of 10 leaves a hole either way.
 */
export function classifyDeleteRun(
  order: readonly string[],
  targets: ReadonlySet<string>
): DeleteRunKind {
  const positions: number[] = [];
  order.forEach((key, index) => {
    if (targets.has(key)) positions.push(index);
  });
  if (positions.length === 0) return 'none';
  if (positions.length === order.length) return 'all';

  const first = positions[0]!;
  const last = positions[positions.length - 1]!;
  // Contiguous iff the run spans exactly as many slots as it has members.
  if (last - first + 1 !== positions.length) return 'mid';
  if (first === 0) return 'prefix';
  if (last === order.length - 1) return 'suffix';
  return 'mid';
}

/** Whether this delete can go straight through, with no split/shift question. */
export function isEndRun(kind: DeleteRunKind): boolean {
  return kind === 'prefix' || kind === 'suffix' || kind === 'all' || kind === 'none';
}
