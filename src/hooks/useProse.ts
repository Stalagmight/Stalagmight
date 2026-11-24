import { useMemo } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { useProjectStore } from '../store/projectStore';
import { proseTokens, type ProseTokens } from '../utils/prose';

/**
 * Prose tokens for the open project (RS-PROSE): `prose.station(ref, true)`
 * where a visible sentence names a station (`labels.station(ref, true)` in a
 * plain-text context), rendered by `<Prose text>`. Same source as
 * `useIdentityLabels`, stable until the project or its survey data changes.
 */
export function useProse(): ProseTokens {
  const { surveyData, stations, lines, closures } = useProjectStore(
    useShallow((s) => ({
      surveyData: s.surveyData,
      stations: s.stations,
      lines: s.project?.lines,
      closures: s.project?.loop_closures,
    })),
  );
  return useMemo(() => proseTokens({ surveyData, stations, lines, closures }), [surveyData, stations, lines, closures]);
}

/** {@link useProse} outside React (event handlers, store actions, toasts). */
export function currentProse(): ProseTokens {
  const { surveyData, stations, project } = useProjectStore.getState();
  return proseTokens({ surveyData, stations, lines: project?.lines, closures: project?.loop_closures });
}
