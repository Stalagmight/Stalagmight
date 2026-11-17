import { useState, useEffect } from 'react';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { normalizeAzimuth } from '../../utils/azimuth';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

export default function EditShotDialog() {
  const { closeDialog, dialogData } = useUIStore();
  const { updateShot, project, isShotFrozen } = useProjectStore();

  const lineId = dialogData.lineId as number | undefined;
  const shotId = dialogData.shotId as number | undefined;

  const line = project?.lines.find((s) => s.id === lineId);
  const shot = line?.shots.find((s) => s.id === shotId);
  const shotIndex = line?.shots.findIndex((s) => s.id === shotId) ?? -1;
  const frozen = lineId !== undefined && shotIndex >= 0 && isShotFrozen(lineId, shotIndex);

  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Form state
  const [azimuthIn, setAzimuthIn] = useState('');
  const [azimuthOut, setAzimuthOut] = useState('');
  const [length, setLength] = useState('');
  const [depthIn, setDepthIn] = useState('');
  const [depthOut, setDepthOut] = useState('');
  const [pitchIn, setPitchIn] = useState('');
  const [pitchOut, setPitchOut] = useState('');
  const [left, setLeft] = useState('');
  const [right, setRight] = useState('');
  const [up, setUp] = useState('');
  const [down, setDown] = useState('');
  const [comment, setComment] = useState('');

  // Initialize form with shot data
  useEffect(() => {
    if (shot) {
      setAzimuthIn(shot.azimuth_in.toString());
      setAzimuthOut(shot.azimuth_out.toString());
      setLength(shot.length.toString());
      setDepthIn(shot.depth_in.toString());
      setDepthOut(shot.depth_out.toString());
      setPitchIn(shot.pitch_in?.toString() ?? '');
      setPitchOut(shot.pitch_out?.toString() ?? '');
      setLeft(shot.left?.toString() ?? '');
      setRight(shot.right?.toString() ?? '');
      setUp(shot.up?.toString() ?? '');
      setDown(shot.down?.toString() ?? '');
      setComment(shot.comment ?? '');
    }
  }, [shot]);

  const validateForm = (): boolean => {
    const lengthVal = parseFloat(length);
    if (isNaN(lengthVal) || lengthVal <= 0) {
      setError('Length must be greater than 0');
      return false;
    }

    const azInVal = parseFloat(azimuthIn);
    if (isNaN(azInVal) || azInVal < -360 || azInVal > 360) {
      setError('Azimuth In must be between -360 and 360');
      return false;
    }

    const azOutVal = parseFloat(azimuthOut);
    if (isNaN(azOutVal) || azOutVal < -360 || azOutVal > 360) {
      setError('Azimuth Out must be between -360 and 360');
      return false;
    }

    return true;
  };

  const handleSave = async () => {
    if (lineId === undefined || shotId === undefined) return;

    if (!validateForm()) return;

    setIsLoading(true);
    setError(null);

    try {
      const result = await updateShot(lineId, shotId, {
        azimuth_in: normalizeAzimuth(parseFloat(azimuthIn)),
        azimuth_out: normalizeAzimuth(parseFloat(azimuthOut)),
        length: parseFloat(length),
        depth_in: parseFloat(depthIn),
        depth_out: parseFloat(depthOut),
        pitch_in: pitchIn ? parseFloat(pitchIn) : null,
        pitch_out: pitchOut ? parseFloat(pitchOut) : null,
        left: left ? parseFloat(left) : null,
        right: right ? parseFloat(right) : null,
        up: up ? parseFloat(up) : null,
        down: down ? parseFloat(down) : null,
        comment: comment.trim() || null,
        kind: shot?.kind ?? 'Normal',
        marker_index: shot?.marker_index ?? null,
        temperature: shot?.temperature ?? null,
      });

      if (result.success) {
        trackDialogComplete('edit-shot');
        closeDialog();
      } else {
        setError(result.error ?? 'Failed to update shot');
      }
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to update shot'));
    } finally {
      setIsLoading(false);
    }
  };

  // Pitch (inclination) is only valid in [-90°, 90°]; highlight the input red
  // (matching the backend INVALID_PITCH error) when a typed value is out of range.
  // Purely presentational - submission is not blocked on this.
  const pitchInVal = parseFloat(pitchIn);
  const pitchInInvalid = Number.isFinite(pitchInVal) && (pitchInVal < -90 || pitchInVal > 90);
  const pitchOutVal = parseFloat(pitchOut);
  const pitchOutInvalid = Number.isFinite(pitchOutVal) && (pitchOutVal < -90 || pitchOutVal > 90);

  // Track if form has been modified
  const isDirty = shot ? (
    azimuthIn !== shot.azimuth_in.toString() ||
    azimuthOut !== shot.azimuth_out.toString() ||
    length !== shot.length.toString() ||
    depthIn !== shot.depth_in.toString() ||
    depthOut !== shot.depth_out.toString() ||
    pitchIn !== (shot.pitch_in?.toString() ?? '') ||
    pitchOut !== (shot.pitch_out?.toString() ?? '') ||
    left !== (shot.left?.toString() ?? '') ||
    right !== (shot.right?.toString() ?? '') ||
    up !== (shot.up?.toString() ?? '') ||
    down !== (shot.down?.toString() ?? '') ||
    comment !== (shot.comment ?? '')
  ) : false;

  if (!shot) {
    return (
      <DialogWrapper title="Edit Shot" onClose={closeDialog} size="lg" data-testid="edit-shot-dialog">
        <div className="text-error">Shot not found</div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper title="Edit Shot" onClose={closeDialog} size="lg" isDirty={isDirty} data-testid="edit-shot-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <div className="text-sm text-text-secondary">
          Line: {line?.name ?? `Line ${lineId}`} | Shot ID: {shotId}
        </div>

        {frozen && (
          <div className="p-2 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
            Geometry fields are locked - this shot is part of an adjusted survey network.
          </div>
        )}

        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className="block text-sm text-text-secondary mb-1">Azimuth In (°)</label>
            <input
              type="number" inputMode="decimal"
              value={azimuthIn}
              onChange={(e) => setAzimuthIn(e.target.value)}
              className="w-full"
              step="0.1"
              disabled={frozen}
              data-testid="edit-shot-azimuth-in-input"
            />
          </div>
          <div>
            <label className="block text-sm text-text-secondary mb-1">Azimuth Out (°)</label>
            <input
              type="number" inputMode="decimal"
              value={azimuthOut}
              onChange={(e) => setAzimuthOut(e.target.value)}
              className="w-full"
              step="0.1"
              disabled={frozen}
              data-testid="edit-shot-azimuth-out-input"
            />
          </div>
        </div>

        <div>
          <label className="block text-sm text-text-secondary mb-1">Length (m)</label>
          <input
            type="number" inputMode="decimal"
            value={length}
            onChange={(e) => setLength(e.target.value)}
            className="w-full"
            step="0.01"
            min="0"
            disabled={frozen}
            data-testid="edit-shot-length-input"
          />
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className="block text-sm text-text-secondary mb-1">Depth In (m)</label>
            <input
              type="number" inputMode="decimal"
              value={depthIn}
              onChange={(e) => setDepthIn(e.target.value)}
              className="w-full"
              step="0.1"
              disabled={frozen}
              data-testid="edit-shot-depth-in-input"
            />
          </div>
          <div>
            <label className="block text-sm text-text-secondary mb-1">Depth Out (m)</label>
            <input
              type="number" inputMode="decimal"
              value={depthOut}
              onChange={(e) => setDepthOut(e.target.value)}
              className="w-full"
              step="0.1"
              disabled={frozen}
              data-testid="edit-shot-depth-out-input"
            />
          </div>
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className="block text-sm text-text-secondary mb-1">Pitch In (°)</label>
            <input
              type="number" inputMode="decimal"
              value={pitchIn}
              onChange={(e) => setPitchIn(e.target.value)}
              className={`w-full ${pitchInInvalid ? 'ring-2 ring-error' : ''}`}
              step="0.1"
              min="-90"
              max="90"
              placeholder="Optional"
              disabled={frozen}
              data-testid="edit-shot-pitch-in-input"
            />
          </div>
          <div>
            <label className="block text-sm text-text-secondary mb-1">Pitch Out (°)</label>
            <input
              type="number" inputMode="decimal"
              value={pitchOut}
              onChange={(e) => setPitchOut(e.target.value)}
              className={`w-full ${pitchOutInvalid ? 'ring-2 ring-error' : ''}`}
              step="0.1"
              min="-90"
              max="90"
              placeholder="Optional"
              disabled={frozen}
              data-testid="edit-shot-pitch-out-input"
            />
          </div>
        </div>

        <div className="border-t border-border pt-3">
          <label className="block text-sm text-text-secondary mb-2">LRUD (m)</label>
          <div className="grid grid-cols-4 gap-2">
            <div>
              <input
                type="number" inputMode="decimal"
                value={left}
                onChange={(e) => setLeft(e.target.value)}
                className="w-full"
                step="0.1"
                placeholder="L"
                data-testid="edit-shot-left-input"
              />
            </div>
            <div>
              <input
                type="number" inputMode="decimal"
                value={right}
                onChange={(e) => setRight(e.target.value)}
                className="w-full"
                step="0.1"
                placeholder="R"
                data-testid="edit-shot-right-input"
              />
            </div>
            <div>
              <input
                type="number" inputMode="decimal"
                value={up}
                onChange={(e) => setUp(e.target.value)}
                className="w-full"
                step="0.1"
                placeholder="U"
                data-testid="edit-shot-up-input"
              />
            </div>
            <div>
              <input
                type="number" inputMode="decimal"
                value={down}
                onChange={(e) => setDown(e.target.value)}
                className="w-full"
                step="0.1"
                placeholder="D"
                data-testid="edit-shot-down-input"
              />
            </div>
          </div>
        </div>

        <div>
          <label className="block text-sm text-text-secondary mb-1">Comment</label>
          <textarea
            value={comment}
            onChange={(e) => setComment(e.target.value)}
            className="w-full h-20 resize-none"
            placeholder="Optional comment"
            data-testid="edit-shot-comment-input"
          />
        </div>

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="edit-shot-cancel">
            Cancel
          </Button>
          <Button variant="primary" onClick={handleSave} disabled={isLoading} data-testid="edit-shot-save">
            {isLoading ? 'Saving...' : 'Save'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
