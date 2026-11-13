/**
 * Show / hide columns, grouped like the column registry, with "Show all",
 * "Reset to default" and the shown count. Writes `setHiddenColumns`, which the
 * view persistence mirrors to config. A hidden column the search is showing
 * temporarily is marked.
 */
import { useTranslation } from 'react-i18next';
import { useDataWorkspaceStore } from '../store';
import { DATA_COLUMN_GROUPS, DATA_COLUMNS, DATA_DEFAULT_HIDDEN } from '../columnsMeta';
import type { DataColumnKey } from '../types';
import { EyeIcon } from '../../common/icons';
import { MenuDivider, ToolbarMenu } from './ToolbarMenu';
import { columnLabel, groupLabel } from './labels';

const sameSet = (a: readonly string[], b: readonly string[]) => a.length === b.length && a.every((k) => b.includes(k));

export function ColumnsMenu({ revealed }: { revealed: ReadonlySet<DataColumnKey> }) {
  const { t, i18n } = useTranslation();
  const hiddenColumns = useDataWorkspaceStore((s) => s.hiddenColumns);
  const setHiddenColumns = useDataWorkspaceStore((s) => s.setHiddenColumns);
  const hidden = new Set(hiddenColumns);
  const total = DATA_COLUMNS.length;
  const shown = DATA_COLUMNS.filter((c) => !hidden.has(c.key)).length;
  const isDefault = sameSet(hiddenColumns, DATA_DEFAULT_HIDDEN);

  const toggle = (key: DataColumnKey) =>
    setHiddenColumns(hidden.has(key) ? hiddenColumns.filter((k) => k !== key) : [...hiddenColumns, key]);

  return (
    <ToolbarMenu
      testId="data-columns-button"
      menuTestId="data-columns-menu"
      icon={<EyeIcon size={14} />}
      label={t('dataToolbar.columns.button')}
      title={t('dataToolbar.columns.title')}
      width={300}
    >
      <div className="flex items-center justify-between gap-2 px-2 py-1">
        <span className="min-w-0 truncate whitespace-nowrap text-text-secondary tabular-nums" data-testid="data-columns-count">
          {t('dataToolbar.columns.shown', { shown, total })}
        </span>
        <span className="flex shrink-0 gap-1 whitespace-nowrap">
          <button
            type="button"
            className="rounded px-1.5 py-0.5 hover:bg-accent-10 disabled:opacity-50"
            disabled={hiddenColumns.length === 0}
            onClick={() => setHiddenColumns([])}
            data-testid="data-columns-show-all"
          >
            {t('dataToolbar.columns.showAll')}
          </button>
          <button
            type="button"
            className="rounded px-1.5 py-0.5 hover:bg-accent-10 disabled:opacity-50"
            disabled={isDefault}
            onClick={() => setHiddenColumns([...DATA_DEFAULT_HIDDEN])}
            data-testid="data-columns-reset"
          >
            {t('dataToolbar.columns.reset')}
          </button>
        </span>
      </div>
      <MenuDivider />
      {DATA_COLUMN_GROUPS.map((group) => {
        const cols = DATA_COLUMNS.filter((c) => c.group === group);
        if (cols.length === 0) return null;
        return (
          <div key={group} className="pb-1">
            <div className="px-2 pt-1 text-[11px] font-medium uppercase tracking-wide text-text-secondary">
              {groupLabel(t, i18n, group)}
            </div>
            {cols.map((c) => (
              <label
                key={c.key}
                className="flex cursor-pointer items-center gap-2 rounded px-2 py-0.5 hover:bg-accent-10"
              >
                <input
                  type="checkbox"
                  checked={!hidden.has(c.key)}
                  onChange={() => toggle(c.key)}
                  data-testid={`data-columns-col-${c.key}`}
                />
                <span className="flex-1">{columnLabel(t, i18n, c)}</span>
                {hidden.has(c.key) && revealed.has(c.key) && (
                  <span className="text-[11px] text-accent" data-testid={`data-columns-revealed-${c.key}`}>
                    {t('dataToolbar.columns.revealed')}
                  </span>
                )}
              </label>
            ))}
          </div>
        );
      })}
    </ToolbarMenu>
  );
}
