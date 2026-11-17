import { useEffect, useState } from 'react';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { invokeIpc, getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import type { DeleteShotImpact, LineRelationships } from '../../types/commands';
import DeleteShotImpactDetails from './DeleteShotImpactDetails';

export default function ConfirmDialog() {
  const { closeDialog, dialogData, clearSelection, setSelection } = useUIStore();
  const { deleteShot, deleteLine, deleteLastShotAndLine, deleteShotAndSplit } = useProjectStore();

  const type = dialogData.type as 'station' | 'shot' | 'line' | 'last-shot' | undefined;
  const message = dialogData.message as string | undefined;
  const lineId = dialogData.lineId as number | undefined;
  const shotId = dialogData.shotId as number | undefined;
  const totalShots = dialogData.totalShots as number | undefined;
  const splitRequired = dialogData.splitRequired as boolean | undefined;
  const lineName = dialogData.lineName as string | undefined;

  // Split line names for mid-shot deletion
  const [splitNameA, setSplitNameA] = useState('');
  const [splitNameB, setSplitNameB] = useState('');

  // Initialize split names when dialog opens
  useEffect(() => {
    if (splitRequired && lineName) {
      setSplitNameA(`${lineName}-A`);
      setSplitNameB(`${lineName}-B`);
    }
  }, [splitRequired, lineName]);

  const [relationships, setRelationships] = useState<LineRelationships | null>(null);
  const [shotImpact, setShotImpact] = useState<DeleteShotImpact | null>(null);
  const [shotImpactError, setShotImpactError] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [isSubmitting, setIsSubmitting] = useState(false);

  // Whether this deletion removes an entire line (explicit last-shot type
  // or legacy shot type with totalShots === 1)
  const removesLine =
    type === 'last-shot' ||
    type === 'line' ||
    (type === 'shot' && totalShots === 1);

  // Fetch relationships when dialog opens for any deletion that removes a line
  useEffect(() => {
    if (removesLine && lineId !== undefined) {
      invokeIpc<LineRelationships>('get_line_relationships', { lineId }, { purpose: 'fetch line relationships for deletion' })
        .then(setRelationships)
        .catch(() => setRelationships(null));
    } else {
      setRelationships(null);
    }
  }, [removesLine, lineId]);

  // Fetch delete impact for shot deletions (edge shots)
  useEffect(() => {
    if (type === 'shot' && !removesLine && !splitRequired && lineId !== undefined && shotId !== undefined) {
      invokeIpc<DeleteShotImpact>('preview_delete_shot', { lineId, shotId }, { purpose: 'preview shot deletion impact' })
        .then((result) => {
          setShotImpact(result);
          setShotImpactError(null);
        })
        .catch((e) => {
          setShotImpact(null);
          setShotImpactError(getErrorMessage(e, 'Failed to load shot impacts'));
        });
    } else {
      setShotImpact(null);
      setShotImpactError(null);
    }
  }, [type, removesLine, splitRequired, lineId, shotId]);

  // Run the delete for the current dialog type. Returns true if it completed and
  // the dialog should close; false if the backend REFUSED (a frozen shot, an edge
  // shot in an adjusted network, a read-only/SpeleoDB-locked project) -- in which
  // case the error is surfaced via setDeleteError and the dialog stays open. Every
  // delete path (not just split) checks result.success, so a backend refusal is
  // never silently closed as a success (#241). Thrown transport errors are handled
  // by the caller.
  const runDelete = async (): Promise<boolean> => {
    if (type === 'last-shot' && lineId !== undefined && shotId !== undefined) {
      // Last remaining shot: delete shot + entire line
      const result = await deleteLastShotAndLine(lineId, shotId);
      if (!result.success) {
        setDeleteError(result.error ?? 'Failed to delete shot');
        return false;
      }
      clearSelection();
      return true;
    }
    if (type === 'shot' && lineId !== undefined && shotId !== undefined) {
      if (splitRequired) {
        // Mid-line shot: delete and split line
        const result = await deleteShotAndSplit(
          lineId,
          shotId,
          splitNameA.trim() || 'Line A',
          splitNameB.trim() || 'Line B',
        );
        if (!result.success) {
          setDeleteError(result.error ?? 'Failed to split line');
          return false;
        }
        // Select line A (keeps original ID) after split
        setSelection({ type: 'line', lineId });
        return true;
      }
      if (totalShots === 1) {
        // Last remaining shot: delete shot + line
        const result = await deleteLastShotAndLine(lineId, shotId);
        if (!result.success) {
          setDeleteError(result.error ?? 'Failed to delete shot');
          return false;
        }
        clearSelection();
        return true;
      }
      // Edge shot (or legacy caller without position data): simple delete
      const result = await deleteShot(lineId, shotId);
      if (!result.success) {
        setDeleteError(result.error ?? 'Failed to delete shot');
        return false;
      }
      clearSelection();
      return true;
    }
    if (type === 'line' && lineId !== undefined) {
      const result = await deleteLine(lineId);
      if (!result.success) {
        setDeleteError(result.error ?? 'Failed to delete line');
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
      setDeleteError(getErrorMessage(e, 'Delete failed'));
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
        return 'Delete Station';
      case 'shot':
        return splitRequired ? 'Delete Shot & Split Line' : 'Delete Shot';
      case 'last-shot':
        return 'Delete Shot & Line';
      case 'line':
        return 'Delete Line';
      default:
        return 'Confirm';
    }
  };

  const getDefaultMessage = () => {
    switch (type) {
      case 'station':
        return 'Deleting a station will delete all connected shots. Are you sure?';
      case 'shot':
        return 'Are you sure you want to delete this shot?';
      case 'last-shot':
        return 'Deleting the last shot will remove the entire line. Continue?';
      case 'line':
        return 'Are you sure you want to delete this line and all its shots?';
      default:
        return 'Are you sure?';
    }
  };

  const hasRelationships =
    relationships !== null &&
    (relationships.merges.length > 0 ||
      relationships.closures.length > 0 ||
      relationships.has_line_offset ||
      relationships.station_offset_count > 0 ||
      relationships.zero_datum_count > 0);

  return (
    <DialogWrapper title={getTitle()} onClose={closeDialog} size="md" data-testid="confirm-dialog">
      <div className="space-y-4">
        <p className="text-text-secondary">{message ?? getDefaultMessage()}</p>

        {type === 'station' && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
            Note: To delete a station, delete the shots connected to it instead.
          </div>
        )}

        {type === 'shot' && splitRequired && (
          <div className="space-y-3">
            <div>
              <label className="block text-sm text-text-secondary mb-1">Line A name (first half)</label>
              <input
                type="text"
                value={splitNameA}
                onChange={(e) => setSplitNameA(e.target.value)}
                className="w-full"
                data-testid="split-name-a"
              />
            </div>
            <div>
              <label className="block text-sm text-text-secondary mb-1">Line B name (second half)</label>
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

        {removesLine && hasRelationships && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-sm space-y-1">
            <p className="font-medium text-warning">The following relationships will also be removed:</p>
            <ul className="list-disc list-inside text-text-secondary">
              {relationships!.merges.length > 0 && (
                <li>{relationships!.merges.length} station merge{relationships!.merges.length !== 1 ? 's' : ''}</li>
              )}
              {relationships!.closures.length > 0 && (
                <li>{relationships!.closures.length} loop closure{relationships!.closures.length !== 1 ? 's' : ''}</li>
              )}
              {relationships!.has_line_offset && (
                <li>1 line offset</li>
              )}
              {relationships!.station_offset_count > 0 && (
                <li>{relationships!.station_offset_count} station offset{relationships!.station_offset_count !== 1 ? 's' : ''}</li>
              )}
              {relationships!.zero_datum_count > 0 && (
                <li>{relationships!.zero_datum_count} zero datum{relationships!.zero_datum_count !== 1 ? 's' : ''}</li>
              )}
            </ul>
          </div>
        )}

        {type === 'shot' && !removesLine && !splitRequired && (
          <>
            {shotImpactError && (
              <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
                {shotImpactError}
              </div>
            )}
            <DeleteShotImpactDetails
              impact={shotImpact}
              title="Deleting this shot will update station relationships:"
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
          <Button onClick={closeDialog} data-testid="confirm-dialog-cancel">Cancel</Button>
          <Button
            data-testid="confirm-dialog-confirm"
            variant="danger"
            onClick={handleConfirm}
            disabled={type === 'station' || isSubmitting}
            data-primary-action=""
          >
            {splitRequired ? 'Delete & Split' : 'Delete'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
