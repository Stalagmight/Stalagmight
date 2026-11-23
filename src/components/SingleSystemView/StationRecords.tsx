import { useTranslation } from 'react-i18next';
import { displayStationName } from '../../utils/formatters';
import type { ArchivedStation } from '../../types/ipc/ArchivedStation';

/**
 * The Station details' "Archived here" (RS 3, 6): the stations merges
 * archived in this one (its records, oldest first). Each keeps its own name,
 * comment and Station ID, and is shown nowhere while archived; a record that
 * names lines was those lines' own station here, and Detach Line from Station
 * on one of them brings it back. A record that names no line is kept only for
 * its Station ID (A5), so a surveyor can still find it from the field notes.
 * Nothing is rendered when the station has none.
 */
export function StationRecords({
  records,
  lineName,
}: {
  records: ReadonlyArray<ArchivedStation> | undefined;
  lineName: (lineId: number) => string;
}) {
  const { t } = useTranslation();
  if (!records || records.length === 0) return null;
  return (
    <div className="pt-2 border-t border-border" data-testid="station-detail-records">
      <span className="text-sm text-text-secondary" title={t('detailPanel.station.archivedHint')}>
        {t('detailPanel.station.archived')}:
      </span>
      <ul className="mt-1 space-y-1 text-sm">
        {records.map((record) => {
          const name = record.name !== '' ? displayStationName(record.name) : String(record.id);
          const comment = (record.comment ?? '').trim();
          return (
            <li key={record.id} data-testid={`station-detail-record-${record.id}`}>
              {record.lines.length > 0
                ? t('detailPanel.station.recordOfLines', {
                    name,
                    id: record.id,
                    lines: record.lines.map(lineName).join(', '),
                  })
                : t('detailPanel.station.recordHistory', { name, id: record.id })}
              {comment !== '' && (
                <span className="block text-xs text-text-secondary" data-testid={`station-detail-record-${record.id}-comment`}>
                  {t('detailPanel.station.recordComment', { comment })}
                </span>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
