/**
 * The data table's survey-credit columns, wired: the column definitions and
 * the popover editor over a credit cell. The grid opens it (`openEditor`) from
 * Enter, F2, Space, a double click or a typed character, which the field then
 * starts with. While it is open the keyboard is the popover's: a key that
 * reaches the grid instead never acts on the grid. Kept out of `DataTable.tsx`,
 * which is past its size ratchet. See `dataTableCreditColumns.tsx` for the cell
 * semantics.
 */
import { useCallback, useMemo, useState, type KeyboardEvent, type ReactNode } from 'react';
import type { TFunction } from 'i18next';
import { useProjectStore } from '../../store/projectStore';
import { usePeopleSuggestions } from '../../hooks/usePeopleSuggestions';
import { disambiguationLabels, personKey } from '../../utils/people';
import { setSource } from '../../utils/telemetry';
import type { GridColumn } from '../common/SpreadsheetGrid/types';
import type { CreditEdit, Shot } from '../../types/ipc.generated';
import { buildCreditColumns, type CreditColumnKey } from './dataTableCreditColumns';
import { CREDIT_EDITOR_FIELD, CREDIT_EDITOR_TEST_ID, CreditCellEditor } from './CreditCellEditor';
import { creditEditForCells } from './dataTableEdits';

interface Options {
  t: TFunction;
  sectionId: number | null;
  /** Grid rows (drafts materialized), by grid row index. */
  rows: readonly Shot[];
  shotIndexAt: (gridRowIndex: number) => number | null;
  editingBlocked: boolean;
}

export interface DataTableCredit {
  columns: GridColumn<Shot>[];
  /** The open popover, if any. */
  editor: ReactNode;
  /** On the grid's wrapper: while the popover is open, keys never reach the grid. */
  onKeyDownCapture: (event: KeyboardEvent<HTMLElement>) => void;
  /** A single grid edit of a credit cell (a one-cell paste or clear). */
  applyCell: (shot: Shot, colKey: string, value: unknown) => void;
}

export function useDataTableCredit({ t, sectionId, rows, shotIndexAt, editingBlocked }: Options): DataTableCredit {
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
        onOpenEditor: open,
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

  // The popover's field normally has the focus (CreditCellEditor takes it back
  // if the grid grabs it). Should a key still reach the grid underneath, the
  // grid must not act on it - a letter went straight to the shot as a
  // one-letter person, Delete would clear the selection. Escape closes the
  // popover; any other key moves the focus into the field, and a printable one
  // is left to land there (the browser types into whatever keydown focused).
  const shot = editing ? rows.find((r) => r.id === editing.shotId) : undefined;
  const isOpen = editing !== null && shot !== undefined && sectionId !== null;
  const onKeyDownCapture = useCallback(
    (event: KeyboardEvent<HTMLElement>) => {
      if (!isOpen || (event.target as Element).closest(`[data-testid="${CREDIT_EDITOR_TEST_ID}"]`)) return;
      event.stopPropagation();
      if (event.key === 'Escape') {
        event.preventDefault();
        setEditing(null);
        return;
      }
      const printable = event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey;
      if (!printable) event.preventDefault();
      document.querySelector<HTMLElement>(CREDIT_EDITOR_FIELD)?.focus();
    },
    [isOpen],
  );

  const editor =
    isOpen ? (
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
