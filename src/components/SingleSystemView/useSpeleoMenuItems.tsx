/**
 * The rows of the SpeleoDB menu for the open remote project: the hand-off verbs
 * (lock / unlock / reconnect, push, refresh, the opt-in force-edit), then the two
 * destinations - the project on speleodb.org and its SpeleoDB settings dialog.
 * Verbs first, destinations last, separated: acting on the project is the common
 * errand, and a destination that sits among the verbs gets pressed by mistake.
 *
 * Split out of `useProjectMenuItems` so the same `AppMenuItem[]` can back a SECOND
 * surface - the right-click menu on the toolbar's SpeleoDB status button
 * (`SpeleoStatusMenu`). Both render these rows with the same ids, so one selector
 * (or one test helper) reaches either surface.
 *
 * Returns `[]` for a local project; the caller decides whether a wrapper row is
 * worth rendering at all.
 */
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore, isUnwoundNotEdited } from '../../store/speleoStore';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { useSpeleoWebLink } from '../../hooks/useSpeleoWebLink';
import {
  SpeleoBadgedIcon,
  PushGlyph,
  RefreshGlyph,
  EditNoLockGlyph,
  ExternalLinkGlyph,
} from '../common/SpeleoBadgedIcon';
import type { AppMenuItem } from '../common/AppMenu';
import { SlidersIcon, PencilIcon, LockIcon } from '../common/icons';
import { holdsWriteLock } from '../../utils/quit';

