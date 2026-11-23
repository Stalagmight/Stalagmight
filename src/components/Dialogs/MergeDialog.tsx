import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { notifyRemovedClosures } from '../../utils/closureRemovalToast';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { StationBlob } from '../common/EntityBlob';
import { DialogWrapper } from './DialogWrapper';
import type { StationRef } from '../../types/project';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { listNames } from '../../utils/listNames';
import type { BatchCommand } from '../../types/commands';
import type { MergeDatumShift } from '../../types/ipc/MergeDatumShift';
import type { MergePreview } from '../../types/ipc/MergePreview';
import type { KeepCandidate } from '../../types/ipc/KeepCandidate';
import type { MergeKeepPreview } from '../../types/ipc/MergeKeepPreview';
import { formatDatumMetres } from '../../utils/zeroDatumPreview';
import { formatFixed } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { useEditingBlocked, isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { CollisionRows, IdentityPill, renamesFromAnswers, useCollisionAnswers, useIdentityLabel } from './keptStationParts';

/** A move shorter than this (metres) is not worth a sentence: the two
 *  stations already sit on one point, as after a snap. */
const MOVE_SAID_FROM_M = 0.01;

/** The keep the dialog starts on: the backend's default (a datum holder, else
 *  rule K, RS 4.1) when it offers it, else the one it offers; null without a
 *  preview of the keep. */
export function defaultKeepOf(preview: MergePreview | undefined): number | null {
  if (!preview || preview.keeps.length === 0) return null;
  return preview.keeps.find((k) => k.keep === preview.default_keep)?.keep ?? preview.keeps[0]!.keep;
}

export default function MergeDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, clearMergeStations } = useUIStore();
  const { mergeStations, executeBatch } = useProjectStore();
  // A merge mutates: never from a read-only view (Plan, Export, the Closures
  // Computed View, a read-only project).
  const editingBlocked = useEditingBlocked();

  const stationA = dialogData.stationA as StationRef | undefined;
  const stationB = dialogData.stationB as StationRef | undefined;

  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Optional callbacks from snap-to-merge drag flow
  const onBeforeConfirm = dialogData.onBeforeConfirm as (() => Promise<void>) | undefined;
  const onCancel = dialogData.onCancel as (() => void) | undefined;
  // Move commands to batch with merge for single-undo snap-to-merge
  const preCommands = dialogData.preCommands as BatchCommand[] | undefined;
  // Both pieces placed by their own GPS (#741): the backend's answer to which
  // one moves, from check_merge_connectivity.
  const datumShift = dialogData.datumShift as MergeDatumShift | undefined;
  // What the merge does (demo issue I-4): the piece that moves, how far, and
  // the depths when the two stations disagree, from check_merge_connectivity.
  const mergePreview = dialogData.mergePreview as MergePreview | undefined;

  // Which station is kept (RS 4.1, 7): one radio per station the preview
  // offers, the backend's default first chosen. Without a preview (an older
  // check) the merge keeps the stationary station and letters collisions.
  const candidates = mergePreview?.candidates ?? [];
  const [keep, setKeep] = useState<number | null>(() => defaultKeepOf(mergePreview));
  useEffect(() => {
    setKeep(defaultKeepOf(mergePreview));
  }, [mergePreview]);
  const keepPreview = mergePreview?.keeps.find((k) => k.keep === keep) ?? null;
  const [answers, setAnswer] = useCollisionAnswers(keepPreview?.collisions);

  const handleClose = () => {
    onCancel?.();
    clearMergeStations();
    closeDialog();
  };

  const handleMerge = async () => {
    if (!stationA || !stationB) return;
    if (isEditingBlockedNow()) {
      setError(t('dialogs.keptStations.editingDisabled'));
      return;
    }

    setIsLoading(true);
    setError(null);

    try {
      if (onBeforeConfirm) await onBeforeConfirm();
      // The dialog's answers (RS 4.1): the station kept, and every collision
      // answered, so the merge refuses rather than letter what the user saw
      // differently. Without a preview, neither (the backend's defaults).
      const answered = keepPreview
        ? { keep: keepPreview.keep, renames: renamesFromAnswers(keepPreview.collisions, answers) }
        : {};
      // When preCommands exist (snap-to-merge), batch move + merge as single undo operation
      const mergeCmd: BatchCommand = { type: 'merge_stations', station_a: stationA, station_b: stationB, ...answered };
      // The store's mergeStations toasts a closure the merge removed (#738/#741);
      // the batch reports it the same way, so toast it here too.
      const result = preCommands
        ? notifyRemovedClosures(await executeBatch([...preCommands, mergeCmd]))
        : await mergeStations(stationA, stationB, answered);
      if (result.success) {
        clearMergeStations();
        trackDialogComplete('merge');
        closeDialog();
      } else {
        setError(result.error ?? 'Failed to merge stations');
      }
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to merge stations'));
    } finally {
      setIsLoading(false);
    }
  };

  if (!stationA || !stationB) {
    return null;
  }

  return (
    <DialogWrapper title="Merge Stations" onClose={handleClose} size="lg" data-testid="merge-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">{error}</div>
        )}

        {mergePreview && keepPreview && candidates.length > 0 ? (
          <KeepPicker
            candidates={candidates}
            keeps={mergePreview.keeps}
            keep={keep}
            onKeep={setKeep}
            forced={mergePreview.keep_forced_by !== undefined}
            disabled={isLoading}
          />
        ) : (
          /* Station details */
          <div className="space-y-2">
            <div className="flex justify-between items-center py-2 border-b border-border">
              <span className="text-text-secondary">{t('dialogs.mergeStations.stationA')}</span>
              <span className="inline-flex items-center gap-1.5">
                <StationBlob station={stationA} withLine />
              </span>
            </div>
            <div className="flex justify-between items-center py-2 border-b border-border">
              <span className="text-text-secondary">{t('dialogs.mergeStations.stationB')}</span>
              <span className="inline-flex items-center gap-1.5">
                <StationBlob station={stationB} withLine />
              </span>
            </div>
          </div>
        )}

        {keepPreview && (
          <KeepOutcome
            preview={keepPreview}
            kept={candidates.find((c) => c.identity.id === keepPreview.keep) ?? null}
          />
        )}

        {keepPreview && (
          <CollisionRows
            collisions={keepPreview.collisions}
            answers={answers}
            onAnswer={setAnswer}
            testIdPrefix="merge"
            disabled={isLoading}
          />
        )}

        {datumShift && (
          <DatumShiftNotice
            shift={datumShift}
            // A snap-to-merge drag moves stations first, so the distance the
            // check measured before the drag is not what the merge will move.
            afterMove={preCommands !== undefined}
          />
        )}

        {mergePreview && (
          <MergePreviewNotice
            preview={mergePreview}
            // The datum notice already says what moves and how far; a snap
            // drag already moved it (the check measured before the drag).
            sayMove={!datumShift && preCommands === undefined}
          />
        )}

        {/* Actions */}
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleClose} disabled={isLoading} data-testid="merge-cancel">
            {t('dialogs.mergeStations.cancel')}
          </Button>
          <Button variant="primary" onClick={handleMerge} disabled={isLoading || editingBlocked} data-testid="merge-confirm">
            {isLoading ? t('dialogs.mergeStations.working') : t('dialogs.mergeStations.confirm')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}

