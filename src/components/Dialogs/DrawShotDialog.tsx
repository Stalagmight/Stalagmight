import { useState, useMemo, useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { normalizeAzimuth } from '../../utils/azimuth';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { Select } from '../common/Select';
import { EntityBlob } from '../common/EntityBlob';
import { formatStationId } from '../../utils/formatters';
import { DialogWrapper } from './DialogWrapper';
import type { StationRef, ShotData, ShotKind } from '../../types/project';
import { SHOT_KIND_CYCLE, SHOT_KIND_LABEL_KEY } from '../../utils/shotKind';
import { reverseTargetShot, type TargetShotLike } from '../../utils/overlapHelpers';
import { useShallow } from 'zustand/react/shallow';

export default function DrawShotDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, setSelection } = useUIStore(
    useShallow((s) => ({ closeDialog: s.closeDialog, dialogData: s.dialogData, setSelection: s.setSelection })),
  );
  const { createShot, project, stations } = useProjectStore(
    useShallow((s) => ({ createShot: s.createShot, project: s.project, stations: s.stations })),
  );

  const fromStation = dialogData.fromStation as StationRef | undefined;
  const allowZeroLength = dialogData.allowZeroLength as boolean | undefined;
  const prependMode = dialogData.prependMode as boolean | undefined;
  const appendMode = dialogData.appendMode as boolean | undefined;
  const sourceGeometry = dialogData.sourceGeometry as {
    azimuth_in: number;
    azimuth_out: number;
    depth_in: number;
    depth_out: number;
    pitch_in: number | null;
    pitch_out: number | null;
  } | undefined;

  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Prefill the source shot's bearings verbatim: prepending does not
  // reciprocate, so what is shown is what will be stored.
  const initialAzimuthIn = sourceGeometry ? String(sourceGeometry.azimuth_in) : '';
  const initialAzimuthOut = sourceGeometry ? String(sourceGeometry.azimuth_out) : '';

  // Form state - prefill from source geometry if provided
  const [azimuthIn, setAzimuthIn] = useState(initialAzimuthIn);
  const [azimuthOut, setAzimuthOut] = useState(initialAzimuthOut);
  const [length, setLength] = useState((prependMode || appendMode) ? '0' : '');
  const [depthIn, setDepthIn] = useState(sourceGeometry ? String(sourceGeometry.depth_in) : '');
  const [depthOut, setDepthOut] = useState(sourceGeometry ? String(sourceGeometry.depth_out) : '');
  const [shotKind, setShotKind] = useState<ShotKind>('Normal');
  const [reverseDirection, setReverseDirection] = useState(false);
  const [left, setLeft] = useState('');
  const [right, setRight] = useState('');
  const [up, setUp] = useState('');
  const [down, setDown] = useState('');

  // Get section info
  const section = useMemo(() => {
    if (!fromStation) return null;
    return project?.sections.find((s) => s.id === fromStation.section_id);
  }, [fromStation, project]);

  const sectionName = section?.name ?? t('dialogs.drawShot.sectionFallback', { id: fromStation?.section_id });

  // Get current station depth for default
  const currentStationDepth = useMemo(() => {
    if (!fromStation) return 0;
    const station = stations.find(
      (s) => s.station_ref.section_id === fromStation.section_id && s.station_ref.station_index === fromStation.station_index
    );
    return station?.coordinates.z ?? 0;
  }, [fromStation, stations]);

  // Track if depth/length were auto-initialized (for isDirty calculation)
  const depthAutoInitialized = useRef(!!sourceGeometry);
  const lengthAutoInitialized = useRef(!!(prependMode || appendMode));

  // Initialize depth_in with current station depth on mount
  useEffect(() => {
    if (depthIn === '' && currentStationDepth !== 0) {
      setDepthIn(currentStationDepth.toFixed(1));
      depthAutoInitialized.current = true;
    }
  }, [currentStationDepth]); // Only run when currentStationDepth changes

  const handleClose = () => {
    closeDialog();
  };

  const validateForm = (): boolean => {
    const lengthVal = parseFloat(length);
    if (!length || isNaN(lengthVal) || (allowZeroLength ? lengthVal < 0 : lengthVal <= 0)) {
      setError(allowZeroLength ? t('dialogs.drawShot.lengthNonNegative') : t('dialogs.drawShot.lengthPositive'));
      return false;
    }
    const azVal = parseFloat(azimuthIn);
    if (!azimuthIn || isNaN(azVal)) {
      setError(t('dialogs.drawShot.azimuthInRequired'));
      return false;
    }
    if (azVal < -360 || azVal > 360) {
      setError(t('dialogs.drawShot.azimuthRange'));
      return false;
    }
    return true;
  };

  const handleCreate = async () => {
    if (!fromStation || !section) return;

    if (!validateForm()) return;

    setIsLoading(true);
    setError(null);

    try {
      // Everything typed here is in ONE frame: the direction the leg was
      // measured in, stored verbatim - no mode reciprocates a bearing behind
      // the user's back, which is also what `completeDraftData` does for a
      // data-table insert. Turning the leg around is the checkbox's job, and it
      // routes through `reverseTargetShot` rather than a local flip: that
      // helper carries the antipodal tie-break matching the backend's
      // `average_azimuth`, and a second copy here is exactly the divergence its
      // comment documents. It is NOT identical to `reverse_shots` - that gates
      // the left/right swap behind the Reverse Section dialog's LRUD opt-in,
      // where this always swaps them.
      const rawAzimuthIn = parseFloat(azimuthIn);
      const rawAzimuthOut = azimuthOut ? parseFloat(azimuthOut) : rawAzimuthIn;
      const enteredDepthIn = depthIn ? parseFloat(depthIn) : currentStationDepth;
      const entered: TargetShotLike = {
        azimuth_in: normalizeAzimuth(rawAzimuthIn),
        azimuth_out: normalizeAzimuth(rawAzimuthOut),
        length: parseFloat(length),
        depth_in: enteredDepthIn,
        depth_out: depthOut ? parseFloat(depthOut) : enteredDepthIn,
        pitch_in: sourceGeometry?.pitch_in ?? null,
        pitch_out: sourceGeometry?.pitch_out ?? null,
        left: left ? parseFloat(left) : null,
        right: right ? parseFloat(right) : null,
        up: up ? parseFloat(up) : null,
        down: down ? parseFloat(down) : null,
      };
      const shotData: ShotData = {
        ...(reverseDirection ? reverseTargetShot(entered) : entered),
        comment: null,
        kind: shotKind,
        marker_index: null,
        temperature: null,
      };

      // Determine where to insert the shot
      let afterShotId: number | undefined;
      let beforeShotId: number | undefined;
      if (prependMode && section.shots.length > 0) {
        // Prepend: insert before the first shot
        beforeShotId = section.shots[0]?.id;
      } else if (fromStation.station_index > 0 && fromStation.station_index <= section.shots.length) {
        // Append after the shot at station_index - 1
        afterShotId = section.shots[fromStation.station_index - 1]?.id;
      }

      const result = await createShot(fromStation.section_id, shotData, afterShotId, beforeShotId);

      if (result.success) {
        // Select the newly created shot
        if (result.shot_id != null) {
          setSelection({
            type: 'shot',
            sectionId: fromStation.section_id,
            shotId: result.shot_id,
          });
        }
        trackDialogComplete('draw-shot');
        closeDialog();
      } else {
        setError(result.error ?? t('dialogs.drawShot.createFailed'));
      }
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.drawShot.createFailed')));
    } finally {
      setIsLoading(false);
    }
  };

  // Auto-fill azimuth out when azimuth in changes
  const handleAzimuthInChange = (value: string) => {
    setAzimuthIn(value);
    if (!azimuthOut && value) {
      setAzimuthOut(value);
    }
  };

  // Auto-fill depth out when depth in changes
  const handleDepthInChange = (value: string) => {
    setDepthIn(value);
    if (!depthOut && value) {
      setDepthOut(value);
    }
  };

  // Track if form has been modified (exclude auto-initialized depth/length from making dirty)
  const userModifiedDepth = depthIn !== '' && !depthAutoInitialized.current;
  const userModifiedLength = length !== '' && !lengthAutoInitialized.current;
  const isDirty = azimuthIn !== '' || userModifiedLength || userModifiedDepth || shotKind !== 'Normal' || reverseDirection || left !== '' || right !== '' || up !== '' || down !== '';

  if (!fromStation) {
    return null;
  }

  return (
    <DialogWrapper title={prependMode ? t('dialogs.drawShot.titlePrepend') : appendMode ? t('dialogs.drawShot.titleAppend', { index: fromStation.station_index }) : t('dialogs.drawShot.title')} onClose={handleClose} size="md" isDirty={isDirty} data-testid="draw-shot-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">{error}</div>
        )}

        {/* From station info */}
        <div className="p-3 bg-background rounded">
          <span className="text-text-secondary">{t('dialogs.drawShot.from')}</span>{' '}
          <span className="inline-flex items-center gap-1.5 align-middle font-medium">
            <EntityBlob entity="station">{formatStationId(fromStation.section_id, fromStation.station_index)}</EntityBlob>
            <EntityBlob entity="section">{sectionName}</EntityBlob>
          </span>
        </div>

        {/* Main fields */}
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="block text-sm text-text-secondary mb-1">
              {t('dialogs.drawShot.azimuthIn')} <span className="text-error">*</span>
            </label>
            <input
              type="number" inputMode="decimal"
              min="0"
              max="359.99"
              step="0.1"
              value={azimuthIn}
              onChange={(e) => handleAzimuthInChange(e.target.value)}
              className="w-full"
              placeholder="0-360"
              data-testid="draw-shot-azimuth-in-input"
            />
          </div>
          <div>
            <label className="block text-sm text-text-secondary mb-1">{t('dialogs.drawShot.azimuthOut')}</label>
            <input
              type="number" inputMode="decimal"
              min="0"
              max="359.99"
              step="0.1"
              value={azimuthOut}
              onChange={(e) => setAzimuthOut(e.target.value)}
              className="w-full"
              placeholder={t('dialogs.drawShot.sameAsIn')}
              data-testid="draw-shot-azimuth-out-input"
            />
          </div>
        </div>

        {/* Both azimuth fields are readings of the same FORWARD line, not a
            fore/back pair (`src-tauri/src/models/shot.rs`), so a leg measured
            the other way needs the whole shot turned around rather than just
            its bearings: the depths swap ends, pitch negates and swaps, and
            left/right trade places. Off by default - the entered bearing is
            the shot's own direction, which is how the section reads. */}
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={reverseDirection}
            onChange={(e) => setReverseDirection(e.target.checked)}
            data-testid="draw-shot-reverse-direction"
          />
          <span>{t('dialogs.drawShot.reverseDirection')}</span>
          <span className="text-text-secondary text-sm">
            {t('dialogs.drawShot.reverseDirectionHint')}
          </span>
        </label>

        <div>
          <label className="block text-sm text-text-secondary mb-1">
            {t('dialogs.drawShot.length')} <span className="text-error">*</span>
          </label>
          <input
            type="number" inputMode="decimal"
            min={allowZeroLength ? '0' : '0.01'}
            step="0.01"
            value={length}
            onChange={(e) => { lengthAutoInitialized.current = false; setLength(e.target.value); }}
            className="w-full"
            placeholder={t('dialogs.drawShot.lengthPlaceholder')}
            data-testid="draw-shot-length-input"
          />
        </div>

        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="block text-sm text-text-secondary mb-1">{t('dialogs.drawShot.depthIn')}</label>
            <input
              type="number" inputMode="decimal"
              step="0.1"
              value={depthIn}
              onChange={(e) => handleDepthInChange(e.target.value)}
              className="w-full"
              placeholder={currentStationDepth.toFixed(1)}
              data-testid="draw-shot-depth-in-input"
            />
          </div>
          <div>
            <label className="block text-sm text-text-secondary mb-1">{t('dialogs.drawShot.depthOut')}</label>
            <input
              type="number" inputMode="decimal"
              step="0.1"
              value={depthOut}
              onChange={(e) => setDepthOut(e.target.value)}
              className="w-full"
              placeholder={t('dialogs.drawShot.sameAsIn')}
              data-testid="draw-shot-depth-out-input"
            />
          </div>
        </div>

        {/* LRUD fields (collapsed by default) */}
        <details className="border border-border rounded">
          <summary
            className="px-3 py-2 cursor-pointer text-text-secondary hover:text-text-primary"
            data-testid="draw-shot-lrud-toggle"
          >
            {t('dialogs.drawShot.lrudOptional')}
          </summary>
          <div className="p-3 pt-0 grid grid-cols-4 gap-2">
            <div>
              <label className="block text-xs text-text-secondary mb-1">{t('dialogs.drawShot.left')}</label>
              <input
                type="number" inputMode="decimal"
                min="0"
                step="0.1"
                value={left}
                onChange={(e) => setLeft(e.target.value)}
                className="w-full"
                data-testid="draw-shot-left-input"
              />
            </div>
            <div>
              <label className="block text-xs text-text-secondary mb-1">{t('dialogs.drawShot.right')}</label>
              <input
                type="number" inputMode="decimal"
                min="0"
                step="0.1"
                value={right}
                onChange={(e) => setRight(e.target.value)}
                className="w-full"
                data-testid="draw-shot-right-input"
              />
            </div>
            <div>
              <label className="block text-xs text-text-secondary mb-1">{t('dialogs.drawShot.up')}</label>
              <input
                type="number" inputMode="decimal"
                min="0"
                step="0.1"
                value={up}
                onChange={(e) => setUp(e.target.value)}
                className="w-full"
                data-testid="draw-shot-up-input"
              />
            </div>
            <div>
              <label className="block text-xs text-text-secondary mb-1">{t('dialogs.drawShot.down')}</label>
              <input
                type="number" inputMode="decimal"
                min="0"
                step="0.1"
                value={down}
                onChange={(e) => setDown(e.target.value)}
                className="w-full"
                data-testid="draw-shot-down-input"
              />
            </div>
          </div>
        </details>

        {/* Shot kind (#27): drawing one shot by hand is exactly the case where
            the user knows whether they swam it, so offer the choice outright
            rather than leaving it to the classifier. */}
        <label className="flex items-center gap-2">
          <span>{t('dialogs.drawShot.shotKind')}</span>
          <Select
            data-testid="draw-shot-kind"
            value={shotKind}
            onChange={(v) => setShotKind(v as ShotKind)}
            options={SHOT_KIND_CYCLE.map((kind) => ({ value: kind, label: t(SHOT_KIND_LABEL_KEY[kind]) }))}
            size="sm"
            aria-label={t('dialogs.drawShot.shotKind')}
          />
          <span className="text-text-secondary text-sm">
            {t('dialogs.drawShot.shotKindHint')}
          </span>
        </label>

        {/* Actions */}
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleClose} disabled={isLoading} data-testid="draw-shot-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button variant="primary" onClick={handleCreate} disabled={isLoading} data-testid="draw-shot-create">
            {isLoading ? t('dialogs.drawShot.creating') : t('dialogs.drawShot.create')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
