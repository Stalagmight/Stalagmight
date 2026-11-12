/**
 * The data table's optional survey-credit columns: Surveyors, Explorers and
 * Survey date, one shot per row. Hidden by default (config `panels.rs`
 * `SHOT_COLUMNS_HIDDEN_ON_ARRIVAL` hides them once for configs saved before
 * they existed).
 *
 * A cell shows compact pills (full name, email and SpeleoDB status on hover).
 * Editing opens a popover (`CreditCellEditor`): Enter, F2, Space or a double
 * click. The grid's bulk paths work as for every other column: a paste, a
 * fill-handle drag or a clear writes the whole list (or date) of each cell,
 * one `edit_shot_credit` per shot, all in one batch. On the clipboard a cell
 * is `Name <email>; Name`.
 */
import type { ReactNode } from 'react';
import type { TFunction } from 'i18next';
import type { GridColumn } from '../common/SpreadsheetGrid/types';
import type { Person, Shot } from '../../types/ipc.generated';
import { Tooltip } from '../common/Tooltip';
import { normalizeName, personKey, splitTypedNames } from '../../utils/people';
import { sanitizePerson, type CreditRole } from '../../utils/creditEdit';
import { dedupePeople } from '../../utils/people';

export const CREDIT_COLUMN_KEYS = ['surveyors', 'explorers', 'survey_date'] as const;
export type CreditColumnKey = (typeof CREDIT_COLUMN_KEYS)[number];

export function isCreditColumn(key: string): key is CreditColumnKey {
  return (CREDIT_COLUMN_KEYS as readonly string[]).includes(key);
}

/** One person as a clipboard cell writes them. */
function formatPerson(person: Person): string {
  const email = (person.email ?? '').trim();
  return email === '' ? normalizeName(person.name) : `${normalizeName(person.name)} <${email}>`;
}

/** A people cell for the clipboard: `Name <email>; Name`. */
export function formatPeopleCell(people: readonly Person[]): string {
  return people.map(formatPerson).join('; ');
}

const WITH_EMAIL = /^(.*?)\s*<([^<>]*)>$/;

/** A pasted people cell: names split on ';' (never ','), `Name <email>` keeps the email. */
export function parsePeopleCell(raw: string): Person[] {
  const people: Person[] = [];
  for (const piece of splitTypedNames(raw)) {
    const match = WITH_EMAIL.exec(piece);
    const person = match ? sanitizePerson({ name: match[1]!, email: match[2]! }) : sanitizePerson({ name: piece });
    if (person) people.push(person);
  }
  return dedupePeople(people);
}

/** A pasted date cell: '' clears, an ISO day sets, anything else is rejected (undefined). */
export function parseDateCell(raw: string): string | null | undefined {
  const text = raw.trim();
  if (text === '') return null;
  if (!/^\d{4}-\d{2}-\d{2}$/.test(text)) return undefined;
  const date = new Date(`${text}T00:00:00Z`);
  return Number.isNaN(date.getTime()) || date.toISOString().slice(0, 10) !== text ? undefined : text;
}

export interface CreditColumnContext {
  t: TFunction;
  geometryReadOnly: boolean;
  /** Grid row -> index into the section's shots; null for a draft row. */
  shotIndexAt: (gridRowIndex: number) => number | null;
  /** Labels that tell same-named people apart (`disambiguationLabels`), by person key. */
  labels: ReadonlyMap<string, string>;
  /** People known as SpeleoDB collaborators of the linked project, by person key. */
  speleoKeys: ReadonlySet<string>;
  /** Open the popover editor over this cell. */
  onOpenEditor: (rowIndex: number, colKey: CreditColumnKey) => void;
}

function PeopleCell({ people, ctx }: { people: readonly Person[]; ctx: CreditColumnContext }) {
  const { t } = ctx;
  return (
    <span className="flex h-full min-w-0 flex-nowrap items-center gap-0.5 overflow-hidden" data-testid="credit-cell-pills">
      {people.map((person) => {
        const key = personKey(person);
        const email = (person.email ?? '').trim();
        return (
          <Tooltip
            key={key}
            className="min-w-0"
            content={
              <span className="flex flex-col gap-0.5 text-xs">
                <span className="font-medium text-text-primary">{normalizeName(person.name)}</span>
                {email && <span className="text-text-secondary">{email}</span>}
                {ctx.speleoKeys.has(key) && (
                  <span className="text-text-secondary">{t('common.peopleInput.speleodbCollaborator')}</span>
                )}
              </span>
            }
          >
            <span className="inline-flex h-5 min-w-0 items-center rounded border border-accent-30 bg-accent-10 px-1 text-xs text-text-primary">
              <span className="truncate">{ctx.labels.get(key) ?? normalizeName(person.name)}</span>
            </span>
          </Tooltip>
        );
      })}
    </span>
  );
}

export function buildCreditColumns(ctx: CreditColumnContext): GridColumn<Shot>[] {
  const { t, geometryReadOnly, shotIndexAt } = ctx;
  // Not on a draft row: a shot that does not exist yet has no credit to edit,
  // and gets its neighbour's when it is created.
  const isEditable = (_row: Shot, rowIndex: number) => !geometryReadOnly && shotIndexAt(rowIndex) !== null;
  const openOnDoubleClick = (rowIndex: number, key: CreditColumnKey) => (event: { stopPropagation: () => void }) => {
    if (geometryReadOnly || shotIndexAt(rowIndex) === null) return;
    event.stopPropagation();
    ctx.onOpenEditor(rowIndex, key);
  };

  const peopleColumn = (key: CreditRole, header: string): GridColumn<Shot> => ({
    key,
    header,
    width: 160,
    kind: 'text',
    getValue: (r) => r.credit?.[key] ?? [],
    format: (value) => formatPeopleCell(Array.isArray(value) ? (value as Person[]) : []),
    parse: (raw) => parsePeopleCell(raw),
    editable: true,
    isEditable,
    noInlineEdit: true,
    activateOnEnter: true,
    activate: (_row, rowIndex) => ctx.onOpenEditor(rowIndex, key),
    render: (shot, rowIndex): ReactNode => (
      <span
        className="block h-full w-full min-w-0"
        onDoubleClick={openOnDoubleClick(rowIndex, key)}
        data-testid={`credit-cell-${key}`}
      >
        <PeopleCell people={shot.credit?.[key] ?? []} ctx={ctx} />
      </span>
    ),
  });

  return [
    peopleColumn('surveyors', t('credit.surveyors')),
    peopleColumn('explorers', t('credit.explorers')),
    {
      key: 'survey_date',
      header: t('credit.dateLabel'),
      width: 100,
      kind: 'text',
      getValue: (r) => r.credit?.date ?? '',
      parse: (raw) => parseDateCell(raw),
      editable: true,
      isEditable,
      noInlineEdit: true,
      activateOnEnter: true,
      activate: (_row, rowIndex) => ctx.onOpenEditor(rowIndex, 'survey_date'),
      render: (shot, rowIndex): ReactNode => (
        <span
          className="flex h-full w-full items-center tabular-nums"
          onDoubleClick={openOnDoubleClick(rowIndex, 'survey_date')}
          data-testid="credit-cell-survey_date"
        >
          {shot.credit?.date ?? ''}
        </span>
      ),
    },
  ];
}
