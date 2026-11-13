/**
 * Edit dispatch for the Data workspace grid. Every path goes through the pure
 * `buildEditCommands` (locks, the averaged-angle expansion, credit, station
 * comment / name), then:
 *   - a single cell edit -> its own command (`updateShot`,
 *     `updateStationComment`) or a one-command batch, one undo step;
 *   - a paste / fill / clear -> ONE `executeBatch`, one undo step;
 *   - the J/R cell's click / Space -> `setShotKind`, as the DataTable does;
 *   - a credit cell -> the shared credit popover (`CreditCellEditor`).
 * Dropped edits are reported with a toast. Nothing runs while editing is
 * blocked (`useEditingBlocked`, carried in the model ctx).
 */
import { useCallback, useState, type KeyboardEvent, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { setSource } from '../../utils/telemetry';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { nextShotKind } from '../../utils/shotKind';
import type { CellEdit } from '../common/SpreadsheetGrid/types';
import type { CreditEdit, Shot } from '../../types/ipc.generated';
import { CREDIT_EDITOR_FIELD, CREDIT_EDITOR_TEST_ID, CreditCellEditor } from '../SingleSystemView/CreditCellEditor';
import type { CreditColumnKey } from '../SingleSystemView/dataTableCreditColumns';
import { usePasteReportToast } from '../SingleSystemView/usePasteReportToast';
import type { DataModel } from './DataModelContext';
import type { DataCellEdit, DataColumnKey, DataEditPlan, DataRow, DataView, ShotRow } from './types';
import { buildEditCommands } from './model/edits';
import { shotKindOf } from './model/columns';
import { isEntityRow } from './model/rows';
import { dataCellTestId } from './dataTestIds';

const CREDIT_FIELD: Partial<Record<DataColumnKey, CreditColumnKey>> = {
  surveyors: 'surveyors',
  explorers: 'explorers',
  date: 'survey_date',
};

export interface DataEditing {
  onEdit: (rowIndex: number, colKey: string, value: unknown) => void;
  onBatchEdit: (edits: CellEdit[]) => void;
  onPasteReport: (info: { written: number; clipped: number; skipped: number }) => void;
  cycleKind: (row: ShotRow, dir: 1 | -1) => void;
  openCreditEditor: (row: DataRow, colKey: DataColumnKey, seed: string) => void;
  /** The open credit popover, if any. */
  creditEditor: ReactNode;
  /** On the grid wrapper: while the popover is open, keys never reach the grid. */
  onKeyDownCapture: (event: KeyboardEvent<HTMLElement>) => void;
}

/** Materialize one shot (for its credit) - reads that one section's shots. */
function shotObject(model: DataModel, row: ShotRow): Shot | undefined {
  const section = model.ctx.project.sections.find((s) => s.id === row.sectionId);
  return section?.shots.find((s) => s.id === row.shotId);
}

export function useDataEditing(model: DataModel | null, view: DataView | null): DataEditing {
  const { t } = useTranslation();
  const pasteReport = usePasteReportToast(TOAST_KEYS.DATA_TABLE_PASTE);
  const [credit, setCredit] = useState<{ row: ShotRow; field: CreditColumnKey; seed: string } | null>(null);

  const reportSkipped = useCallback(
    (plan: DataEditPlan) => {
      if (plan.skipped.length === 0) return;
      const reasons = [...new Set(plan.skipped.map((s) => s.reason))].map((r) => t(`dataWorkspace.skipped.${r}`)).join(', ');
      useUIStore
        .getState()
        .showToast(t('dataWorkspace.skipped.summary', { count: plan.skipped.length, reasons }), TOAST_KEYS.DATA_TABLE_PASTE, 'warning');
    },
    [t],
  );

  const run = useCallback(
    (edits: DataCellEdit[], single: boolean) => {
      if (!model || model.ctx.editingBlocked || edits.length === 0) return;
      const plan = buildEditCommands(edits, model.ctx);
      reportSkipped(plan);
      if (plan.commands.length === 0) return;
      setSource('data-table');
      const store = useProjectStore.getState();
      const only = plan.commands.length === 1 ? plan.commands[0]! : null;
      if (single && only?.type === 'update_shot') {
        void store.updateShot(only.section_id, only.shot_id, only.updates);
      } else if (single && only?.type === 'set_station_comment') {
        void store.updateStationComment(only.station_ref, only.comment);
      } else {
        void store.executeBatch(plan.commands);
      }
    },
    [model, reportSkipped],
  );

  const rowAt = useCallback((index: number) => view?.rows[index], [view]);

  const onEdit = useCallback(
    (rowIndex: number, colKey: string, value: unknown) => {
      const row = rowAt(rowIndex);
      if (!isEntityRow(row)) return;
      run([{ row, colKey: colKey as DataColumnKey, value }], true);
    },
    [rowAt, run],
  );

  const onBatchEdit = useCallback(
    (edits: CellEdit[]) => {
      const out: DataCellEdit[] = [];
      for (const e of edits) {
        const row = rowAt(e.row);
        if (isEntityRow(row)) out.push({ row, colKey: e.colKey as DataColumnKey, value: e.value });
      }
      run(out, false);
    },
    [rowAt, run],
  );

  const cycleKind = useCallback(
    (row: ShotRow, dir: 1 | -1) => {
      if (!model || model.ctx.editingBlocked) return;
      setSource('data-table');
      void useProjectStore.getState().setShotKind(row.sectionId, row.shotId, nextShotKind(shotKindOf(model.ctx, row), dir));
    },
    [model],
  );

  const openCreditEditor = useCallback(
    (row: DataRow, colKey: DataColumnKey, seed: string) => {
      const field = CREDIT_FIELD[colKey];
      if (!model || model.ctx.editingBlocked || row.kind !== 'shot' || !field) return;
      setCredit({ row, field, seed });
    },
    [model],
  );

  const shot = credit && model ? shotObject(model, credit.row) : undefined;
  const isOpen = credit !== null && shot !== undefined;
  const cellSelector = credit
    ? `[data-testid="${dataCellTestId(credit.row, credit.field === 'survey_date' ? 'date' : credit.field)}"]`
    : '';

  const onKeyDownCapture = useCallback(
    (event: KeyboardEvent<HTMLElement>) => {
      if (!isOpen || (event.target as Element).closest(`[data-testid="${CREDIT_EDITOR_TEST_ID}"]`)) return;
      event.stopPropagation();
      if (event.key === 'Escape') {
        event.preventDefault();
        setCredit(null);
        return;
      }
      const printable = event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey;
      if (!printable) event.preventDefault();
      document.querySelector<HTMLElement>(CREDIT_EDITOR_FIELD)?.focus();
    },
    [isOpen],
  );

  const creditEditor =
    isOpen && credit && shot ? (
      <CreditCellEditor
        key={`${credit.row.key}:${credit.field}`}
        shot={shot}
        field={credit.field}
        seed={credit.seed}
        anchor={document.querySelector<HTMLElement>(cellSelector)}
        onCommit={(edit: CreditEdit) => {
          if (!model || model.ctx.editingBlocked) return;
          setSource('data-table');
          void useProjectStore.getState().editShotCredit({ type: 'shots', shots: [[credit.row.sectionId, credit.row.shotId]] }, edit);
        }}
        onClose={() => {
          const cell = document.querySelector<HTMLElement>(cellSelector);
          setCredit(null);
          cell?.closest<HTMLElement>('[tabindex]')?.focus();
        }}
      />
    ) : null;

  return { onEdit, onBatchEdit, onPasteReport: pasteReport, cycleKind, openCreditEditor, creditEditor, onKeyDownCapture };
}
