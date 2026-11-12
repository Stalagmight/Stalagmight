/**
 * Editing the survey credit of one or many shots from a form: the pending
 * edit a dialog accumulates, the backend's `CreditEdit::apply` (so the form
 * shows exactly what Save will store), and the aggregate a field displays -
 * each person with how many of the shots carry them, and the date or range.
 *
 * Pure functions only; the shots' credits are read, never written.
 */
import type { CreditEdit, Person, SurveyCredit } from '../types/ipc.generated';
import { dedupePeople, personKey, type PersonLike } from './people';
import { EMPTY_CREDIT } from './sectionCredit';

export type CreditRole = 'surveyors' | 'explorers';
export const CREDIT_ROLES: readonly CreditRole[] = ['surveyors', 'explorers'];

/** `MAX_PERSON_TEXT_CHARS` / `MAX_PEOPLE_PER_ROLE` in `crates/models/src/credit.rs`. */
const MAX_PERSON_TEXT_CHARS = 256;
const MAX_PEOPLE_PER_ROLE = 200;

/** People added to / removed from every shot, per role, and the date to set (`undefined` = untouched). */
export interface PendingCredit {
  add: Readonly<Record<CreditRole, readonly Person[]>>;
  remove: Readonly<Record<CreditRole, readonly Person[]>>;
  date?: string | null;
}

export const NO_PENDING: PendingCredit = Object.freeze({
  add: Object.freeze({ surveyors: [], explorers: [] }),
  remove: Object.freeze({ surveyors: [], explorers: [] }),
});

function clamp(text: string): string {
  return [...text].slice(0, MAX_PERSON_TEXT_CHARS).join('');
}

/** `Person::sanitized`: trimmed, clamped, empty email dropped; null for no name. */
export function sanitizePerson(person: PersonLike): Person | null {
  const name = clamp(person.name.trim());
  if (name === '') return null;
  const email = clamp((person.email ?? '').trim());
  return email === '' ? { name } : { name, email };
}

const without = (list: readonly Person[], person: PersonLike) => {
  const key = personKey(person);
  return list.filter((p) => personKey(p) !== key);
};

/** Put `person` on every shot (and stop removing them). */
export function pendAdd(pending: PendingCredit, role: CreditRole, person: PersonLike): PendingCredit {
  const clean = sanitizePerson(person);
  if (!clean) return pending;
  return {
    ...pending,
    add: { ...pending.add, [role]: dedupePeople([...pending.add[role], clean]) },
    remove: { ...pending.remove, [role]: without(pending.remove[role], clean) },
  };
}

/** Take `person` off every shot (and stop adding them). */
export function pendRemove(pending: PendingCredit, role: CreditRole, person: PersonLike): PendingCredit {
  const clean = sanitizePerson(person) ?? { name: person.name };
  return {
    ...pending,
    add: { ...pending.add, [role]: without(pending.add[role], clean) },
    remove: { ...pending.remove, [role]: dedupePeople([...pending.remove[role], clean]) },
  };
}

/** Set (a date), clear (`null`) or stop touching (`undefined`) the date of every shot. */
export function pendDate(pending: PendingCredit, date: string | null | undefined): PendingCredit {
  const { date: _old, ...rest } = pending;
  void _old;
  return date === undefined ? rest : { ...rest, date };
}

/** The pending edit as one `edit_shot_credit` payload; only what it touches. */
export function toCreditEdit(pending: PendingCredit): CreditEdit {
  const edit: CreditEdit = {};
  if (pending.add.surveyors.length > 0) edit.add_surveyors = [...pending.add.surveyors];
  if (pending.remove.surveyors.length > 0) edit.remove_surveyors = [...pending.remove.surveyors];
  if (pending.add.explorers.length > 0) edit.add_explorers = [...pending.add.explorers];
  if (pending.remove.explorers.length > 0) edit.remove_explorers = [...pending.remove.explorers];
  if (pending.date !== undefined) edit.set_date = pending.date;
  return edit;
}

/** True when the edit names no change at all. */
export function isEditEmpty(edit: CreditEdit): boolean {
  return Object.keys(edit).length === 0;
}

