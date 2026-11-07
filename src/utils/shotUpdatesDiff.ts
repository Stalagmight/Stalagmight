/**
 * [#576 F32-EDIT] Edit paths send only the shot fields that changed.
 *
 * The project the frontend holds is decoded from the binary envelope, which
 * carries every shot reading as an f32 (see `core::load_binary`, the PRECISION
 * note): a length of 1.2 reaches the store as 1.2000000476837158. An edit path
 * that re-sends the whole row therefore writes that rounded value back over the
 * exact f64 the backend still holds for every field the user never touched, and
 * the noise is saved with the file. Sending only what differs from the store's
 * own value leaves every untouched field to the backend's copy, bit for bit.
 */
import type { ShotUpdates } from '../types/ipc/ShotUpdates';
import { sameF32 } from './f32Text';

/** A shot's fields as the store (or an editor's local copy) holds them. */
type ShotFields = { readonly [K in keyof ShotUpdates]?: ShotUpdates[K] };

/**
 * How a proposed number is compared with the store's:
 * - `exact`: identical only. For values a path computes or takes from a file.
 * - `f32` ([#576 NF-W7F-1]): the same once rounded to f32, the precision the store
 *   holds. For values the user TYPED into an editor seeded with the store's value:
 *   1.20 typed over a stored 1.2 is the reading the user already had, not an edit.
 *   By design a typed value that differs only beyond f32 precision (about the
 *   8th significant digit: 12.3000001 over 12.3) is unchanged too: the store
 *   cannot show that difference, so the editors cannot express it, and survey
 *   readings never carry it ([#576 RV8b-2]). A difference at the 7th significant
 *   digit (12.30001) is an edit.
 */
export type ShotFieldCompare = 'exact' | 'f32';

/** Same value for patch purposes: identical (NaN included), or both unset. */
function sameValue(a: unknown, b: unknown, compare: ShotFieldCompare): boolean {
  if (Object.is(a, b) || (a == null && b == null)) return true;
  return compare === 'f32' && typeof a === 'number' && typeof b === 'number' && sameF32(a, b);
}

/**
 * The entries of `proposed` whose value differs from `current`'s, i.e. the patch
 * that changes exactly those fields. An empty result means nothing would change:
 * callers skip the command rather than record an empty undo step.
 */
export function changedShotFields(
  current: ShotFields,
  proposed: Partial<ShotUpdates>,
  compare: ShotFieldCompare = 'exact',
): Partial<ShotUpdates> {
  const patch: Partial<ShotUpdates> = {};
  for (const key of Object.keys(proposed) as (keyof ShotUpdates)[]) {
    const next = proposed[key];
    if (next === undefined || sameValue(current[key], next, compare)) continue;
    (patch as Record<string, unknown>)[key] = next;
  }
  return patch;
}
