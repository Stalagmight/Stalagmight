/**
 * The find bar's options: which rows and columns the search looks in, and
 * "Show only rows with matches". Searching hidden rows / columns shows the ones
 * that hold a match temporarily (marked as revealed by the grid).
 */
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useDataWorkspaceStore } from '../store';
import { DATA_COLUMN_GROUPS, DATA_COLUMNS } from '../columnsMeta';
import type { DataColumnKey, SearchState } from '../types';
import { MoreIcon } from './toolbarIcons';
import { MenuDivider, MenuHeading, ToolbarMenu } from './ToolbarMenu';
import { columnLabel, groupLabel } from './labels';

const SEARCHABLE = DATA_COLUMNS.filter((c) => c.searchable);

/** Non-default scope options are on (tints the trigger). */
export function findOptionsActive(s: Pick<SearchState, 'rowScope' | 'columnScope' | 'onlyMatchingRows'>): boolean {
  return s.rowScope !== 'visible' || s.columnScope !== 'visible' || s.onlyMatchingRows;
}

function Radio({
  checked,
  onChange,
  label,
  testId,
  name,
}: {
  checked: boolean;
  onChange: () => void;
  label: string;
  testId: string;
  name: string;
}) {
  return (
    <label className="flex cursor-pointer items-start gap-2 rounded px-2 py-1 hover:bg-accent-10">
      <input type="radio" className="mt-0.5" name={name} checked={checked} onChange={onChange} data-testid={testId} />
      <span>{label}</span>
    </label>
  );
}

export function FindOptionsMenu() {
  const { t, i18n } = useTranslation();
  const search = useDataWorkspaceStore((s) => s.search);
  const hiddenColumns = useDataWorkspaceStore((s) => s.hiddenColumns);
  const setSearch = useDataWorkspaceStore((s) => s.setSearch);
  const chosen = useMemo(() => new Set(search.columns), [search.columns]);

  const pickCustom = () => {
    // Start from what the user sees, so "Choose columns" is never empty.
    const columns =
      search.columns.length > 0
        ? search.columns
        : SEARCHABLE.filter((c) => !hiddenColumns.includes(c.key)).map((c) => c.key);
    setSearch({ columnScope: 'custom', columns });
  };
  const toggleColumn = (key: DataColumnKey) =>
    setSearch({ columns: chosen.has(key) ? search.columns.filter((k) => k !== key) : [...search.columns, key] });

  return (
    <ToolbarMenu
      testId="data-find-options"
      menuTestId="data-find-options-menu"
      label={null}
      icon={<MoreIcon size={14} />}
      iconOnly
      title={t('dataToolbar.find.options')}
      ariaLabel={t('dataToolbar.find.options')}
      active={findOptionsActive(search)}
      width={300}
      remeasureKey={`${search.columnScope}:${search.columns.length}`}
    >
      <MenuHeading>{t('dataToolbar.find.rowsHeading')}</MenuHeading>
      <Radio
        name="data-find-rows"
        checked={search.rowScope === 'visible'}
        onChange={() => setSearch({ rowScope: 'visible' })}
        label={t('dataToolbar.find.rowsVisible')}
        testId="data-find-rows-visible"
      />
      <Radio
        name="data-find-rows"
        checked={search.rowScope === 'all'}
        onChange={() => setSearch({ rowScope: 'all' })}
        label={t('dataToolbar.find.rowsAll')}
        testId="data-find-rows-all"
      />
      <MenuDivider />
      <MenuHeading>{t('dataToolbar.find.columnsHeading')}</MenuHeading>
      <Radio
        name="data-find-cols"
        checked={search.columnScope === 'visible'}
        onChange={() => setSearch({ columnScope: 'visible' })}
        label={t('dataToolbar.find.colsVisible')}
        testId="data-find-cols-visible"
      />
      <Radio
        name="data-find-cols"
        checked={search.columnScope === 'all'}
        onChange={() => setSearch({ columnScope: 'all' })}
        label={t('dataToolbar.find.colsAll')}
        testId="data-find-cols-all"
      />
      <Radio
        name="data-find-cols"
        checked={search.columnScope === 'custom'}
        onChange={pickCustom}
        label={t('dataToolbar.find.colsCustom')}
        testId="data-find-cols-custom"
      />
      {search.columnScope === 'custom' && (
        <div className="ml-6 mr-1 mt-0.5 rounded border border-border p-1" data-testid="data-find-cols-list">
          <div className="flex items-center justify-between px-1 pb-1 text-text-secondary">
            <span data-testid="data-find-cols-count">
              {t('dataToolbar.find.colsCustomCount', { count: chosen.size, total: SEARCHABLE.length })}
            </span>
            <span className="flex gap-2">
              <button
                type="button"
                className="hover:text-text-primary"
                onClick={() => setSearch({ columns: SEARCHABLE.map((c) => c.key) })}
                data-testid="data-find-cols-select-all"
              >
                {t('dataToolbar.find.colsSelectAll')}
              </button>
              <button
                type="button"
                className="hover:text-text-primary"
                onClick={() => setSearch({ columns: [] })}
                data-testid="data-find-cols-select-none"
              >
                {t('dataToolbar.find.colsSelectNone')}
              </button>
            </span>
          </div>
          {DATA_COLUMN_GROUPS.map((group) => {
            const cols = SEARCHABLE.filter((c) => c.group === group);
            if (cols.length === 0) return null;
            return (
              <div key={group}>
                <div className="px-1 pt-1 text-[11px] text-text-secondary">{groupLabel(t, i18n, group)}</div>
                {cols.map((c) => (
                  <label key={c.key} className="flex cursor-pointer items-center gap-2 rounded px-1 py-0.5 hover:bg-accent-10">
                    <input
                      type="checkbox"
                      checked={chosen.has(c.key)}
                      onChange={() => toggleColumn(c.key)}
                      data-testid={`data-find-col-${c.key}`}
                    />
                    <span className={hiddenColumns.includes(c.key) ? 'text-text-secondary' : undefined}>
                      {columnLabel(t, i18n, c)}
                    </span>
                  </label>
                ))}
              </div>
            );
          })}
        </div>
      )}
      <MenuDivider />
      <label className="flex cursor-pointer items-center gap-2 rounded px-2 py-1 hover:bg-accent-10">
        <input
          type="checkbox"
          checked={search.onlyMatchingRows}
          onChange={(e) => setSearch({ onlyMatchingRows: e.target.checked })}
          data-testid="data-find-only-matching"
        />
        {t('dataToolbar.find.onlyMatching')}
      </label>
    </ToolbarMenu>
  );
}