function editRole(
  current: readonly Person[],
  set: readonly Person[] | undefined,
  remove: readonly Person[] | undefined,
  add: readonly Person[] | undefined,
): Person[] {
  let list: Person[] = set
    ? dedupePeople(set.map(sanitizePerson).filter((p): p is Person => p !== null))
    : [...current];
  if (remove && remove.length > 0) {
    const gone = new Set(remove.map(personKey));
    list = list.filter((p) => !gone.has(personKey(p)));
  }
  if (add) {
    const have = new Set(list.map(personKey));
    for (const raw of add) {
      if (list.length >= MAX_PEOPLE_PER_ROLE) break;
      const person = sanitizePerson(raw);
      if (!person || have.has(personKey(person))) continue;
      have.add(personKey(person));
      list.push(person);
    }
  }
  return list.slice(0, MAX_PEOPLE_PER_ROLE);
}

/** The backend's `CreditEdit::apply`: per role set, then remove, then add; then the date. */
export function applyCreditEdit(before: SurveyCredit, edit: CreditEdit): SurveyCredit {
  return {
    surveyors: editRole(before.surveyors, edit.set_surveyors, edit.remove_surveyors, edit.add_surveyors),
    explorers: editRole(before.explorers, edit.set_explorers, edit.remove_explorers, edit.add_explorers),
    date: edit.set_date !== undefined ? edit.set_date : before.date,
  };
}

function samePeopleList(a: readonly Person[], b: readonly Person[]): boolean {
  return a.length === b.length && a.every((p, i) => p.name === b[i]!.name && (p.email ?? '') === (b[i]!.email ?? ''));
}

function creditsDiffer(a: SurveyCredit, b: SurveyCredit): boolean {
  return a.date !== b.date || !samePeopleList(a.surveyors, b.surveyors) || !samePeopleList(a.explorers, b.explorers);
}

/** Whether applying `edit` changes `credit` at all. */
export function editChanges(credit: SurveyCredit, edit: CreditEdit): boolean {
  return creditsDiffer(applyCreditEdit(credit, edit), credit);
}

/** A person as a field shows them across `total` shots. */
export interface AggregatedPerson {
  person: Person;
  count: number;
  total: number;
}

/** The date of several shots: one shared value (or all undated), or a range. */
export type CreditDateAggregate =
  | { kind: 'uniform'; date: string | null }
  | { kind: 'mixed'; min: string | null; max: string | null; undated: number };

export interface CreditAggregate {
  surveyors: AggregatedPerson[];
  explorers: AggregatedPerson[];
  date: CreditDateAggregate;
  total: number;
}

/** Each person of `credits` with how many carry them (first-appearance order), and the date. */
export function aggregateCredits(credits: readonly SurveyCredit[]): CreditAggregate {
  const total = credits.length;
  const tally = (role: CreditRole): AggregatedPerson[] => {
    const byKey = new Map<string, AggregatedPerson>();
    for (const credit of credits) {
      const seenHere = new Set<string>();
      for (const person of credit[role]) {
        const key = personKey(person);
        if (seenHere.has(key) || person.name.trim() === '') continue;
        seenHere.add(key);
        const entry = byKey.get(key);
        if (entry) entry.count += 1;
        else byKey.set(key, { person, count: 1, total });
      }
    }
    return [...byKey.values()];
  };
  let min: string | null = null;
  let max: string | null = null;
  let undated = 0;
  for (const { date } of credits) {
    if (!date) {
      undated += 1;
      continue;
    }
    if (min === null || date < min) min = date;
    if (max === null || date > max) max = date;
  }
  const uniform = total === 0 || undated === total || (undated === 0 && min === max);
  return {
    surveyors: tally('surveyors'),
    explorers: tally('explorers'),
    date: uniform ? { kind: 'uniform', date: undated === total ? null : min } : { kind: 'mixed', min, max, undated },
    total,
  };
}

/** The aggregate the shots would show once `edit` is applied to each, and whether any changes. */
export function previewCreditEdit(
  credits: readonly (SurveyCredit | undefined)[],
  edit: CreditEdit,
): { aggregate: CreditAggregate; changed: boolean } {
  const base = credits.map((c) => c ?? EMPTY_CREDIT);
  if (isEditEmpty(edit)) return { aggregate: aggregateCredits(base), changed: false };
  let changed = false;
  const after = base.map((credit) => {
    const next = applyCreditEdit(credit, edit);
    if (!changed && creditsDiffer(next, credit)) changed = true;
    return next;
  });
  return { aggregate: aggregateCredits(after), changed };
}
