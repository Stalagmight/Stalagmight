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
 * I-3): a line merged onto it keeps its own name for it. The Name row is the
 * name on the selected slot's line, which the Line row right under it names;
 * it no longer says "(on North Passage)" too (demo finding V-8: the line was
 * named three times). The names the other lines show are on their "Also on"
 * rows (`StationOtherLines`), said once there (demo finding N-3: a Junction
 * row repeated them).
 */
export function StationNameRows({ station, labels }: { station: Station; labels: IdentityLabels }) {
  const { t } = useTranslation();
  const name = displayStationName(labels.stationName(station.station_ref));
  return <DetailRow label={t('detailPanel.labels.name')} value={name} testId="station-detail-name" />;
}
