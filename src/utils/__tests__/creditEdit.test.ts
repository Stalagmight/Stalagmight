/**
 * utils/creditEdit: the TS mirror of `CreditEdit::apply` and the aggregate a
 * credit field shows over several shots (partial people, a date or a range).
 */
import { describe, expect, it } from 'vitest';
import {
  NO_PENDING,
  aggregateCredits,
  applyCreditEdit,
  pendAdd,
  pendDate,
  pendRemove,
  previewCreditEdit,
  toCreditEdit,
} from '../creditEdit';
import type { SurveyCredit } from '../../types/ipc.generated';

const credit = (surveyors: string[], explorers: string[] = [], date: string | null = null): SurveyCredit => ({
  surveyors: surveyors.map((name) => ({ name })),
  explorers: explorers.map((name) => ({ name })),
  date,
});

describe('applyCreditEdit (CreditEdit::apply)', () => {
  it('sets, then removes, then adds, by identity, keeping order', () => {
    const after = applyCreditEdit(credit(['Ann', 'Bo']), {
      set_surveyors: [{ name: 'Cy' }, { name: 'Ann' }, { name: 'ann ' }],
      remove_surveyors: [{ name: 'CY' }],
      add_surveyors: [{ name: 'Bo' }, { name: 'ann' }],
    });
    expect(after.surveyors).toEqual([{ name: 'Ann' }, { name: 'Bo' }]);
  });

  it('trims, drops nameless people and empty emails, clamps long text', () => {
    const long = 'x'.repeat(300);
    const after = applyCreditEdit(credit([]), {
      add_explorers: [{ name: '  Ann  ', email: '  ' }, { name: '   ' }, { name: long }],
    });
    expect(after.explorers[0]).toEqual({ name: 'Ann' });
    expect(after.explorers).toHaveLength(2);
    expect(after.explorers[1]!.name).toHaveLength(256);
  });

  it('sets, clears or keeps the date', () => {
    const base = credit(['Ann'], [], '2026-03-02');
    expect(applyCreditEdit(base, { set_date: '2026-04-01' }).date).toBe('2026-04-01');
    expect(applyCreditEdit(base, { set_date: null }).date).toBeNull();
    expect(applyCreditEdit(base, {}).date).toBe('2026-03-02');
  });

  it('caps a role at 200 people', () => {
    const many = Array.from({ length: 250 }, (_, i) => ({ name: `P${i}` }));
    expect(applyCreditEdit(credit([]), { add_surveyors: many }).surveyors).toHaveLength(200);
  });
});

describe('aggregateCredits', () => {
  it('counts each person once per shot, in first-appearance order', () => {
    const agg = aggregateCredits([credit(['Ann', 'ann']), credit(['Bo', 'Ann']), credit([], ['Ann'])]);
    expect(agg.total).toBe(3);
    expect(agg.surveyors.map((p) => [p.person.name, p.count, p.total])).toEqual([
      ['Ann', 2, 3],
      ['Bo', 1, 3],
    ]);
    expect(agg.explorers.map((p) => [p.person.name, p.count])).toEqual([['Ann', 1]]);
  });

  it('reports one shared date, no date, or the range with the undated count', () => {
    expect(aggregateCredits([credit([], [], '2026-03-02'), credit([], [], '2026-03-02')]).date).toEqual({
      kind: 'uniform',
      date: '2026-03-02',
    });
    expect(aggregateCredits([credit([]), credit([])]).date).toEqual({ kind: 'uniform', date: null });
    expect(aggregateCredits([credit([], [], '2026-04-11'), credit([], [], '2026-03-02'), credit([])]).date).toEqual({
      kind: 'mixed',
      min: '2026-03-02',
      max: '2026-04-11',
      undated: 1,
    });
    expect(aggregateCredits([credit([], [], '2026-03-02'), credit([])]).date.kind).toBe('mixed');
  });
});

describe('pending edits', () => {
  it('add and remove cancel each other out per role and become one edit', () => {
    let p = pendAdd(NO_PENDING, 'surveyors', { name: 'Ann' });
    p = pendAdd(p, 'explorers', { name: 'Bo' });
    p = pendRemove(p, 'surveyors', { name: 'ann' });
    p = pendDate(p, '2026-03-02');
    expect(toCreditEdit(p)).toEqual({
      remove_surveyors: [{ name: 'ann' }],
      add_explorers: [{ name: 'Bo' }],
      set_date: '2026-03-02',
    });
    expect(toCreditEdit(pendDate(p, undefined))).not.toHaveProperty('set_date');
  });

  it('previews the shots after the edit and says whether anything changes', () => {
    const shots = [credit(['Ann']), credit(['Bo'])];
    const promote = previewCreditEdit(shots, toCreditEdit(pendAdd(NO_PENDING, 'surveyors', { name: 'Ann' })));
    expect(promote.changed).toBe(true);
    expect(promote.aggregate.surveyors.map((p) => [p.person.name, p.count])).toEqual([
      ['Ann', 2],
      ['Bo', 1],
    ]);
    // Removing someone nobody carries changes nothing.
    expect(previewCreditEdit(shots, toCreditEdit(pendRemove(NO_PENDING, 'surveyors', { name: 'Cy' }))).changed).toBe(false);
  });
});
