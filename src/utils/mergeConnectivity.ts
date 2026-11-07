import type { ConnectivityResult } from '../types/commands';

/**
 * [#576 NF-W7E-1] Where a refused merge goes: a pair that is already connected,
 * or whose two merge groups are both fixed by a zero datum, is offered as a loop
 * closure instead; any other refusal is reported (the refusal toast). Shared by
 * the merge tool (`useMapTools`) and the move-onto-station snap (`useMoveDrag`).
 *
 * Decided on `reason_code`. The two call sites used to compare `reason` - display
 * text - with the English line "Both sections have fixed coordinates", so a
 * reworded message would have sent a fixed pair to the toast without a word.
 */
export function refusedMergeOffersClosure(
  result: Pick<ConnectivityResult, 'connected' | 'reason_code'>,
): boolean {
  return result.connected || result.reason_code === 'BOTH_FIXED_COORDINATES';
}
