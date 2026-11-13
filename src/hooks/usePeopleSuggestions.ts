import { useMemo } from 'react';
import { useProjectStore } from '../store/projectStore';
import { useRecentPeople } from '../store/recentPeople';
import { useLinkedCollaborators } from '../store/speleoCollaboratorsStore';
import { projectPeople } from '../utils/projectPeople';
import type { PersonLike, Suggestion } from '../utils/people';

/**
 * The autocomplete candidates of a people field, in source order: the people
 * the open project already credits (any shot, either role), the open linked
 * project's SpeleoDB collaborators, then the people remembered from earlier
 * entries in any project. `rankSuggestions` merges one person offered by
 * several sources into one row. `extra` adds people the host knows about that
 * are not saved yet (a dialog's other field), as project people.
 */
export function usePeopleSuggestions(extra: readonly PersonLike[] = NONE): Suggestion[] {
  const project = useProjectStore((s) => s.project);
  const collaborators = useLinkedCollaborators();
  const recent = useRecentPeople();
  const inProject = useMemo(() => projectPeople(project), [project]);
  return useMemo(
    () => [
      ...inProject.map((person) => ({ person, source: 'project' as const })),
      ...extra.map((person) => ({ person, source: 'project' as const })),
      ...collaborators.map((person) => ({ person, source: 'speleodb' as const })),
      ...recent.map((person) => ({ person, source: 'recent' as const })),
    ],
    [inProject, extra, collaborators, recent],
  );
}

const NONE: readonly PersonLike[] = Object.freeze([]) as readonly PersonLike[];
