/**
 * "Whose station is this?" (RS 5.4, U-P2-5). A rename or comment edit through
 * a slot changes the identity the slot SHOWS, on every line it is shown on. On
 * a foreign slot (a junction whose kept station is another line's) or a loop
 * end (a station a loop closure shows as its other end) that is not the
 * line's own station, so the Edit Station dialog and the data table first say
 * whose it is and offer the way back to the line's own station:
 *
 * - "12 belongs to Main Line. Rename Main Line / 12 to 12J on Main Line and
 *   Side / Detach Side to get back its own station 0 / Cancel"
 * - "Shown as Main / 20 through a loop closure. Rename Main / 20 to 20E on
 *   Main and Loop / Remove the closure / Cancel"
 *
 * Both variants word the action the same way, with the new name and the
 * lines it changes on (RS-FX6, P3-11): "Rename Main / 8" and "Rename it on
 * every line" were one action in two wordings, neither saying to what, while
 * the cell cursor had already moved on.
 *
 * The facts come from `preview_station_identity`; while it is out, the words
 * come from the slot's own shown fields.
 */
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
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
import type { StationGridEdit } from '../SingleSystemView/stationGridEdits';
import { Button } from '../common/Button';
import { useIdentityLabel } from './keptStationParts';

/** What the edit changes: the name, the comment, or both. */
export type ShownEdit = 'name' | 'comment' | 'both';

/** Whether an edit through `ref` must say whose station it is first: the
 *  slot is foreign or a loop end (RS 5.4). */
export function editNeedsOwnerPrompt(shown: { foreign: boolean; loop_end: boolean } | undefined): boolean {
  return shown !== undefined && (shown.foreign || shown.loop_end);
}

/**
 * The go-on button's words: what the edit does to which station, to what
 * name, and on which lines ("Rename Main / 8 to 8E on Main and Loop"). The
 * lines once the preview has answered; the new name when the caller gives it.
 */
export function proceedText(
  t: TFunction,
  edit: ShownEdit,
  station: string,
  to: string | undefined,
  lines: string | null,
): string {
  if ((edit === 'name' || edit === 'both') && (to === undefined || to === '')) {
    return t(`dialogs.shownIdentity.proceedLoop.${edit}`, { station });
  }
  const key = lines === null ? 'proceedTo' : 'proceedOn';
  return t(`dialogs.shownIdentity.${key}.${edit}`, { station, to, lines });
}

export function ShownIdentityPrompt({
  stationRef,
  edit,
  newName,
  onProceed,
  onCancel,
  busy = false,
}: {
  stationRef: StationRef;
  edit: ShownEdit;
  /** The name the edit sets, said on the go-on button. */
  newName?: string | undefined;
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
          {/* The lines are named once, on the button (RS-FX8, N6). */}
          {t(`dialogs.shownIdentity.changes.${edit}`)}
        </p>
      )}
      <div className="flex flex-col gap-2">
        <Button
          variant="primary"
          onClick={onProceed}
          disabled={busy || editingBlocked}
          data-testid="shown-identity-prompt-proceed"
        >
          {proceedText(t, edit, shownLabel, newName?.trim(), lines)}
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

/**
 * The prompt for one grid commit over several cells (a paste, a fill, a range
 * clear; review finding ui-2): every station it changes that other lines
 * show too, each with the lines the edit changes it on, then one answer for
 * the whole commit. Going on sends every edit of it, the native ones too;
 * Cancel drops them all, so the commit is applied whole or not at all.
 */
export function ShownIdentityBatchPrompt({
  edits,
  onProceed,
  onCancel,
  busy = false,
}: {
  edits: readonly StationGridEdit[];
  onProceed: () => void;
  onCancel: () => void;
  busy?: boolean;
}) {
  const { t } = useTranslation();
  const labels = useIdentityLabels();
  const label = useIdentityLabel();
  const editingBlocked = useEditingBlocked();
  // One row per station asked about, in the commit's order.
  const asked: StationRef[] = [];
  for (const e of edits) {
    if (!e.ask) continue;
    const same = (r: StationRef) => r.line_id === e.stationRef.line_id && r.station_index === e.stationRef.station_index;
    if (!asked.some(same)) asked.push(e.stationRef);
  }
  const askedKey = asked.map((r) => `${r.line_id}:${r.station_index}`).join(',');
  const [owners, setOwners] = useState<ReadonlyMap<string, StationOwner>>(new Map());
  useEffect(() => {
    let live = true;
    const refs = askedKey === '' ? [] : askedKey.split(',').map((k) => {
      const [line_id, station_index] = k.split(':').map(Number);
      return { line_id: line_id!, station_index: station_index! };
    });
    void Promise.all(
      refs.map((stationRef) =>
        Promise.resolve()
          .then(() =>
            invokeIpc<StationOwner>('preview_station_identity', { stationRef }, { purpose: 'preview station identity' }),
          )
          .then((owner): [string, StationOwner] | null =>
            owner ? [`${stationRef.line_id}:${stationRef.station_index}`, owner] : null,
          )
          .catch(() => null),
      ),
    ).then((answers) => {
      if (live) setOwners(new Map(answers.filter((a): a is [string, StationOwner] => a !== null)));
    });
    return () => {
      live = false;
    };
  }, [askedKey]);

  return (
    <div className="space-y-3 text-sm" data-testid="shown-identity-batch-prompt">
      <p className="text-text-primary" data-testid="shown-identity-batch-lead">
        {t('dialogs.shownIdentity.batchLead', { count: asked.length })}
      </p>
      <ul className="list-disc pl-5 space-y-1">
        {asked.map((ref, i) => {
          const owner = owners.get(`${ref.line_id}:${ref.station_index}`);
          const shown = labels.shown(ref);
          const loopEnd = owner ? owner.loop_closure !== undefined : shown?.loop_end === true;
          const home = owner?.shown.home_line ?? shown?.home_line;
          const lead = loopEnd
            ? t('dialogs.shownIdentity.batchLoopEnd', {
                line: labels.lineName(ref.line_id),
                name: displayStationName(labels.stationName(ref)),
                station: owner ? label(owner.shown) : labels.station(ref, true),
              })
            : t('dialogs.shownIdentity.batchBelongs', {
                line: labels.lineName(ref.line_id),
                name: displayStationName(labels.stationName(ref)),
                home: home !== undefined ? labels.lineName(home) : labels.lineName(ref.line_id),
              });
          const lines = owner && owner.lines.length > 0 ? listNames(owner.lines.map(labels.lineName), t) : null;
          return (
            <li key={`${ref.line_id}:${ref.station_index}`} data-testid={`shown-identity-batch-row-${i}`}>
              {lead}
              {lines !== null && ` ${t('dialogs.shownIdentity.batchLines', { lines })}`}
            </li>
          );
        })}
      </ul>
      <p className="text-text-secondary" data-testid="shown-identity-batch-held">
        {t('dialogs.shownIdentity.batchHeld')}
      </p>
      <div className="flex flex-col gap-2">
        <Button
          variant="primary"
          onClick={onProceed}
          disabled={busy || editingBlocked}
          data-testid="shown-identity-prompt-proceed"
        >
          {t('dialogs.shownIdentity.batchProceed', { count: edits.length })}
        </Button>
        <Button onClick={onCancel} disabled={busy} data-testid="shown-identity-prompt-cancel">
          {t('dialogs.shownIdentity.cancel')}
        </Button>
      </div>
    </div>
  );
}
