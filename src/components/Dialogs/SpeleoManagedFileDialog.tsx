import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useOpenRemoteProject } from '../../hooks/useOpenRemoteProject';
import { invokeIpc } from '../../utils/ipc';
import { humanizeSpeleoError } from '../../utils/speleoError';
import { endpointsMatch } from '../../utils/speleoEndpoint';
import { extractFilename } from '../../utils/path';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { SpeleoAdoptConfirm } from './SpeleoAdoptConfirm';
import type { SpeleoManagedFile, SpeleoProjectSummary } from '../../types/ipc.generated';

/**
 * Canonical-storage prompt: the user opened a `.stalx` that carries a
 * `speleo_link` row but is NOT the managed (cache) copy of that project, so the
 * backend refused the load (`SPELEODB_MANAGED`, see `speleodb_foreign.rs`) and
 * `loadProject` raised this instead of a dead-end error. Two ways forward:
 *
 * - Open from SpeleoDB: open the MANAGED copy through the shared
 *   `useOpenRemoteProject` flow, with all its guards (permission seeding, the
 *   Ariane adopt route, the unpushed-local prompt, the hand-back refusal). The
 *   foreign file is left untouched - reopening it raises this prompt again.
 *   Looked up in `speleodb_list_projects` rather than synthesized from the
 *   file's link row, because only the list carries `latest_commit_sha` and
 *   `permission`; a project the account cannot see simply is not in the list,
 *   which is also how a foreign-endpoint link surfaces (inline, no special
 *   backend case).
 * - Remove the SpeleoDB link: `speleodb_remove_link_at` strips the row from
 *   the file in place, then the SAME path is re-opened as a plain local
 *   project. The retry inherits whatever open-guards the route that raised
 *   this prompt already ran (routeOpenedFile prompts before the first
 *   attempt; recents clicks never prompt), so it is no weaker than any other
 *   open of an unmanaged file.
 *
 * Offline / signed out, the remote action is disabled with the reason on
 * screen and the strip action remains - a cave-expedition user with no network
 * must never be locked out of their own file.
 */
