import { describe, expect, it } from 'vitest';
import type { Person, Shot, SurveyCredit } from '../../types/ipc.generated';
import {
  EMPTY_CREDIT,
  dedupePeople,
  formatCreditDateRange,
  formatPeople,
  isCreditEmpty,
  legacyPeople,
  samePerson,
  sectionCreditSummary,
  sectionFirstDate,
  sectionSurveyorText,
  shotCredit,
} from '../sectionCredit';

const p = (name: string, email?: string): Person => (email === undefined ? { name } : { name, email });
const credit = (surveyors: string[], explorers: string[] = [], date: string | null = null): SurveyCredit => ({
  surveyors: surveyors.map((n) => p(n)),
  explorers: explorers.map((n) => p(n)),
  date,
});
const shot = (c?: SurveyCredit): Pick<Shot, 'credit'> => (c === undefined ? {} : { credit: c });
const names = (list: Person[]): string[] => list.map((x) => x.name);

describe('sectionCredit', () => {
  it('reads an absent credit as empty', () => {
    expect(shotCredit(shot())).toBe(EMPTY_CREDIT);
    expect(isCreditEmpty(shotCredit(shot()))).toBe(true);
    expect(isCreditEmpty(credit(['A']))).toBe(false);
  });

  it('matches people on casefolded name and lowercased email', () => {
    expect(samePerson(p('John  Doe'), p(' john doe '))).toBe(true);
    expect(samePerson(p('John'), p('John', 'j@x.org'))).toBe(false);
    expect(samePerson(p('John', 'J@X.org'), p('john', 'j@x.ORG'))).toBe(true);
    expect(names(dedupePeople([p('B'), p('b'), p('  '), p('A')]))).toEqual(['B', 'A']);
  });

  it('aggregates a section like the backend summary', () => {
    const section = {
      shots: [
        shot(credit(['B', 'A', 'b'], [], '2024-01-05')),
        shot(credit(['a'], ['E'], '2023-12-30')),
        shot(),
      ] as Shot[],
    };
    const summary = sectionCreditSummary(section);
    expect(summary.shot_count).toBe(3);
    expect(summary.surveyors.map((c) => [c.person.name, c.shots])).toEqual([
      ['B', 1],
      ['A', 2],
    ]);
    expect(summary.explorers).toHaveLength(1);
    expect(summary.date_min).toBe('2023-12-30');
    expect(summary.date_max).toBe('2024-01-05');
    expect(formatPeople(summary.surveyors)).toBe('B, A');
    expect(formatCreditDateRange(summary)).toBe('2023-12-30 - 2024-01-05');
    expect(formatCreditDateRange({ date_min: '2024-01-01', date_max: '2024-01-01' })).toBe('2024-01-01');
    expect(formatCreditDateRange({ date_min: null, date_max: null })).toBeNull();
    // The editable legacy line joins with "; " so the splitter reads two people back.
    expect(sectionSurveyorText(section)).toBe('B; A');
    expect(sectionFirstDate(section)).toBe('2023-12-30');
    expect(sectionFirstDate({ shots: [] })).toBe('');
  });

  it('splits legacy text exactly like the backend', () => {
    const cases: Array<[string, string[], string[]]> = [
      ['nh / ds', ['nh'], ['ds']],
      ['Doe, John', [], ['Doe, John']],
      ['A, B; C', [], ['A', 'B', 'C']],
      ['unknown / ds', [], ['ds']],
      ['?', [], []],
      ['A, a, B', [], ['A', 'B']],
      ['a / b / c', [], ['a / b / c']],
      ['B; A', [], ['B', 'A']],
    ];
    for (const [text, explorers, surveyors] of cases) {
      const split = legacyPeople(text);
      expect(names(split.explorers), text).toEqual(explorers);
      expect(names(split.surveyors), text).toEqual(surveyors);
    }
  });
});
