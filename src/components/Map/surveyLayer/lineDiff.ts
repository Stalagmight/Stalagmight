/**
 * Line-set diff helper.
 *
 * Extracted from the (deleted) stock `labelRestore.ts` so `SurveyLayer` keeps
 * the removed-line computation (used to hide stale custom labels for
 * lines a flush deleted) without depending on a stock-pipeline module.
 */

/**
 * Line ids present in `prev` but absent from `next` - the lines a flush
 * removed (delete line / delete last-shot-of-line, or undo of an add).
 * These are the ids whose stale text labels must be hidden until they reload
 * without them. Pure - unit-tested independently of the live map.
 */
export function diffRemovedLineIds(
  prev: readonly { id: number }[],
  next: readonly { id: number }[],
): number[] {
  if (prev.length === 0) return [];
  const kept = new Set<number>();
  for (const s of next) kept.add(s.id);
  const removed: number[] = [];
  for (const s of prev) if (!kept.has(s.id)) removed.push(s.id);
  return removed;
}
