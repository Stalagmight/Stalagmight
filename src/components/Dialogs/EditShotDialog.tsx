import { useState, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { normalizeAzimuth } from '../../utils/azimuth';
import { f32Text } from '../../utils/f32Text';
import { changedShotFields } from '../../utils/shotUpdatesDiff';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { useShallow } from 'zustand/react/shallow';
import type { Shot } from '../../types/project';
import type { ShotUpdates } from '../../types/ipc/ShotUpdates';

export default function EditShotDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData } = useUIStore(
    useShallow((s) => ({ closeDialog: s.closeDialog, dialogData: s.dialogData })),
  );
  // `frozenShots` is picked but not destructured: isShotFrozen reads it through
  // get(), so a change to the frozen set must still re-render this dialog.
  const { updateShot, project, isShotFrozen } = useProjectStore(
    useShallow((s) => ({
      updateShot: s.updateShot,
      project: s.project,
      isShotFrozen: s.isShotFrozen,
      frozenShots: s.frozenShots,
    })),
  );

  const sectionId = dialogData.sectionId as number | undefined;
  const shotId = dialogData.shotId as number | undefined;

  const section = project?.sections.find((s) => s.id === sectionId);
  const shot = section?.shots.find((s) => s.id === shotId);
  const shotIndex = section?.shots.findIndex((s) => s.id === shotId) ?? -1;
  const frozen = sectionId !== undefined && shotIndex >= 0 && isShotFrozen(sectionId, shotIndex);

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

  // Initialize form with shot data. [#576 NF-W7F-1] Each reading as entered: the
  // store holds it as an f32 (1.2000000476837158 for 1.2), so `seeded` shows the
  // shortest decimal that is the same f32.
  useEffect(() => {
    if (shot) {
      setAzimuthIn(seeded(shot.azimuth_in));
      setAzimuthOut(seeded(shot.azimuth_out));
      setLength(seeded(shot.length));
      setDepthIn(seeded(shot.depth_in));
      setDepthOut(seeded(shot.depth_out));
      setPitchIn(seeded(shot.pitch_in));
      setPitchOut(seeded(shot.pitch_out));
      setLeft(seeded(shot.left));
      setRight(seeded(shot.right));
      setUp(seeded(shot.up));
      setDown(seeded(shot.down));
      setComment(shot.comment ?? '');
    }
  }, [shot]);

  const validateForm = (): boolean => {
    const lengthVal = parseFloat(length);
    if (isNaN(lengthVal) || lengthVal <= 0) {
      setError(t('dialogs.editShot.lengthPositive'));
      return false;
    }

    const azInVal = parseFloat(azimuthIn);
    if (isNaN(azInVal) || azInVal < -360 || azInVal > 360) {
      setError(t('dialogs.editShot.azimuthInRange'));
      return false;
    }

    const azOutVal = parseFloat(azimuthOut);
    if (isNaN(azOutVal) || azOutVal < -360 || azOutVal > 360) {
      setError(t('dialogs.editShot.azimuthOutRange'));
      return false;
    }

    return true;
  };

  const handleSave = async () => {
    if (sectionId === undefined || shotId === undefined) return;

    if (!validateForm()) return;

    setIsLoading(true);
    setError(null);

    try {
      // [#576 F32-EDIT] Only the fields whose text the user changed: the store holds
      // every reading rounded through an f32 (the binary envelope), so re-sending an
      // untouched one would write that rounding over the backend's exact value.
      const updates = shot
        ? changedShotFormFields(shot, {
            azimuthIn, azimuthOut, length, depthIn, depthOut, pitchIn, pitchOut,
            left, right, up, down, comment,
          })
        : {};
      if (Object.keys(updates).length === 0) {
        trackDialogComplete('edit-shot');
        closeDialog();
        return;
      }
      const result = await updateShot(sectionId, shotId, updates);

      if (result.success) {
        trackDialogComplete('edit-shot');
        closeDialog();
      } else {
        setError(result.error ?? t('dialogs.editShot.updateFailed'));
      }
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.editShot.updateFailed')));
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

  // Track if form has been modified: whether Save would send anything.
  const isDirty = shot
    ? Object.keys(
        changedShotFormFields(shot, {
          azimuthIn, azimuthOut, length, depthIn, depthOut, pitchIn, pitchOut,
          left, right, up, down, comment,
        }),
      ).length > 0
    : false;

  if (!shot) {
    return (
      <DialogWrapper title={t('dialogs.editShot.title')} onClose={closeDialog} size="lg" data-testid="edit-shot-dialog">
        <div className="text-error">{t('dialogs.editShot.notFound')}</div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper title={t('dialogs.editShot.title')} onClose={closeDialog} size="lg" isDirty={isDirty} data-testid="edit-shot-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <div className="text-sm text-text-secondary">
          {t('dialogs.editShot.summary', {
            section: section?.name ?? t('dialogs.editShot.sectionFallback', { id: sectionId }),
            shotId,
          })}
        </div>

        {frozen && (
          <div className="p-2 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
            {t('dialogs.editShot.frozen')}
          </div>
        )}

        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className="block text-sm text-text-secondary mb-1">{t('dialogs.editShot.azimuthIn')}</label>
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
            <label className="block text-sm text-text-secondary mb-1">{t('dialogs.editShot.azimuthOut')}</label>
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
          <label className="block text-sm text-text-secondary mb-1">{t('dialogs.editShot.length')}</label>
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
            <label className="block text-sm text-text-secondary mb-1">{t('dialogs.editShot.depthIn')}</label>
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
            <label className="block text-sm text-text-secondary mb-1">{t('dialogs.editShot.depthOut')}</label>
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
            <label className="block text-sm text-text-secondary mb-1">{t('dialogs.editShot.pitchIn')}</label>
            <input
              type="number" inputMode="decimal"
              value={pitchIn}
              onChange={(e) => setPitchIn(e.target.value)}
              className={`w-full ${pitchInInvalid ? 'ring-2 ring-error' : ''}`}
              step="0.1"
              min="-90"
              max="90"
              placeholder={t('dialogs.editShot.optional')}
              disabled={frozen}
              data-testid="edit-shot-pitch-in-input"
            />
          </div>
          <div>
            <label className="block text-sm text-text-secondary mb-1">{t('dialogs.editShot.pitchOut')}</label>
            <input
              type="number" inputMode="decimal"
              value={pitchOut}
              onChange={(e) => setPitchOut(e.target.value)}
              className={`w-full ${pitchOutInvalid ? 'ring-2 ring-error' : ''}`}
              step="0.1"
              min="-90"
              max="90"
              placeholder={t('dialogs.editShot.optional')}
              disabled={frozen}
              data-testid="edit-shot-pitch-out-input"
            />
          </div>
        </div>

        <div className="border-t border-border pt-3">
          <label className="block text-sm text-text-secondary mb-2">{t('dialogs.editShot.lrud')}</label>
          <div className="grid grid-cols-4 gap-2">
            <div>
              <input
                type="number" inputMode="decimal"
                value={left}
                onChange={(e) => setLeft(e.target.value)}
                className="w-full"
                step="0.1"
                placeholder={t('dialogs.editShot.leftShort')}
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
                placeholder={t('dialogs.editShot.rightShort')}
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
                placeholder={t('dialogs.editShot.upShort')}
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
                placeholder={t('dialogs.editShot.downShort')}
                data-testid="edit-shot-down-input"
              />
            </div>
          </div>
        </div>

        <div>
          <label className="block text-sm text-text-secondary mb-1">{t('dialogs.editShot.comment')}</label>
          <textarea
            value={comment}
            onChange={(e) => setComment(e.target.value)}
            className="w-full h-20 resize-none"
            placeholder={t('dialogs.editShot.commentPlaceholder')}
            data-testid="edit-shot-comment-input"
          />
        </div>

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="edit-shot-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button variant="primary" onClick={handleSave} disabled={isLoading} data-testid="edit-shot-save">
            {isLoading ? t('dialogs.editShot.saving') : t('common.actions.save')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}

/** The form's text fields, as typed. */
interface ShotForm {
  azimuthIn: string;
  azimuthOut: string;
  length: string;
  depthIn: string;
  depthOut: string;
  pitchIn: string;
  pitchOut: string;
  left: string;
  right: string;
  up: string;
  down: string;
  comment: string;
}

/** An input's text for a stored reading: blank when unset (#576 NF-W7F-1, `f32Text`). */
const seeded = (v: number | null | undefined) => (v == null ? '' : f32Text(v));

/**
 * The update for the fields whose text differs from what the form was seeded
 * with (`shot`'s own values), less any whose typed value is the same f32 as the
 * stored one (#576 NF-W7F-1: 1.20 re-typed over 1.2 is not an edit). `isDirty`
 * is this patch being non-empty. Kind, marker index and temperature have no input
 * here and are never sent.
 */
function changedShotFormFields(shot: Shot, form: ShotForm): Partial<ShotUpdates> {
  const updates: Partial<ShotUpdates> = {};
  const optional = (v: string) => (v ? parseFloat(v) : null);
  if (form.azimuthIn !== seeded(shot.azimuth_in)) updates.azimuth_in = normalizeAzimuth(parseFloat(form.azimuthIn));
  if (form.azimuthOut !== seeded(shot.azimuth_out)) updates.azimuth_out = normalizeAzimuth(parseFloat(form.azimuthOut));
  if (form.length !== seeded(shot.length)) updates.length = parseFloat(form.length);
  if (form.depthIn !== seeded(shot.depth_in)) updates.depth_in = parseFloat(form.depthIn);
  if (form.depthOut !== seeded(shot.depth_out)) updates.depth_out = parseFloat(form.depthOut);
  if (form.pitchIn !== seeded(shot.pitch_in)) updates.pitch_in = optional(form.pitchIn);
  if (form.pitchOut !== seeded(shot.pitch_out)) updates.pitch_out = optional(form.pitchOut);
  if (form.left !== seeded(shot.left)) updates.left = optional(form.left);
  if (form.right !== seeded(shot.right)) updates.right = optional(form.right);
  if (form.up !== seeded(shot.up)) updates.up = optional(form.up);
  if (form.down !== seeded(shot.down)) updates.down = optional(form.down);
  if (form.comment !== (shot.comment ?? '')) updates.comment = form.comment.trim() || null;
  return changedShotFields(shot, updates, 'f32');
}
