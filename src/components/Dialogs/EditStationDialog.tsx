import { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { formatFixed } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import {
  parseGps,
  detectFormat,
  gpsSwapHint,
  validateCoordinates,
  type GpsFormat,
  type GpsParseError,
  type GpsParseResult,
} from '../../utils/gps';
import type { GpsCoordinates } from '../../types/project';
import { MAX_DEPTH_REFERENCE_M } from '../../utils/gpsBounds';
import type { StationRef } from '../../types/project';
import { useShallow } from 'zustand/react/shallow';

export default function EditStationDialog() {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  const { closeDialog, dialogData } = useUIStore(
    useShallow((s) => ({ closeDialog: s.closeDialog, dialogData: s.dialogData })),
  );
  const { stations, project, setZeroDatum, removeZeroDatum, updateStationComment } = useProjectStore(
    useShallow((s) => ({
      stations: s.stations,
      project: s.project,
      setZeroDatum: s.setZeroDatum,
      removeZeroDatum: s.removeZeroDatum,
      updateStationComment: s.updateStationComment,
    })),
  );

  const stationRef = dialogData.stationRef as StationRef | undefined;
  const forceEnableZeroDatum = dialogData.forceEnableZeroDatum === true;
  const focusGpsInput = dialogData.focusGpsInput === true;

  // Memoized on the store's array: the binary decoder's lazy station view (after a
  // load and every refresh, #576 A9-D2) builds a NEW Station object on every read,
  // and the form re-initializes whenever `station` changes - looked up per render,
  // that reset the user's own toggle and comment on their next keystroke.
  const station = useMemo(
    () =>
      stations.find(
        (s) =>
          s.station_ref.section_id === stationRef?.section_id &&
          s.station_ref.station_index === stationRef?.station_index
      ),
    [stations, stationRef?.section_id, stationRef?.station_index]
  );

  const section = project?.sections.find(
    (s) => s.id === stationRef?.section_id
  );

  // Form state
  const [isZeroDatum, setIsZeroDatumState] = useState(
    () => forceEnableZeroDatum || station?.is_zero_datum || false
  );
  const [gpsInput, setGpsInput] = useState('');
  const [gpsCoords, setGpsCoords] = useState<GpsCoordinates | null>(null);
  const [gpsFormat, setGpsFormat] = useState<GpsFormat | null>(null);
  const [gpsError, setGpsError] = useState<string | null>(null);
  // How an ambiguous part of the text was read (e.g. UTM "33S" as the southern
  // hemisphere), and the advisory "may be swapped" hint - same as the Zero
  // Datum config dialog.
  const [gpsNote, setGpsNote] = useState<string | null>(null);
  const [gpsSwapWarning, setGpsSwapWarning] = useState(false);
  const [comment, setComment] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const gpsInputRef = useRef<HTMLInputElement>(null);

  // The stored GPS value as the box shows it (decimal degrees), '' without one.
  // It was confirmed when it was saved, so the advisory "may be swapped" hint
  // is only for edited text: a Svalbard datum (78.9, 15.5) is not a swap.
  const originalGpsInput = station?.gps_coordinates
    ? `${station.gps_coordinates.latitude}, ${station.gps_coordinates.longitude}`
    : '';
  // [#576 B35(g)] A stored tie point is no longer necessarily valid: a finite but
  // impossible one (latitude past the pole, a depth reference beyond the bound an
  // older build did not enforce) opens as it is and the integrity banner sends the
  // user HERE to correct it. So the stored value is not assumed good below, and
  // Save rewrites it even when the coordinates in the box are unchanged.
  const storedGps = station?.gps_coordinates;
  const storedGpsInvalid =
    !!storedGps &&
    (!validateCoordinates(storedGps.latitude, storedGps.longitude).valid ||
      !(Math.abs(storedGps.depth_reference) <= MAX_DEPTH_REFERENCE_M));

  // Show what parsing `value` gave. The badge names the format actually read
  // (or, for text that did not parse, the one it was read as / looks like).
  const showGpsResult = useCallback(
    (value: string, result: GpsParseResult | GpsParseError) => {
      setGpsFormat(result.format ?? detectFormat(value) ?? null);
      if ('coordinates' in result) {
        setGpsCoords(result.coordinates);
        setGpsError(null);
        setGpsNote(result.note ?? null);
      } else {
        setGpsCoords(null);
        setGpsError(result.message);
        setGpsNote(null);
      }
      setGpsSwapWarning(value !== originalGpsInput && gpsSwapHint(value, result));
    },
    [originalGpsInput]
  );

  // Initialize form state from station data. The stored GPS value goes through
  // the same parse path as typed text, so the badge, note and hint describe the
  // text shown - also when the station is refreshed while the dialog is open,
  // which must not leave the note of text typed before.
  useEffect(() => {
    if (station) {
      setIsZeroDatumState(forceEnableZeroDatum ? true : station.is_zero_datum);
      setComment(station.comment ?? '');
      if (station.gps_coordinates) {
        const result = parseGps(originalGpsInput);
        setGpsInput(originalGpsInput);
        // A valid stored value whose plain text does not read back (a JS
        // exponent such as "1e-7") keeps the stored value as before. An invalid
        // one (#576 B35(g)) shows its parse error: it is what needs fixing.
        const storedLatLonValid = validateCoordinates(
          station.gps_coordinates.latitude,
          station.gps_coordinates.longitude
        ).valid;
        showGpsResult(
          originalGpsInput,
          'coordinates' in result || !storedLatLonValid
            ? result
            : { coordinates: station.gps_coordinates, format: 'DD', original: originalGpsInput }
        );
      }
    }
  }, [forceEnableZeroDatum, station, originalGpsInput, showGpsResult]);

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
      setGpsNote(null);
      setGpsSwapWarning(false);
      return;
    }
    showGpsResult(value, parseGps(value));
  }, [showGpsResult]);

  const handleSave = async () => {
    if (!stationRef || !station) return;

    setIsLoading(true);
    setError(null);

    try {
      const wasZeroDatum = station.is_zero_datum;
      const willBeZeroDatum = isZeroDatum;

      if (!wasZeroDatum && willBeZeroDatum) {
        // Setting new zero datum
        if (!gpsCoords) {
          setError(t('dialogs.editStation.invalidGps'));
          return;
        }

        const result = await setZeroDatum(stationRef, {
          latitude: gpsCoords.latitude,
          longitude: gpsCoords.longitude,
          depth_reference: station.coordinates.z,
        });

        if (!result.success) {
          setError(result.error ?? t('dialogs.editStation.setZeroDatumFailed'));
          return;
        }
      } else if (wasZeroDatum && !willBeZeroDatum) {
        // Removing zero datum
        const result = await removeZeroDatum(stationRef);
        if (!result.success) {
          setError(result.error ?? t('dialogs.editStation.removeZeroDatumFailed'));
          return;
        }
      } else if (wasZeroDatum && willBeZeroDatum) {
        // Updating GPS coordinates - check if changed
        if (!gpsCoords) {
          setError(t('dialogs.editStation.invalidGps'));
          return;
        }

        const oldLat = station.gps_coordinates?.latitude;
        const oldLon = station.gps_coordinates?.longitude;

        if (gpsCoords.latitude !== oldLat || gpsCoords.longitude !== oldLon || storedGpsInvalid) {
          const result = await setZeroDatum(stationRef, {
            latitude: gpsCoords.latitude,
            longitude: gpsCoords.longitude,
            depth_reference: station.coordinates.z,
          });

          if (!result.success) {
            setError(result.error ?? t('dialogs.editStation.updateZeroDatumFailed'));
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
          setError(commentResult.error ?? t('dialogs.editStation.updateCommentFailed'));
          return;
        }
      }

      trackDialogComplete('edit-station');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.editStation.genericError')));
    } finally {
      setIsLoading(false);
    }
  };

  // Track if form has been modified
  const isDirty = station ? (
    isZeroDatum !== station.is_zero_datum ||
    gpsInput !== originalGpsInput ||
    comment !== (station.comment ?? '')
  ) : false;

  if (!station || !stationRef) {
    return (
      <DialogWrapper title={t('dialogs.editStation.title')} onClose={closeDialog} size="md" data-testid="edit-station-dialog">
        <div className="text-error">{t('dialogs.editStation.notFound')}</div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper title={t('dialogs.editStation.title')} onClose={closeDialog} size="md" isDirty={isDirty} data-testid="edit-station-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <div className="text-sm text-text-secondary">
          {t('dialogs.editStation.coordinatesNote')}
        </div>

        <div className="space-y-3 border-t border-border pt-3">
          <DetailRow
            label={t('dialogs.editStation.id')}
            value={`${stationRef.section_id}:${stationRef.station_index}`}
          />
          <DetailRow
            label={t('common.labels.section')}
            value={section?.name ?? t('dialogs.editStation.sectionFallback', { id: stationRef.section_id })}
          />
          <DetailRow label={t('dialogs.editStation.x')} value={t('dialogs.editStation.metersValue', { value: formatFixed(station.coordinates.x, undefined, precision.coordinates) })} />
          <DetailRow label={t('dialogs.editStation.y')} value={t('dialogs.editStation.metersValue', { value: formatFixed(station.coordinates.y, undefined, precision.coordinates) })} />
          <DetailRow label={t('common.labels.depth')} value={t('dialogs.editStation.metersValue', { value: formatFixed(station.coordinates.z, undefined, precision.depth) })} />
        </div>

        {station.merged_with.length > 0 && (
          <div className="border-t border-border pt-3">
            <span className="text-sm text-text-secondary">{t('dialogs.editStation.mergedWith')}</span>
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
            {t('dialogs.editStation.comment')}
          </label>
          <textarea
            id="station-comment"
            data-testid="station-comment-input"
            value={comment}
            onChange={(e) => setComment(e.target.value)}
            className="w-full px-3 py-2 text-sm bg-background border border-border rounded resize-y min-h-[60px]"
            placeholder={t('dialogs.editStation.commentPlaceholder')}
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
              {t('dialogs.editStation.setZeroDatum')}
            </label>
          </div>

          {isZeroDatum && (
            <div className="space-y-1 pl-6">
              <div className="flex items-center gap-2">
                <label className="text-sm text-text-secondary shrink-0">
                  {t('dialogs.editStation.gps')}
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
                  {t('dialogs.editStation.gpsParsed', {
                    lat: formatFixed(gpsCoords.latitude, undefined, precision.gps),
                    lon: formatFixed(gpsCoords.longitude, undefined, precision.gps),
                  })}
                </div>
              )}
              {gpsCoords && gpsNote && (
                <div className="text-xs text-warning pl-1" data-testid="station-gps-note">
                  {gpsNote}
                </div>
              )}
              {gpsSwapWarning && (
                <div className="text-xs text-warning pl-1" data-testid="station-gps-transposed-warning">
                  {t('dialogs.zeroDatumConfig.transposedWarning')}
                </div>
              )}
              <p className="text-xs text-text-secondary pt-1">
                {t('dialogs.editStation.gpsHelp')}
              </p>
            </div>
          )}
        </div>

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="edit-station-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button variant="primary" onClick={handleSave} disabled={isLoading} data-testid="edit-station-save">
            {isLoading ? t('dialogs.editStation.saving') : t('common.actions.save')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
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