export function useSpeleoMenuItems(): AppMenuItem[] {
  const { t } = useTranslation();
  const openDialog = useUIStore((s) => s.openDialog);
  // Select the four PRIMITIVES this hook reads, not the link object: the status
  // poll hands out a fresh object on every tick (`seedFromStatus` allocates), so
  // memoizing on the object meant the memo never held - all seven rows and their
  // `SpeleoBadgedIcon` SVG trees were rebuilt on every poll, in both consumers
  // (the hamburger and the status button's right-click menu), for a menu that is
  // usually closed. `useSpeleoProjectMetadata` already does exactly this, for
  // exactly this reason.
  const isRemote = useSpeleoStore((s) => s.link !== null);
  const editState = useSpeleoStore((s) => s.link?.editState ?? null);
  const forceEditing = useSpeleoStore((s) => s.link?.forceEditing ?? false);
  const hasUnpushedEdits = useSpeleoStore((s) => s.link?.hasUnpushedEdits ?? false);
  // [#303] Dirty only because a chain step walked the copy back - pushing that publishes
  // a revert, so the rows that would push route to their own confirmation instead.
  const unwoundNotEdited = useSpeleoStore((s) => isUnwoundNotEdited(s.link));
  const forceEditAllowed = useSpeleoStore((s) => s.link?.forceEditAllowed ?? false);
  // Derived in the SELECTOR so this stays a boolean primitive like its neighbours -
  // the shared predicate applied to the link object, not a sixth hand-rolled copy of
  // `editState === 'editing' && !forceEditing`. That spelling had already drifted at
  // one of its six call sites, which is why `holdsWriteLock` exists.
  const holdsLock = useSpeleoStore((s) => holdsWriteLock(s.link));
  // [#576 RV4-FE-SP4] Discard replaces the survey, which the read-only Plan and Export
  // workspaces refuse (as RV2-R-15 does for Restore); this menu is reachable there.
  const workspaceReadOnly = useUIStore((s) => s.appMode !== 'survey');
  const { busy, startEditing, update, done } = useSpeleoActions();
  // No page argument: the plain project page is the one a "take me to this project"
  // row should land on.
  const web = useSpeleoWebLink();

  return useMemo<AppMenuItem[]>(() => {
    if (!isRemote) return [];
    const sz = 18;

    return [
      editState === 'editing' && forceEditing
        ? {
            // #213: a force-editor holds no lock, so the top action is
            // Reconnect (acquire + reconcile), NOT Unlock. The release and push
            // verbs both need the lock, so neither is offered in this state - the
            // Unlock row below is replaced by this one, and the Push row keys on
            // `holdsLock`, which a force-editor does not have.
            id: 'speleodb-menu-reconnect',
            label: t('speleodb.forceEdit.reconnect'),
            icon: (
              <SpeleoBadgedIcon
                badge={<LockIcon size={9} />}
                badgeColor="text-yellow-400"
                size={sz}
                testIdPrefix="speleodb-menu-reconnect-icon"
              />
            ),
            disabled: busy,
            onClick: () => {
              void startEditing();
            },
          }
        : holdsLock
        ? {
            id: 'speleodb-menu-unlock',
            label: t('speleodb.menu.unlock'),
            icon: (
              <SpeleoBadgedIcon
                badge={<LockIcon size={9} />}
                badgeColor="text-green-400"
                size={sz}
                testIdPrefix="speleodb-menu-unlock-icon"
              />
            ),
            disabled: busy,
            onClick: () => {
              // Release; the push&release dialog when there are local edits.
              // [#303] An unwound copy is not work to push - see the status button.
              if (unwoundNotEdited) openDialog('speleodb-unwound-push');
              else if (hasUnpushedEdits) openDialog('speleodb-done');
              else void done(null);
            },
          }
        : {
            id: 'speleodb-menu-lock',
            label: t('speleodb.menu.lock'),
            icon: (
              <SpeleoBadgedIcon
                badge={<PencilIcon size={9} />}
                badgeColor="text-yellow-400"
                size={sz}
                testIdPrefix="speleodb-menu-lock-icon"
              />
            ),
            disabled: busy,
            onClick: () => {
              void startEditing();
            },
          },
      {
        id: 'speleodb-menu-push',
        label: t('speleodb.menu.push'),
        icon: (
          <SpeleoBadgedIcon
            badge={<PushGlyph size={9} />}
            badgeColor="text-green-400"
            size={sz}
            testIdPrefix="speleodb-menu-push-icon"
          />
        ),
        // Push-without-unlock needs the WRITE LOCK and something to push. Keyed on
        // `editState === 'editing'` alone, this row was enabled for a force-editor -
        // who has the edit gate open with no lock (#213) and therefore normally has
        // unpushed edits, that being the point of force-editing - and the backend can
        // only refuse it ("reconnect to acquire the write lock before pushing").
        // Newly reachable from the far more discoverable right-click status menu.
        disabled: busy || !holdsLock || !hasUnpushedEdits,
        // Collect a commit message via the done dialog in push mode (like
        // push & release, but the lock is retained - fix 8).
        onClick: () => openDialog('speleodb-done', { mode: 'push' }),
      },
      // [#303] The way out of a copy the user does not want to keep: drop it and come back
      // at the server tip. Whether the lock is kept or handed back is the dialog's
      // question, not this row's - both are ordinary answers to "discard". Offered
      // whenever there IS something to discard - most often a copy a chain step walked
      // back through history, where the dirty bit means "this is older", not "you have
      // work to save", but deliberately available over ordinary edits too. Destructive, so
      // it confirms first.
      {
        id: 'speleodb-menu-discard-changes',
        label: t('speleodb.menu.discardChanges'),
        icon: (
          <SpeleoBadgedIcon
            badge={<RefreshGlyph size={11} />}
            badgeColor="text-red-500"
            size={sz}
            testIdPrefix="speleodb-menu-discard-changes-icon"
          />
        ),
        disabled: busy || !hasUnpushedEdits || workspaceReadOnly,
        onClick: () => openDialog('speleodb-discard-changes'),
      },
      {
        id: 'speleodb-menu-refresh',
        label: t('speleodb.menu.refresh'),
        icon: (
          <SpeleoBadgedIcon
            badge={<RefreshGlyph size={11} />}
            badgeColor="text-blue-400"
            size={sz}
            testIdPrefix="speleodb-menu-refresh-icon"
          />
        ),
        disabled: busy,
        onClick: () => {
          void update();
        },
      },
      // #213: optimistic force-edit - edit WITHOUT the lock, behind the
      // `allow_force_edit` opt-in (hidden otherwise) and a big-red confirm.
      // Only offered while read-only; a force-editor uses Reconnect above.
      ...(forceEditAllowed && editState !== 'editing'
        ? [
            {
              id: 'speleodb-menu-edit-without-lock',
              label: t('speleodb.menu.editWithoutLock'),
              icon: (
                <SpeleoBadgedIcon
                  badge={<EditNoLockGlyph size={11} />}
                  badgeColor="text-red-500"
                  size={sz}
                  testIdPrefix="speleodb-menu-edit-without-lock-icon"
                />
              ),
              disabled: busy,
              onClick: () => openDialog('speleodb-force-edit'),
            },
          ]
        : []),
      { id: 'speleodb-menu-sep-1', label: '', separator: true },
      {
        id: 'speleodb-menu-open-web',
        label: t('speleodb.web.openProject'),
        // The SpeleoDB mark with the open-on-the-web glyph badged onto it, like
        // every other row here. A bare external-link icon read as a generic link
        // rather than "this project, over there", and it was the only row in the
        // submenu not carrying the mark.
        icon: (
          <SpeleoBadgedIcon
            badge={<ExternalLinkGlyph size={9} />}
            badgeColor="text-blue-400"
            size={sz}
            testIdPrefix="speleodb-menu-open-web-icon"
          />
        ),
        // Signed out (or no remote id yet) there is no URL to resolve, and a row
        // that silently does nothing is worse than a greyed one.
        disabled: web.url === null,
        onClick: web.open,
      },
      {
        id: 'speleodb-menu-project-settings',
        label: t('speleodb.projectDialog.open'),
        icon: (
          <SpeleoBadgedIcon
            badge={<SlidersIcon size={9} />}
            badgeColor="text-blue-400"
            size={sz}
            testIdPrefix="speleodb-menu-project-settings-icon"
          />
        ),
        onClick: () => openDialog('speleodb-project'),
      },
    ];
  }, [
    t,
    openDialog,
    isRemote,
    editState,
    forceEditing,
    holdsLock,
    hasUnpushedEdits,
    workspaceReadOnly,
    unwoundNotEdited,
    forceEditAllowed,
    busy,
    startEditing,
    update,
    done,
    web.url,
    web.open,
  ]);
}
