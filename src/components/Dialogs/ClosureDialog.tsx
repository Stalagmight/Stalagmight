import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage, invokeIpc } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { StationBlob } from '../common/EntityBlob';
import { DialogWrapper } from './DialogWrapper';
import { getErrorColor } from '../../utils/colors';
import { formatFixed, formatPercent } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import type { StationRef, ClosureStrategy } from '../../types/project';
import type { ClosurePreview, BatchCommand } from '../../types/commands';
import type { LoopClosureNaming } from '../../types/ipc/LoopClosureNaming';
import type { ClosureKeepPreview } from '../../types/ipc/ClosureKeepPreview';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { useEditingBlocked, isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { listNames } from '../../utils/listNames';
import { CollisionRows, renamesFromAnswers, useCollisionAnswers, useIdentityLabel } from './keptStationParts';

/** "Keep both names" in the keep choice (no end archived). */
const BOTH = 'both';

/** The choice the dialog starts on (RS 5.2): rule K's end when it may be
 *  kept, else "Keep both names". */
export function defaultClosureChoice(naming: LoopClosureNaming): number | typeof BOTH {
  return naming.default_keep ?? BOTH;
}

export default function ClosureDialog() {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  const { closeDialog, dialogData, clearMergeStations } = useUIStore();
  const { createLoopClosure, executeBatch } = useProjectStore();
  const editingBlocked = useEditingBlocked();

  const stationA = dialogData.stationA as StationRef | undefined;
  const stationB = dialogData.stationB as StationRef | undefined;
  const preview = (dialogData.preview as ClosurePreview | null) ?? undefined;
  const reason = dialogData.reason as string | undefined;
  // Opened by Merge (the tool, Merge With..., or a snap-to-merge drag) on two
  // stations that are already connected: say why the user who asked for a
  // merge gets a loop closure (demo finding I-9; the backend's reason was only
  // "Creates survey loop").
  const fromMerge = dialogData.fromMerge === true;
  // Why the backend would refuse this closure (#737): it would tie the two ends
  // of one shot together. The dialog says so up front and offers no Create; the
  // backend refuses the pair regardless.
  const closureRefusal = dialogData.closureRefusal as string | undefined;

  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);


  // Optional callbacks from snap-to-merge drag flow
  const onBeforeConfirm = dialogData.onBeforeConfirm as (() => Promise<void>) | undefined;
  const onCancel = dialogData.onCancel as (() => void) | undefined;
  // Move commands to batch with closure for single-undo snap-to-merge
  const preCommands = dialogData.preCommands as BatchCommand[] | undefined;
  const isFromMove = !!preCommands;
  const strategy: ClosureStrategy = isFromMove ? 'Move' : 'Batch';

  // Which end both ends show (RS 5.2, 7), from `preview_loop_closure`: the
  // ends that may be kept and "Keep both names", and what each does to names.
  // Until it answers (or when it is refused, as a refused pair is), Create
  // keeps both names, as it did before.
  const [naming, setNaming] = useState<LoopClosureNaming | null>(null);
  const [choice, setChoice] = useState<number | typeof BOTH>(BOTH);
  useEffect(() => {
    setNaming(null);
    if (!stationA || !stationB || closureRefusal) return;
    let live = true;
    Promise.resolve(
      invokeIpc<LoopClosureNaming>(
        'preview_loop_closure',
        { stationA, stationB, closureStrategy: strategy },
        { purpose: 'preview loop closure names' },
      ),
    )
      .then((answer) => {
        if (!live || !answer) return;
        setNaming(answer);
        setChoice(defaultClosureChoice(answer));
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [stationA, stationB, strategy, closureRefusal]);
  const option = naming?.options.find((o) => (o.keep ?? BOTH) === choice) ?? null;
  const [answers, setAnswer] = useCollisionAnswers(option?.collisions);

  const handleClose = () => {
    onCancel?.();
    clearMergeStations();
    closeDialog();
  };

  const handleCreate = async () => {
    if (!stationA || !stationB) return;
    if (isEditingBlockedNow()) {
      setError(t('dialogs.keptStations.editingDisabled'));
      return;
    }

    setIsLoading(true);
    setError(null);

    try {
      if (onBeforeConfirm) await onBeforeConfirm();
      // The dialog's answers (RS 5.2): the end kept (none: keep both
      // names) and every collision answered; without the naming preview,
      // neither, and the closure keeps both names.
      const answered = option
        ? {
            ...(option.keep !== undefined ? { keep: option.keep } : {}),
            renames: renamesFromAnswers(option.collisions, answers),
          }
        : null;
      // When preCommands exist (snap-to-merge), batch move + closure as single undo operation
      const closureCmd: BatchCommand = {
        type: 'create_loop_closure',
        station_a: stationA,
        station_b: stationB,
        closure_strategy: strategy,
        ...(answered ?? {}),
      };
      const result = preCommands
        ? await executeBatch([...preCommands, closureCmd])
        : answered
          ? await createLoopClosure(stationA, stationB, strategy, answered)
          : await createLoopClosure(stationA, stationB, strategy);
      if (result.success) {
        clearMergeStations();
        trackDialogComplete('closure');
        closeDialog();
      } else {
        setError(result.error ?? 'Failed to create loop closure');
      }
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to create loop closure'));
    } finally {
      setIsLoading(false);
    }
  };

  if (!stationA || !stationB) {
    return null;
  }

  return (
    <DialogWrapper title="Create Loop Closure" onClose={handleClose} size="lg" data-testid="closure-dialog">
      <div className="space-y-4">
        {/* The redirect reason ("Creates survey loop") would contradict a refusal
            that refuses it, so the refusal replaces it. */}
        {(fromMerge || reason) && !closureRefusal && (
          <p className="text-sm text-text-secondary" data-testid="closure-reason">
            {fromMerge ? t('dialogs.mergeStations.becomesClosure') : reason}
          </p>
        )}
        {closureRefusal && (
          <div
            role="alert"
            className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm"
            data-testid="closure-refusal"
          >
            {closureRefusal}
          </div>
        )}
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">{error}</div>
        )}

        {/* Station details */}
        <div className="space-y-2">
          <div className="flex justify-between items-center py-2 border-b border-border">
            <span className="text-text-secondary">Station A:</span>
            <span className="inline-flex items-center gap-1.5">
              <StationBlob station={stationA} withLine />
            </span>
          </div>
          <div className="flex justify-between items-center py-2 border-b border-border">
            <span className="text-text-secondary">Station B:</span>
            <span className="inline-flex items-center gap-1.5">
              <StationBlob station={stationB} withLine />
            </span>
          </div>
        </div>

        {naming && naming.options.length > 1 && (
          <ClosureKeepChoice
            naming={naming}
            choice={choice}
            onChoice={setChoice}
            disabled={isLoading || !!closureRefusal}
          />
        )}
        {option && <ClosureChoiceOutcome option={option} />}
        {option && (
          <CollisionRows
            collisions={option.collisions}
            answers={answers}
            onAnswer={setAnswer}
            testIdPrefix="closure"
            disabled={isLoading}
          />
        )}

        {/* Preview data */}
        {preview && (
          <div className="p-3 bg-background rounded space-y-1">
            <div className="flex justify-between">
              <span className="text-text-secondary">Error:</span>
              <span className="font-medium" style={{ color: getErrorColor(preview.error_percentage) }}>
                {formatPercent(preview.error_percentage, undefined, 1)}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-text-secondary">Path length:</span>
              <span>{formatFixed(preview.path_length, undefined, precision.length)}m</span>
            </div>
            <div className="flex justify-between">
              <span className="text-text-secondary">Misclosure distance:</span>
              <span>{formatFixed(preview.misclosure_distance, undefined, precision.length)}m</span>
            </div>
            <div className="flex justify-between">
              <span className="text-text-secondary">Horizontal misclosure:</span>
              <span>{formatFixed(preview.horizontal_misclosure, undefined, precision.length)}m</span>
            </div>
            <div className="flex justify-between">
              <span className="text-text-secondary">Vertical misclosure:</span>
              <span>{formatFixed(preview.vertical_misclosure, undefined, precision.length)}m</span>
            </div>
          </div>
        )}

        {/* Actions */}
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleClose} disabled={isLoading} data-testid="closure-cancel">
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={handleCreate}
            disabled={isLoading || !!closureRefusal || editingBlocked}
            title={closureRefusal}
            data-testid="closure-create"
          >
            {isLoading ? 'Creating...' : 'Create Closure'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}

/**
 * "Both ends show:" (RS 5.2, 7): one radio per end that may be kept, each
 * that end's pill and the name both ends then show, and "Keep both names"
 * (the closure hides neither). An end a loop closure already shows as
 * another says so; an end the rules do not allow (C1, C2) is not offered.
 */
function ClosureKeepChoice({
  naming,
  choice,
  onChoice,
  disabled,
}: {
  naming: LoopClosureNaming;
  choice: number | typeof BOTH;
  onChoice: (choice: number | typeof BOTH) => void;
  disabled: boolean;
}) {
  const { t } = useTranslation();
  const label = useIdentityLabel();
  return (
    <fieldset className="space-y-1" data-testid="closure-keep">
      <legend className="text-sm text-text-primary">{t('dialogs.closure.keepLead')}</legend>
      {naming.allowed.map((id) => {
        const end = naming.ends.find((e) => e.identity.id === id);
        if (!end) return null;
        return (
          <label key={id} className="flex flex-wrap items-center gap-2 p-1 rounded text-sm hover:bg-background-50">
            <input
              type="radio"
              name="closure-keep"
              value={id}
              checked={choice === id}
              disabled={disabled}
              onChange={() => onChoice(id)}
              data-testid={`closure-keep-${id}`}
            />
            <StationBlob station={end.station} withLine />
            {end.shown_as && (
              <span className="text-text-secondary" data-testid={`closure-keep-${id}-shown-as`}>
                {t('dialogs.keptStations.shownAsThroughClosure', { station: label(end.shown_as) })}
              </span>
            )}
          </label>
        );
      })}
      <label className="flex items-center gap-2 p-1 rounded text-sm hover:bg-background-50">
        <input
          type="radio"
          name="closure-keep"
          value={BOTH}
          checked={choice === BOTH}
          disabled={disabled}
          onChange={() => onChoice(BOTH)}
          data-testid="closure-keep-both"
        />
        <span>{t('dialogs.closure.keepBoth')}</span>
      </label>
      {naming.rule_k !== undefined && !naming.allowed.includes(naming.rule_k) && (
        <p className="text-xs text-text-secondary" data-testid="closure-keep-not-allowed">
          {t('dialogs.closure.ruleKNotAllowed')}
        </p>
      )}
    </fieldset>
  );
}

/** What the chosen end does (RS 7): the end hidden (its point stays, without
 *  a label; Remove Closure brings it back) and the lines that show another
 *  name there, or that keeping both names hides nothing. */
function ClosureChoiceOutcome({ option }: { option: ClosureKeepPreview }) {
  const { t } = useTranslation();
  const labels = useIdentityLabels();
  const label = useIdentityLabel();
  if (!option.hidden) {
    return (
      <p className="text-sm text-text-secondary" data-testid="closure-keep-outcome">
        {t('dialogs.closure.keepsBoth')}
      </p>
    );
  }
  const hasComment = (option.hidden.comment ?? '').trim() !== '';
  return (
    <div className="space-y-1 text-sm" data-testid="closure-keep-outcome">
      <p className="text-text-primary" data-testid="closure-hidden">
        {t(hasComment ? 'dialogs.closure.hiddenWithComment' : 'dialogs.closure.hidden', { station: label(option.hidden) })}
      </p>
      {option.renamed_lines.length > 0 && (
        <p className="text-text-secondary" data-testid="closure-renamed-lines">
          {t('dialogs.closure.renamedLines', {
            count: option.renamed_lines.length,
            lines: listNames(option.renamed_lines.map(labels.lineName), t),
          })}
        </p>
      )}
    </div>
  );
}
