/**
 * Everyone the open project credits, for the people autocomplete: the
 * surveyors and explorers of every shot of every line, once each.
 *
 * A binary-loaded line keeps its shots unmaterialized until something reads
 * `line.shots` (see `loadProjectBinary.ts`: materializing every line up
 * front is what the lazy getter exists to avoid). Such a line registers its
 * distinct credits here instead, so collecting the project's people never
 * forces its shots into existence. Results are cached per line object (and
 * per `shots` array, which a refresh replaces), so a project refresh only
 * re-reads the lines that actually changed.
 */
import type { Person, Project, Line, SurveyCredit } from '../types/ipc.generated';
import { dedupePeople } from './people';

const lazyCredits = new WeakMap<object, () => readonly SurveyCredit[]>();
const cache = new WeakMap<object, { shots: unknown; people: Person[] }>();

/**
 * Register the distinct credits of a line whose `shots` are still a lazy
 * getter. `credits` is only called when the line is read while still lazy.
 */
export function noteLazyLineCredits(line: object, credits: () => readonly SurveyCredit[]): void {
  lazyCredits.set(line, credits);
}

/** True while `line.shots` is the binary loader's unmaterialized getter. */
function isLazy(line: object): boolean {
  const descriptor = Object.getOwnPropertyDescriptor(line, 'shots');
  return descriptor !== undefined && typeof descriptor.get === 'function';
}

function peopleOf(credits: Iterable<SurveyCredit | undefined>): Person[] {
  const out: Person[] = [];
  // Binary-loaded shots share one credit object per distinct credit, so most
  // shots repeat one already seen.
  const seen = new Set<SurveyCredit>();
  for (const credit of credits) {
    if (!credit || seen.has(credit)) continue;
    seen.add(credit);
    out.push(...credit.surveyors, ...credit.explorers);
  }
  return dedupePeople(out);
}

/** The people one line credits, surveyors and explorers alike, first-seen order. */
export function linePeople(line: Pick<Line, 'shots'>): Person[] {
  const hint = isLazy(line) ? lazyCredits.get(line) : undefined;
  if (hint) {
    const cached = cache.get(line);
    if (cached && cached.shots === hint) return cached.people;
    const people = peopleOf(hint());
    cache.set(line, { shots: hint, people });
    return people;
  }
  const shots = line.shots;
  const cached = cache.get(line);
  if (cached && cached.shots === shots) return cached.people;
  const people = peopleOf(shots.map((shot) => shot.credit));
  cache.set(line, { shots, people });
  return people;
}

/** Everyone the project credits, once each, in line order. */
export function projectPeople(project: Pick<Project, 'lines'> | null | undefined): Person[] {
  if (!project || !Array.isArray(project.lines)) return [];
  return dedupePeople(project.lines.flatMap(linePeople));
}
