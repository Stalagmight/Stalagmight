import { invokeIpc } from './ipc';
import type { SectionId, StationRef } from '../types/project';
import type { CombineCandidate } from '../types/ipc/CombineCandidate';

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

/** The map preview for a picker of candidates at one junction: every section involved, and the join. */
export function combinePreviewAll(candidates: CombineCandidate[]): { sections: SectionId[]; stations: StationRef[] } {
  const sections = new Set<SectionId>();
  const stations = new Map<string, StationRef>();
  for (const c of candidates) {
    sections.add(c.section_a);
    sections.add(c.section_b);
    for (const s of [c.station_a, c.station_b]) stations.set(`${s.section_id}:${s.station_index}`, s);
  }
  return { sections: [...sections], stations: [...stations.values()] };
}
