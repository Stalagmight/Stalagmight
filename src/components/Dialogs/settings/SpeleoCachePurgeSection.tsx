import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '../../common/Button';
import { DialogWrapper } from '../DialogWrapper';
import { SettingsSection } from './SettingsSection';
import { invokeIpc } from '../../../utils/ipc';
import { formatNumber } from '../../../i18n/format';
import { useSpeleoCacheStore } from '../../../store/speleoCacheStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useSpeleoOperationBusy } from '../../../hooks/useSpeleoOperationBusy';
import { cachedCopyAtRisk, isListedCachedCopy } from '../../../utils/speleoCachedCopies';
import type { SpeleoCachedProject, SpeleoPurgeReport } from '../../../types/ipc.generated';

type PurgeStep =
  | { kind: 'confirm'; copies: SpeleoCachedProject[] }
  | { kind: 'lost'; copies: SpeleoCachedProject[] };

interface PurgeMessage {
  lines: string[];
  tone: 'info' | 'warning';
}

/**
 * SpeleoDB settings: "Purge all cached files". Removes the SpeleoDB copies this
 * computer downloaded, except the project open right now: signed in, the copies
 * the home screen lists for the account (its own, and older ones with no recorded
 * account from the same instance); signed out, every account's. Another account's
 * copy is never counted, listed or removed while signed in, and the backend
 * applies the same rule when it purges.
 *
 * Copies with changes that were never uploaded (or that could not be read) are
 * never removed silently: the confirmation lists them and defaults to keeping
 * them; deleting them too takes a second, explicit confirmation. The backend
 * re-checks every copy at removal time and keeps at-risk ones unless told.
 *
 * Signed in, the backend then hands back this account's editing lock on each
 * removed copy of its own, when the server says the account holds it; the
 * confirmation says so, and the result lists what was and was not handed back.
 *
 * Self-contained (its own state, dialogs and IPC) so the SpeleoDB settings panel
 * only has to render it.
 */
export function SpeleoCachePurgeSection() {
  const { t } = useTranslation();
  const busy = useSpeleoOperationBusy();
  const refresh = useSpeleoCacheStore((s) => s.refresh);
  // Nothing to purge once the listing says so (no copy of this session's, or only
  // the open project's): the button is disabled and says why, instead of a red
  // button that does nothing.
  const nothingToPurge = useSpeleoCacheStore((s) => s.loaded && purgeable(s.copies).every((copy) => copy.is_open));
  const [step, setStep] = useState<PurgeStep | null>(null);
  const [working, setWorking] = useState(false);
  const [message, setMessage] = useState<PurgeMessage | null>(null);
  const resultRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    void refresh();
  }, [refresh]);
  // The section is the panel's last, so its result lands below the fold: bring it
  // into view when it arrives.
  useEffect(() => {
    if (message) resultRef.current?.scrollIntoView?.({ block: 'nearest' });
  }, [message]);

  const start = async () => {
    setMessage(null);
    const copies = purgeable(await refresh());
    // Re-checked at click time: the refreshed listing disables the button if empty.
    if (copies.every((copy) => copy.is_open)) return;
    setStep({ kind: 'confirm', copies });
  };

  const purge = async (discardUnpushed: boolean) => {
    setStep(null);
    setWorking(true);
    try {
      const report = await invokeIpc<SpeleoPurgeReport>(
        'speleodb_purge_cached_copies',
        { discardUnpushed },
        { purpose: 'purge downloaded SpeleoDB copies (user confirmed)' },
      );
      setMessage(purgeMessage(t, report));
      // Handed-back locks change the home screen's lock badges.
      if (report.locks_released.length > 0) void useSpeleoStore.getState().scanLocksOnOpen();
    } catch (e) {
      setMessage({ lines: [e instanceof Error ? e.message : String(e)], tone: 'warning' });
    } finally {
      setWorking(false);
      void refresh();
      void useProjectStore.getState().loadRecentProjects();
    }
  };

  return (
    <SettingsSection title={t('speleodb.cachedCopies.purge.title')} data-testid="section-speleodb-cache">
      <p className="mb-3 text-xs text-text-secondary">{t('speleodb.cachedCopies.purge.help')}</p>
      <Button
        variant="danger"
        disabled={busy || working || nothingToPurge}
        onClick={() => void start()}
        aria-describedby={nothingToPurge ? 'speleodb-purge-empty' : undefined}
        data-testid="speleodb-purge-cache"
      >
        {working ? t('speleodb.cachedCopies.purge.working') : t('speleodb.cachedCopies.purge.button')}
      </Button>
      {nothingToPurge && (
        <p id="speleodb-purge-empty" className="mt-3 text-sm text-text-secondary" data-testid="speleodb-purge-empty">
          {t('speleodb.cachedCopies.purge.empty')}
        </p>
      )}
      {/* Mounted before any result: a live region inserted together with its text
          is unreliably announced. */}
      <div ref={resultRef} role="status" aria-live="polite" data-testid="speleodb-purge-status">
        {message && (
          <div
            className={`mt-3 space-y-1 text-sm ${message.tone === 'warning' ? 'text-warning-text' : 'text-text-secondary'}`}
            data-testid="speleodb-purge-result"
          >
            {message.lines.map((line) => <p key={line}>{line}</p>)}
          </div>
        )}
      </div>
      {step && (
        <PurgeConfirmDialog
          step={step}
          onCancel={() => setStep(null)}
          onStep={setStep}
          onPurge={(discardUnpushed) => void purge(discardUnpushed)}
        />
      )}
    </SettingsSection>
  );
}

