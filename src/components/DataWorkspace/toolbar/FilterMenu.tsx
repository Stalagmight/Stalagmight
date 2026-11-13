/**
 * The Filter menu: quick filters (issues, comments, links, frozen, virtual
 * shots), the ANDed conditions with remove, the condition builder and
 * "Clear filters". Row type and sections have their own controls but count
 * towards "Clear filters" too.
 */
import { useTranslation } from 'react-i18next';
import { useDataWorkspaceStore } from '../store';
import { DATA_COLUMN_BY_KEY } from '../columnsMeta';
import { filtersActive } from '../model/filter';
import type { DataModel } from '../DataModelContext';
import type { FilterState, QuickFilters } from '../types';
import { CloseIcon } from '../../common/icons';
import { testIdPart } from '../../../utils/testId';
import { FilterIcon } from './toolbarIcons';
import { MenuDivider, MenuHeading, ToolbarMenu } from './ToolbarMenu';
import { ConditionBuilder } from './ConditionBuilder';
import { conditionText } from './labels';

export const QUICK_FILTER_KEYS: readonly (keyof QuickFilters)[] = ['issues', 'comments', 'links', 'frozen', 'virtualShots'];

/** Quick filters + conditions in force (what the Filter button counts). */
export function filterMenuCount(f: FilterState): number {
  return QUICK_FILTER_KEYS.filter((k) => f.quick[k]).length + f.conditions.length;
}

export function FilterMenu({ model }: { model: DataModel }) {
  const { t, i18n } = useTranslation();
  const filters = useDataWorkspaceStore((s) => s.filters);
  const setQuickFilter = useDataWorkspaceStore((s) => s.setQuickFilter);
  const addCondition = useDataWorkspaceStore((s) => s.addCondition);
  const removeCondition = useDataWorkspaceStore((s) => s.removeCondition);
  const clearFilters = useDataWorkspaceStore((s) => s.clearFilters);
  const count = filterMenuCount(filters);

  return (
    <ToolbarMenu
      testId="data-filter-button"
      menuTestId="data-filter-menu"
      icon={<FilterIcon size={14} />}
      label={count > 0 ? t('dataToolbar.filter.buttonCount', { count }) : t('dataToolbar.filter.button')}
      title={t('dataToolbar.filter.title')}
      active={count > 0}
      width={360}
      remeasureKey={filters.conditions.length}
    >
      <MenuHeading>{t('dataToolbar.filter.quickHeading')}</MenuHeading>
      {QUICK_FILTER_KEYS.map((key) => (
        <label
          key={key}
          className="flex cursor-pointer items-start gap-2 rounded px-2 py-1 hover:bg-accent-10"
        >
          <input
            type="checkbox"
            className="mt-0.5"
            checked={filters.quick[key]}
            onChange={(e) => setQuickFilter(key, e.target.checked)}
            data-testid={`data-filter-quick-${key}`}
          />
          <span className="flex flex-col">
            <span>{t(`dataToolbar.filter.quick.${key}`)}</span>
            <span className="text-[11px] leading-4 text-text-secondary">{t(`dataToolbar.filter.quickHint.${key}`)}</span>
          </span>
        </label>
      ))}
      <MenuDivider />
      <MenuHeading>{t('dataToolbar.filter.conditionsHeading')}</MenuHeading>
      {filters.conditions.length === 0 ? (
        <div className="px-2 py-0.5 text-text-secondary">{t('dataToolbar.filter.noConditions')}</div>
      ) : (
        <ul className="px-1" data-testid="data-filter-conditions">
          {filters.conditions.map((c) => {
            const meta = DATA_COLUMN_BY_KEY.get(c.colKey);
            return (
              <li
                key={c.id}
                className="flex items-center gap-2 rounded px-1 py-0.5 hover:bg-accent-10"
                data-testid={`data-filter-condition-${testIdPart(c.id)}`}
              >
                <span className="min-w-0 flex-1 truncate">{meta ? conditionText(t, i18n, c, meta) : c.colKey}</span>
                <button
                  type="button"
                  className="shrink-0 rounded p-0.5 text-text-secondary hover:text-text-primary"
                  title={t('dataToolbar.filter.remove')}
                  aria-label={t('dataToolbar.filter.remove')}
                  onClick={() => removeCondition(c.id)}
                  data-testid={`data-filter-condition-remove-${testIdPart(c.id)}`}
                >
                  <CloseIcon size={10} />
                </button>
              </li>
            );
          })}
        </ul>
      )}
      <ConditionBuilder model={model} onAdd={addCondition} />
      <MenuDivider />
      <div className="flex justify-end px-1 pb-0.5">
        <button
          type="button"
          className="rounded px-2 py-0.5 text-text-secondary hover:bg-accent-10 hover:text-text-primary disabled:opacity-50"
          disabled={!filtersActive(filters)}
          onClick={clearFilters}
          data-testid="data-filter-clear"
        >
          {t('dataToolbar.filter.clear')}
        </button>
      </div>
    </ToolbarMenu>
  );
}
