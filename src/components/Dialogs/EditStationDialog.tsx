import { useState, useEffect, useCallback, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { formatFixed } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { useEditingBlocked, isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { useZeroDatumPreview } from '../../hooks/useZeroDatumPreview';
import { parseGps, detectFormat, type GpsFormat } from '../../utils/gps';
import { DEFAULT_DATUM_ACCURACY_M, compassPoint, parseDatumAccuracy } from '../../utils/zeroDatumPreview';
import type { GpsCoordinates } from '../../types/project';
import type { StationRef } from '../../types/project';
import type { DatumGap } from '../../types/ipc/DatumGap';

const DIALOG_ID = 'edit-station';

/** Metres for the gap readout: one decimal under 10 m, whole metres above. */
function metres(value: number): string {
  return formatFixed(value, undefined, value < 10 ? 1 : 0);
}

export default function EditStationDialog() {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  const { closeDialog, dialogData } = useUIStore();
  const { stations, project, setZeroDatum, removeZeroDatum, updateStationComment } = useProjectStore();
  // Every save path here mutates; the Closures Computed View and Plan/Export
  // have no server backstop, so the gate lives in the UI (useEditingBlocked).
  const editingBlocked = useEditingBlocked();

  const stationRef = dialogData.stationRef as StationRef | undefined;
  const forceEnableZeroDatum = dialogData.forceEnableZeroDatum === true;
  const focusGpsInput = dialogData.focusGpsInput === true;

  const station = stations.find(
    (s) =>
      s.station_ref.section_id === stationRef?.section_id &&
      s.station_ref.station_index === stationRef?.station_index
  );

  const section = project?.sections.find(
    (s) => s.id === stationRef?.section_id
  );

  // The datum already stored on this station, if any: its role and accuracy.
  const storedDatum = project?.zero_datums.find(
    (d) =>
      d.station_ref.section_id === stationRef?.section_id &&
      d.station_ref.station_index === stationRef?.station_index
  );
  const storedAccuracy = storedDatum?.accuracy_m ?? DEFAULT_DATUM_ACCURACY_M;

  // Form state
  const [isZeroDatum, setIsZeroDatumState] = useState(
    () => forceEnableZeroDatum || station?.is_zero_datum || false
  );
  const [gpsInput, setGpsInput] = useState('');
  const [gpsCoords, setGpsCoords] = useState<GpsCoordinates | null>(null);
  const [gpsFormat, setGpsFormat] = useState<GpsFormat | null>(null);
  const [gpsError, setGpsError] = useState<string | null>(null);
  const [accuracyInput, setAccuracyInput] = useState(() => String(storedAccuracy));
  const [comment, setComment] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const gpsInputRef = useRef<HTMLInputElement>(null);

  const accuracy = parseDatumAccuracy(accuracyInput);

  // What this datum would do, as soon as valid GPS is entered (#741).
  const preview = useZeroDatumPreview(
    stationRef,
    gpsCoords && station
      ? { latitude: gpsCoords.latitude, longitude: gpsCoords.longitude, depth_reference: station.coordinates.z }
      : null,
    isZeroDatum
  );

  // Initialize form state from station data
  useEffect(() => {
    if (station) {
      setIsZeroDatumState(forceEnableZeroDatum ? true : station.is_zero_datum);
      setComment(station.comment ?? '');
      if (station.gps_coordinates) {
        const initialInput = `${station.gps_coordinates.latitude}, ${station.gps_coordinates.longitude}`;
        setGpsInput(initialInput);
        setGpsCoords(station.gps_coordinates);
        setGpsFormat('DD');
      }
    }
  }, [forceEnableZeroDatum, station]);

  useEffect(() => {
    setAccuracyInput(String(storedAccuracy));
  }, [storedAccuracy]);

  // Focus GPS input when dialog is opened from "Set Zero Datum" action.
  useEffect(() => {
    if (!focusGpsInput || !isZeroDatum) {
      return;
    }
    const timeoutId = window.setTimeout(() => {
      gpsInputRef.current?.focus();
      gpsInputRef.current?.select();
    }, 0);
    return () => window.clearTimeout(timeoutId);
  }, [focusGpsInput, isZeroDatum]);

  // GPS input handler
  const handleGpsInputChange = useCallback((value: string) => {
    setGpsInput(value);
    if (!value.trim()) {
      setGpsCoords(null);
      setGpsFormat(null);
      setGpsError(null);
      return;
    }
    const fmt = detectFormat(value);
    setGpsFormat(fmt ?? null);
    const result = parseGps(value);
    if ('coordinates' in result) {
      setGpsCoords(result.coordinates);
      setGpsError(null);
    } else {
      setGpsCoords(null);
      setGpsError(result.message);
    }
  }, []);

  const handleSave = async () => {
    if (!stationRef || !station || isLoading) return;
    if (isEditingBlockedNow()) {
      setError(t('dialogs.editStation.editingDisabled'));
      return;
    }

    setIsLoading(true);
    setError(null);

    try {
      const wasZeroDatum = station.is_zero_datum;
      const willBeZeroDatum = isZeroDatum;

      if (willBeZeroDatum && accuracy === null) {
        setError(t('dialogs.editStation.accuracyInvalid'));
        return;
      }

      if (!wasZeroDatum && willBeZeroDatum) {
        // Setting new zero datum
        if (!gpsCoords) {
          setError('Please enter valid GPS coordinates');
          return;
        }

        const result = await setZeroDatum(
          stationRef,
          {
            latitude: gpsCoords.latitude,
            longitude: gpsCoords.longitude,
            depth_reference: station.coordinates.z,
          },
          { accuracyM: accuracy ?? DEFAULT_DATUM_ACCURACY_M }
        );

        if (!result.success) {
          setError(result.error ?? 'Failed to set zero datum');
          return;
        }
      } else if (wasZeroDatum && !willBeZeroDatum) {
        // Removing zero datum
        const result = await removeZeroDatum(stationRef);
        if (!result.success) {
          setError(result.error ?? 'Failed to remove zero datum');
          return;
        }
      } else if (wasZeroDatum && willBeZeroDatum) {
        // Updating GPS coordinates - check if changed
        if (!gpsCoords) {
          setError('Please enter valid GPS coordinates');
          return;
        }

        const oldLat = station.gps_coordinates?.latitude;
        const oldLon = station.gps_coordinates?.longitude;

        const gpsChanged = gpsCoords.latitude !== oldLat || gpsCoords.longitude !== oldLon;
        if (gpsChanged || accuracy !== storedAccuracy) {
          const result = await setZeroDatum(
            stationRef,
            {
              latitude: gpsCoords.latitude,
              longitude: gpsCoords.longitude,
              depth_reference: station.coordinates.z,
            },
            { accuracyM: accuracy ?? DEFAULT_DATUM_ACCURACY_M }
          );

          if (!result.success) {
            setError(result.error ?? 'Failed to update zero datum');
            return;
          }
        }
      }

      // Save comment if changed
      const oldComment = station.comment ?? '';
      if (comment !== oldComment) {
        const commentResult = await updateStationComment(
          stationRef,
          comment.trim() || null
        );
        if (!commentResult.success) {
          setError(commentResult.error ?? 'Failed to update comment');
          return;
        }
      }

      // closeDialog is global: if the user dismissed this dialog (and maybe
      // opened another) while the mutation was in flight, leave that one be.
      if (useUIStore.getState().activeDialog === DIALOG_ID) {
        trackDialogComplete(DIALOG_ID);
        closeDialog();
      }
    } catch (e) {
      setError(getErrorMessage(e, 'An error occurred'));
    } finally {
      setIsLoading(false);
    }
  };

  // Track if form has been modified
  const originalGpsInput = station?.gps_coordinates
    ? `${station.gps_coordinates.latitude}, ${station.gps_coordinates.longitude}`
    : '';
  const isDirty = station ? (
    isZeroDatum !== station.is_zero_datum ||
    gpsInput !== originalGpsInput ||
    (isZeroDatum && accuracyInput.trim() !== String(storedAccuracy)) ||
    comment !== (station.comment ?? '')
  ) : false;

  if (!station || !stationRef) {
    return (
      <DialogWrapper title="Edit Station" onClose={closeDialog} size="md" data-testid="edit-station-dialog">
        <div className="text-error">Station not found</div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper title="Edit Station" onClose={closeDialog} size="md" isDirty={isDirty} data-testid="edit-station-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <div className="text-sm text-text-secondary">
          Station coordinates are calculated from shots and cannot be edited directly.
        </div>

        <div className="space-y-3 border-t border-border pt-3">
          <DetailRow
            label="ID"
            value={`${stationRef.section_id}:${stationRef.station_index}`}
          />
          <DetailRow
            label="Section"
            value={section?.name ?? `Section ${stationRef.section_id}`}
          />
          <DetailRow label="X" value={`${formatFixed(station.coordinates.x, undefined, precision.coordinates)} m`} />
          <DetailRow label="Y" value={`${formatFixed(station.coordinates.y, undefined, precision.coordinates)} m`} />
          <DetailRow label="Depth" value={`${formatFixed(station.coordinates.z, undefined, precision.depth)} m`} />
        </div>

        {station.merged_with.length > 0 && (
          <div className="border-t border-border pt-3">
            <span className="text-sm text-text-secondary">Merged with:</span>
            <ul className="mt-1">
              {station.merged_with.map((ref, i) => (
                <li key={i} className="text-sm">
                  {ref.section_id}:{ref.station_index}
                </li>
              ))}
            </ul>
          </div>
        )}

        {/* Comment section */}
        <div className="border-t border-border pt-3 space-y-2">
          <label className="block text-sm font-medium" htmlFor="station-comment">
            Comment
          </label>
          <textarea
            id="station-comment"
            data-testid="station-comment-input"
            value={comment}
            onChange={(e) => setComment(e.target.value)}
            className="w-full px-3 py-2 text-sm bg-background border border-border rounded resize-y min-h-[60px]"
            placeholder="Add a comment..."
            rows={2}
          />
        </div>

        {/* Zero Datum section */}
        <div className="border-t border-border pt-3 space-y-3">
          <div className="flex items-center gap-2">
            <input
              type="checkbox"
              id="zeroDatum"
              data-testid="station-zero-datum-input"
              checked={isZeroDatum}
              onChange={(e) => setIsZeroDatumState(e.target.checked)}
              className="w-4 h-4"
            />
            <label htmlFor="zeroDatum" className="text-sm font-medium">
              Set as Zero Datum
            </label>
          </div>

          {isZeroDatum && (
            <div className="space-y-1 pl-6">
              <div className="flex items-center gap-2">
                <label className="text-sm text-text-secondary shrink-0">
                  GPS:
                </label>
                <input
                  ref={gpsInputRef}
                  data-autofocus={focusGpsInput ? 'true' : undefined}
                  type="text"
                  value={gpsInput}
                  onChange={(e) => handleGpsInputChange(e.target.value)}
                  placeholder="48.8584, 2.2945"
                  className="flex-1 px-2 py-1.5 bg-background border border-border rounded text-sm"
                  data-testid="station-gps-input"
                />
                {gpsFormat && (
                  <span
                    className="text-xs px-2 py-1 bg-accent-10 text-accent rounded"
                    data-testid="station-gps-format"
                  >
                    {gpsFormat}
                  </span>
                )}
              </div>
              {gpsError && (
                <div className="text-xs text-error pl-1" data-testid="station-gps-error">
                  {gpsError}
                </div>
              )}
              {gpsCoords && (
                <div className="text-xs text-success pl-1" data-testid="station-gps-parsed">
                  Lat: {formatFixed(gpsCoords.latitude, undefined, precision.gps)}, Lon:{' '}
                  {formatFixed(gpsCoords.longitude, undefined, precision.gps)}
                </div>
              )}
              <p className="text-xs text-text-secondary pt-1">
                GPS coordinates for this station. Other stations will have their
                GPS calculated relative to this datum.
              </p>
              <div className="flex items-center gap-2 pt-2">
                <label htmlFor="station-datum-accuracy" className="text-sm text-text-secondary shrink-0">
                  {t('dialogs.editStation.accuracyLabel')}
                </label>
                <input
                  id="station-datum-accuracy"
                  type="number"
                  inputMode="decimal"
                  min={0}
                  step="any"
                  value={accuracyInput}
                  onChange={(e) => setAccuracyInput(e.target.value)}
                  aria-invalid={accuracy === null}
                  className="w-24 px-2 py-1.5 bg-background border border-border rounded text-sm"
                  data-testid="station-datum-accuracy-input"
                />
              </div>
              {accuracy === null ? (
                <div className="text-xs text-error pl-1" data-testid="station-datum-accuracy-error">
                  {t('dialogs.editStation.accuracyInvalid')}
                </div>
              ) : (
                <p className="text-xs text-text-secondary">{t('dialogs.editStation.accuracyHint')}</p>
              )}
              {storedDatum && (
                <p className="text-xs text-text-secondary pt-1" data-testid="station-datum-role">
                  {t('dialogs.editStation.roleLabel')}{' '}
                  {storedDatum.role === 'GpsFix'
                    ? t('dialogs.editStation.roleGpsFix')
                    : t('dialogs.editStation.rolePlacement')}
                </p>
              )}
              {preview?.role === 'GpsFix' && (
                <div className="mt-2 p-2 rounded bg-accent-10 text-xs space-y-1" data-testid="station-datum-preview">
                  {!storedDatum && (
                    <p data-testid="station-datum-fix-notice">{t('dialogs.editStation.gpsFixNotice')}</p>
                  )}
                  {preview.gap && <GapReadout gap={preview.gap} />}
                </div>
              )}
            </div>
          )}
        </div>

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="edit-station-cancel">
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={handleSave}
            disabled={isLoading || editingBlocked}
            title={editingBlocked ? t('dialogs.editStation.editingDisabled') : undefined}
            data-testid="edit-station-save"
          >
            {isLoading ? 'Saving...' : 'Save'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}

/** How far a GPS fix is from its surveyed position, and the sanity warning when
 *  that is more than the backend's threshold. A warning, never a block. */
function GapReadout({ gap }: { gap: DatumGap }) {
  const { t } = useTranslation();
  return (
    <>
      <p data-testid="station-datum-gap">
        {t('dialogs.editStation.gap', { distance: metres(gap.gap_m), direction: compassPoint(gap.bearing_deg) })}
      </p>
      {gap.exceeds_threshold && (
        <p className="p-2 rounded bg-warning-10 text-warning-text" data-testid="station-datum-gap-warning">
          {gap.survey_distance_m === null
            ? t('dialogs.editStation.gapExceedsNoDistance', { threshold: metres(gap.threshold_m) })
            : t('dialogs.editStation.gapExceeds', {
                threshold: metres(gap.threshold_m),
                distance: metres(gap.survey_distance_m),
              })}
        </p>
      )}
    </>
  );
}

function DetailRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex justify-between text-sm">
      <span className="text-text-secondary">{label}:</span>
      <span>{value}</span>
    </div>
  );
}
