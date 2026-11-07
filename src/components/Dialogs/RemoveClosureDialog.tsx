import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { getErrorMessage } from '../../utils/ipc';
import type { StationRef } from '../../types/project';
import { formatStationId } from '../../utils/formatters';
import { EntityBlob } from '../common/EntityBlob';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';
import { useShallow } from 'zustand/react/shallow';

interface ClosureChoice {
  closureId: number;
  stationRef: StationRef;
}

function sameStationRef(a: StationRef, b: StationRef): boolean {
  return a.section_id === b.section_id && a.station_index === b.station_index;
}

export default function RemoveClosureDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData } = useUIStore(
    useShallow((s) => ({ closeDialog: s.closeDialog, dialogData: s.dialogData })),
  );
  const { project, stations, removeLoopClosure } = useProjectStore(
    useShallow((s) => ({ project: s.project, stations: s.stations, removeLoopClosure: s.removeLoopClosure })),
  );

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

  const getStationDisplayLabel = (ref: StationRef): ReactNode => {
    const section = project?.sections.find((item) => item.id === ref.section_id);
    const sectionLabel = section?.name?.trim() || t('dialogs.removeClosure.sectionFallback', { id: ref.section_id });
    return (
      <span className="inline-flex items-center gap-1.5 align-middle">
        <EntityBlob entity="station">{formatStationId(ref.section_id, ref.station_index)}</EntityBlob>
        <EntityBlob entity="section">{sectionLabel}</EntityBlob>
      </span>
    );
  };

  const handleConfirm = async () => {
    if (!selectedClosure) return;

    setIsLoading(true);
    setError(null);
    try {
      const result = await removeLoopClosure(selectedClosure.closureId);
      if (!result.success) {
        setError(result.error ?? t('dialogs.removeClosure.removeFailed'));
        return;
      }
      trackDialogComplete('remove-closure');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.removeClosure.removeFailed')));
    } finally {
      setIsLoading(false);
    }
  };

  if (!anchorStation) {
    return (
      <DialogWrapper title={t('dialogs.removeClosure.title')} onClose={closeDialog} size="md" data-testid="remove-closure-dialog">
        <div className="space-y-4">
          <p className="text-error text-sm">{t('dialogs.removeClosure.notFound')}</p>
          <div className="flex justify-end">
            <Button onClick={closeDialog} data-testid="remove-closure-close">{t('common.actions.close')}</Button>
          </div>
        </div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper title={t('dialogs.removeClosure.title')} onClose={closeDialog} size="md" data-testid="remove-closure-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <p className="text-sm text-text-secondary">
          {t('dialogs.removeClosure.intro')}
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
              </label>
            );
          })}
          {closures.length === 0 && (
            <p className="text-sm text-text-secondary" data-testid="remove-closure-empty">
              {t('dialogs.removeClosure.empty')}
            </p>
          )}
        </div>

        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="remove-closure-cancel">{t('common.actions.cancel')}</Button>
          <Button
            variant="primary"
            onClick={handleConfirm}
            disabled={isLoading || !selectedClosure}
            data-testid="remove-closure-confirm"
          >
            {isLoading ? t('dialogs.removeClosure.removing') : t('common.actions.remove')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
