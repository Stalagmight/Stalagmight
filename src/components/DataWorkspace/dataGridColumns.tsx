/**
 * DataColumnDefs -> SpreadsheetGrid columns. The cell text is `def.text()` (the
 * display and the find target are one string), the value is `def.value()`
 * (raw numbers seed the editor at full precision), parsing is the column's
 * coercion + the DataTable validators (an interactive edit flashes the
 * reason), and a cell takes an edit iff `def.lockReason()` is null.
 *
 * Three columns render their own content: Links (chips that navigate),
 * Issues (the DataTable warning / lock icons) and Kind (the J/R glyph that
 * cycles on click, like the DataTable).
 */
import type { CSSProperties, ReactNode } from 'react';
import type { TFunction } from 'i18next';
import type { GridColumn } from '../common/SpreadsheetGrid/types';
import { AlertCircleIcon, AlertTriangleIcon, LockIcon } from '../common/icons';
import { getErrorColor } from '../../utils/colors';
import { SHOT_KIND_CLASS, SHOT_KIND_LABEL_KEY, nextShotKind } from '../../utils/shotKind';
import { testIdPart } from '../../utils/testId';
import type { DataColumnDef, DataColumnKey, DataModelCtx, DataRow, DataView, ShotRow } from './types';
import { coerceCell, isShotFrozen, misclosureText, rowWarnings, shotKindOf, validationError } from './model/columns';
import { decimalsOf, fmt, shotCredit, stationName } from './model/ctx';
import { stationLinks, stationRefKey } from './model/links';
import { isEntityRow } from './model/rows';
import { dataRowPart } from './dataTestIds';
import type { DataNav } from './useDataNavigation';

const NULLABLE_NUMBERS = new Set<DataColumnKey>(['inclination', 'pitch_in', 'pitch_out', 'left', 'right', 'up', 'down', 'temperature']);
const CREDIT = new Set<DataColumnKey>(['surveyors', 'explorers', 'date']);

export interface GridColumnDeps {
  t: TFunction;
  ctx: DataModelCtx;
  view: DataView;
  nav: DataNav;
  onCycleKind: (row: ShotRow, dir: 1 | -1) => void;
  onOpenCredit: (row: DataRow, colKey: DataColumnKey, seed: string) => void;
  onResolveWarning: (row: ShotRow) => void;
}

const stop = (e: { stopPropagation(): void }) => e.stopPropagation();

function chipStyle(color: string): CSSProperties {
  return { '--o': color } as CSSProperties;
}

const CHIP_CLS =
  'dw-chip sh-pill inline-flex min-w-0 shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-xs leading-none text-text-primary';

