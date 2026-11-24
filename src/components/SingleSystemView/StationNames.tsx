import { useTranslation } from 'react-i18next';
import { displayStationName } from '../../utils/formatters';
import type { Station } from '../../types/project';
import type { LoopClosure } from '../../types/ipc/LoopClosure';
import type { IdentityLabels } from '../../utils/stationIdentity';
import { useProse } from '../../hooks/useProse';
import { proseToken } from '../../utils/prose';
import { ownStationPart, reworded } from '../../utils/proseNameParts';
import { Prose } from '../common/Prose';
import { DetailRow } from './DetailPanelParts';

/**
 * The Station details' Name and Station ID: the identity the slot shows (RS
 * 6). A junction shows its kept station on every line (RS 4.1), so on a line
 * that knew another station there (a foreign slot) the details say whose
 * station it is ("12 belongs to Main Line"). A loop end (a node a loop closure
 * shows as its other end, RS 5) opens with "Loop closure end, shown as Main /
 * 3" and its own station, hidden until the closure is removed; its position,
 * depth and datum below are its own. The other lines are on the "Also on"
 * rows (`StationOtherLines`), without names of their own.
 *
 * Each sentence draws the stations and lines it names as prose pills
 * (RS-PROSE); the own name of a loop end's sentence, which states its Station
 * ID, draws none.
 */
export function StationNameRows({
  station,
  labels,
  closures,
}: {
  station: Station;
  labels: IdentityLabels;
  closures?: ReadonlyArray<LoopClosure> | undefined;
}) {
  const { t } = useTranslation();
  const prose = useProse();
  const ref = station.station_ref;
  const name = displayStationName(labels.stationName(ref));
  const shown = labels.shown(ref);
  const shownId = labels.shownStationId(ref);
  const archive =
    shown?.loop_end && shown.loop_closure !== undefined
      ? closures?.find((c) => c.id === shown.loop_closure)?.archived
      : undefined;
  return (
    <>
      {shown?.loop_end && (
        <div className="p-2 rounded bg-accent-10 text-sm space-y-0.5" data-testid="station-detail-loop-end">
          <p data-testid="station-detail-loop-end-shown">
            <Prose
              text={t('detailPanel.station.loopEnd', {
                station: archive ? prose.identity(archive.shown_as) : prose.station(ref, true),
              })}
            />
          </p>
          {archive && (
            <p className="text-xs text-text-secondary" data-testid="station-detail-loop-end-own">
              <Prose
                text={t('detailPanel.station.loopEndOwn', {
                  name: proseToken(ownStationPart(displayStationName(archive.station.name), archive.station.home_line)),
                  id: archive.station.id,
                })}
              />
            </p>
          )}
        </div>
      )}
      <DetailRow label={t('detailPanel.labels.name')} value={name} testId="station-detail-name" />
      {shown?.foreign && !shown.loop_end && shown.home_line !== undefined && shown.home_line !== ref.line_id && (
        <p className="text-xs text-text-secondary" data-testid="station-detail-belongs">
          <Prose
            text={t('detailPanel.station.belongsTo', {
              name: reworded(prose.station(ref), name),
              line: prose.line(shown.home_line),
            })}
          />
        </p>
      )}
      {shownId !== null && (
        <DetailRow label={t('identity.stationId')} value={String(shownId)} testId="station-detail-id" />
      )}
    </>
  );
}
