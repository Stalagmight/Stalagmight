import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { Button } from '../common/Button';
import { Select } from '../common/Select';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';
import { SHOT_KIND_CYCLE, SHOT_KIND_LABEL_KEY } from '../../utils/shotKind';
import { formatShotLabel } from '../../utils/formatters';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import type { ShotKind } from '../../types/ipc/ShotKind';
import type { UnrecognisedShotKind } from '../../types/ipc/UnrecognisedShotKind';

/**
 * Ask what to do with shots whose stored `kind` this build could not read (#27).
 *
 * The backend already failed them closed to `Reference` - `Normal` is swimmable,
 * so an unreadable byte would otherwise enter gas, penetration and the ruler at
 * normal cost. That substitution is safe but it is a guess, and a silently
 * altered leg is exactly the kind of thing a diver should not discover from a
 * gas number. So the choice is surfaced, pre-set to the safe answer.
 *
 * Cancel is a legitimate outcome: everything stays `Reference`, which is what
 * the loader already applied. Nothing here can leave the project in a state the
 * user did not see.
 */
export default function UnrecognisedShotKindDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  const executeBatch = useProjectStore((s) => s.executeBatch);
  const labels = useIdentityLabels();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const shots = dialogData.shots as UnrecognisedShotKind[] | undefined;
  // Per-shot choice, keyed by shot_id. Defaults to the safe fallback the
  // backend already applied, so confirming without touching anything is a no-op.
  const [choices, setChoices] = useState<Record<number, ShotKind>>({});

  if (!shots || shots.length === 0) return null;

  const choiceFor = (s: UnrecognisedShotKind): ShotKind => choices[s.shot_id] ?? 'Reference';

  const handleApply = async () => {
    setBusy(true);
    setError(null);
    try {
      // Only the ones the user actually changed: the rest are already Reference
      // both in memory AND on disk (the loader repairs the row), so re-sending
      // them would be undo noise for no effect.
      const changed = shots
        .filter((shot) => choiceFor(shot) !== 'Reference')
        .map((shot) => ({
          type: 'update_shot' as const,
          line_id: shot.line_id,
          shot_id: shot.shot_id,
          updates: { kind: choiceFor(shot) },
        }));

      if (changed.length > 0) {
        // ONE batch, not a call per shot. A newer file can use an unknown
        // discriminant on every shot in the survey, and each executeCommand
        // refetches the whole project state, undo state, warnings and the
        // integrity check - so a per-shot loop turns a compatibility prompt into
        // N full refresh cycles. `update_shot` is batch-allowed, and a kind-only
        // update is classified INERT, so the batch skips the geometry reconcile.
        //
        // The result REPORTS failure rather than throwing - a read-only SpeleoDB
        // project, or a lock re-verification that rejects. Closing anyway would
        // lose the user's choices for good, since the backend list has already
        // been consumed. A rejection keeps the dialog open, state intact.
        const res = await executeBatch(changed);
        if (!res.success) {
          setError(res.error ?? t('dialogs.unrecognisedShotKind.applyFailed'));
          return;
        }
      }
      trackDialogComplete('unrecognised-shot-kind');
      closeDialog();
    } finally {
      setBusy(false);
    }
  };

  return (
    <DialogWrapper
      title={t('dialogs.unrecognisedShotKind.title')}
      onClose={closeDialog}
      data-testid="unrecognised-shot-kind-dialog"
    >
      <div className="space-y-4">
        <p className="text-sm text-text-secondary">
          {t('dialogs.unrecognisedShotKind.body', { count: shots.length })}
        </p>

        <div className="max-h-64 overflow-y-auto border border-border rounded">
          <table className="w-full text-sm">
            <thead className="sticky top-0 bg-background-secondary">
              <tr>
                <th className="text-left p-2 font-medium">
                  {t('dialogs.unrecognisedShotKind.shot')}
                </th>
                <th className="text-left p-2 font-medium">
                  {t('dialogs.unrecognisedShotKind.found')}
                </th>
                <th className="text-left p-2 font-medium">
                  {t('dialogs.unrecognisedShotKind.treatAs')}
                </th>
              </tr>
            </thead>
            <tbody>
              {shots.map((shot) => (
                <tr key={shot.shot_id} className="border-t border-border">
                  <td className="p-2">
                    {/* `Main Line: 4 → 5`: shot number n (1-based) runs from
                        station n - 1 to station n of its line; each end is
                        its plain label (`12 (Main)` for a foreign slot). The
                        line's name is the report's. */}
                    {formatShotLabel(
                      labels.station({ line_id: shot.line_id, station_index: shot.shot_number - 1 }),
                      labels.station({ line_id: shot.line_id, station_index: shot.shot_number }),
                      shot.line_name ?? t('dialogs.unrecognisedShotKind.unnamedSection'),
                    )}
                  </td>
                  <td className="p-2 font-mono text-text-secondary">{shot.raw_value}</td>
                  <td className="p-2">
                    <Select
                      value={choiceFor(shot)}
                      onChange={(v) =>
                        setChoices((prev) => ({
                          ...prev,
                          [shot.shot_id]: v as ShotKind,
                        }))
                      }
                      options={SHOT_KIND_CYCLE.map((kind) => ({
                        value: kind,
                        label: t(SHOT_KIND_LABEL_KEY[kind]),
                      }))}
                      size="sm"
                      aria-label={t('dialogs.unrecognisedShotKind.treatAsAria', {
                        number: shot.shot_number,
                      })}
                      data-testid={`unrecognised-kind-select-${shot.shot_id}`}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        <p className="text-xs text-text-secondary">
          {t('dialogs.unrecognisedShotKind.note')}
        </p>

        {error !== null && (
          <p className="text-sm text-error" role="alert" data-testid="unrecognised-kind-error">
            {error}
          </p>
        )}

        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={closeDialog} disabled={busy} data-testid="unrecognised-kind-keep-reference">
            {t('dialogs.unrecognisedShotKind.keepAsReference')}
          </Button>
          <Button
            variant="primary"
            onClick={handleApply}
            disabled={busy}
            data-testid="unrecognised-kind-apply"
          >
            {t('dialogs.unrecognisedShotKind.apply')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
