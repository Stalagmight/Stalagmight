/**
 * "Open in Data" in the docked DataTable's sub-header: opens the Data
 * workspace filtered to the table's section (a removable filter chip there)
 * at the row selected in Survey - the selected shot or station, else the
 * section's first station.
 */
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { setSource } from '../../utils/telemetry';
import { ListIcon } from '../common/icons';
import { useDataWorkspaceStore } from './store';
import { shotRowKey, stationRowKey } from './model/rows';

/** The Data row key for Survey's current selection inside `sectionId`. */
export function rowKeyForSurveySelection(sectionId: number): string {
  const sel = useUIStore.getState().selection;
  if (sel?.type === 'shot' && sel.sectionId === sectionId && sel.shotId !== undefined) return shotRowKey(sectionId, sel.shotId);
  if (sel?.type === 'station' && sel.stationRef?.section_id === sectionId) return stationRowKey(sectionId, sel.stationRef.station_index);
  return stationRowKey(sectionId, 0);
}

export function openSectionInData(sectionId: number): void {
  const store = useDataWorkspaceStore.getState();
  store.setFilters({ sectionIds: [sectionId] });
  store.requestFocus({ rowKey: rowKeyForSurveySelection(sectionId), align: 'center' });
  setSource('data-table');
  useUIStore.getState().setAppMode('data');
}

export function OpenInDataButton({ sectionId }: { sectionId: number | null }) {
  const { t } = useTranslation();
  if (sectionId === null) return null;
  return (
    <button
      type="button"
      onClick={() => openSectionInData(sectionId)}
      className="text-text-secondary hover:text-text"
      title={t('dataWorkspace.openInData.title')}
      aria-label={t('dataWorkspace.openInData.label')}
      data-testid="data-table-open-in-data"
    >
      <ListIcon size={14} />
    </button>
  );
}
