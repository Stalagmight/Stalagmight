import { useState, useEffect, useCallback, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { EntityBlob } from '../common/EntityBlob';
import { Prose } from '../common/Prose';
import { DialogWrapper } from './DialogWrapper';
import { formatFixed } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { useEditingBlocked, isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { useZeroDatumPreview } from '../../hooks/useZeroDatumPreview';
import { parseGps, detectFormat, type GpsFormat } from '../../utils/gps';
import {
  DEFAULT_DATUM_ACCURACY_M,
  MAX_DATUM_ACCURACY_M,
  compassPoint,
  datumAccuracyToSend,
  parseDatumAccuracy,
} from '../../utils/zeroDatumPreview';
import type { GpsCoordinates } from '../../types/project';
import type { StationRef } from '../../types/project';
import type { DatumGap } from '../../types/ipc/DatumGap';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { useProse } from '../../hooks/useProse';
import { stationRenameProblem, stationNameProblemMessage } from '../../utils/nameValidation';
import { editNeedsOwnerPrompt, ShownIdentityPrompt } from './ShownIdentityPrompt';

const DIALOG_ID = 'edit-station';

/** Metres for the gap readout: one decimal under 10 m, whole metres above. */
function metres(value: number): string {
  return formatFixed(value, undefined, value < 10 ? 1 : 0);
}

export default function EditStationDialog() {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  const { closeDialog, dialogData } = useUIStore();
  const { stations, project, setZeroDatum, removeZeroDatum, updateStationComment, setStationName } = useProjectStore();
  const labels = useIdentityLabels();
  const prose = useProse();
  // Every save path here mutates; the Closures Computed View and Plan/Export
  // have no server backstop, so the gate lives in the UI (useEditingBlocked).
  const editingBlocked = useEditingBlocked();

  const stationRef = dialogData.stationRef as StationRef | undefined;
  const forceEnableZeroDatum = dialogData.forceEnableZeroDatum === true;
  const focusGpsInput = dialogData.focusGpsInput === true;

  const station = stations.find(
    (s) =>
      s.station_ref.line_id === stationRef?.line_id &&
      s.station_ref.station_index === stationRef?.station_index
  );

  const lineName = stationRef ? labels.lineName(stationRef.line_id) : '';
  const storedName = stationRef ? labels.stationName(stationRef) : '';
  // The Station ID the slot shows (RS 6): a foreign slot or a loop end shows
  // its kept station's.
  const stationId = stationRef ? labels.shownStationId(stationRef) : null;
  const shownSlot = stationRef ? labels.shown(stationRef) : undefined;

  // The datum already stored on this station, if any: its role and accuracy.
  // On a junction it may be listed on another of the station's slots.
  const storedDatum = stationRef ? labels.datumOf(project?.zero_datums, stationRef) : undefined;
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
  // The station's name (SPEC 4.1): required, unique on each of its lines. Checked
  // inline as typed; a backend refusal lands on the same field.
  const [name, setName] = useState(storedName);
  const [nameRefusal, setNameRefusal] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // A rename or comment edit through a foreign slot or a loop end says whose
  // station it changes first (RS 5.4); Save then shows the prompt instead of
  // saving, and going on saves.
  const [askingOwner, setAskingOwner] = useState(false);
  const gpsInputRef = useRef<HTMLInputElement>(null);

  const accuracy = parseDatumAccuracy(accuracyInput);

  const nameChanged = name.trim() !== storedName;
  // The name must be free among the native slots of every line the shown
  // station is native on (plan 5.3, RS-FX5), so the message names the line
  // the name is taken on.
  const nameTakenOn = stationRef && nameChanged ? labels.stationNameTakenOn(stationRef, name.trim()) : null;
  const nameProblem = stationRef && nameChanged ? stationRenameProblem(name, nameTakenOn) : null;
  const nameError = nameProblem
    ? stationNameProblemMessage(nameProblem, nameTakenOn !== null ? labels.lineName(nameTakenOn) : lineName, name)
    : nameRefusal;
  // The same message as the field shows it: the line it names as a pill (the
  // Save button's tooltip and the field's description read the plain words).
  const takenLine = nameTakenOn ?? stationRef?.line_id;
  const nameErrorProse = nameProblem
    ? stationNameProblemMessage(nameProblem, takenLine === undefined ? '' : prose.line(takenLine), name)
    : nameRefusal;

  // What this datum would do, as soon as valid GPS is entered (#741).
  const preview = useZeroDatumPreview(
    stationRef,
    gpsCoords && station
      ? { latitude: gpsCoords.latitude, longitude: gpsCoords.longitude, depth_reference: station.coordinates.z }
      : null,
    isZeroDatum
  );
  // The accuracy only weighs a GPS fix (#741): a placement datum puts its piece
  // on its GPS whatever it says. So the field shows only for a stored fix or a
  // datum the preview says will be one; otherwise its stored value is sent back
  // unchanged.
  const showAccuracy = (preview?.role ?? storedDatum?.role) === 'GpsFix';

  // Entrance (#741). The backend resolves whether this datum is an entrance
  // (the stored datum's `is_entrance`, or the preview's for a new one), so the
  // dialog never re-derives the rule. A placement datum always is one; for a
  // GPS fix the user's click is sent only when it differs from that answer.
  const datumRole = preview?.role ?? storedDatum?.role;
  const backendEntrance = storedDatum?.is_entrance ?? preview?.is_entrance;
  const [entranceChoice, setEntranceChoice] = useState<boolean | null>(null);
  const entranceChecked =
    datumRole === 'Placement' ? true : (entranceChoice ?? backendEntrance ?? false);
  const entranceToSend =
    datumRole === 'GpsFix' && entranceChoice !== null && entranceChoice !== backendEntrance
      ? entranceChoice
      : undefined;

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

  // Follow the stored name only when it changes (a rename elsewhere, an undo):
  // a refresh after a refused save must not throw away what was typed.
  useEffect(() => {
    setName(storedName);
  }, [storedName]);

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

  const commentChanged = station ? comment !== (station.comment ?? '') : false;

  const handleSave = async (ownerConfirmed = false) => {
    if (!stationRef || !station || isLoading) return;
    if (isEditingBlockedNow()) {
      setError(t('dialogs.editStation.editingDisabled'));
      return;
    }

    if (nameProblem !== null) return;
    if (!ownerConfirmed && (nameChanged || commentChanged) && editNeedsOwnerPrompt(shownSlot)) {
      setAskingOwner(true);
      return;
    }
    setAskingOwner(false);

    // Every check the dialog can make runs BEFORE anything is sent, so a Save
    // with a field to fix changes nothing (no rename landing ahead of a
    // datum error). Only the backend's own refusals can still stop it part
    // way: set_station_comment and remove_zero_datum are not batchable, so the
    // changes go as separate commands, the rename first.
    const wasZeroDatum = station.is_zero_datum;
    const willBeZeroDatum = isZeroDatum;
    if (willBeZeroDatum && showAccuracy && accuracy === null) {
      setError(t('dialogs.editStation.accuracyInvalid', { max: MAX_DATUM_ACCURACY_M }));
      return;
    }
    if (willBeZeroDatum && !gpsCoords) {
      setError('Please enter valid GPS coordinates');
      return;
    }

    setIsLoading(true);
    setError(null);
    setNameRefusal(null);

    try {
      // Rename first: a refused name (taken meanwhile, e.g. by an undo) stops
      // the save before anything else changes, and shows at the field. A name
      // that only differs by surrounding spaces is no rename (`nameChanged`).
      if (nameChanged) {
        const nameResult = await setStationName(stationRef, name.trim());
        if (!nameResult.success) {
          setNameRefusal(nameResult.error ?? t('identity.stationNameEmpty'));
          return;
        }
      }
      // Send the stored accuracy back unless the user changed it, which may be
      // none at all. The backend stores what it is sent (nothing means the
      // default), so a placement datum, whose field is hidden, still sends
      // its stored value: it applies again the day the datum becomes a fix.
      const accuracyM = !showAccuracy
        ? storedDatum?.accuracy_m
        : accuracy === null
          ? undefined
          : datumAccuracyToSend(accuracy, storedDatum?.accuracy_m);
      const accuracyOptions = {
        ...(accuracyM === undefined ? {} : { accuracyM }),
        ...(entranceToSend === undefined ? {} : { entrance: entranceToSend }),
      };

      if (!wasZeroDatum && willBeZeroDatum && gpsCoords) {
        // Setting new zero datum
        const result = await setZeroDatum(
          stationRef,
          {
            latitude: gpsCoords.latitude,
            longitude: gpsCoords.longitude,
            depth_reference: station.coordinates.z,
          },
          accuracyOptions
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
      } else if (wasZeroDatum && willBeZeroDatum && gpsCoords) {
        // Updating GPS coordinates - check if changed
        const oldLat = station.gps_coordinates?.latitude;
        const oldLon = station.gps_coordinates?.longitude;

        const gpsChanged = gpsCoords.latitude !== oldLat || gpsCoords.longitude !== oldLon;
        if (gpsChanged || (showAccuracy && accuracy !== storedAccuracy) || entranceToSend !== undefined) {
          const result = await setZeroDatum(
            stationRef,
            {
              latitude: gpsCoords.latitude,
              longitude: gpsCoords.longitude,
              depth_reference: station.coordinates.z,
            },
            accuracyOptions
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
    (isZeroDatum && showAccuracy && accuracyInput.trim() !== String(storedAccuracy)) ||
    (isZeroDatum && entranceToSend !== undefined) ||
    commentChanged ||
    nameChanged
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

        <div className="space-y-1">
          <label className="block text-sm font-medium" htmlFor="station-name">
            {t('identity.stationName')}
          </label>
          <input
            id="station-name"
            type="text"
            value={name}
            onChange={(e) => {
              setName(e.target.value);
              setNameRefusal(null);
            }}
            disabled={editingBlocked}
            aria-invalid={nameError !== null}
            {...(nameError ? { 'aria-describedby': 'station-name-error' } : {})}
            className="w-full px-3 py-2 text-sm bg-background border border-border rounded"
            data-testid="station-name-input"
          />
          {nameError ? (
            <p id="station-name-error" className="text-xs text-error" data-testid="station-name-error">
              <Prose text={nameErrorProse ?? nameError} />
            </p>
          ) : (
            <p className="text-xs text-text-secondary">{t('dialogs.editStation.nameHint')}</p>
          )}
        </div>

        <div className="text-sm text-text-secondary">
          Station coordinates are calculated from shots and cannot be edited directly.
        </div>

        <div className="space-y-3 border-t border-border pt-3">
          {stationId !== null && (
            <DetailRow label={t('identity.stationId')} value={String(stationId)} testId="edit-station-id" />
          )}
          <DetailRow label="Line" value={lineName} />
          <DetailRow label="X" value={`${formatFixed(station.coordinates.x, undefined, precision.coordinates)} m`} />
          <DetailRow label="Y" value={`${formatFixed(station.coordinates.y, undefined, precision.coordinates)} m`} />
          <DetailRow label="Depth" value={`${formatFixed(station.coordinates.z, undefined, precision.depth)} m`} />
        </div>

        {station.merged_with.length > 0 && (
          <div className="border-t border-border pt-3">
            {/* Every line shows the junction's one station (RS 6): the
                other lines are named, not their names for it. */}
            <span className="text-sm text-text-secondary">{t('detailPanel.station.alsoOn')}:</span>
            <ul className="mt-1" data-testid="edit-station-also-on">
              {/* A list of names, not a sentence: each a line pill, as the
                  detail panel's Also on list draws them. */}
              {station.merged_with.map((ref, i) => (
                <li key={i} className="text-sm">
                  <EntityBlob entity="line" lineId={ref.line_id}>
                    {labels.lineName(ref.line_id)}
                  </EntityBlob>
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
                GPS coordinates for this station.
                {(preview?.role ?? storedDatum?.role) !== 'GpsFix' &&
                  ' Other stations will have their GPS calculated relative to this datum.'}
              </p>
              {showAccuracy && (
                <>
                  <div className="flex items-center gap-2 pt-2">
                    <label htmlFor="station-datum-accuracy" className="text-sm text-text-secondary shrink-0">
                      {t('dialogs.editStation.accuracyLabel')}
                    </label>
                    <input
                      id="station-datum-accuracy"
                      type="number"
                      inputMode="decimal"
                      min={0}
                      max={MAX_DATUM_ACCURACY_M}
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
                      {t('dialogs.editStation.accuracyInvalid', { max: MAX_DATUM_ACCURACY_M })}
                    </div>
                  ) : (
                    <p className="text-xs text-text-secondary">{t('dialogs.editStation.accuracyHint')}</p>
                  )}
                </>
              )}
              {shownSlot?.loop_end && (
                <p className="text-xs text-text-secondary pt-1" data-testid="station-datum-loop-end">
                  <Prose text={t('dialogs.editStation.loopEndDatum', { station: prose.station(stationRef, true) })} />
                </p>
              )}
              {storedDatum && (
                <p className="text-xs text-text-secondary pt-1" data-testid="station-datum-role">
                  {t('dialogs.editStation.roleLabel')}{' '}
                  {storedDatum.role === 'GpsFix'
                    ? t('dialogs.editStation.roleGpsFix')
                    : t('dialogs.editStation.rolePlacement')}
                </p>
              )}
              {datumRole && (
                <div className="pt-2 space-y-1">
                  <div className="flex items-center gap-2">
                    <input
                      type="checkbox"
                      id="station-datum-entrance"
                      data-testid="station-datum-entrance-input"
                      checked={entranceChecked}
                      disabled={datumRole === 'Placement'}
                      onChange={(e) => setEntranceChoice(e.target.checked)}
                      className="w-4 h-4"
                    />
                    <label htmlFor="station-datum-entrance" className="text-sm">
                      {t('dialogs.editStation.entranceLabel')}
                    </label>
                  </div>
                  <p className="text-xs text-text-secondary" data-testid="station-datum-entrance-hint">
                    {datumRole === 'Placement'
                      ? t('dialogs.editStation.entrancePlacementHint')
                      : t('dialogs.editStation.entranceHint')}
                  </p>
                </div>
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

        {askingOwner ? (
          <div className="pt-4 border-t border-border">
            <ShownIdentityPrompt
              stationRef={stationRef}
              edit={nameChanged && commentChanged ? 'both' : nameChanged ? 'name' : 'comment'}
              newName={nameChanged ? name.trim() : undefined}
              onProceed={() => void handleSave(true)}
              onCancel={() => setAskingOwner(false)}
              busy={isLoading}
            />
          </div>
        ) : (
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="edit-station-cancel">
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={() => void handleSave()}
            disabled={isLoading || editingBlocked || nameProblem !== null}
            title={editingBlocked ? t('dialogs.editStation.editingDisabled') : (nameError ?? undefined)}
            data-testid="edit-station-save"
          >
            {isLoading ? 'Saving...' : 'Save'}
          </Button>
        </div>
        )}
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

function DetailRow({ label, value, testId }: { label: string; value: string; testId?: string }) {
  return (
    <div className="flex justify-between text-sm">
      <span className="text-text-secondary">{label}:</span>
      <span data-testid={testId}>{value}</span>
    </div>
  );
}
