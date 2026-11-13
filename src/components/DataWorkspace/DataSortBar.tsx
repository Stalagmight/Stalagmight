/**
 * "Sorted by X (ascending). Survey order and the link rail are off.
 * [Back to survey order]" - shown under the toolbar while a sort is active, in
 * the quiet info style of the app's banners (UpdateBanner).
 */
import { useTranslation } from 'react-i18next';
import { useDataWorkspaceStore } from './store';
import { DATA_COLUMN_BY_KEY } from './columnsMeta';
import { InfoIcon } from '../common/icons';
import { columnLabel } from './toolbar/labels';

export function DataSortBar() {
  const { t, i18n } = useTranslation();
  const sort = useDataWorkspaceStore((s) => s.sort);
  const setSort = useDataWorkspaceStore((s) => s.setSort);
  if (!sort) return null;
  const meta = DATA_COLUMN_BY_KEY.get(sort.colKey);
  const column = meta ? columnLabel(t, i18n, meta) : sort.colKey;
  return (
    <div
      className="flex shrink-0 items-center gap-3 border-b border-accent-20 bg-accent-10 px-3 py-1 text-xs text-text-primary"
      role="status"
      data-testid="data-sort-bar"
    >
      <span className="shrink-0 text-accent">
        <InfoIcon size={14} />
      </span>
      <span className="min-w-0 flex-1" data-testid="data-sort-bar-text">
        {t('dataToolbar.sortBar.text', { column, dir: t(`dataToolbar.sortBar.${sort.dir}`) })}
      </span>
      <button
        type="button"
        className="shrink-0 rounded border border-border bg-button-secondary px-2 py-0.5 hover:bg-button-secondary-hover"
        onClick={() => setSort(null)}
        data-testid="data-sort-reset"
      >
        {t('dataToolbar.sortBar.reset')}
      </button>
    </div>
  );
}
