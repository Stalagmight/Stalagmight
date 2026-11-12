/**
 * The data table's survey-credit columns, wired: the column definitions, the
 * popover editor over the active credit cell, and type-to-edit (a printable
 * key on a credit cell opens the popover seeded with it, instead of writing a
 * one-letter person). Kept out of `DataTable.tsx`, which is past its size
 * ratchet. See `dataTableCreditColumns.tsx` for the cell semantics.
 */
import { useCallback, useMemo, useState, type KeyboardEvent, type ReactNode } from 'react';
import type { TFunction } from 'i18next';
import { useProjectStore } from '../../store/projectStore';
import { usePeopleSuggestions } from '../../hooks/usePeopleSuggestions';
import { disambiguationLabels, personKey } from '../../utils/people';
import { setSource } from '../../utils/telemetry';
import type { GridColumn } from '../common/SpreadsheetGrid/types';
import type { CreditEdit, Shot } from '../../types/ipc.generated';
import { buildCreditColumns, isCreditColumn, type CreditColumnKey } from './dataTableCreditColumns';
import { CreditCellEditor } from './CreditCellEditor';
import { creditEditForCells } from './dataTableEdits';

interface Options {
  t: TFunction;
  sectionId: number | null;
  /** Grid rows (drafts materialized), by grid row index. */
  rows: readonly Shot[];
  shotIndexAt: (gridRowIndex: number) => number | null;
  editingBlocked: boolean;
  /** The grid's active cell, for type-to-edit. */
  active: { row: number; colKey: string } | null;
}

export interface DataTableCredit {
  columns: GridColumn<Shot>[];
  /** The open popover, if any. */
  editor: ReactNode;
  /** On the grid's wrapper: a printable key on a credit cell opens its popover. */
  onKeyDownCapture: (event: KeyboardEvent<HTMLElement>) => void;
  /** A single grid edit of a credit cell (a one-cell paste or clear). */
  applyCell: (shot: Shot, colKey: string, value: unknown) => void;
}

export function useDataTableCredit({ t, sectionId, rows, shotIndexAt, editingBlocked, active }: Options): DataTableCredit {
  const editShotCredit = useProjectStore((s) => s.editShotCredit);
  const [editing, setEditing] = useState<{ shotId: number; field: CreditColumnKey; seed: string } | null>(null);

  const suggestions = usePeopleSuggestions();
  const { labels, speleoKeys } = useMemo(() => {
    const people = suggestions.map((s) => s.person);
    return {
      labels: disambiguationLabels(people, { noEmail: t('common.peopleInput.noEmail') }),
      speleoKeys: new Set(suggestions.filter((s) => s.source === 'speleodb').map((s) => personKey(s.person))),
    };
  }, [suggestions, t]);

  const open = useCallback(
    (rowIndex: number, field: CreditColumnKey, seed = '') => {
      const shot = rows[rowIndex];
      if (editingBlocked || !shot || shotIndexAt(rowIndex) === null) return;
      setEditing({ shotId: shot.id, field, seed });
    },
    [rows, editingBlocked, shotIndexAt],
  );

  const columns = useMemo(
    () =>
      buildCreditColumns({
        t,
        geometryReadOnly: editingBlocked,
        shotIndexAt,
        labels,
        speleoKeys,
        onOpenEditor: (rowIndex, field) => open(rowIndex, field),
      }),
    [t, editingBlocked, shotIndexAt, labels, speleoKeys, open],
  );

  const send = useCallback(
    (shotId: number, edit: CreditEdit) => {
      if (sectionId === null || editingBlocked) return;
      setSource('data-table');
      void editShotCredit({ type: 'shots', shots: [[sectionId, shotId]] }, edit);
    },
    [sectionId, editingBlocked, editShotCredit],
  );

  const applyCell = useCallback(
    (shot: Shot, colKey: string, value: unknown) => {
      const edit = creditEditForCells(shot, [{ colKey, value }]);
      if (edit) send(shot.id, edit);
    },
    [send],
  );

  const onKeyDownCapture = useCallback(
    (event: KeyboardEvent<HTMLElement>) => {
      if (editing || !active || !isCreditColumn(active.colKey)) return;
      if (event.ctrlKey || event.metaKey || event.altKey || event.key.length !== 1 || event.key === ' ') return;
      if (event.nativeEvent.isComposing) return;
      // Only from the grid itself, never from an input inside it.
      const target = event.target as HTMLElement;
      if (target.closest('input, textarea, [contenteditable="true"]')) return;
      event.preventDefault();
      event.stopPropagation();
      open(active.row, active.colKey, active.colKey === 'survey_date' ? '' : event.key);
    },
    [editing, active, open],
  );

  const shot = editing ? rows.find((r) => r.id === editing.shotId) : undefined;
  const editor =
    editing && shot && sectionId !== null ? (
      <CreditCellEditor
        key={`${editing.shotId}:${editing.field}`}
        shot={shot}
        field={editing.field}
        seed={editing.seed}
        anchor={document.querySelector<HTMLElement>(
          `[data-testid="shot-cell-${sectionId}-${editing.shotId}-${editing.field}"]`,
        )}
        onCommit={(edit) => send(editing.shotId, edit)}
        onClose={() => {
          const cell = document.querySelector<HTMLElement>(
            `[data-testid="shot-cell-${sectionId}-${editing.shotId}-${editing.field}"]`,
          );
          setEditing(null);
          // Back to the grid, so the keyboard carries on where it was.
          cell?.closest<HTMLElement>('[tabindex]')?.focus();
        }}
      />
    ) : null;

  return { columns, editor, onKeyDownCapture, applyCell };
}