export default function SpeleoManagedFileDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const openDialog = useUIStore((s) => s.openDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  const session = useSpeleoStore((s) => s.session);
  // Close the prompt right before loadProject so its progress is visible.
  const { openRemote, adoptRemote } = useOpenRemoteProject({ onBeforeLoad: closeDialog });

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The managed copy turned out to be an Ariane (.tml-only) project - offer the
  // adopt inline, the same shape SpeleoUnpushedLocalDialog renders.
  const [pendingAdopt, setPendingAdopt] = useState<SpeleoProjectSummary | null>(null);

  const managed = dialogData.managed as SpeleoManagedFile | undefined;
  const cancelReturnsTo =
    typeof dialogData.cancelReturnsTo === 'string' ? dialogData.cancelReturnsTo : null;

  // A `null` session only means "not asked yet" - resolve it so the remote
  // action's disabled state settles instead of sticking on the pessimistic
  // default.
  useEffect(() => {
    if (useSpeleoStore.getState().session === null) {
      void useSpeleoStore.getState().refreshSession();
    }
  }, []);

  // When a newer refusal replaces this prompt, the instance's transient state
  // belongs to the prompt that is gone. Left alone, prompt B inherits A's
  // `busy` - every button disabled, Escape inert - and A's stale-result path
  // deliberately updates nothing, so B would stay unusable for good. Keyed on
  // the whole identity, since a replacement can keep the path.
  const identity = `${managed?.path ?? ''}|${managed?.projectId ?? ''}|${managed?.endpoint ?? ''}`;
  useEffect(() => {
    setBusy(false);
    setError(null);
    setPendingAdopt(null);
  }, [identity]);

  if (!managed) return null;

  const loggedIn = session?.loggedIn === true;
  // Why the remote action cannot run, or null when it can. `canOpenRemote` is
  // false when the file's project id failed validation backend-side - that is a
  // property of the FILE, so it wins over the fixable signed-out state.
  const remoteBlockedReason = !managed.canOpenRemote
    ? t('speleodb.managedFile.openRemoteDisabledBadId')
    : loggedIn
      ? null
      : t('speleodb.managedFile.openRemoteDisabledLoggedOut');

  // This dialog is a singleton component: a newer refusal replaces
  // `dialogData.managed` in place, and React keeps the same instance with its
  // in-flight handlers still closed over the OLD file. Anything awaited has to
  // re-check that the prompt is still about the file it started on, or file A's
  // result opens A's project over B's prompt - or writes A's error into it.
  //
  // The whole identity, not just the path: a replaced file can be refused at
  // the SAME path while naming a different project or a different deployment,
  // which is precisely the case the strip command re-checks for backend-side.
  const stillPrompting = (about: SpeleoManagedFile) => {
    const now = useUIStore.getState().dialogData.managed as SpeleoManagedFile | undefined;
    return (
      now?.path === about.path &&
      now?.projectId === about.projectId &&
      now?.endpoint === about.endpoint
    );
  };

  const dismiss = () => {
    if (cancelReturnsTo) openDialog(cancelReturnsTo);
    else closeDialog();
  };

  const handleOpenRemote = async () => {
    setBusy(true);
    setError(null);
    // The file names the deployment it belongs to, and a project id only means
    // anything ON that deployment. Signed in elsewhere, the id is checked BEFORE
    // the list is consulted rather than only when the lookup misses: a cloned
    // deployment or a restored database can hold the very same UUID, and
    // matching on the id alone would open THAT project - a different cave
    // system, under a file that says plainly which server it came from.
    if (session?.endpoint && !endpointsMatch(managed.endpoint, session.endpoint)) {
      setError(
        t('speleodb.managedFile.notOnAccount', { name: managed.projectName }) +
          ` ${t('speleodb.managedFile.endpointMismatch', { endpoint: managed.endpoint })}`
      );
      setBusy(false);
      return;
    }
    let summary: SpeleoProjectSummary | undefined;
    try {
      const projects = await invokeIpc('speleodb_list_projects');
      if (!stillPrompting(managed)) return;
      summary = projects.find((p) => p.id === managed.projectId);
    } catch (e) {
      if (!stillPrompting(managed)) return;
      // The list fetch IS the offline path: no special-casing, just the reason.
      setError(humanizeSpeleoError(e, t('speleodb.errors.openFailed')));
      setBusy(false);
      return;
    }
    if (!summary) {
      setError(t('speleodb.managedFile.notOnAccount', { name: managed.projectName }));
      setBusy(false);
      return;
    }
    const outcome = await openRemote(summary);
    if (!stillPrompting(managed)) return;
    if (outcome.status === 'adopt') {
      setPendingAdopt(summary);
      setBusy(false);
    } else if (outcome.status === 'error') {
      setError(outcome.message);
      setBusy(false);
    }
    // 'opened': onBeforeLoad already closed this prompt. 'unpushed_local': that
    // prompt replaced this one in the dialog slot. 'load_failed': loadProject
    // surfaced its own error after the hand-off. Nothing to do for any of them.
  };

  const handleRemoveLink = async () => {
    setBusy(true);
    setError(null);
    try {
      // The refusal payload goes back with the request: a prompt can sit open
      // while the file at that path is replaced, and the backend re-checks this
      // against the link it finds before deleting anything.
      await invokeIpc('speleodb_remove_link_at', { path: managed.path, expected: managed });
    } catch (e) {
      if (!stillPrompting(managed)) return;
      // Keep the prompt open - closing it would leave the user with nothing
      // opened and no explanation.
      setError(humanizeSpeleoError(e, t('speleodb.managedFile.removeFailed')));
      setBusy(false);
      return;
    }
    // The strip succeeded either way - the row is gone, which is what the user
    // asked for - but only the prompt that asked may close and load.
    if (!stillPrompting(managed)) return;
    closeDialog();
    // Now unmanaged: the same open that was refused goes through normally. A
    // failed load surfaces its own error through loadProject.
    await useProjectStore.getState().loadProject(managed.path);
  };

  const handleConfirmAdopt = async (summary: SpeleoProjectSummary) => {
    setPendingAdopt(null);
    setBusy(true);
    setError(null);
    const outcome = await adoptRemote(summary);
    if (!stillPrompting(managed)) return;
    if (outcome.status === 'error') {
      setError(outcome.message);
      setBusy(false);
    }
  };

  if (pendingAdopt) {
    const summary = pendingAdopt;
    return (
      <SpeleoAdoptConfirm
        projectName={summary.name}
        busy={busy}
        onCancel={() => setPendingAdopt(null)}
        onConfirm={() => void handleConfirmAdopt(summary)}
      />
    );
  }

  return (
    <DialogWrapper
      title={t('speleodb.managedFile.title')}
      // Not dismissable while an action is in flight - Escape/backdrop would
      // unmount the only surface an in-flight error has (the
      // SpeleoUnpushedLocalDialog hardening).
      onClose={busy ? () => {} : dismiss}
      size="md"
      data-testid="speleodb-managed-file-dialog"
    >
      <div className="space-y-4">
        <p className="text-sm text-text-secondary">
          {t('speleodb.managedFile.message', {
            fileName: extractFilename(managed.path),
            name: managed.projectName,
          })}
        </p>
        <p className="text-xs text-text-secondary">{t('speleodb.managedFile.hint')}</p>

        {remoteBlockedReason && (
          <p
            className="text-xs text-text-secondary"
            data-testid="speleodb-managed-file-remote-blocked"
          >
            {remoteBlockedReason}
          </p>
        )}

        {error && (
          <div
            className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm"
            role="alert"
            data-testid="speleodb-managed-file-error"
          >
            {error}
          </div>
        )}

        <div className="flex flex-wrap justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={dismiss} disabled={busy} data-testid="speleodb-managed-file-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button
            onClick={() => void handleRemoveLink()}
            disabled={busy}
            data-testid="speleodb-managed-file-remove-link"
          >
            {t('speleodb.managedFile.removeLink')}
          </Button>
          {/* Primary + [data-primary-action]: a stray Enter opens the managed
              copy - it can never mutate the user's file. */}
          <Button
            variant="primary"
            onClick={() => void handleOpenRemote()}
            disabled={busy || remoteBlockedReason !== null || session === null}
            data-primary-action=""
            data-testid="speleodb-managed-file-open-remote"
          >
            {t('speleodb.managedFile.openRemote')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
