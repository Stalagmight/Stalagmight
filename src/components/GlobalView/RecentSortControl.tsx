import { useTranslation } from 'react-i18next';
import { Select } from '../common/Select';
import { IconButton, ChevronDownIcon, ChevronUpIcon } from '../common';
import type {
  RecentSortDirection,
  RecentSortField,
} from './useRecentSort';

interface RecentSortControlProps {
  field: RecentSortField;
  direction: RecentSortDirection;
  onFieldChange: (field: RecentSortField) => void;
  onToggleDirection: () => void;
}

/**
 * "Sort by" control for the recent-projects list: a field selector plus a
 * direction toggle. State is owned by the caller (persisted via useRecentSort).
 */
export function RecentSortControl({
  field,
  direction,
  onFieldChange,
  onToggleDirection,
}: RecentSortControlProps) {
  const { t } = useTranslation();

  const options = [
    { value: 'last_opened', label: t('globalView.recent.sort.lastOpened') },
    { value: 'last_edited', label: t('globalView.recent.sort.lastEdited') },
    { value: 'name', label: t('globalView.recent.sort.name') },
    { value: 'lines', label: t('globalView.recent.sort.sections') },
    { value: 'length', label: t('globalView.recent.sort.length') },
  ];

  const directionLabel =
    direction === 'asc'
      ? t('globalView.recent.sort.ascending')
      : t('globalView.recent.sort.descending');

  return (
    <div className="flex items-center gap-2">
      <span className="text-xs text-text-secondary whitespace-nowrap">
        {t('globalView.recent.sort.label')}
      </span>
      <Select
        value={field}
        onChange={(value) => onFieldChange(value as RecentSortField)}
        options={options}
        className="min-w-36"
        data-testid="recent-sort-field"
        aria-label={t('globalView.recent.sort.label')}
      />
      <IconButton
        icon={direction === 'asc' ? <ChevronUpIcon /> : <ChevronDownIcon />}
        size="sm"
        variant="secondary"
        onClick={onToggleDirection}
        tooltip={directionLabel}
        aria-label={directionLabel}
        data-testid="recent-sort-direction"
      />
    </div>
  );
}

export default RecentSortControl;
