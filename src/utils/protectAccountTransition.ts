import i18n from '../i18n';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore } from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import { isProjectRequestCurrent, reserveProjectRequest } from './projectTransition';
import { discardWorkspaceDrafts, workspaceDraftNotice, type AccountTarget } from './workspaceDrafts';
import { askExitChoice } from './exitConfirm';
import { saveOpenProjectLocally } from './saveOpenProjectLocally';

/**
 * Account replacement protects the unsaved survey snapshot and unsent SpeleoDB
 * input, in ONE in-app question.
 *
 * SpeleoDB locks and un-uploaded work are NOT this guard's business: the sign-out
 * guard (`utils/speleoSignOut`) runs first and resolves them (upload / discard /
 * hand back). What is left here:
 *
 * - unsaved edits outside a held lock (`unsaved`): the project is reopened from
 *   disk without them - or, frontend#6, saved on this computer first when the
 *   project has a file of its own, so they are never only discardable;
 * - SpeleoDB form drafts this change of account drops (`account`, see
 *   `utils/workspaceDrafts`): listed in the same question, and dropped by either
 *   answer that goes on.
 *
 * Without an `account` no draft is at stake (a server change only signs out,
 * which a draft survives). `false` = cancelled, nothing changed here.
 */
export async function protectAccountTransition(
  opts: { account?: AccountTarget; unsaved?: boolean } = {},
): Promise<boolean> {
  const before = useProjectStore.getState();
  const project = before.project;
  const generation = useSpeleoStore.getState().sessionGeneration;
  const dirty = (opts.unsaved ?? true) && before.isDirty;
  const drafts = opts.account ? workspaceDraftNotice(opts.account) : null;
  if (!dirty && !drafts) return true;
  const request = reserveProjectRequest();
  const current = () => isProjectRequestCurrent(request) &&
    useSpeleoStore.getState().sessionGeneration === generation;
  const ownsProject = () => current() && useProjectStore.getState().project === project;
  const canSave = dirty && !!project?.file_path;
  const choice = await askExitChoice(dirty ? {
    title: i18n.t('speleoExitPrompt.account.title'),
    message: drafts ? `${i18n.t('speleoExitPrompt.account.message')} ${drafts}` : i18n.t('speleoExitPrompt.account.message'),
    confirmLabel: i18n.t('speleoExitPrompt.account.confirm'),
    cancelLabel: i18n.t('common.actions.cancel'),
    ...(canSave ? { saveLabel: i18n.t('speleoExitPrompt.saveLocally') } : {}),
  } : {
    title: i18n.t('speleoExitPrompt.drafts.title'),
    message: drafts ?? '',
    confirmLabel: i18n.t('speleoExitPrompt.drafts.confirm'),
    cancelLabel: i18n.t('common.actions.cancel'),
  });
  if (choice === 'cancel' || !ownsProject()) return false;
  if (choice === 'save') {
    const saved = await saveOpenProjectLocally();
    if (!saved?.success || !ownsProject() || useProjectStore.getState().isDirty) return false;
  }
  if (opts.account) discardWorkspaceDrafts(opts.account);
  if (choice === 'save' || !dirty) return true;
  const closed = await useProjectStore.getState().closeProject({ request, isCurrent: ownsProject });
  if (!closed.success || !current() || useProjectStore.getState().project !== null) return false;
  if (project?.file_path) {
    const opened = await useProjectStore.getState().loadProject(project.file_path, { accessConfirmed: true, acquireLock: false, request });
    return opened.success && current();
  }
  useUIStore.getState().setView('global');
  return true;
}