/**
 * "Keep": one radio per station the merge may keep (RS 4.1, 7), each its pill
 * (line, own name, Station ID), a station holding a datum says so, and a
 * station a loop closure shows as another says "shown as K through a loop
 * closure". When a loop closure between the two forces the keep (RS 4.1 step
 * 4), the one station offered is checked and the dialog says why.
 */
function KeepPicker({
  candidates,
  keeps,
  keep,
  onKeep,
  forced,
  disabled,
}: {
  candidates: readonly KeepCandidate[];
  keeps: readonly MergeKeepPreview[];
  keep: number | null;
  onKeep: (id: number) => void;
  forced: boolean;
  disabled: boolean;
}) {
  const { t } = useTranslation();
  const label = useIdentityLabel();
  const offered = new Set(keeps.map((k) => k.keep));
  return (
    <fieldset className="space-y-1" data-testid="merge-keep">
      <legend className="text-sm text-text-primary">{t('dialogs.mergeStations.keepLead')}</legend>
      {candidates.map((candidate) => {
        const id = candidate.identity.id;
        return (
          <label
            key={`${candidate.station.line_id}:${candidate.station.station_index}`}
            className="flex flex-wrap items-center gap-2 p-1 rounded text-sm hover:bg-background-50"
          >
            <input
              type="radio"
              name="merge-keep"
              value={id}
              checked={keep === id}
              disabled={disabled || !offered.has(id)}
              onChange={() => onKeep(id)}
              data-testid={`merge-keep-${id}`}
            />
            <IdentityPill slot={candidate.station} identity={candidate.identity} testId={`merge-keep-${id}-pill`} />
            {candidate.holds_datum && (
              <span className="text-text-secondary" data-testid={`merge-keep-${id}-datum`}>
                {t('dialogs.mergeStations.holdsDatum')}
              </span>
            )}
            {candidate.shown_as && (
              <span className="text-text-secondary" data-testid={`merge-keep-${id}-shown-as`}>
                {t('dialogs.keptStations.shownAsThroughClosure', { station: label(candidate.shown_as) })}
              </span>
            )}
          </label>
        );
      })}
      {forced && (
        <p className="text-xs text-text-secondary" data-testid="merge-keep-forced">
          {t('dialogs.mergeStations.keepForced')}
        </p>
      )}
    </fieldset>
  );
}

/**
 * What the chosen keep does to names (RS 7): the lines that will show the kept
 * name, the station archived ("with its comment" only when it has one), and
 * that Detach Line from Station brings it back.
 */
