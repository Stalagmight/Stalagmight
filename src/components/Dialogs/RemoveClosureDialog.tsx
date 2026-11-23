import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { getErrorMessage, invokeIpc } from '../../utils/ipc';
import type { StationRef } from '../../types/project';
import type { RemoveClosureNaming } from '../../types/ipc/RemoveClosureNaming';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { useEditingBlocked, isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { listNames } from '../../utils/listNames';
import { displayStationName } from '../../utils/formatters';
import { StationBlob } from '../common/EntityBlob';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';
import {
  answeredName,
  CollisionRows,
  renamesFromAnswers,
  StickyDialogFooter,
  useCollisionAnswers,
  useIdentityLabel,
} from './keptStationParts';

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
  const editingBlocked = useEditingBlocked();

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
  // only hides the sentence; Remove then letters any collision itself. While
  // it is out, Remove waits (review finding ui-7): sending no renames then
  // would letter, behind a notice, a collision the dialog is about to show.
  const [naming, setNaming] = useState<RemoveClosureNaming | null>(null);
  // The closure whose preview has answered or failed.
  const [settledFor, setSettledFor] = useState<string | null>(null);
  useEffect(() => {
    setNaming(null);
    setSettledFor(null);
    if (!selectedClosure) return;
    const key = selectedClosure.closureId.toString();
    let live = true;
    Promise.resolve()
      .then(() =>
        invokeIpc<RemoveClosureNaming>(
          'preview_remove_loop_closure',
          { closureId: Number(selectedClosure.closureId) },
          { purpose: 'preview remove loop closure' },
        ),
      )
      .then((answer) => {
        if (live && answer && String(answer.closure_id) === key) setNaming(answer);
      })
      .catch(() => undefined)
      .finally(() => {
        if (live) setSettledFor(key);
      });
    return () => {
      live = false;
    };
  }, [selectedClosure]);
  const previewSettled = selectedClosure !== null && settledFor === selectedClosure.closureId.toString();
  const [answers, setAnswer] = useCollisionAnswers(naming?.collisions);
  // The station that comes back as the chosen answers leave it (RS-FX8, N2,
  // as Detach's P2-2): with rule L's letter it comes back as L8a, not L8.
  const comesBack = naming?.comes_back
    ? { ...naming.comes_back, name: answeredName(naming.comes_back.id, naming.comes_back.name, naming.collisions, answers) }
    : undefined;

  /** The station a closure hides, by closure id (the rows' "17 comes back"):
   *  for the closure chosen, under the name the answers give it. */
  const hiddenOf = (closureId: bigint): string | null => {
    const archive = project?.loop_closures?.find((c) => c.id === closureId)?.archived;
    if (!archive) return null;
    const answered = comesBack && naming?.closure_id === closureId && comesBack.id === archive.station.id;
    return displayStationName(answered ? comesBack.name : archive.station.name);
  };

  // The ends of the chosen closure that another closure shows as another
  // station (ui-6): removing one that hides nothing leaves them so, which
  // "both ends show their own names" would deny.
  const staysShown: string[] = [];
  const chosen = project?.loop_closures?.find((c) => c.id === selectedClosure?.closureId);
  for (const end of chosen ? [chosen.station_a, chosen.station_b] : []) {
    const by = labels.shown(end)?.loop_closure;
    const archive = by === undefined ? undefined : project?.loop_closures?.find((c) => c.id === by)?.archived;
    if (by !== undefined && by !== chosen?.id && archive) {
      staysShown.push(
        t('dialogs.closure.staysShownAs', {
          station: identityLabel(archive.station),
          shown: identityLabel(archive.shown_as),
        }),
      );
    }
  }

  const getStationDisplayLabel = (ref: StationRef): ReactNode => (
    <StationBlob station={ref} withLine />
  );

  const handleConfirm = async () => {
    if (!selectedClosure) return;
    if (isEditingBlockedNow()) {
      setError(t('dialogs.keptStations.editingDisabled'));
      return;
    }

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

        {naming && comesBack && (
          <div className="space-y-1 text-sm" data-testid="remove-closure-comes-back">
            <p className="text-text-primary">
              {t(naming.renamed_lines.length > 0 && naming.shown_as ? 'dialogs.removeClosure.comesBack' : 'dialogs.removeClosure.comesBackPlain', {
                station: identityLabel(comesBack),
                shown: naming.shown_as ? identityLabel(naming.shown_as) : '',
                count: naming.renamed_lines.length,
                lines: listNames(naming.renamed_lines.map(labels.lineName), t),
              })}
            </p>
            {(comesBack.comment ?? '').trim() !== '' && (
              <p className="text-text-secondary" data-testid="remove-closure-comes-back-comment">
                {t('dialogs.removeClosure.commentComesBack', { comment: comesBack.comment })}
              </p>
            )}
          </div>
        )}
        {naming && naming.comes_back === undefined && (
          <p className="text-sm text-text-secondary" data-testid="remove-closure-nothing-back">
            {staysShown.length === 0
              ? t('dialogs.removeClosure.nothingBack')
              : [t('dialogs.removeClosure.nothingBackShown'), ...staysShown].join(' ')}
          </p>
        )}
        {naming && (
          <CollisionRows
            collisions={naming.collisions}
            answers={answers}
            onAnswer={setAnswer}
            testIdPrefix="remove-closure"
            subject={naming.comes_back ? { id: naming.comes_back.id, role: 'comesBack' } : undefined}
            disabled={isLoading}
          />
        )}

        <StickyDialogFooter testId="remove-closure-footer">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="remove-closure-cancel">Cancel</Button>
          <Button
            variant="primary"
            onClick={handleConfirm}
            disabled={isLoading || !selectedClosure || !previewSettled || editingBlocked}
            data-testid="remove-closure-confirm"
          >
            {isLoading ? 'Removing...' : 'Remove'}
          </Button>
        </StickyDialogFooter>
      </div>
    </DialogWrapper>
  );
}
