import { useTranslation } from 'react-i18next';
import { displayStationName } from '../../utils/formatters';
import type { Station } from '../../types/project';
import type { IdentityLabels } from '../../utils/stationIdentity';
import { DetailRow } from './DetailPanelParts';

/** The other lines of a junction, grouped by the name each shows it by, in slot order. */
export function junctionNames(station: Station, labels: IdentityLabels): { name: string; lines: string[] }[] {
  const groups: { name: string; lines: string[] }[] = [];
  for (const ref of station.merged_with) {
    const name = labels.stationName(ref);
    const line = labels.lineName(ref.line_id);
    const group = groups.find((g) => g.name === name);
    if (group) {
      if (!group.lines.includes(line)) group.lines.push(line);
    } else {
      groups.push({ name, lines: [line] });
    }
  }
  return groups;
}

/**
 * The Station details' Name, and for a junction how its other lines show it.
 *
 * A junction is ONE station with one Station ID that several lines pass, and
 * each line shows it by the name that line knows it by (plan 5.3 per line,
 * I-3): a line merged onto it keeps its own name for it. So when the lines show
 * it by different names, the Name row says which line this name is on ("0 (on
 * North Passage)"), and the Junction row lists the others ("3 on Main Line").
 */
export function StationNameRows({ station, labels }: { station: Station; labels: IdentityLabels }) {
  const { t } = useTranslation();
  const own = labels.stationName(station.station_ref);
  const others = junctionNames(station, labels);
  const differs = others.some((group) => group.name !== own);
  const name = displayStationName(own);
  const value = differs
    ? t('detailPanel.station.nameOnLine', { name, line: labels.lineName(station.station_ref.line_id) })
    : name;
  return (
    <>
      <DetailRow label={t('detailPanel.labels.name')} value={value} testId="station-detail-name" />
      {others.length > 0 && (
        <DetailRow
          label={t('detailPanel.station.junction')}
          value={others
            .map((group) =>
              t('detailPanel.station.junctionName', {
                name: displayStationName(group.name),
                lines: group.lines.join(', '),
              })
            )
            .join('; ')}
          testId="station-detail-junction"
        />
      )}
    </>
  );
}
