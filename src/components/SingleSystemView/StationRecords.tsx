import { useTranslation } from 'react-i18next';
import { displayStationName } from '../../utils/formatters';
import type { ArchivedStation } from '../../types/ipc/ArchivedStation';
import { useProse } from '../../hooks/useProse';
import { proseToken } from '../../utils/prose';
import { ownStationPart } from '../../utils/proseNameParts';
import { Prose } from '../common/Prose';

/**
 * The records of a node grouped by the station each was archived under
 * (RS-FX4: `into`), in list order: the top-level ones (archived under the
 * node's own identity) with, under each, the records archived inside it. A
 * record whose owner is not in the list, or whose owners cycle, is listed
 * at the top level, so every record is listed exactly once.
 */
export function recordTree(
  records: ReadonlyArray<ArchivedStation>,
): { top: ArchivedStation[]; under: Map<number, ArchivedStation[]> } {
  const byId = new Map(records.map((r) => [r.id, r] as const));
  const reachesTop = (record: ArchivedStation): boolean => {
    const seen = new Set<number>([record.id]);
    let owner = record.into;
    while (owner !== undefined) {
      const next = byId.get(owner);
      if (next === undefined) return true;
      if (seen.has(owner)) return false;
      seen.add(owner);
      owner = next.into;
    }
    return true;
  };
  const top: ArchivedStation[] = [];
  const under = new Map<number, ArchivedStation[]>();
  for (const record of records) {
    if (record.into === undefined || !byId.has(record.into) || !reachesTop(record)) {
      top.push(record);
    } else {
      const list = under.get(record.into) ?? [];
      list.push(record);
      under.set(record.into, list);
    }
  }
  return { top, under };
}

/**
 * The Station details' "Archived here" (RS 3, 6): the stations merges
 * archived in this one (its records, oldest first). Each keeps its own name,
 * comment and Station ID, and is shown nowhere while archived; a record that
 * names lines was those lines' own station here, and Detach Line from Station
 * on one of them brings it back. A record that names no line is kept only for
 * its Station ID (A5), so a surveyor can still find it from the field notes.
 * A record a later merge archived together with the station it was archived
 * in is listed under that station (RS-FX4): Detach brings the two back
 * together, the one still inside the other. Nothing is rendered when the
 * station has none.
 *
 * Each record's sentence draws its name and the lines it names as prose
 * pills (RS-PROSE): the name coloured by the record's first line (its home,
 * as `recordIdentity` reads it), without the superscript Station ID the
 * sentence states itself.
 */
export function StationRecords({ records }: { records: ReadonlyArray<ArchivedStation> | undefined }) {
  const { t } = useTranslation();
  const prose = useProse();
  if (!records || records.length === 0) return null;
  const { top, under } = recordTree(records);
  const item = (record: ArchivedStation, depth: number) => {
    const name = record.name !== '' ? displayStationName(record.name) : String(record.id);
    const comment = (record.comment ?? '').trim();
    const inside = under.get(record.id) ?? [];
    const namePill = proseToken(ownStationPart(name, record.lines[0]));
    return (
      <li key={record.id} data-testid={`station-detail-record-${record.id}`}>
        <span data-testid={`station-detail-record-${record.id}-text`}>
          <Prose
            text={
              record.lines.length > 0
                ? t('detailPanel.station.recordOfLines', {
                    name: namePill,
                    id: record.id,
                    lines: record.lines.map((lineId) => prose.line(lineId)).join(', '),
                  })
                : t('detailPanel.station.recordHistory', { name: namePill, id: record.id })
            }
          />
        </span>
        {comment !== '' && (
          <span className="block text-xs text-text-secondary" data-testid={`station-detail-record-${record.id}-comment`}>
            {t('detailPanel.station.recordComment', { comment })}
          </span>
        )}
        {inside.length > 0 && depth < records.length && (
          <ul
            className="mt-1 ml-3 space-y-1 border-l border-border pl-2"
            data-testid={`station-detail-record-${record.id}-inside`}
            aria-label={t('detailPanel.station.recordInside', { name })}
          >
            {inside.map((r) => item(r, depth + 1))}
          </ul>
        )}
      </li>
    );
  };
  return (
    <div className="pt-2 border-t border-border" data-testid="station-detail-records">
      <span className="text-sm text-text-secondary" title={t('detailPanel.station.archivedHint')}>
        {t('detailPanel.station.archived')}:
      </span>
      <ul className="mt-1 space-y-1 text-sm">{top.map((record) => item(record, 0))}</ul>
    </div>
  );
}
