/**
 * The Data workspace's status bar: what the table holds ("N rows · S stations
 * · H shots · K sections"), how much of it the filters show, and a summary of
 * the selected cells (count, and Σ / avg / min / max for one numeric column).
 */
import { useTranslation } from 'react-i18next';
import { formatFixed, formatNumber } from '../../i18n/format';
import type { DataModelCtx, DataRow, DataView } from './types';
import { countRows } from './model/rows';
import { decimalsOf } from './model/ctx';
import { columnUnit, type SelectionSummary } from './selectionSummary';
import type { DataColumnKey } from './types';

function placesFor(key: DataColumnKey, ctx: DataModelCtx): number {
  const d = decimalsOf(ctx);
  if (key.startsWith('azimuth')) return d.azimuth;
  if (key === 'inclination' || key.startsWith('pitch')) return d.inclination;
  if (key === 'x' || key === 'y') return d.coordinates;
  if (key === 'lat' || key === 'long') return d.gps;
  if (key.startsWith('depth')) return d.depth;
  return d.length;
}

export function DataStatusBar({
  rows,
  view,
  ctx,
  selection,
}: {
  rows: readonly DataRow[];
  view: DataView;
  ctx: DataModelCtx;
  selection: SelectionSummary | null;
}) {
  const { t } = useTranslation();
  const all = countRows(rows);
  const total = all.stations + all.shots;
  const shownRows = countRows(view.rows);
  const shown = shownRows.stations + shownRows.shots;
  const n = (key: string, count: number) => t(key, { count, countFormatted: formatNumber(count) });

  let selectionText: string | null = null;
  if (selection) {
    const parts = [n('dataWorkspace.status.cells', selection.cells)];
    const num = selection.numeric;
    if (num) {
      const unit = columnUnit(num.colKey);
      const places = placesFor(num.colKey, ctx);
      const v = (x: number) => `${formatFixed(x, ctx.locale, places)}${unit ? (unit === '°' ? unit : ` ${unit}`) : ''}`;
      // The column's name without its unit suffix ("Length (m)" -> "Length").
      const name = t(`dataWorkspace.columns.${num.colKey}`).replace(/\s*\([^)]*\)$/u, '');
      parts.push(t('dataWorkspace.status.sum', { name, value: v(num.sum) }));
      parts.push(t('dataWorkspace.status.avg', { value: v(num.avg) }));
      parts.push(t('dataWorkspace.status.min', { value: v(num.min) }));
      parts.push(t('dataWorkspace.status.max', { value: v(num.max) }));
    }
    selectionText = parts.join(' · ');
  }

  return (
    <div
      className="flex h-6 shrink-0 items-center gap-3 border-t border-border bg-panel px-3 text-xs text-text-secondary tabular-nums"
      data-testid="data-status-bar"
    >
      <span className="truncate" data-testid="data-status-counts">
        {[
          n('dataWorkspace.status.rows', total),
          n('dataWorkspace.status.stations', all.stations),
          n('dataWorkspace.status.shots', all.shots),
          n('dataWorkspace.status.sections', all.sections),
        ].join(' · ')}
      </span>
      {shown !== total && (
        <span className="shrink-0 font-medium text-accent" data-testid="data-status-filtered">
          {t('dataWorkspace.status.showing', { shown: formatNumber(shown), total: formatNumber(total) })}
        </span>
      )}
      {selectionText && (
        <span
          className="ml-auto truncate text-text-primary"
          aria-label={t('dataWorkspace.status.selectionAria')}
          aria-live="polite"
          data-testid="data-status-selection"
        >
          {selectionText}
        </span>
      )}
    </div>
  );
}