function LinksCell({ row, deps }: { row: DataRow; deps: GridColumnDeps }) {
  if (row.kind !== 'station') return null;
  const { ctx, nav, t } = deps;
  const links = stationLinks(ctx, row.sectionId, row.stationIndex, row.g);
  const part = dataRowPart(row);
  const chips: ReactNode[] = [];
  if (links.zeroDatum) {
    chips.push(
      <span
        key="zd"
        className={`${CHIP_CLS} cursor-default`}
        style={chipStyle('var(--cc-zero-datum)')}
        title={t('dataWorkspace.links.zeroDatumTitle')}
        data-testid={`data-chip-zero-${part}`}
      >
        {t('dataWorkspace.links.zeroDatum')}
      </span>,
    );
  }
  const go = (key: string) => (e: { stopPropagation(): void }) => {
    e.stopPropagation();
    nav.goToRow(key);
  };
  if (links.branchFrom) {
    const name = stationName(ctx, links.branchFrom.section_id, links.branchFrom.station_index);
    const target = `${links.branchFrom.section_id}-${links.branchFrom.station_index}`;
    chips.push(
      <button
        key="from"
        type="button"
        className={CHIP_CLS}
        style={chipStyle('var(--sh-section)')}
        title={t('dataWorkspace.links.continuesFromTitle', { name })}
        onMouseDown={stop}
        onClick={go(stationRefKey(links.branchFrom))}
        data-testid={`data-chip-branch-${part}-${target}`}
      >
        <span className="truncate">{t('dataWorkspace.links.continuesFrom', { name })}</span>
      </button>,
    );
  }
  for (const m of links.merges) {
    if (links.branchFrom && m.section_id === links.branchFrom.section_id && m.station_index === links.branchFrom.station_index) continue;
    const name = stationName(ctx, m.section_id, m.station_index);
    chips.push(
      <button
        key={`m-${m.section_id}-${m.station_index}`}
        type="button"
        className={CHIP_CLS}
        style={chipStyle('var(--sh-merge)')}
        title={t('dataWorkspace.links.mergeTitle', { name })}
        onMouseDown={stop}
        onClick={go(stationRefKey(m))}
        data-testid={`data-chip-merge-${part}-${m.section_id}-${m.station_index}`}
      >
        <span className="truncate">{`⇄ ${name}`}</span>
      </button>,
    );
  }
  for (const c of links.closures) {
    const name = stationName(ctx, c.partner.section_id, c.partner.station_index);
    const target = `${c.partner.section_id}-${c.partner.station_index}`;
    chips.push(
      <button
        key={`c-${c.closureId}-${target}`}
        type="button"
        className={CHIP_CLS}
        style={chipStyle('var(--sh-station)')}
        title={t('dataWorkspace.links.closureTitle', { name })}
        onMouseDown={stop}
        onClick={go(stationRefKey(c.partner))}
        data-testid={`data-chip-closure-${part}-${testIdPart(c.closureId)}-${target}`}
      >
        <span className="truncate">{t('dataWorkspace.links.closesTo', { name })}</span>
      </button>,
    );
    const mis = misclosureText(ctx, c.misclosure, c.errorPct);
    if (mis && c.errorPct !== null && c.misclosure !== null) {
      chips.push(
        <span
          key={`d-${c.closureId}-${target}`}
          className={`${CHIP_CLS} cursor-default tabular-nums`}
          style={chipStyle(getErrorColor(c.errorPct))}
          title={t('dataWorkspace.links.misclosureTitle', {
            distance: fmt(ctx, c.misclosure, decimalsOf(ctx).length),
            percent: fmt(ctx, c.errorPct, 1),
          })}
          data-testid={`data-chip-misclosure-${part}-${testIdPart(c.closureId)}`}
        >
          {mis}
        </span>,
      );
    }
  }
  if (chips.length === 0) return null;
  const marked = deps.view.matchIndex.get(row.key)?.has('links');
  return (
    <span className={`flex min-w-0 items-center gap-1 overflow-hidden ${marked ? 'dw-cell-match' : ''}`} data-testid={`data-links-${part}`}>
      {chips}
    </span>
  );
}

function IssuesCell({ row, deps }: { row: DataRow; deps: GridColumnDeps }) {
  if (!isEntityRow(row)) return null;
  const { ctx, t } = deps;
  const frozen = row.kind === 'shot' && isShotFrozen(ctx, row);
  const open = rowWarnings(ctx, row).filter((w) => !w.isResolved);
  if (open.length === 0 && !frozen) return null;
  const hasError = open.some((w) => w.severity === 'error');
  const tooltip = [...(frozen ? [t('dataTable.frozen')] : []), ...open.map((w) => w.message)].join('\n');
  const colorClass = hasError ? 'text-error' : frozen && open.length === 0 ? 'text-text-secondary' : 'text-warning';
  const icon =
    frozen && open.length === 0 ? <LockIcon size={13} /> : hasError ? <AlertCircleIcon size={14} /> : <AlertTriangleIcon size={14} />;
  const part = dataRowPart(row);
  const content = (
    <>
      {icon}
      {open.length > 1 && <span className="ml-0.5 text-[11px] tabular-nums">{open.length}</span>}
      {frozen && open.length > 0 && <LockIcon size={11} />}
    </>
  );
  if (open.length > 0 && row.kind === 'shot' && !ctx.editingBlocked) {
    return (
      <button
        type="button"
        className={`${colorClass} inline-flex cursor-pointer items-center rounded hover:opacity-80 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset`}
        title={tooltip}
        aria-label={t('detailPanel.warnings.resolve')}
        onMouseDown={stop}
        onClick={(e) => {
          e.stopPropagation();
          deps.onResolveWarning(row);
        }}
        data-testid={`data-issues-${part}`}
      >
        {content}
      </button>
    );
  }
  return (
    <span className={`${colorClass} inline-flex items-center`} title={tooltip} data-testid={`data-issues-${part}`}>
      {content}
    </span>
  );
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
    if (key === 'links') col.render = (row) => <LinksCell row={row} deps={deps} />;
    if (key === 'issues') col.render = (row) => <IssuesCell row={row} deps={deps} />;
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
