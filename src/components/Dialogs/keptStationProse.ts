/**
 * Prose tokens for the kept-station dialogs (RS-PROSE): Merge Stations,
 * Create Loop Closure, Detach Line from Station, Remove Closure, the
 * "whose station is this?" prompt and Edit Station. `useProse()`'s builders,
 * plus the two shapes these sentences need:
 *
 * - `identityNoId`: an identity the sentence follows with its Station ID in
 *   words ("Side / 0 (Station ID 15) stays archived inside it"), drawn
 *   without the superscript so the pill does not say the number twice.
 * - `slotName`: a slot named by the bare name the sentence says ("On Side,
 *   12 belongs to Main"): the words stay the bare name, the pill draws the
 *   slot (its line's colour, the home-line dot, the loop mark, the Station ID).
 *
 * A bare name a sentence says as a name (a rename's new name, "Side shows 12
 * twice", a name a preview gives a station that is archived now) stays text.
 */
import { useMemo } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { useProjectStore } from '../../store/projectStore';
import { identityPartOf, proseToken, proseTokens, stationPartOf, type ProsePart, type ProseTokens } from '../../utils/prose';
import { lineNameOf, type IdentitySource } from '../../utils/stationIdentity';
import type { StationIdentity } from '../../types/ipc/StationIdentity';
import type { StationRef } from '../../types/project';

export interface KeptProse extends ProseTokens {
  /** {@link ProseTokens.identity} without the Station ID superscript: the sentence says it in words. */
  identityNoId: (identity: StationIdentity, withLine?: boolean) => string;
  /** Slot `ref` by the bare name `name` the sentence says: the slot's pill, no line context. */
  slotName: (ref: StationRef, name: string) => string;
}

/** `part` without its Station ID superscript. */
function withoutStationId(part: ProsePart): ProsePart {
  if (part.kind !== 'station') return part;
  const out = { ...part };
  delete out.station_id;
  return out;
}

export function keptProse(src: IdentitySource): KeptProse {
  const lineName = (lineId: number) => lineNameOf(src, lineId);
  return {
    ...proseTokens(src),
    identityNoId: (identity, withLine) => proseToken(withoutStationId(identityPartOf(identity, lineName, withLine))),
    slotName: (ref, name) => {
      const part = stationPartOf(src, ref, false);
      return proseToken(part.kind === 'station' ? { ...part, text: name, name } : part);
    },
  };
}

/** {@link keptProse} for the open project, stable until it or its survey data changes. */
export function useKeptProse(): KeptProse {
  const { surveyData, stations, lines, closures } = useProjectStore(
    useShallow((s) => ({
      surveyData: s.surveyData,
      stations: s.stations,
      lines: s.project?.lines,
      closures: s.project?.loop_closures,
    })),
  );
  return useMemo(() => keptProse({ surveyData, stations, lines, closures }), [surveyData, stations, lines, closures]);
}
