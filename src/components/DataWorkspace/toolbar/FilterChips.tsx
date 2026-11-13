/**
 * The active-filter chips row: one removable chip per thing that narrows the
 * view (row type, sections, each quick filter, each condition), a chip for rows
 * / columns shown only temporarily (search or navigation) with [Hide], and
 * "Clear all". Hidden when nothing is active. The sort is not a chip: the sort
 * bar right under the toolbar already says it and undoes it.
 */
import { useTranslation } from 'react-i18next';
import { useDataWorkspaceStore } from '../store';
import { DATA_COLUMN_BY_KEY } from '../columnsMeta';
import { filtersActive } from '../model/filter';
import type { DataModel } from '../DataModelContext';
import type { DataView } from '../types';
import { CloseIcon } from '../../common/icons';
import { testIdPart } from '../../../utils/testId';
import { QUICK_FILTER_KEYS } from './FilterMenu';
import { conditionText, tCount } from './labels';

const SECTION_NAMES_SHOWN = 2;

function Chip({
  label,
  onRemove,
  removeLabel,
  testId,
  tone = 'normal',
  action,
}: {
  label: string;
  onRemove?: () => void;
  removeLabel?: string;
  testId: string;
  tone?: 'normal' | 'reveal';
  action?: { label: string; title: string; onClick: () => void; testId: string };
}) {
  return (
    <span
      className={`inline-flex max-w-[22rem] items-center gap-1 rounded-full border py-0.5 pl-2 ${
        onRemove || action ? 'pr-1' : 'pr-2'
      } ${tone === 'reveal' ? 'border-accent-30 bg-accent-10' : 'border-border bg-background'}`}
      data-testid={testId}
    >
      <span className="truncate" title={label}>
        {label}
      </span>
      {action && (
        <button
          type="button"
          className="rounded-full px-1.5 font-medium text-accent hover:bg-accent-20"
          title={action.title}
          onClick={action.onClick}
          data-testid={action.testId}
        >
          {action.label}
        </button>
      )}
      {onRemove && (
        <button
          type="button"
          className="rounded-full p-0.5 text-text-secondary hover:bg-accent-10 hover:text-text-primary"
          title={removeLabel}
          aria-label={removeLabel ? `${removeLabel}: ${label}` : label}
          onClick={onRemove}
          data-testid={`${testId}-remove`}
        >
          <CloseIcon size={9} />
        </button>
      )}
    </span>
  );
}

export function FilterChips({ model, view }: { model: DataModel; view: DataView }) {
  const { t, i18n } = useTranslation();
  const filters = useDataWorkspaceStore((s) => s.filters);
  const revealRowKeys = useDataWorkspaceStore((s) => s.revealRowKeys);
  const search = useDataWorkspaceStore((s) => s.search);
  const store = useDataWorkspaceStore.getState;

  const navKeys = new Set(revealRowKeys);
  let byNav = 0;
  for (const k of view.revealedRowKeys) if (navKeys.has(k)) byNav++;
  const bySearch = view.revealedRowKeys.size - byNav;
  const revealedCols = view.revealedColumns.size;

  const anything = filtersActive(filters) || view.revealedRowKeys.size > 0 || revealedCols > 0;
  if (!anything) return null;

  const remove = t('dataToolbar.chips.remove');
  const chips = [];

  if (filters.rowTypes !== 'all') {
    chips.push(
      <Chip
        key="rowtype"
        label={t(`dataToolbar.chips.${filters.rowTypes}`)}
        onRemove={() => store().setFilters({ rowTypes: 'all' })}
        removeLabel={remove}
        testId="data-chip-rowtype"
      />,
    );
  }
  if (filters.sectionIds !== null) {
    const names = filters.sectionIds.map((id) => model.ctx.sectionNames.get(id) ?? String(id));
    const shown = names.slice(0, SECTION_NAMES_SHOWN).join(', ');
    const label =
      names.length === 0
        ? t('dataToolbar.chips.sectionsNone')
        : names.length > SECTION_NAMES_SHOWN
          ? t('dataToolbar.chips.sectionsMore', { names: shown, count: names.length - SECTION_NAMES_SHOWN })
          : t('dataToolbar.chips.sections', { names: shown });
    chips.push(
      <Chip
        key="sections"
        label={label}
        onRemove={() => store().setFilters({ sectionIds: null })}
        removeLabel={remove}
        testId="data-chip-sections"
      />,
    );
  }
  for (const key of QUICK_FILTER_KEYS) {
    if (!filters.quick[key]) continue;
    chips.push(
      <Chip
        key={`quick-${key}`}
        label={t(`dataToolbar.chips.quick.${key}`)}
        onRemove={() => store().setQuickFilter(key, false)}
        removeLabel={remove}
        testId={`data-chip-quick-${key}`}
      />,
    );
  }
  for (const c of filters.conditions) {
    const meta = DATA_COLUMN_BY_KEY.get(c.colKey);
    chips.push(
      <Chip
        key={`cond-${c.id}`}
        label={meta ? conditionText(t, i18n, c, meta) : c.colKey}
        onRemove={() => store().removeCondition(c.id)}
        removeLabel={remove}
        testId={`data-chip-condition-${testIdPart(c.id)}`}
      />,
    );
  }
  if (view.revealedRowKeys.size > 0) {
    const key = byNav > 0 && bySearch > 0 ? 'revealedBoth' : byNav > 0 ? 'revealedNav' : 'revealedSearch';
    chips.push(
      <Chip
        key="revealed"
        tone="reveal"
        label={tCount(t, `dataToolbar.chips.${key}`, view.revealedRowKeys.size)}
        testId="data-chip-revealed"
        action={{
          label: t('dataToolbar.chips.hide'),
          title: t('dataToolbar.chips.hideTitle'),
          testId: 'data-chip-revealed-hide',
          onClick: () => {
            store().clearRevealed();
            if (search.rowScope === 'all') store().setSearch({ rowScope: 'visible' });
          },
        }}
      />,
    );
  }
  if (revealedCols > 0) {
    chips.push(
      <Chip
        key="revealed-cols"
        tone="reveal"
        label={tCount(t, 'dataToolbar.chips.revealedColumns', revealedCols)}
        testId="data-chip-revealed-columns"
        action={{
          label: t('dataToolbar.chips.hide'),
          title: t('dataToolbar.chips.hideTitle'),
          testId: 'data-chip-revealed-columns-hide',
          onClick: () => store().setSearch({ columnScope: 'visible' }),
        }}
      />,
    );
  }

  const clearAll = () => {
    const s = store();
    s.clearFilters();
    s.clearRevealed();
    if (s.search.rowScope === 'all' || s.search.columnScope !== 'visible') {
      s.setSearch({ rowScope: 'visible', columnScope: 'visible' });
    }
  };

  return (
    <div
      className="flex flex-wrap items-center gap-1.5 border-t border-border px-3 py-1 text-xs"
      role="group"
      aria-label={t('dataToolbar.chips.aria')}
      data-testid="data-chips"
    >
      {chips}
      <button
        type="button"
        className="ml-1 rounded px-1.5 py-0.5 text-text-secondary hover:bg-accent-10 hover:text-text-primary"
        title={t('dataToolbar.chips.clearAllTitle')}
        onClick={clearAll}
        data-testid="data-chips-clear-all"
      >
        {t('dataToolbar.chips.clearAll')}
      </button>
    </div>
  );
}
