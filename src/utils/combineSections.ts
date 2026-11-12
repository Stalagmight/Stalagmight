import { invokeIpc } from './ipc';
import type { StationRef } from '../types/project';
import type { CombineCandidate } from '../types/ipc/CombineCandidate';
import type { CombinePairing } from '../types/ipc/CombinePairing';

/** Whether sections `a` and `b` can be combined (#707); null when no end of one meets an end of the other. */
export const combineCandidate = (a: number, b: number) =>
  invokeIpc<CombineCandidate | null>('combine_candidate', { a, b }, { purpose: 'check combine sections', fields: { a, b } });

/** The combine offered at a section-end station; null unless exactly one other section's end meets it. */
export const combineCandidateAtStation = (station: StationRef) =>
  invokeIpc<CombineCandidate | null>('combine_candidate_at_station', { station }, {
    purpose: 'check combine sections at station',
    fields: { station },
  });

const SWAPPED_PAIRING: Record<CombinePairing, CombinePairing> = {
  kept_end_other_start: 'kept_start_other_end',
  kept_start_other_end: 'kept_end_other_start',
  kept_end_other_end: 'kept_end_other_end',
  kept_start_other_start: 'kept_start_other_start',
};

/** The same join seen from the other section, which is kept instead. */
export function swapCandidate(candidate: CombineCandidate): CombineCandidate {
  return { ...candidate, kept: candidate.other, other: candidate.kept, pairing: SWAPPED_PAIRING[candidate.pairing] };
}

/** Whether the other section's shots go in front of the kept section's. */
export const prependsOther = (pairing: CombinePairing) => pairing.startsWith('kept_start');

/** The kept name, or `X` when the two are a split's `X-A` and `X-B`. */
export function defaultCombinedName(kept: string, other: string): string {
  const base = kept.slice(0, -2);
  const halves = [kept, other].sort();
  return base && halves[0] === `${base}-A` && halves[1] === `${base}-B` ? base : kept;
}
