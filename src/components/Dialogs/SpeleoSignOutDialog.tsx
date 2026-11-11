import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { selectPushFailure, useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { useScreenOccupant } from '../../hooks/useScreenOccupant';
import { getErrorMessage } from '../../utils/ipc';
import { closeDiscardingLocal } from '../../utils/speleoExit';
import { commitMessageTooLong } from '../../utils/commitMessage';
import { saveOpenProjectLocally } from '../../utils/saveOpenProjectLocally';
import { TOAST_KEYS } from '../../utils/toastKeys';
import {
  releaseSignOutLocks,
  signOutHasWork,
  surveySignOut,
  useSpeleoSignOut,
  type SignOutKind,
  type SignOutProject,
  type SignOutSurvey,
} from '../../utils/speleoSignOut';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { SpeleoExitUploadFields, SpeleoExitUploadProgress, useSpeleoExitUpload } from './SpeleoExitUpload';

type Phase = 'checking' | 'checkFailed' | 'review' | 'releaseFailed';
/** The open project's changes: choosing, describing the upload, or confirming the discard. */
type Step = 'choose' | 'upload' | 'discard';
/** The answer being carried out, so the button that started it can say so. */
type Answer = 'go' | 'keep' | 'upload' | 'discard' | 'retry';

/**
 * What signing out would leave behind, and the user's one answer for it (see
 * `utils/speleoSignOut`). Laid out like every other dialog in the app
 * (`UnsavedChangesDialog`): the body says what is going on, and every button is
 * in the one footer row, Cancel first and the primary last.
 *
 * - The OPEN project, when it has changes SpeleoDB does not have: Discard (after
 *   a "lost forever" step), Keep on this computer, or Upload and disconnect
 *   (through the description step). Each one then hands the locks back and
 *   disconnects.
 * - Other cached copies with changes are always kept on this computer, their
 *   un-uploaded mark intact; the body names them and says they can be opened
 *   and uploaded after connecting again. Nothing here ever discards another
 *   project's work. A lock held on one is handed back with the others.
 * - Nothing to answer: Cancel or Disconnect.
 *
 * A failed hand-back is reported with "Try again" / "Disconnect anyway" instead
 * of a silent sign-out.
 */
export default function SpeleoSignOutDialog() {
  const request = useSpeleoSignOut((s) => s.request);
  if (!request) return null;
  return <SignOutPrompt key={request.id} kind={request.kind} initial={request.survey} settle={request.settle} />;
}

function SignOutPrompt({ kind, initial, settle }: {
  kind: SignOutKind;
  initial: SignOutSurvey | null;
  settle: (proceed: boolean) => void;
}) {
  const { t } = useTranslation();
  useScreenOccupant(true);
  const { upload, uploading, slow, unwound } = useSpeleoExitUpload();
  const pushFailure = useSpeleoStore(selectPushFailure);
  // Upload needs the lock it is uploading under; without it, Keep and Discard remain.
  const canUpload = useSpeleoStore((s) => s.link?.editState === 'editing' && !s.link.noAccess);
  const unsaved = useProjectStore((s) => s.isDirty);
  const unuploadedSaved = useSpeleoStore((s) => s.link?.hasUnpushedEdits === true);
  const [survey, setSurvey] = useState<SignOutSurvey | null>(initial);
  const [phase, setPhase] = useState<Phase>(initial ? 'review' : 'checkFailed');
  const [step, setStep] = useState<Step>('choose');
  const [doing, setDoing] = useState<Answer | null>(null);
  const [releasing, setReleasing] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [failed, setFailed] = useState<SignOutProject[]>([]);
  const busy = phase === 'checking' || doing !== null || uploading;

  const recheck = async () => {
    setPhase('checking');
    setError(null);
    try {
      const fresh = await surveySignOut();
      if (!signOutHasWork(fresh)) {
        settle(true);
        return;
      }
      setSurvey(fresh);
      setStep('choose');
      setPhase('review');
    } catch (e) {
      setError(getErrorMessage(e));
      setPhase('checkFailed');
    }
  };

  const open = survey?.openProject ?? null;
  const others = survey?.unuploaded ?? [];
  // Every lock but the open project's: the clean ones, and those of the other
  // copies with changes, which stay on this computer.
  const otherLocks = survey ? [...survey.releasable, ...others.filter((p) => p.holdsLock)] : [];
  const [openPath] = useState(() => useProjectStore.getState().project?.file_path ?? null);
  const openCurrent = () => (useProjectStore.getState().project?.file_path ?? null) === openPath;

  /** Hand back `projects`, then sign out; a failure gets Try again / Disconnect anyway. */
  const release = async (projects: SignOutProject[], answer: Answer) => {
    setDoing(answer);
    setReleasing(true);
    setError(null);
    const left = await releaseSignOutLocks(projects);
    if (left.length === 0) {
      settle(true);
      return;
    }
    setFailed(left);
    setReleasing(false);
    setDoing(null);
    setPhase('releaseFailed');
  };

  /** Upload and disconnect: the normal upload (it hands the open project back), then the rest. */
  const uploadOpen = async () => {
    if (!open || !message.trim() || commitMessageTooLong(message)) return;
    setDoing('upload');
    setError(null);
    const result = await upload(message.trim(), openCurrent);
    if (result === 'released') {
      await release(otherLocks, 'upload');
      return;
    }
    setDoing(null);
    if (result === 'diverged') {
      // SpeleoDB moved on: the divergence dialog is the way through, so the
      // sign-out stops here and the user resolves it first.
      settle(false);
    } else if (result === 'newer') {
      setError(t('speleoExitPrompt.errors.newerChanges'));
    }
  };

  // Keep on this computer (decision 10: never discard-only). The open project's
  // unsaved edits are saved first; the copy keeps its edits, its un-uploaded mark
  // and its recents row, and its lock is handed back with the others - the user's
  // explicit choice standing in for the consent a silent release never has.
  const keepOpen = async () => {
    if (!open) return;
    setDoing('keep');
    setError(null);
    if (useProjectStore.getState().isDirty) {
      const saved = await saveOpenProjectLocally();
      if (!saved?.success || useProjectStore.getState().isDirty) {
        setDoing(null);
        setError(t('speleoExitPrompt.signOut.keepFailed'));
        return;
      }
    }
    await release([...otherLocks, open], 'keep');
  };

  /** Discard, confirmed: the open project only. The discarding close also hands its lock back. */
  const discardOpen = async () => {
    if (!open) return;
    setDoing('discard');
    setError(null);
    let outcome: { ok: boolean; error?: string | undefined; warning?: string | undefined };
    if (!openCurrent()) {
      outcome = { ok: false };
    } else {
      try {
        const closed = await closeDiscardingLocal();
        outcome = closed.ok ? { ok: true, warning: closed.warning } : { ok: false, error: closed.error };
      } catch (e) {
        outcome = { ok: false, error: getErrorMessage(e) };
      }
    }
    if (!outcome.ok) {
      setDoing(null);
      setError(outcome.error || t('speleoExitPrompt.signOut.discardFailed', { name: open.summary.name }));
      return;
    }
    // A close that happened but left something behind (the lock could not be
    // handed back, or the copy could not be removed and stays reserved) is said,
    // as the exit prompt says it, before the sign-out goes on: a lock left held
    // is never silent.
    if (outcome.warning) {
      useUIStore.getState().showToast(outcome.warning, TOAST_KEYS.SPELEODB_LOCK, 'warning');
    }
    void useProjectStore.getState().loadRecentProjects();
    await release(otherLocks, 'discard');
  };

  const switching = kind === 'switch';
  const goLabel = switching ? t('speleoExitPrompt.signOut.switch') : t('speleoExitPrompt.signOut.signOut');
  const name = (p: SignOutProject) => p.summary.name || t('speleoExitPrompt.signOut.unnamed');
  const names = (projects: SignOutProject[]) => projects.map(name).join(', ');
  /** A button's label, or what it is doing while its answer is carried out. */
  const label = (answer: Answer, idle: string) => doing !== answer ? idle
    : releasing ? t('speleoExitPrompt.signOut.releasingShort')
    : answer === 'upload' ? t('speleoExitPrompt.uploading')
    : t('speleoExitPrompt.working');
  const back = () => { setError(null); setStep('choose'); };
  const choosing = phase === 'review' && step === 'choose';

  const title = phase === 'review' && step === 'discard'
    ? t('speleoExitPrompt.discardTitle')
    : switching ? t('speleoExitPrompt.signOut.switchTitle') : t('speleoExitPrompt.signOut.title');

  return (
    <>
      <DialogWrapper
        title={title}
        onClose={busy ? () => {} : () => settle(false)}
        size={open ? 'xl' : 'md'}
        data-testid="speleodb-signout-dialog"
      >
        <div className="space-y-4" data-phase={phase} data-step={step}>
          {phase === 'checking' && (
            <p role="status" className="text-sm text-text-secondary" data-testid="speleodb-signout-checking">
              {t('speleoExitPrompt.signOut.checking')}
            </p>
          )}

          {phase === 'checkFailed' && (
            <div className="space-y-2" data-testid="speleodb-signout-check-failed">
              <p className="text-sm font-medium text-text-primary">{t('speleoExitPrompt.signOut.checkFailedTitle')}</p>
              <p className="text-sm text-text-secondary">{t('speleoExitPrompt.signOut.checkFailedBody')}</p>
            </div>
          )}

          {choosing && survey && (
            <>
              <p className="text-sm text-text-secondary">{t('speleoExitPrompt.signOut.intro')}</p>

              {open && (
                <p className="text-sm text-text-primary" data-testid="speleodb-signout-open">
                  {t('speleoExitPrompt.signOut.openProjectBody', { name: name(open) })}
                </p>
              )}

              {others.length > 0 && (
                <section className="space-y-1" data-testid="speleodb-signout-unuploaded">
                  <p className="text-sm text-text-secondary">{t('speleoExitPrompt.signOut.otherChangesBody')}</p>
                  <ul className="list-disc pl-5 text-sm text-text-primary">
                    {others.map((p) => (
                      <li key={p.summary.id} data-testid={`speleodb-signout-row-${p.summary.id}`}>{name(p)}</li>
                    ))}
                  </ul>
                </section>
              )}

              {otherLocks.length > 0 && (
                <section className="space-y-1" data-testid="speleodb-signout-releasable">
                  <p className="text-sm text-text-secondary">{t('speleoExitPrompt.signOut.willRelease')}</p>
                  <ul className="list-disc pl-5 text-sm text-text-primary">
                    {otherLocks.map((p) => <li key={p.summary.id}>{name(p)}</li>)}
                  </ul>
                </section>
              )}
            </>
          )}

          {phase === 'review' && step === 'upload' && (
            <SpeleoExitUploadFields message={message} onMessage={setMessage} disabled={busy}
              uploading={uploading} slow={slow} unwound={unwound} testIdPrefix="speleodb-signout" />
          )}

          {phase === 'review' && step === 'discard' && open && (
            <div className="space-y-2" data-testid="speleodb-signout-discard">
              <p className="text-sm font-medium text-text-primary" data-testid="speleodb-signout-open-discard-body">
                {t('speleoExitPrompt.signOut.discardBody', { name: name(open) })}
              </p>
              <ul className="list-disc pl-5 text-sm text-text-primary" data-testid="speleodb-signout-discard-lost">
                {unsaved && <li>{t('speleoExitPrompt.signOut.lostUnsaved')}</li>}
                {(unuploadedSaved || !unsaved) && <li>{t('speleoExitPrompt.signOut.lostUnuploaded')}</li>}
              </ul>
              <p className="text-sm text-text-secondary">{t('speleoExitPrompt.signOut.discardDetail')}</p>
              {others.length > 0 && <p className="text-sm text-text-secondary">{t('speleoExitPrompt.signOut.discardOthersKept')}</p>}
            </div>
          )}

          {phase === 'releaseFailed' && (
            <div className="space-y-2" data-testid="speleodb-signout-release-failed">
              <p className="text-sm font-medium text-text-primary">{t('speleoExitPrompt.signOut.releaseFailedTitle')}</p>
              <p className="text-sm text-text-secondary">{t('speleoExitPrompt.signOut.releaseFailedBody', { names: names(failed) })}</p>
              <p className="text-sm text-warning-text">{t('speleoExitPrompt.signOut.signOutAnywayHint')}</p>
            </div>
          )}

          {releasing && (
            <p role="status" className="text-sm text-text-secondary" data-testid="speleodb-signout-releasing">
              {t('speleoExitPrompt.signOut.releasing')}
            </p>
          )}

          {error && !pushFailure && (
            <p role="alert" className="text-sm text-error" data-testid="speleodb-signout-error">{error}</p>
          )}

          <div className="flex flex-wrap justify-end gap-2 pt-4 border-t border-border" data-testid="speleodb-signout-footer">
            {phase !== 'review' && (
              <>
                <Button onClick={() => settle(false)} disabled={busy} data-testid="speleodb-signout-cancel">
                  {t('common.actions.cancel')}
                </Button>
                {(phase === 'checkFailed' || phase === 'releaseFailed') && (
                  <>
                    <Button onClick={() => settle(true)} disabled={busy} data-testid="speleodb-signout-anyway">
                      {t('speleoExitPrompt.signOut.signOutAnyway')}
                    </Button>
                    <Button variant="primary" disabled={busy} data-testid="speleodb-signout-retry"
                      onClick={() => void (phase === 'checkFailed' ? recheck() : release(failed, 'retry'))}>
                      {label('retry', t('speleoExitPrompt.signOut.retry'))}
                    </Button>
                  </>
                )}
              </>
            )}
            {choosing && (
              <>
                <Button onClick={() => settle(false)} disabled={busy} data-testid="speleodb-signout-cancel">
                  {t('common.actions.cancel')}
                </Button>
                {open ? (
                  <>
                    <Button variant="danger" onClick={() => { setError(null); setStep('discard'); }} disabled={busy}
                      data-testid="speleodb-signout-open-discard">
                      {t('speleoExitPrompt.signOut.discard')}
                    </Button>
                    <Button onClick={() => void keepOpen()} disabled={busy} data-testid="speleodb-signout-open-keep">
                      {label('keep', t('speleoExitPrompt.signOut.keepOpen'))}
                    </Button>
                    {canUpload && (
                      <Button variant="primary" onClick={() => { setError(null); setStep('upload'); }} disabled={busy}
                        data-testid="speleodb-signout-open-upload">
                        {switching ? t('speleoExitPrompt.signOut.uploadAndSwitch') : t('speleoExitPrompt.signOut.uploadAndSignOut')}
                      </Button>
                    )}
                  </>
                ) : (
                  <Button variant="primary" onClick={() => void release(otherLocks, 'go')} disabled={busy}
                    data-testid="speleodb-signout-go">
                    {label('go', goLabel)}
                  </Button>
                )}
              </>
            )}
            {phase === 'review' && step === 'upload' && (
              <>
                <Button onClick={back} disabled={busy} data-testid="speleodb-signout-open-back">
                  {t('speleoExitPrompt.back')}
                </Button>
                <Button variant="primary" onClick={() => void uploadOpen()}
                  disabled={busy || !message.trim() || commitMessageTooLong(message)}
                  data-testid="speleodb-signout-open-upload-confirm">
                  {label('upload', switching ? t('speleoExitPrompt.signOut.uploadAndSwitch') : t('speleoExitPrompt.signOut.uploadAndSignOut'))}
                </Button>
              </>
            )}
            {phase === 'review' && step === 'discard' && (
              <>
                <Button onClick={back} disabled={busy} data-testid="speleodb-signout-open-discard-back">
                  {t('speleoExitPrompt.back')}
                </Button>
                <Button variant="danger" onClick={() => void discardOpen()} disabled={busy}
                  data-testid="speleodb-signout-open-discard-confirm">
                  {label('discard', switching ? t('speleoExitPrompt.signOut.discardAndSwitch') : t('speleoExitPrompt.signOut.discardAndSignOut'))}
                </Button>
              </>
            )}
          </div>
        </div>
      </DialogWrapper>
      <SpeleoExitUploadProgress uploading={uploading} slow={slow} testId="speleodb-signout-upload-progress" />
    </>
  );
}