/** The copies the purge reaches: the ones the home screen lists for this session. */
function purgeable(copies: SpeleoCachedProject[]): SpeleoCachedProject[] {
  return copies.filter(isListedCachedCopy);
}

function purgeMessage(t: ReturnType<typeof useTranslation>['t'], report: SpeleoPurgeReport): PurgeMessage {
  const lines = [t('speleodb.cachedCopies.purge.result', {
    count: report.removed,
    countFormatted: formatNumber(report.removed),
  })];
  if (report.kept_unpushed.length > 0) {
    lines.push(t('speleodb.cachedCopies.purge.keptUnpushed', { names: report.kept_unpushed.join(', ') }));
  }
  if (report.kept_open.length > 0) {
    lines.push(t('speleodb.cachedCopies.purge.keptOpen', { names: report.kept_open.join(', ') }));
  }
  if (report.locks_released.length > 0) {
    lines.push(t('speleodb.cachedCopies.purge.locksReleased', { names: report.locks_released.join(', ') }));
  }
  if (report.locks_unchecked.length > 0) {
    lines.push(t('speleodb.cachedCopies.purge.locksUnchecked', { names: report.locks_unchecked.join(', ') }));
  }
  if (report.failed.length > 0) {
    lines.push(t('speleodb.cachedCopies.purge.failed', { details: report.failed.join('; ') }));
  }
  const warn = report.failed.length > 0 || report.locks_unchecked.length > 0;
  return { lines, tone: warn ? 'warning' : 'info' };
}

/** The purge confirmation, then (only on request) the "lose them for good" step. */
function PurgeConfirmDialog({
  step,
  onCancel,
  onStep,
  onPurge,
}: {
  step: PurgeStep;
  onCancel: () => void;
  onStep: (step: PurgeStep) => void;
  onPurge: (discardUnpushed: boolean) => void;
}) {
  const { t } = useTranslation();
  const removable = step.copies.filter((copy) => !copy.is_open);
  const atRisk = removable.filter(cachedCopyAtRisk);
  const safeCount = removable.length - atRisk.length;
  const openKept = step.copies.some((copy) => copy.is_open);
  // The backend classified every copy against the session it has right now.
  const signedOut = step.copies.some((copy) => copy.ownership === 'signed_out');
  const names = (
    <ul className="list-disc pl-5" data-testid="speleodb-purge-at-risk-list">
      {atRisk.map((copy) => <li key={copy.project_id}>{copy.project_name}</li>)}
    </ul>
  );

  if (step.kind === 'lost') {
    return (
      <DialogWrapper
        title={t('speleodb.cachedCopies.purge.lostTitle')}
        onClose={onCancel}
        size="md"
        data-testid="speleodb-purge-lost-dialog"
      >
        <div className="space-y-4 text-sm">
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning-text space-y-2">
            <p>{t('speleodb.cachedCopies.purge.lostBody')}</p>
            {names}
          </div>
          <div className="flex justify-end gap-2 pt-4 border-t border-border">
            <Button onClick={() => onStep({ kind: 'confirm', copies: step.copies })} data-testid="speleodb-purge-lost-back">
              {t('speleodb.cachedCopies.purge.back')}
            </Button>
            <Button variant="danger" onClick={() => onPurge(true)} data-testid="speleodb-purge-lost-confirm">
              {t('speleodb.cachedCopies.purge.lostConfirm')}
            </Button>
          </div>
        </div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper
      title={t('speleodb.cachedCopies.purge.confirmTitle')}
      onClose={onCancel}
      size="md"
      data-testid="speleodb-purge-dialog"
    >
      <div className="space-y-4 text-sm">
        <p className="text-text-secondary">
          {t('speleodb.cachedCopies.purge.confirmBody', {
            count: removable.length,
            countFormatted: formatNumber(removable.length),
          })}
        </p>
        {openKept && <p className="text-text-secondary">{t('speleodb.cachedCopies.purge.openKept')}</p>}
        <p className="text-text-secondary" data-testid="speleodb-purge-locks">
          {t(signedOut ? 'speleodb.cachedCopies.purge.locksSignedOut' : 'speleodb.cachedCopies.purge.locksSignedIn')}
        </p>
        {atRisk.length > 0 && (
          <div
            className="p-3 bg-warning-10 border border-warning-20 rounded text-warning-text space-y-2"
            data-testid="speleodb-purge-at-risk"
          >
            <p>
              {t('speleodb.cachedCopies.purge.atRisk', {
                count: atRisk.length,
                countFormatted: formatNumber(atRisk.length),
              })}
            </p>
            {names}
          </div>
        )}
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={onCancel} data-testid="speleodb-purge-cancel">
            {t('speleodb.cachedCopies.purge.cancel')}
          </Button>
          {atRisk.length > 0 ? (
            <>
              <Button
                variant="danger"
                onClick={() => onStep({ kind: 'lost', copies: step.copies })}
                data-testid="speleodb-purge-delete-all"
              >
                {t('speleodb.cachedCopies.purge.deleteAll')}
              </Button>
              {safeCount > 0 && (
                <Button variant="primary" onClick={() => onPurge(false)} data-testid="speleodb-purge-keep-at-risk">
                  {t('speleodb.cachedCopies.purge.keepAtRisk')}
                </Button>
              )}
            </>
          ) : (
            <Button variant="danger" onClick={() => onPurge(false)} data-testid="speleodb-purge-confirm">
              {t('speleodb.cachedCopies.purge.purge')}
            </Button>
          )}
        </div>
      </div>
    </DialogWrapper>
  );
}
