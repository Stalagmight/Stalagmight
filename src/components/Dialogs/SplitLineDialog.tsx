import { useState, useEffect } from 'react';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

export default function SplitLineDialog() {
  const { closeDialog, dialogData, setSelection } = useUIStore();
  const { deleteShotAndSplit } = useProjectStore();

  const lineId = dialogData.lineId as number | undefined;
  const shotId = dialogData.shotId as number | undefined;
  const lineName = dialogData.lineName as string | undefined;

  const defaultNameA = lineName ? `${lineName}-A` : 'Line-A';
  const defaultNameB = lineName ? `${lineName}-B` : 'Line-B';

  const [nameA, setNameA] = useState(defaultNameA);
  const [nameB, setNameB] = useState(defaultNameB);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Re-initialize names when lineName changes
  useEffect(() => {
    setNameA(lineName ? `${lineName}-A` : 'Line-A');
    setNameB(lineName ? `${lineName}-B` : 'Line-B');
  }, [lineName]);

  const isDirty = nameA !== defaultNameA || nameB !== defaultNameB;

  const isValid = nameA.trim().length > 0 && nameB.trim().length > 0;

  const handleConfirm = async () => {
    if (lineId === undefined || shotId === undefined || !isValid) return;

    setIsLoading(true);
    setError(null);

    try {
      const result = await deleteShotAndSplit(lineId, shotId, nameA.trim(), nameB.trim());
      if (result.success) {
        // Select line A (keeps original ID) after split
        setSelection({ type: 'line', lineId });
        trackDialogComplete('split-line');
        closeDialog();
      } else {
        setError(result.error ?? 'Failed to split line');
      }
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to split line'));
    } finally {
      setIsLoading(false);
    }
  };

  const handleClose = isLoading ? () => {} : closeDialog;

  return (
    <DialogWrapper
      title="Delete Shot & Split Line"
      onClose={handleClose}
      size="md"
      isDirty={isDirty}
      closeOnBackdropClick={!isLoading}
      data-testid="split-line-dialog"
    >
      <div className="space-y-4">
        <p className="text-text-secondary">
          Deleting this shot will split the line into two. Name the resulting lines:
        </p>

        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <div className="space-y-3">
          <div>
            <label className="block text-sm text-text-secondary mb-1">Line A name (first half)</label>
            <input
              type="text"
              value={nameA}
              onChange={(e) => setNameA(e.target.value)}
              className="w-full"
              data-testid="split-name-a"
              disabled={isLoading}
            />
          </div>
          <div>
            <label className="block text-sm text-text-secondary mb-1">Line B name (second half)</label>
            <input
              type="text"
              value={nameB}
              onChange={(e) => setNameB(e.target.value)}
              className="w-full"
              data-testid="split-name-b"
              disabled={isLoading}
            />
          </div>
        </div>

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleClose} disabled={isLoading} data-testid="split-line-cancel">
            Cancel
          </Button>
          <Button
            variant="danger"
            onClick={handleConfirm}
            disabled={isLoading || !isValid}
            data-testid="split-confirm"
          >
            {isLoading ? 'Splitting...' : 'Delete & Split'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
