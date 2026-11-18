import { useState, useEffect } from 'react';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import {
  defaultSplitNames,
  lineNameProblemMessage,
  splitNameProblems,
} from '../../utils/nameValidation';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

export default function SplitLineDialog() {
  const { closeDialog, dialogData, setSelection } = useUIStore();
  const { deleteShotAndSplit, project } = useProjectStore();

  const lineId = dialogData.lineId as number | undefined;
  const shotId = dialogData.shotId as number | undefined;
  const lines = project?.lines;
  // The project's own name for the line; the opener's copy only as a fallback.
  const lineName = lines?.find((l) => l.id === lineId)?.name ?? (dialogData.lineName as string | undefined);

  const defaults = defaultSplitNames(lineName, lines, lineId);

  const [nameA, setNameA] = useState(defaults.a);
  const [nameB, setNameB] = useState(defaults.b);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Re-initialize names when lineName changes
  useEffect(() => {
    const next = defaultSplitNames(lineName, useProjectStore.getState().project?.lines, lineId);
    setNameA(next.a);
    setNameB(next.b);
  }, [lineName, lineId]);

  const isDirty = nameA !== defaults.a || nameB !== defaults.b;

  // Half A keeps the line (and may keep its name); B becomes a new line. Both
  // must be named, not after another line, and not after each other.
  const problems = splitNameProblems(nameA, nameB, lines, lineId);
  const errorA = problems.a ? lineNameProblemMessage(problems.a, nameA) : null;
  const errorB = problems.b ? lineNameProblemMessage(problems.b, nameB) : null;
  const isValid = !errorA && !errorB;

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

  const nameField = (key: 'a' | 'b', label: string, value: string, set: (v: string) => void, problem: string | null) => (
    <div>
      <label className="block text-sm text-text-secondary mb-1" htmlFor={`split-name-${key}`}>
        {label}
      </label>
      <input
        id={`split-name-${key}`}
        type="text"
        value={value}
        onChange={(e) => set(e.target.value)}
        className="w-full"
        aria-invalid={problem !== null}
        {...(problem ? { 'aria-describedby': `split-name-${key}-error` } : {})}
        data-testid={`split-name-${key}`}
        disabled={isLoading}
      />
      {problem && (
        <p id={`split-name-${key}-error`} className="mt-1 text-sm text-error" data-testid={`split-name-${key}-error`}>
          {problem}
        </p>
      )}
    </div>
  );

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
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm" data-testid="split-line-error">
            {error}
          </div>
        )}

        <div className="space-y-3">
          {nameField('a', 'Line A name (first half)', nameA, setNameA, errorA)}
          {nameField('b', 'Line B name (second half)', nameB, setNameB, errorB)}
        </div>

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleClose} disabled={isLoading} data-testid="split-line-cancel">
            Cancel
          </Button>
          <Button
            variant="danger"
            onClick={handleConfirm}
            disabled={isLoading || !isValid}
            title={errorA ?? errorB ?? undefined}
            data-testid="split-confirm"
          >
            {isLoading ? 'Splitting...' : 'Delete & Split'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
