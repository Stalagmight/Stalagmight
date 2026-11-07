import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import i18n from '../../i18n';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { invokeIpc, getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import type { DeleteShotImpact, SectionRelationships } from '../../types/commands';
import DeleteShotImpactDetails from './DeleteShotImpactDetails';
import { useShallow } from 'zustand/react/shallow';

export default function ConfirmDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, clearSelection, setSelection } = useUIStore(
    useShallow((s) => ({
      closeDialog: s.closeDialog,
      dialogData: s.dialogData,
      clearSelection: s.clearSelection,
      setSelection: s.setSelection,
    })),
  );
  const { deleteShot, deleteSection, deleteLastShotAndSection, deleteShotAndSplit } = useProjectStore(
    useShallow((s) => ({
      deleteShot: s.deleteShot,
      deleteSection: s.deleteSection,
      deleteLastShotAndSection: s.deleteLastShotAndSection,
      deleteShotAndSplit: s.deleteShotAndSplit,
    })),
  );

  const type = dialogData.type as 'station' | 'shot' | 'section' | 'last-shot' | undefined;
  const message = dialogData.message as string | undefined;
  const sectionId = dialogData.sectionId as number | undefined;
  const shotId = dialogData.shotId as number | undefined;
  const totalShots = dialogData.totalShots as number | undefined;
  const splitRequired = dialogData.splitRequired as boolean | undefined;
  const sectionName = dialogData.sectionName as string | undefined;

  // Split section names for mid-shot deletion
  const [splitNameA, setSplitNameA] = useState('');
  const [splitNameB, setSplitNameB] = useState('');

  // Initialize split names when dialog opens
  useEffect(() => {
    if (splitRequired && sectionName) {
      setSplitNameA(`${sectionName}-A`);
      setSplitNameB(`${sectionName}-B`);
    }
  }, [splitRequired, sectionName]);

  const [relationships, setRelationships] = useState<SectionRelationships | null>(null);
  const [shotImpact, setShotImpact] = useState<DeleteShotImpact | null>(null);
  const [shotImpactError, setShotImpactError] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [isSubmitting, setIsSubmitting] = useState(false);

  // Whether this deletion removes an entire section (explicit last-shot type
  // or legacy shot type with totalShots === 1)
  const removesSection =
    type === 'last-shot' ||
    type === 'section' ||
    (type === 'shot' && totalShots === 1);

  // Fetch relationships when dialog opens for any deletion that removes a section
  useEffect(() => {
    if (removesSection && sectionId !== undefined) {
      invokeIpc('get_section_relationships', { sectionId }, { purpose: 'fetch section relationships for deletion' })
        .then(setRelationships)
        .catch(() => setRelationships(null));
    } else {
      setRelationships(null);
    }
  }, [removesSection, sectionId]);

  // Fetch delete impact for shot deletions (edge shots)
  useEffect(() => {
    if (type === 'shot' && !removesSection && !splitRequired && sectionId !== undefined && shotId !== undefined) {
      invokeIpc('preview_delete_shot', { sectionId, shotId }, { purpose: 'preview shot deletion impact' })
        .then((result) => {
          setShotImpact(result);
          setShotImpactError(null);
        })
        .catch((e) => {
          setShotImpact(null);
          // i18n.t, not the hook's t: this IPC effect must not re-fetch on a language change.
          setShotImpactError(getErrorMessage(e, i18n.t('dialogs.confirm.errors.loadImpactsFailed')));
        });
    } else {
      setShotImpact(null);
      setShotImpactError(null);
    }
  }, [type, removesSection, splitRequired, sectionId, shotId]);

  // Run the delete for the current dialog type. Returns true if it completed and
  // the dialog should close; false if the backend REFUSED (a frozen shot, an edge
  // shot in an adjusted network, a read-only/SpeleoDB-locked project) -- in which
  // case the error is surfaced via setDeleteError and the dialog stays open. Every
  // delete path (not just split) checks result.success, so a backend refusal is
  // never silently closed as a success (#241). Thrown transport errors are handled
  // by the caller.
  const runDelete = async (): Promise<boolean> => {
    if (type === 'last-shot' && sectionId !== undefined && shotId !== undefined) {
      // Last remaining shot: delete shot + entire section
      const result = await deleteLastShotAndSection(sectionId, shotId);
      if (!result.success) {
        setDeleteError(result.error ?? t('dialogs.confirm.errors.deleteShot'));
        return false;
      }
      clearSelection();
      return true;
    }
    if (type === 'shot' && sectionId !== undefined && shotId !== undefined) {
      if (splitRequired) {
        // Mid-section shot: delete and split section
        const result = await deleteShotAndSplit(
          sectionId,
          shotId,
          splitNameA.trim() || t('dialogs.confirm.defaultSplitNameA'),
          splitNameB.trim() || t('dialogs.confirm.defaultSplitNameB'),
        );
        if (!result.success) {
          setDeleteError(result.error ?? t('dialogs.confirm.errors.splitSection'));
          return false;
        }
        // Select section A (keeps original ID) after split
        setSelection({ type: 'section', sectionId });
        return true;
      }
      if (totalShots === 1) {
        // Last remaining shot: delete shot + section
        const result = await deleteLastShotAndSection(sectionId, shotId);
        if (!result.success) {
          setDeleteError(result.error ?? t('dialogs.confirm.errors.deleteShot'));
          return false;
        }
        clearSelection();
        return true;
      }
      // Edge shot (or legacy caller without position data): simple delete
      const result = await deleteShot(sectionId, shotId);
      if (!result.success) {
        setDeleteError(result.error ?? t('dialogs.confirm.errors.deleteShot'));
        return false;
      }
      clearSelection();
      return true;
    }
    if (type === 'section' && sectionId !== undefined) {
      const result = await deleteSection(sectionId);
      if (!result.success) {
        setDeleteError(result.error ?? t('dialogs.confirm.errors.deleteSection'));
        return false;
      }
      clearSelection();
      return true;
    }
    // type === 'station': deleting a station is not directly supported; the user
    // deletes the connected shots instead (see the note rendered for this type).
    return true;
  };

  const handleConfirm = async () => {
    if (isSubmitting) return; // guard against a double-click firing a second delete
    setIsSubmitting(true);
    setDeleteError(null);
    let shouldClose = false;
    try {
      shouldClose = await runDelete();
    } catch (e) {
      // A genuine IPC/transport failure rejects (distinct from a returned
      // {success:false}); surface it too rather than leaving the click dead.
      setDeleteError(getErrorMessage(e, t('dialogs.confirm.errors.deleteFailed')));
    }
    if (!shouldClose) {
      setIsSubmitting(false);
      return;
    }
    // The delete IPC is async; if the user dismissed this dialog while it was in
    // flight, activeDialog has already moved on. closeDialog() is global UI state,
    // so calling it now would close whatever dialog is open instead -- only
    // finalize if this confirm dialog is still the active one.
    if (useUIStore.getState().activeDialog === 'confirm-delete') {
      trackDialogComplete('confirm-delete');
      closeDialog();
    }
  };

  const getTitle = () => {
    switch (type) {
      case 'station':
        return t('dialogs.confirm.title.station');
      case 'shot':
        return splitRequired ? t('dialogs.confirm.title.shotSplit') : t('dialogs.confirm.title.shot');
      case 'last-shot':
        return t('dialogs.confirm.title.lastShot');
      case 'section':
        return t('dialogs.confirm.title.section');
      default:
        return t('dialogs.confirm.title.default');
    }
  };

  const getDefaultMessage = () => {
    switch (type) {
      case 'station':
        return t('dialogs.confirm.message.station');
      case 'shot':
        return t('dialogs.confirm.message.shot');
      case 'last-shot':
        return t('dialogs.confirm.message.lastShot');
      case 'section':
        return t('dialogs.confirm.message.section');
      default:
        return t('dialogs.confirm.message.default');
    }
  };

  const hasRelationships =
    relationships !== null &&
    (relationships.merges.length > 0 ||
      relationships.closures.length > 0 ||
      relationships.has_section_offset ||
      relationships.station_offset_count > 0 ||
      relationships.zero_datum_count > 0);

  return (
    <DialogWrapper title={getTitle()} onClose={closeDialog} size="md" data-testid="confirm-dialog">
      <div className="space-y-4">
        <p className="text-text-secondary">{message ?? getDefaultMessage()}</p>

        {type === 'station' && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
            {t('dialogs.confirm.stationNote')}
          </div>
        )}

        {type === 'shot' && splitRequired && (
          <div className="space-y-3">
            <div>
              <label className="block text-sm text-text-secondary mb-1">{t('dialogs.confirm.splitNameA')}</label>
              <input
                type="text"
                value={splitNameA}
                onChange={(e) => setSplitNameA(e.target.value)}
                className="w-full"
                data-testid="split-name-a"
              />
            </div>
            <div>
              <label className="block text-sm text-text-secondary mb-1">{t('dialogs.confirm.splitNameB')}</label>
              <input
                type="text"
                value={splitNameB}
                onChange={(e) => setSplitNameB(e.target.value)}
                className="w-full"
                data-testid="split-name-b"
              />
            </div>
          </div>
        )}

        {removesSection && hasRelationships && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-sm space-y-1">
            <p className="font-medium text-warning">{t('dialogs.confirm.relationshipsRemoved')}</p>
            <ul className="list-disc list-inside text-text-secondary">
              {relationships!.merges.length > 0 && (
                <li>{t('dialogs.confirm.stationMerges', { count: relationships!.merges.length })}</li>
              )}
              {relationships!.closures.length > 0 && (
                <li>{t('dialogs.confirm.loopClosures', { count: relationships!.closures.length })}</li>
              )}
              {relationships!.has_section_offset && (
                <li>{t('dialogs.confirm.sectionOffset')}</li>
              )}
              {relationships!.station_offset_count > 0 && (
                <li>{t('dialogs.confirm.stationOffsets', { count: relationships!.station_offset_count })}</li>
              )}
              {relationships!.zero_datum_count > 0 && (
                <li>{t('dialogs.confirm.zeroDatums', { count: relationships!.zero_datum_count })}</li>
              )}
            </ul>
          </div>
        )}

        {type === 'shot' && !removesSection && !splitRequired && (
          <>
            {shotImpactError && (
              <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
                {shotImpactError}
              </div>
            )}
            <DeleteShotImpactDetails
              impact={shotImpact}
              title={t('dialogs.confirm.impactTitle')}
            />
          </>
        )}

        {deleteError && (
          <div
            className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm"
            data-testid="delete-error"
          >
            {deleteError}
          </div>
        )}

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} data-testid="confirm-dialog-cancel">{t('common.actions.cancel')}</Button>
          <Button
            data-testid="confirm-dialog-confirm"
            variant="danger"
            onClick={handleConfirm}
            disabled={type === 'station' || isSubmitting}
            data-primary-action=""
          >
            {splitRequired ? t('dialogs.confirm.deleteAndSplit') : t('common.actions.delete')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
