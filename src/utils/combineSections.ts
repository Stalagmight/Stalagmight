import { invokeIpc } from './ipc';
import type { SectionId, StationRef } from '../types/project';
import type { CombineCandidate } from '../types/ipc/CombineCandidate';
import type { CombinePlan } from '../types/ipc/CombinePlan';

/**
 * Combine Sections: the read-only queries the context menu and the Sections
 * panel ask before offering the action, and the dialog's defaults.
 */

/**
 * What the section ending at `station` can be combined with: one candidate per
 * other section ending there (a junction lists several, each with its own
 * reason when blocked). Empty: nothing to offer.
 */
export function queryCombineCandidates(station: StationRef): Promise<CombineCandidate[]> {
  return invokeIpc<CombineCandidate[]>('combine_candidate', { station }, {
    purpose: 'check whether sections can be combined at a station',
    fields: { station },
  });
}

/** Can sections `a` and `b` be combined? `null` when either is missing. */
export function queryCombineCandidateForSections(a: SectionId, b: SectionId): Promise<CombineCandidate | null> {
  return invokeIpc<CombineCandidate | null>('combine_candidate_for_sections', { a, b }, {
    purpose: 'check whether two sections can be combined',
    fields: { a, b },
  });
}

/**
 * The combined section's default name: the kept section's, except that the two
 * halves of a split (`X-A` and `X-B`, either kept) give back `X`.
 */
export function defaultCombinedName(keptName: string, otherName: string): string {
  const half = /^(.*)-([AB])$/;
  const kept = half.exec(keptName);
  const other = half.exec(otherName);
  if (kept && other && kept[1] === other[1] && kept[2] !== other[2] && kept[1]!.trim().length > 0) {
    return kept[1]!;
  }
  return keptName;
}

/** What the map previews while the dialog is open: both sections, and the join ringed. */
export function combinePreview(candidate: CombineCandidate): { sections: SectionId[]; stations: StationRef[] } {
  return combinePreviewAll([candidate]);
}

/**
 * The map preview for a picker of candidates at one junction: every section
 * involved, the join, and the stations of every tie the combine would turn
 * into a loop closure or remove (#746), and of every closure it would
 * replace with what takes its place (#749), for either kept section.
 */
export function combinePreviewAll(candidates: CombineCandidate[]): { sections: SectionId[]; stations: StationRef[] } {
  const sections = new Set<SectionId>();
  const stations = new Map<string, StationRef>();
  const ring = (s: StationRef) => stations.set(`${s.section_id}:${s.station_index}`, s);
  for (const c of candidates) {
    sections.add(c.section_a);
    sections.add(c.section_b);
    [c.station_a, c.station_b].forEach(ring);
    for (const plan of [c.plan_keeping_a, c.plan_keeping_b]) {
      for (const tie of [...(plan?.torn_ties ?? []), ...(plan?.closures_removed_across_one_shot ?? [])]) {
        [tie.station_a, tie.station_b].forEach(ring);
      }
      for (const replaced of plan?.closures_replaced ?? []) {
        [replaced.station_a, replaced.station_b, replaced.by_station_a, replaced.by_station_b].forEach(ring);
      }
    }
  }
  return { sections: [...sections], stations: [...stations.values()] };
}

/** What changes beyond the join when `kept` is the kept section. */
export function combinePlanFor(candidate: CombineCandidate, kept: SectionId): CombinePlan {
  return kept === candidate.section_b ? candidate.plan_keeping_b : candidate.plan_keeping_a;
}

/**
 * How far apart (metres) two section ends may be and still be the same point,
 * side by side: the backend's `COMBINE_TOLERANCE_M`, which a tripwire test
 * (`src/test/__tests__/combineTolerance.test.ts`) keeps this equal to.
 */
export const COMBINE_SIDE_BY_SIDE_M = 0.001;

/**
 * Does the dialog list the plan's move? A side-by-side join moves a side by
 * its residual, under 1 mm (#740): the plan names that side, what moves with
 * it and why, exactly. On its own a move that small is noise next to "Side by
 * side here", so the dialog lists it only when it changes a tie: a snap from
 * the moving side made ordinary, or (both sides held by zero datums) the
 * moving section's merge torn. A snap's why is the move, so the move is
 * listed too (`combineMoveIsSmall` picks its wording); a tie torn for a zero
 * datum says which section the datum holds, and tears with no move at all
 * when the two ends are one point (#754), so its why is the datum, not the
 * move. A move past the tolerance is always listed; with no moving section
 * there is none to list.
 */
export function combineMoveShown(plan: CombinePlan): boolean {
  return plan.moving_section !== null && (plan.shift_m > COMBINE_SIDE_BY_SIDE_M || plan.torn_ties.length > 0);
}

/** A listed move within the side-by-side tolerance (#740), shown to 0.1 mm. */
export function combineMoveIsSmall(plan: CombinePlan): boolean {
  return plan.shift_m <= COMBINE_SIDE_BY_SIDE_M;
}

/**
 * A signed angle for the dialog: "+5.0°", "-0.3°", and a small one to three
 * decimals ("+0.040°") so a real rotation never reads as zero.
 */
export function formatRotation(degrees: number): string {
  const digits = Math.abs(degrees) < 0.05 ? 3 : 1;
  return `${degrees > 0 ? '+' : degrees < 0 ? '-' : ''}${Math.abs(degrees).toFixed(digits)}°`;
}
