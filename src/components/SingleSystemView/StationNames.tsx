import { useTranslation } from 'react-i18next';
import { displayStationName } from '../../utils/formatters';
import type { Station } from '../../types/project';
import type { IdentityLabels } from '../../utils/stationIdentity';
import { DetailRow } from './DetailPanelParts';

/**
 * The Station details' Name.
 *
 * A junction is ONE station with one Station ID that several lines pass, and
 * each line shows it by the name that line knows it by (plan 5.3 per line,
 * I-3): a line merged onto it keeps its own name for it. So when the lines show
 * it by different names, the Name row says which line this name is on ("0 (on
 * North Passage)"). The names the other lines show are on their "Also on" rows
 * (`StationOtherLines`), said once there (demo finding N-3: a Junction row
 * repeated them).
 */
export function StationNameRows({ station, labels }: { station: Station; labels: IdentityLabels }) {
  const { t } = useTranslation();
  const own = labels.stationName(station.station_ref);
  const differs = station.merged_with.some((ref) => labels.stationName(ref) !== own);
  const name = displayStationName(own);
  const value = differs
    ? t('detailPanel.station.nameOnLine', { name, line: labels.lineName(station.station_ref.line_id) })
    : name;
  return <DetailRow label={t('detailPanel.labels.name')} value={value} testId="station-detail-name" />;
}
