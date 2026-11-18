import { useMemo } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { useProjectStore } from '../store/projectStore';
import { identityLabels, type IdentityLabels } from '../utils/stationIdentity';

/**
 * Station, shot and line labels for the open project (SPEC 4.1/4.2): station
 * `17` / `Main Line / 17`, shot `4 → 5` / `Main Line: 4 → 5`, a line by its
 * name. The returned functions are stable until the project or its survey data
 * changes, so they are safe in dependency arrays.
 */
export function useIdentityLabels(): IdentityLabels {
  const { surveyData, stations, lines } = useProjectStore(
    useShallow((s) => ({ surveyData: s.surveyData, stations: s.stations, lines: s.project?.lines }))
  );
  return useMemo(() => identityLabels({ surveyData, stations, lines }), [surveyData, stations, lines]);
}

/** {@link useIdentityLabels} outside React (event handlers, store actions). */
export function currentIdentityLabels(): IdentityLabels {
  const { surveyData, stations, project } = useProjectStore.getState();
  return identityLabels({ surveyData, stations, lines: project?.lines });
}
