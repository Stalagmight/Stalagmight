/**
 * "Whose station is this?" (RS 5.4, U-P2-5). A rename or comment edit through
 * a slot changes the identity the slot SHOWS, on every line it is shown on. On
 * a foreign slot (a junction whose kept station is another line's) or a loop
 * end (a station a loop closure shows as its other end) that is not the
 * line's own station, so the Edit Station dialog and the data table first say
 * whose it is and offer the way back to the line's own station:
 *
 * - "12 belongs to Main Line. Rename it on every line / Detach Side to get
 *   back its own station 0 / Cancel"
 * - "Shown as Main / 20 through a loop closure. Rename Main / 20 / Remove the
 *   closure / Cancel"
 *
 * The facts come from `preview_station_identity`; while it is out, the words
 * come from the slot's own shown fields.
 */
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invokeIpc } from '../../utils/ipc';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useEditingBlocked } from '../../hooks/useEditingBlocked';
import { listNames } from '../../utils/listNames';
import { displayStationName } from '../../utils/formatters';
import { removeClosureAsking } from '../../utils/keptStationActions';
import type { StationRef } from '../../types/project';
import type { StationOwner } from '../../types/ipc/StationOwner';
import { Button } from '../common/Button';
import { useIdentityLabel } from './keptStationParts';

/** What the edit changes: the name, the comment, or both. */
export type ShownEdit = 'name' | 'comment' | 'both';

/** Whether an edit through `ref` must say whose station it is first: the
 *  slot is foreign or a loop end (RS 5.4). */
export function editNeedsOwnerPrompt(shown: { foreign: boolean; loop_end: boolean } | undefined): boolean {
  return shown !== undefined && (shown.foreign || shown.loop_end);
}

export function ShownIdentityPrompt({
  stationRef,
  edit,
  onProceed,
  onCancel,
  busy = false,
}: {
  stationRef: StationRef;
  edit: ShownEdit;
  /** Go on with the edit: it changes the shown station on every line. */
  onProceed: () => void;
  /** Back to editing (the Edit Station dialog) or drop the edit (the table). */
  onCancel: () => void;
  busy?: boolean;
}) {
  const { t } = useTranslation();
  const labels = useIdentityLabels();
  const label = useIdentityLabel();
  const stations = useProjectStore((s) => s.stations);
  // Every way on edits (ui-5): the read-only gate disables them; Cancel stays.
  const editingBlocked = useEditingBlocked();
  const [owner, setOwner] = useState<StationOwner | null>(null);
  useEffect(() => {
    let live = true;
    Promise.resolve(
      invokeIpc<StationOwner>('preview_station_identity', { stationRef }, { purpose: 'preview station identity' }),
    )
      .then((answer) => {
        if (live && answer) setOwner(answer);
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [stationRef]);

  const shown = labels.shown(stationRef);
  const loopEnd = owner ? owner.loop_closure !== undefined : shown?.loop_end === true;
  const loopClosure = owner?.loop_closure ?? shown?.loop_closure;
  const name = owner ? displayStationName(owner.shown.name) : displayStationName(labels.stationName(stationRef));
  const shownLabel = owner ? label(owner.shown) : labels.station(stationRef, true);
  const homeLine = owner?.shown.home_line ?? shown?.home_line;
  const line = labels.lineName(stationRef.line_id);
  const lines = owner && owner.lines.length > 0 ? listNames(owner.lines.map(labels.lineName), t) : null;

  const detach = () => {
    const station = stations.find(
      (s) => s.station_ref.line_id === stationRef.line_id && s.station_ref.station_index === stationRef.station_index,
    );
    useUIStore.getState().openDialog('unmerge-stations', {
      stationRef,
      mergedStations: station?.merged_with ?? [],
      chosenLine: stationRef.line_id,
    });
  };
  const removeClosure = () => {
    if (loopClosure === undefined) return;
    useUIStore.getState().closeDialog();
    void removeClosureAsking(BigInt(loopClosure), stationRef);
  };

  return (
    <div className="space-y-3 text-sm" data-testid="shown-identity-prompt">
      <p className="text-text-primary" data-testid="shown-identity-prompt-lead">
        {loopEnd
          ? t('dialogs.shownIdentity.loopEnd', { station: shownLabel })
          : t('dialogs.shownIdentity.belongs', {
              name,
              line: homeLine !== undefined ? labels.lineName(homeLine) : line,
            })}
      </p>
      {lines !== null && (
        <p className="text-text-secondary" data-testid="shown-identity-prompt-lines">
          {t(`dialogs.shownIdentity.changes.${edit}`, { lines, count: owner?.lines.length ?? 0 })}
        </p>
      )}
      <div className="flex flex-col gap-2">
        <Button
          variant="primary"
          onClick={onProceed}
          disabled={busy || editingBlocked}
          data-testid="shown-identity-prompt-proceed"
        >
          {loopEnd
            ? t(`dialogs.shownIdentity.proceedLoop.${edit}`, { station: shownLabel })
            : t(`dialogs.shownIdentity.proceed.${edit}`)}
        </Button>
        {loopEnd && loopClosure !== undefined && (
          <Button
            onClick={removeClosure}
            disabled={busy || editingBlocked}
            data-testid="shown-identity-prompt-remove-closure"
          >
            {t('dialogs.shownIdentity.removeClosure')}
          </Button>
        )}
        {!loopEnd && owner?.own_station && (
          <Button onClick={detach} disabled={busy || editingBlocked} data-testid="shown-identity-prompt-detach">
            {t('dialogs.shownIdentity.detach', {
              line,
              name: displayStationName(owner.own_station.name),
            })}
          </Button>
        )}
        <Button onClick={onCancel} disabled={busy} data-testid="shown-identity-prompt-cancel">
          {t('dialogs.shownIdentity.cancel')}
        </Button>
      </div>
    </div>
  );
}
