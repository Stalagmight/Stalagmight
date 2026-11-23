import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { getErrorMessage, invokeIpc } from '../../utils/ipc';
import type { StationRef } from '../../types/project';
import type { RemoveClosureNaming } from '../../types/ipc/RemoveClosureNaming';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { listNames } from '../../utils/listNames';
import { displayStationName } from '../../utils/formatters';
import { StationBlob } from '../common/EntityBlob';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';
import { CollisionRows, renamesFromAnswers, useCollisionAnswers, useIdentityLabel } from './keptStationParts';

interface ClosureChoice {
  closureId: bigint;
  stationRef: StationRef;
}

function sameStationRef(a: StationRef, b: StationRef): boolean {
  return a.line_id === b.line_id && a.station_index === b.station_index;
}

/**
 * Remove a loop closure through one of its stations (RS 5.3, 7). Each row is
 * a closure, named by its other end, and says which station comes back when
 * the closure hides one ("17 comes back"); for the closure chosen the dialog
 * says what comes back (its name, its comment) and where, from
 * `preview_remove_loop_closure`, with one row per name that would then repeat
 * on a line. Remove carries the answers, so nothing is lettered unasked.
 */
export default function RemoveClosureDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData } = useUIStore();
  const { stations, removeLoopClosure, project } = useProjectStore();
  const labels = useIdentityLabels();
  const identityLabel = useIdentityLabel();

  const anchorStation = dialogData.anchorStation as StationRef | undefined;
  const closuresFromDialog = dialogData.closures as ClosureChoice[] | undefined;

  const closures = useMemo(() => {
    if (!anchorStation) return [];

    const fromDialog = Array.isArray(closuresFromDialog) ? closuresFromDialog : [];
    const fromStation = stations.find((station) =>
      sameStationRef(station.station_ref, anchorStation)
    )?.closures_with ?? [];

    const fromStore: ClosureChoice[] = fromStation
      .filter((closure) => !closure.is_transitive)
      .map((closure) => ({
        closureId: closure.closure_id,
        stationRef: closure.station_ref,
      }));

    const merged = fromDialog.length > 0 ? fromDialog : fromStore;
    const deduped = new Map<string, ClosureChoice>();
    for (const closure of merged) {
      deduped.set(closure.closureId.toString(), closure);
    }

    return Array.from(deduped.values());
  }, [anchorStation, closuresFromDialog, stations]);

  const [selectedClosureId, setSelectedClosureId] = useState<string>('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (closures.length > 0) {
      setSelectedClosureId(closures[0]!.closureId.toString());
    } else {
      setSelectedClosureId('');
    }
  }, [closures]);

  const selectedClosure = useMemo(
    () => closures.find((closure) => closure.closureId.toString() === selectedClosureId) ?? null,
    [closures, selectedClosureId]
  );

  // What removing the chosen closure brings back (RS 5.3). A failed preview
  // only hides the sentence; Remove then letters any collision itself.
  const [naming, setNaming] = useState<RemoveClosureNaming | null>(null);
  useEffect(() => {
    setNaming(null);
    if (!selectedClosure) return;
    let live = true;
    Promise.resolve(
      invokeIpc<RemoveClosureNaming>(
        'preview_remove_loop_closure',
        { closureId: Number(selectedClosure.closureId) },
        { purpose: 'preview remove loop closure' },
      ),
    )
      .then((answer) => {
        if (live && answer && String(answer.closure_id) === selectedClosure.closureId.toString()) setNaming(answer);
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [selectedClosure]);
  const [answers, setAnswer] = useCollisionAnswers(naming?.collisions);

  /** The station a closure hides, by closure id (the rows' "17 comes back"). */
  const hiddenOf = (closureId: bigint): string | null => {
    const archive = project?.loop_closures.find((c) => c.id === closureId)?.archived;
    return archive ? displayStationName(archive.station.name) : null;
  };

  const getStationDisplayLabel = (ref: StationRef): ReactNode => (
    <StationBlob station={ref} withLine />
  );

  const handleConfirm = async () => {
    if (!selectedClosure) return;

    setIsLoading(true);
    setError(null);
    try {
      const result = naming
        ? await removeLoopClosure(selectedClosure.closureId, {
            renames: renamesFromAnswers(naming.collisions, answers),
          })
        : await removeLoopClosure(selectedClosure.closureId);
      if (!result.success) {
        setError(result.error ?? 'Failed to remove closure');
        return;
      }
      trackDialogComplete('remove-closure');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to remove closure'));
    } finally {
      setIsLoading(false);
    }
  };

  if (!anchorStation) {
    return (
      <DialogWrapper title="Remove Closure" onClose={closeDialog} size="md" data-testid="remove-closure-dialog">
        <div className="space-y-4">
          <p className="text-error text-sm">Station not found.</p>
          <div className="flex justify-end">
            <Button onClick={closeDialog} data-testid="remove-closure-close">Close</Button>
          </div>
        </div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper title="Remove Closure" onClose={closeDialog} size="md" data-testid="remove-closure-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <p className="text-sm text-text-secondary">
          Select a closure connected to this station to remove.
        </p>

        <div className="space-y-2 max-h-56 overflow-y-auto border border-border rounded p-2">
          {closures.map((closure) => {
            const closureIdKey = closure.closureId.toString();
            return (
              <label key={closureIdKey} className="flex items-center gap-2 text-sm p-1 rounded hover:bg-background-50">
                <input
                  type="radio"
                  name="remove-closure"
                  value={closureIdKey}
                  checked={selectedClosureId === closureIdKey}
                  onChange={() => setSelectedClosureId(closureIdKey)}
                  data-testid={`remove-closure-option-${closureIdKey}`}
                />
                <span>
                  #{closureIdKey} - {getStationDisplayLabel(closure.stationRef)}
                </span>
                {hiddenOf(closure.closureId) !== null && (
                  <span className="text-text-secondary" data-testid={`remove-closure-option-${closureIdKey}-back`}>
                    {t('dialogs.removeClosure.rowComesBack', { name: hiddenOf(closure.closureId) })}
                  </span>
                )}
              </label>
            );
          })}
          {closures.length === 0 && (
            <p className="text-sm text-text-secondary" data-testid="remove-closure-empty">
              No closures found.
            </p>
          )}
        </div>

        {naming?.comes_back && (
          <div className="space-y-1 text-sm" data-testid="remove-closure-comes-back">
            <p className="text-text-primary">
              {t(naming.renamed_lines.length > 0 && naming.shown_as ? 'dialogs.removeClosure.comesBack' : 'dialogs.removeClosure.comesBackPlain', {
                station: identityLabel(naming.comes_back),
                shown: naming.shown_as ? identityLabel(naming.shown_as) : '',
                count: naming.renamed_lines.length,
                lines: listNames(naming.renamed_lines.map(labels.lineName), t),
              })}
            </p>
            {(naming.comes_back.comment ?? '').trim() !== '' && (
              <p className="text-text-secondary" data-testid="remove-closure-comes-back-comment">
                {t('dialogs.removeClosure.commentComesBack', { comment: naming.comes_back.comment })}
              </p>
            )}
          </div>
        )}
        {naming && naming.comes_back === undefined && (
          <p className="text-sm text-text-secondary" data-testid="remove-closure-nothing-back">
            {t('dialogs.removeClosure.nothingBack')}
          </p>
        )}
        {naming && (
          <CollisionRows
            collisions={naming.collisions}
            answers={answers}
            onAnswer={setAnswer}
            testIdPrefix="remove-closure"
            disabled={isLoading}
          />
        )}

        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="remove-closure-cancel">Cancel</Button>
          <Button
            variant="primary"
            onClick={handleConfirm}
            disabled={isLoading || !selectedClosure}
            data-testid="remove-closure-confirm"
          >
            {isLoading ? 'Removing...' : 'Remove'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
