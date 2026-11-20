/**
 * Everyone the open project credits, for the people autocomplete: the
 * surveyors and explorers of every shot of every section, once each.
 *
 * A binary-loaded section keeps its shots unmaterialized until something reads
 * `section.shots` (see `loadProjectBinary.ts`: materializing every section up
 * front is what the lazy getter exists to avoid). Such a section registers its
 * distinct credits here instead, so collecting the project's people never
 * forces its shots into existence. Results are cached per section object (and
 * per `shots` array, which a refresh replaces), so a project refresh only
 * re-reads the sections that actually changed.
 */
import type { Person, Project, Section, SurveyCredit } from '../types/ipc.generated';
import { dedupePeople } from './people';

const lazyCredits = new WeakMap<object, () => readonly SurveyCredit[]>();
const cache = new WeakMap<object, { shots: unknown; people: Person[] }>();

/**
 * Register the distinct credits of a section whose `shots` are still a lazy
 * getter. `credits` is only called when the section is read while still lazy.
 */
export function noteLazySectionCredits(section: object, credits: () => readonly SurveyCredit[]): void {
  lazyCredits.set(section, credits);
}

/** True while `section.shots` is the binary loader's unmaterialized getter. */
function isLazy(section: object): boolean {
  const descriptor = Object.getOwnPropertyDescriptor(section, 'shots');
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

/**
 * True when a still-lazy section is known to credit nobody (no distinct credit
 * at all, people or date), so a per-shot credit read can skip materializing
 * its shots. False whenever that is not known for sure.
 */
export function lazySectionHasNoCredits(section: object): boolean {
  const hint = isLazy(section) ? lazyCredits.get(section) : undefined;
  return hint !== undefined && hint().length === 0;
}

/** The people one section credits, surveyors and explorers alike, first-seen order. */
export function sectionPeople(section: Pick<Section, 'shots'>): Person[] {
  const hint = isLazy(section) ? lazyCredits.get(section) : undefined;
  if (hint) {
    const cached = cache.get(section);
    if (cached && cached.shots === hint) return cached.people;
    const people = peopleOf(hint());
    cache.set(section, { shots: hint, people });
    return people;
  }
  const shots = section.shots;
  const cached = cache.get(section);
  if (cached && cached.shots === shots) return cached.people;
  const people = peopleOf(shots.map((shot) => shot.credit));
  cache.set(section, { shots, people });
  return people;
}

/** Everyone the project credits, once each, in section order. */
export function projectPeople(project: Pick<Project, 'sections'> | null | undefined): Person[] {
  if (!project || !Array.isArray(project.sections)) return [];
  return dedupePeople(project.sections.flatMap(sectionPeople));
}
