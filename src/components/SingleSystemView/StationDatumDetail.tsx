import { useTranslation } from 'react-i18next';
import type { GpsCoordinates } from '../../types/project';
import type { ZeroDatum } from '../../types/ipc/ZeroDatum';
import { formatFixed } from '../../i18n/format';
import { compassPoint, formatDatumMetres } from '../../utils/zeroDatumPreview';
import { DetailRow } from './DetailPanelParts';

/**
 * The zero-datum part of the station Details (#741): the datum's role and, for
 * a GPS fix, how far its GPS is from the surveyed position. A fix's lat/lon is
 * labelled as the GPS reading, because the Raw Data View does not draw the
 * station there. The role and the gap come from the backend on the wire datum;
 * nothing here re-derives them. `datum` is undefined when the wire dropped it
 * (a dangling datum), and then only the plain label shows.
 */
export function StationDatumDetail({
  datum,
  gps,
  gpsPrecision,
}: {
  datum: ZeroDatum | undefined;
  gps: GpsCoordinates | null;
  gpsPrecision: number;
}) {
  const { t } = useTranslation();
  const isFix = datum?.role === 'GpsFix';
  const gap = isFix ? datum.gap : undefined;
  const title = !datum
    ? t('detailPanel.station.zeroDatum')
    : isFix
      ? t('detailPanel.station.gpsFix')
      : t('detailPanel.station.placementDatum');
  return (
    <div className="pt-2 border-t border-border" data-testid="station-datum-detail">
      <span className="text-warning" data-testid="station-datum-detail-role">&#9733; {title}</span>
      {datum && (
        <p className="mt-1 text-xs text-text-secondary">
          {isFix ? t('detailPanel.station.gpsFixHint') : t('detailPanel.station.placementDatumHint')}
        </p>
      )}
      {gap && (
        <div className="mt-1 text-sm space-y-1">
          <p data-testid="station-datum-detail-gap">
            {t('detailPanel.station.gpsFixGap', {
              distance: formatDatumMetres(gap.gap_m),
              direction: compassPoint(gap.bearing_deg),
            })}
          </p>
          {gap.exceeds_threshold && (
            <p className="p-2 rounded bg-warning-10 text-warning-text text-xs" data-testid="station-datum-detail-gap-warning">
              {t('detailPanel.station.gpsFixGapExceeds', { threshold: formatDatumMetres(gap.threshold_m) })}
            </p>
          )}
        </div>
      )}
      {gps && (
        <div className="mt-2 text-sm">
          {isFix && (
            <div className="text-xs text-text-secondary" data-testid="station-datum-detail-gps-reading">
              {t('detailPanel.station.gpsReading')}
            </div>
          )}
          <DetailRow
            label={t('detailPanel.labels.latitude')}
            value={formatFixed(gps.latitude, undefined, gpsPrecision)}
          />
          <DetailRow
            label={t('detailPanel.labels.longitude')}
            value={formatFixed(gps.longitude, undefined, gpsPrecision)}
          />
        </div>
      )}
    </div>
  );
}
