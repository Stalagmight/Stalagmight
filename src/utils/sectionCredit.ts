/**
 * Per-shot survey credit on the frontend: who surveyed and explored each shot,
 * and when. Mirrors the Rust model in `src-tauri/crates/models/src/credit.rs`.
 *
 * Every shot carries its own `credit` (absent = none); a section holds none of
 * its own and only aggregates its shots, exactly as `Section::credit_summary`
 * does on the backend.
 */
import type {
  CreditedPerson,
  Person,
  Section,
  SectionCreditSummary,
  Shot,
  SurveyCredit,
} from '../types/ipc.generated';
import { dedupePeople, isValidPerson, personKey } from './people';

/** The credit of a shot that has none. Frozen: never mutate it. */
export const EMPTY_CREDIT: SurveyCredit = Object.freeze({
  surveyors: [],
  explorers: [],
  date: null,
}) as unknown as SurveyCredit;

/** A shot's credit, or the empty credit when it has none. */
export function shotCredit(shot: Pick<Shot, 'credit'>): SurveyCredit {
  return shot.credit ?? EMPTY_CREDIT;
}

/** True when a credit names nobody and no date. */
export function isCreditEmpty(credit: SurveyCredit | undefined | null): boolean {
  return !credit || (credit.surveyors.length === 0 && credit.explorers.length === 0 && credit.date === null);
}

/**
 * Person identity lives in `utils/people` (one implementation, the backend's
 * `Person::same_as`); re-exported so credit code keeps one import.
 */
export { dedupePeople, personKey, samePerson } from './people';

/** Aggregate a run of shot credits; mirrors `SectionCreditSummary::from_credits`. */
export function summarizeCredits(credits: Iterable<SurveyCredit>): SectionCreditSummary {
  const surveyors = new Map<string, CreditedPerson>();
  const explorers = new Map<string, CreditedPerson>();
  let dateMin: string | null = null;
  let dateMax: string | null = null;
  let shotCount = 0;
  const tally = (acc: Map<string, CreditedPerson>, people: readonly Person[]): void => {
    const seenHere = new Set<string>();
    for (const person of people) {
      if (!isValidPerson(person)) continue;
      const key = personKey(person);
      if (seenHere.has(key)) continue;
      seenHere.add(key);
      const entry = acc.get(key);
      if (entry) entry.shots += 1;
      else acc.set(key, { person, shots: 1 });
    }
  };
  for (const credit of credits) {
    shotCount += 1;
    tally(surveyors, credit.surveyors);
    tally(explorers, credit.explorers);
    // ISO `YYYY-MM-DD` dates order lexicographically.
    if (credit.date) {
      if (dateMin === null || credit.date < dateMin) dateMin = credit.date;
      if (dateMax === null || credit.date > dateMax) dateMax = credit.date;
    }
  }
  return {
    surveyors: [...surveyors.values()],
    explorers: [...explorers.values()],
    date_min: dateMin,
    date_max: dateMax,
    shot_count: shotCount,
  };
}

/**
 * Who surveyed and explored a section and when, aggregated over its shots:
 * each person with how many shots credit them (first-appearance order), the
 * earliest and latest date, and the shot count. Reads `section.shots`, so on a
 * binary-loaded project it materializes that one section's shots.
 */
export function sectionCreditSummary(section: Pick<Section, 'shots'>): SectionCreditSummary {
  return summarizeCredits(section.shots.map(shotCredit));
}

/** Names of `people` (or credited people) joined with `separator`; '' for none. */
export function formatPeople(
  people: readonly Person[] | readonly CreditedPerson[],
  separator = ', ',
): string {
  return people
    .map((p) => ('person' in p ? p.person.name : p.name))
    .join(separator);
}

/** A summary's date as text: one day, `first - last`, or null when undated. */
export function formatCreditDateRange(summary: Pick<SectionCreditSummary, 'date_min' | 'date_max'>): string | null {
  if (!summary.date_min) return null;
  if (!summary.date_max || summary.date_max === summary.date_min) return summary.date_min;
  return `${summary.date_min} - ${summary.date_max}`;
}

/**
 * The section's surveyors as ONE editable line for the legacy free-text
 * Surveyor inputs, joined with `"; "` so the backend's legacy splitter reads
 * the same people back (a `", "` join of two one-word names would read as one
 * `Last, First` person). Empty when no shot names a surveyor.
 */
export function sectionSurveyorText(section: Pick<Section, 'shots'>): string {
  return formatPeople(sectionCreditSummary(section).surveyors, '; ');
}

/** The section's earliest survey date (ISO), or '' when no shot has one. */
export function sectionFirstDate(section: Pick<Section, 'shots'>): string {
  return sectionCreditSummary(section).date_min ?? '';
}

const PLACEHOLDERS = new Set(['unknown', 'none', '?']);

/** Split one role's free text the way the backend's `split_people` does. */
export function splitPeople(half: string): Person[] {
  const text = half.trim();
  const out: Person[] = [];
  const push = (piece: string): void => {
    const name = piece.trim();
    if (!name || PLACEHOLDERS.has(name.toLowerCase())) return;
    out.push({ name });
  };
  const parts = text.split(',').map((p) => p.trim());
  const lastFirst = !text.includes(';') && parts.length === 2 && parts.every((p) => p !== '' && !/\s/.test(p));
  if (lastFirst) push(text);
  else for (const piece of text.split(';')) for (const name of piece.split(',')) push(name);
  return dedupePeople(out);
}

/**
 * The `[explorers, surveyors]` halves of legacy text in the Ariane form, or
 * null for text that names surveyors only, exactly as the backend's
 * `credit::legacy_halves`: two `" / "` halves (the Ariane importer's join),
 * or no `" / "` and a text ending in `" /"` once trimmed (explorers only,
 * `"Ida Ice /"`, which survives the trim an older build's Edit Section does).
 */
export function legacyHalves(text: string): [string, string] | null {
  const halves = text.split(' / ');
  if (halves.length === 2) return [halves[0]!, halves[1]!];
  if (halves.length === 1) {
    const trimmed = text.trimEnd();
    if (trimmed.endsWith(' /')) return [trimmed.slice(0, -2), ''];
  }
  return null;
}

/**
 * Split a legacy free-text surveyor into explorers and surveyors, exactly as
 * the backend's `credit::legacy_people` does: text in the Ariane form
 * ({@link legacyHalves}) is explorers then surveyors; anything else is
 * surveyors only.
 */
export function legacyPeople(text: string): { explorers: Person[]; surveyors: Person[] } {
  const halves = legacyHalves(text);
  if (halves) {
    return { explorers: splitPeople(halves[0]), surveyors: splitPeople(halves[1]) };
  }
  return { explorers: [], surveyors: splitPeople(text) };
}
