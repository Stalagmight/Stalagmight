import { useState, useEffect } from 'react';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { MIN_SPLIT_CHILD_LENGTH_M } from '../../utils/shotSplit';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { useProse } from '../../hooks/useProse';
import { Prose } from '../common/Prose';

/** Clamp a 0..1 fraction; `projectParamOnSegment` already clamps, this is belt
 *  and braces against a NaN or an out-of-range value reaching the arithmetic. */
function clamp01(t: number): number {
  return Math.min(1, Math.max(0, t));
}

/**
 * Both fields round for display, and the metres field is what actually gets
 * split, so what the user reads is exactly what they get. `Number()` strips the
 * trailing zeros `toFixed` adds, so a clean value shows as "5" not "5.00".
 */
function formatMeters(meters: number): string {
  return String(Number(meters.toFixed(2)));
}

function formatPercent(pct: number): string {
  return String(Number(pct.toFixed(2)));
}

export default function InsertStationDialog() {
  const { closeDialog, dialogData, setSelection } = useUIStore();
  const { splitShotAtDistance } = useProjectStore();
  const labels = useIdentityLabels();
  const prose = useProse();

  const lineId = dialogData.lineId as number | undefined;
  const shotId = dialogData.shotId as number | undefined;
  const shotLength = (dialogData.shotLength as number | undefined) ?? 0;
  const lineName = lineId !== undefined ? labels.lineName(lineId) : undefined;
  // `4 → 5` as a prose pill (RS-PROSE): the line is in the (plain) title.
  const shotLabel = lineId !== undefined && shotId !== undefined ? prose.shot(lineId, shotId) : null;
  /**
   * Where along the shot the map right-click landed, 0..1 from the from-station.
   * Only present for a map context menu; the data-table menu has no position on
   * the line, so the dialog opens blank there.
   */
  const alongT = dialogData.alongT as number | undefined;

  /**
   * Distance in metres from the shot's start, and the same position as a
   * percentage of the shot length. Both are held as raw strings so a
   * half-typed value ("1.", "-", "") survives editing; whichever field the user
   * is in stays untouched while the other is recomputed from it.
   */
  const [distance, setDistance] = useState('');
  const [percent, setPercent] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Re-initialize when the target shot changes. A map right-click prefills the
  // exact clicked spot; anything else opens blank.
  useEffect(() => {
    const prefill =
      alongT !== undefined && Number.isFinite(alongT) && shotLength > 0
        ? clamp01(alongT) * shotLength
        : null;
    setDistance(prefill === null ? '' : formatMeters(prefill));
    setPercent(prefill === null ? '' : formatPercent((prefill / shotLength) * 100));
    setError(null);
  }, [lineId, shotId, alongT, shotLength]);

  /** Distance typed (or prefilled): the percentage follows. */
  const handleDistanceChange = (raw: string) => {
    setDistance(raw);
    const meters = parseFloat(raw);
    setPercent(
      raw.trim().length > 0 && Number.isFinite(meters) && shotLength > 0
        ? formatPercent((meters / shotLength) * 100)
        : ''
    );
  };

  /** Percentage typed: the distance follows, and IS what gets split - the
   *  rounded metres shown are exactly what the user gets. */
  const handlePercentChange = (raw: string) => {
    setPercent(raw);
    const pct = parseFloat(raw);
    setDistance(
      raw.trim().length > 0 && Number.isFinite(pct) && shotLength > 0
        ? formatMeters((pct / 100) * shotLength)
        : ''
    );
  };

  const parsedDistance = parseFloat(distance);
  const isValid =
    distance.trim().length > 0 &&
    Number.isFinite(parsedDistance) &&
    parsedDistance >= MIN_SPLIT_CHILD_LENGTH_M &&
    shotLength - parsedDistance >= MIN_SPLIT_CHILD_LENGTH_M;
  const remaining = isValid ? shotLength - parsedDistance : null;

  // A prefilled value is not a user edit, so re-opening and immediately closing
  // a map-triggered dialog must not trip the unsaved-changes guard.
  const prefilledDistance =
    alongT !== undefined && Number.isFinite(alongT) && shotLength > 0
      ? formatMeters(clamp01(alongT) * shotLength)
      : '';
  const isDirty = distance.trim().length > 0 && distance !== prefilledDistance;

  const handleConfirm = async () => {
    if (isLoading || lineId === undefined || shotId === undefined || !isValid) return;

    // Read-only self-check at confirm time: a split has NO server backstop for
    // the local closure-adjusted / Plan / Export case (see useEditingBlocked's
    // backstop-asymmetry note), so a dialog left open across a view switch to a
    // read-only mode must refuse here rather than mutate. Re-checked now, not at
    // open time, because the mode can flip while the dialog is up.
    if (isEditingBlockedNow()) {
      setError('Editing is disabled in the current view.');
      return;
    }

    setIsLoading(true);
    setError(null);

    try {
      const result = await splitShotAtDistance(lineId, shotId, parsedDistance);
      if (result.success) {
        // Re-derive the new station's index from the REFRESHED store state
        // rather than a pre-await snapshot (#59 fix D): the tail keeps its
        // shot id, and its post-split index IS the new station's index
        // (station i sits between shots i-1 and i). If the line/shot is
        // gone by the time the split resolves (project switched, or a racing
        // mutation), skip the selection and just close.
        const refreshedProject = useProjectStore.getState().project;
        const line = refreshedProject?.lines.find((s) => s.id === lineId);
        const newStationIndex = line?.shots.findIndex((sh) => sh.id === shotId) ?? -1;
        if (newStationIndex >= 0) {
          setSelection({
            type: 'station',
            stationRef: { line_id: lineId, station_index: newStationIndex },
          });
        }
        trackDialogComplete('insert-station');
        closeDialog();
      } else {
        setError(result.error ?? 'Failed to insert station');
      }
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to insert station'));
    } finally {
      setIsLoading(false);
    }
  };

  const handleClose = isLoading ? () => {} : closeDialog;

  return (
    <DialogWrapper
      title={lineName ? `Insert Station in ${lineName}` : 'Insert Station'}
      onClose={handleClose}
      size="md"
      isDirty={isDirty}
      closeOnBackdropClick={!isLoading}
      data-testid="insert-station-dialog"
    >
      <div className="space-y-4">
        <p className="text-text-secondary" data-testid="insert-station-lead">
          <Prose
            text={
              shotLabel
                ? `Splits the shot ${shotLabel} into two, adding a station at the given distance from its start.`
                : 'Splits the shot into two, adding a station at the given distance from its start.'
            }
          />
        </p>

        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <div className="flex gap-3">
          <div className="flex-1">
            <label className="block text-sm text-text-secondary mb-1" htmlFor="insert-station-distance">
              Distance from start (meters)
            </label>
            <input
              id="insert-station-distance"
              type="number" inputMode="decimal"
              min={MIN_SPLIT_CHILD_LENGTH_M}
              max={shotLength - MIN_SPLIT_CHILD_LENGTH_M}
              step="0.01"
              value={distance}
              onChange={(e) => handleDistanceChange(e.target.value)}
              className="w-full"
              placeholder={`0-${shotLength}`}
              data-testid="insert-station-distance"
              disabled={isLoading}
            />
          </div>
          <div className="flex-1">
            <label className="block text-sm text-text-secondary mb-1" htmlFor="insert-station-percent">
              Distance from start (%)
            </label>
            <input
              id="insert-station-percent"
              type="number" inputMode="decimal"
              min={0}
              max={100}
              step="0.1"
              value={percent}
              onChange={(e) => handlePercentChange(e.target.value)}
              className="w-full"
              placeholder="0-100"
              data-testid="insert-station-percent"
              disabled={isLoading}
            />
          </div>
        </div>

        <p className="text-sm text-text-secondary" data-testid="insert-station-preview">
          {remaining !== null
            ? `Shot A: ${parsedDistance.toFixed(2)} m / Shot B: ${remaining.toFixed(2)} m`
            : `Total shot length: ${shotLength.toFixed(2)} m (both sides must be at least ${MIN_SPLIT_CHILD_LENGTH_M.toFixed(2)} m)`}
        </p>

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleClose} disabled={isLoading} data-testid="insert-station-cancel">
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={handleConfirm}
            disabled={isLoading || !isValid}
            data-testid="insert-station-confirm"
          >
            {isLoading ? 'Inserting...' : 'Insert Station'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
