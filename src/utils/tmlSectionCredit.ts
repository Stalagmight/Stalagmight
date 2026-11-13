/**
 * Who surveyed and explored an Ariane section, for the import preview: the
 * union of the credit of the shots that will be imported, each person with
 * how many of them carry them, and the date or date range.
 *
 * Ariane records the people per row, and the importer keeps them per shot, so
 * the shots are the truth. `TmlSection.surveyor` is only the first row's raw
 * people text; it is used (through the legacy splitter) only when no shot
 * carries a credit at all.
 */
import type { TmlSection } from '../types/commands';
import { aggregateCredits, type AggregatedPerson } from './creditEdit';
import { EMPTY_CREDIT, legacyPeople } from './sectionCredit';
import { importableShots } from './tmlImportStats';

export interface TmlSectionCredit {
  surveyors: AggregatedPerson[];
  explorers: AggregatedPerson[];
  /** One day, `first - last`, or null when no imported shot is dated. */
  date: string | null;
}

export function tmlSectionCredit(section: TmlSection, includeExcluded: boolean): TmlSectionCredit {
  const shots = importableShots(section, includeExcluded);
  const total = shots.length;
  if (shots.some((shot) => shot.credit)) {
    const aggregate = aggregateCredits(shots.map((shot) => shot.credit ?? EMPTY_CREDIT));
    const { date } = aggregate;
    return {
      surveyors: aggregate.surveyors,
      explorers: aggregate.explorers,
      date:
        date.kind === 'uniform'
          ? date.date
          : date.min === date.max
            ? date.min
            : `${date.min ?? ''} - ${date.max ?? ''}`,
    };
  }
  const legacy = legacyPeople(section.surveyor ?? '');
  const everyShot = (person: AggregatedPerson['person']): AggregatedPerson => ({ person, count: total, total });
  return {
    surveyors: legacy.surveyors.map(everyShot),
    explorers: legacy.explorers.map(everyShot),
    date: section.date,
  };
}
