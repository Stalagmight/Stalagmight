/**
 * DataColumnDefs -> SpreadsheetGrid columns. The cell text is `def.text()` (the
 * display and the find target are one string), the value is `def.value()`
 * (raw numbers seed the editor at full precision), parsing is the column's
 * coercion + the DataTable validators (an interactive edit flashes the
 * reason), and a cell takes an edit iff `def.lockReason()` is null.
 *
 * Kind renders its own content (the J/R glyph that cycles on click, like the
 * DataTable). A row's issues and a station's links are no columns: the issue
 * icon sits in the gutter (RowIssueIcon), the link chips span the row's
 * shot-only cells (StationLinkChips).
 */
import type { TFunction } from 'i18next';
import type { GridColumn } from '../common/SpreadsheetGrid/types';
import { SHOT_KIND_CLASS, SHOT_KIND_LABEL_KEY, nextShotKind } from '../../utils/shotKind';
import type { DataColumnDef, DataColumnKey, DataModelCtx, DataRow, DataView, ShotRow } from './types';
import { coerceCell, shotKindOf, validationError } from './model/columns';
import { shotCredit } from './model/ctx';
import { isEntityRow } from './model/rows';
import { dataRowPart } from './dataTestIds';

const NULLABLE_NUMBERS = new Set<DataColumnKey>(['inclination', 'pitch_in', 'pitch_out', 'left', 'right', 'up', 'down', 'temperature']);
const CREDIT = new Set<DataColumnKey>(['surveyors', 'explorers', 'date']);

export interface GridColumnDeps {
  t: TFunction;
  ctx: DataModelCtx;
  view: DataView;
  onCycleKind: (row: ShotRow, dir: 1 | -1) => void;
  onOpenCredit: (row: DataRow, colKey: DataColumnKey, seed: string) => void;
}

function KindCell({ row, deps }: { row: DataRow; deps: GridColumnDeps }) {
  if (row.kind !== 'shot') return null;
  const { ctx, t } = deps;
  const kind = shotKindOf(ctx, row);
  const readOnly = ctx.editingBlocked;
  const glyph = kind === 'Jump' ? 'J' : kind === 'Reference' ? 'R' : '';
  return (
    <span
      data-testid={`data-kind-${dataRowPart(row)}`}
      data-kind={kind}
      role={readOnly ? undefined : 'button'}
      tabIndex={readOnly ? undefined : -1}
      aria-label={t(SHOT_KIND_LABEL_KEY[kind])}
      className={`group flex h-full w-full items-center justify-center font-semibold ${SHOT_KIND_CLASS[kind]} ${
        readOnly ? 'cursor-default opacity-60' : 'cursor-pointer'
      }`}
      onClick={(e) => {
        e.stopPropagation();
        if (!readOnly) deps.onCycleKind(row, e.shiftKey ? -1 : 1);
      }}
      title={t('dataTable.columns.kindCycleHint', {
        current: t(SHOT_KIND_LABEL_KEY[kind]),
        next: t(SHOT_KIND_LABEL_KEY[nextShotKind(kind, 1)]),
      })}
    >
      {glyph === '' ? <span className="opacity-0 group-hover:opacity-40">J</span> : glyph}
    </span>
  );
}

export function buildGridColumns(defs: readonly DataColumnDef[], deps: GridColumnDeps): GridColumn<DataRow>[] {
  const { t, ctx, view } = deps;
  return defs.map((def): GridColumn<DataRow> => {
    const key = def.key;
    const revealed = view.revealedColumns.has(key);
    const col: GridColumn<DataRow> = {
      key,
      header: t(`dataWorkspace.headers.${key}`),
      width: def.width,
      grow: key === 'comment',
      align: def.align,
      getValue: (row) => (isEntityRow(row) ? def.value(row, ctx) : null),
      format: (_v, row) => (isEntityRow(row) ? def.text(row, ctx) : ''),
      isApplicable: (row) => isEntityRow(row) && (def.appliesTo === 'both' || def.appliesTo === row.kind),
      sortable: def.sortable,
      ...(revealed ? { headerClassName: 'dw-header-revealed', headerTitle: t('dataWorkspace.gutter.revealedColumn') } : {}),
    };
    if (def.editable) {
      col.editable = true;
      col.isEditable = (row) => isEntityRow(row) && def.lockReason(row, ctx) === null;
      col.kind = def.valueType === 'number' ? (NULLABLE_NUMBERS.has(key) ? 'number-nullable' : 'number') : 'text';
      col.parse = (raw, row) => (isEntityRow(row) ? coerceCell(key, raw, row, ctx) : undefined);
      col.validate = (value, row) => {
        if (!isEntityRow(row)) return null;
        const err = validationError(key, value, row, ctx);
        return err ? t(err) : null;
      };
    }
    if (key === 'comment') col.multiline = true;
    if (key === 'kind') {
      col.render = (row) => <KindCell row={row} deps={deps} />;
      col.noInlineEdit = true;
      col.activate = (row, _i, dir) => {
        if (row.kind === 'shot' && !ctx.editingBlocked) deps.onCycleKind(row, dir);
      };
    }
    if (key === 'surveyors' || key === 'explorers') {
      // The value is the people list (what a fill / move transfers), the text
      // its "Name <email>; Name" form.
      col.getValue = (row) => (row.kind === 'shot' ? (shotCredit(ctx, row)?.[key] ?? []) : null);
    }
    if (CREDIT.has(key)) {
      col.noInlineEdit = true;
      col.openEditor = (row, _i, seed) => deps.onOpenCredit(row, key, key === 'date' ? '' : seed);
    }
    return col;
  });
}