function KeepOutcome({ preview, kept }: { preview: MergeKeepPreview; kept: KeepCandidate | null }) {
  const { t } = useTranslation();
  const labels = useIdentityLabels();
  const label = useIdentityLabel();
  const keptName = kept ? label(kept.shown_as ?? kept.identity) : null;
  const hasComment = (preview.hidden.comment ?? '').trim() !== '';
  return (
    <div className="space-y-1 text-sm">
      {preview.renamed_lines.length > 0 && keptName !== null && (
        <p className="text-text-primary" data-testid="merge-renamed-lines">
          {t('dialogs.mergeStations.renamedLines', {
            count: preview.renamed_lines.length,
            lines: listNames(preview.renamed_lines.map(labels.lineName), t),
            station: keptName,
          })}
        </p>
      )}
      <p className="text-text-primary" data-testid="merge-hidden">
        {t(hasComment ? 'dialogs.mergeStations.archivedWithComment' : 'dialogs.mergeStations.archived', {
          station: label(preview.hidden),
        })}
      </p>
      <p className="text-xs text-text-secondary" data-testid="merge-hidden-back">
        {t('dialogs.mergeStations.detachBringsBack')}
      </p>
    </div>
  );
}

/** What the merge does before the user confirms it (demo issue I-4): which
 *  piece moves, how far and onto which station, that a picked station is
 *  already a junction and of which lines, and, when the two stations' depths
 *  differ by more than the backend's threshold, every depth reading the joined
 *  station's mean takes, by line, and that mean (as 0.6.0 computed it). At a
 *  junction the two stations' depths alone did not add up to it (demo finding
 *  V-2: "4.0 m and 12.5 m become 9.7 m" counted 12.5 m twice). */
function MergePreviewNotice({ preview, sayMove }: { preview: MergePreview; sayMove: boolean }) {
  const { t } = useTranslation();
  const labels = useIdentityLabels();
  const precision = useDecimalPrecision();
  const depth = (v: number) => `${formatFixed(v, undefined, precision.depth)} ${t('common.units.meters')}`;
  const piece = listNames(preview.moving_line_ids.map(labels.lineName), t);
  const moves = sayMove && preview.shift_m >= MOVE_SAID_FROM_M && preview.moving_line_ids.length > 0;
  if (!moves && !preview.depths && preview.junctions.length === 0) return null;
  return (
    <div className="space-y-2 text-sm">
      {preview.junctions.map((junction) => (
        <p
          key={`${junction.station.line_id}:${junction.station.station_index}`}
          className="p-2 rounded bg-accent-10 text-text-primary"
          data-testid={`merge-preview-junction-${junction.station.line_id}-${junction.station.station_index}`}
        >
          {t('dialogs.mergeStations.alreadyJoined', {
            station: labels.station(junction.station, true),
            lines: listNames(junction.line_ids.map(labels.lineName), t),
          })}
        </p>
      ))}
      {moves && (
        <p className="p-2 rounded bg-accent-10 text-text-primary" data-testid="merge-preview-move">
          {t('dialogs.mergeStations.moves', {
            // The verb agrees with the piece: "Upstream moves", "Upstream and
            // Upstream Side move" (the lines named, up to three, N-13).
            count: preview.moving_line_ids.length,
            piece,
            distance: formatDatumMetres(preview.shift_m),
            station: labels.station(preview.joins, true),
          })}
        </p>
      )}
      {preview.depths && (
        <p className="p-2 rounded bg-warning-10 text-warning-text" data-testid="merge-preview-depths">
          {t('dialogs.mergeStations.depths', {
            // Every reading, by line, none counted away: a list that stopped
            // at three would hide what the mean takes.
            readings: preview.depths.readings
              .map((r) => t('dialogs.mergeStations.depthReading', { depth: depth(r.depth_m), line: labels.lineName(r.line_id) }))
              .join(', '),
            joined: depth(preview.depths.joined_m),
          })}
        </p>
      )}
    </div>
  );
}

/** What a merge of two placed pieces does to the later-placed one (#741):
 *  which piece moves, how far, and that its datum becomes a GPS fix. */
function DatumShiftNotice({ shift, afterMove }: { shift: MergeDatumShift; afterMove: boolean }) {
  const { t } = useTranslation();
  const labels = useIdentityLabels();
  const piece = listNames(shift.moving_line_ids.map(labels.lineName), t);
  const station = labels.station(shift.datum_station, true);
  // The verb agrees with the lines named: "is placed", "are placed".
  const count = shift.moving_line_ids.length;
  return (
    <p className="p-2 rounded bg-warning-10 text-warning-text text-sm" data-testid="merge-datum-shift">
      {afterMove
        ? t('dialogs.mergeStations.datumShiftAfterMove', { piece, station, count })
        : t('dialogs.mergeStations.datumShift', { piece, station, count, distance: formatDatumMetres(shift.shift_m) })}
    </p>
  );
}
