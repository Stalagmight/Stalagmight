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
 * into a loop closure (for either kept section).
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
      for (const tie of plan?.torn_ties ?? []) [tie.station_a, tie.station_b].forEach(ring);
    }
  }
  return { sections: [...sections], stations: [...stations.values()] };
}

/** What changes beyond the join when `kept` is the kept section. */
export function combinePlanFor(candidate: CombineCandidate, kept: SectionId): CombinePlan {
  return kept === candidate.section_b ? candidate.plan_keeping_b : candidate.plan_keeping_a;
}

/** A signed angle for the dialog: "+5.0°", "-0.3°". */
export function formatRotation(degrees: number): string {
  return `${degrees > 0 ? '+' : degrees < 0 ? '-' : ''}${Math.abs(degrees).toFixed(1)}°`;
}
