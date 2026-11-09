import { useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { DialogWrapper } from './DialogWrapper';
import { SpeleoDbGeneralTab } from './settings/SpeleoDbGeneralTab';
import { SpeleoAdminTab } from './settings/SpeleoAdminTab';
import { SpeleoRevisionHistory } from './settings/SpeleoRevisionHistory';
import { Button } from '../common/Button';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useSpeleoProjectMetadata } from '../../hooks/useSpeleoProjectMetadata';
import { TOAST_KEYS } from '../../utils/toastKeys';

type SpeleoProjectTab = 'general' | 'history' | 'admin';
const TABS: readonly SpeleoProjectTab[] = ['general', 'history', 'admin'];

function isTab(value: unknown): value is SpeleoProjectTab {
  return typeof value === 'string' && (TABS as readonly string[]).includes(value);
}

/**
 * Server-side administration of the open remote project: its SpeleoDB details,
 * metadata, revision history and lock release.
 *
 * Split out of the local Project Settings dialog, where all of this used to sit
 * behind one "SpeleoDB" tab: none of it is stored in the .stalx, none of it honors
 * that dialog's Save/Cancel, and most of it takes effect on the server the moment
 * it is clicked.
 */
export function SpeleoDbProjectDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const showToast = useUIStore((s) => s.showToast);
  const initialTab = useUIStore((s) => s.dialogData.tab);
  // A project can unlink from another surface while this sits open (the delete flow
  // closes the dialog itself), which leaves every tab with nothing to administer.
  // The BOOLEAN, not the link object - `seedFromStatus` reallocates the link on
  // every poll tick, and selecting it re-rendered this whole tabbed dialog (roster
  // rows, revision list, metadata form) on each one.
  const isRemote = useSpeleoStore((s) => s.link !== null);
  const metadata = useSpeleoProjectMetadata();

  // WHICH project this dialog is about. Everything in it - the loaded metadata form,
  // the roster, the revision list, the admin actions - describes the project that was
  // open when it mounted, but not one of the server verbs behind them carries a
  // project id: `speleodb_patch_project`, `speleodb_delete_project`,
  // `speleodb_release_current_lock` and the metadata/revision calls are each
  // addressed backend-side via `speleodb_validated_link()`, i.e. to whatever project
  // is open NOW.
  //
  // Nothing used to reconcile the two. `loadProject` leaves `activeDialog` alone, so
  // this dialog survives a project switch, and `syncProjectLink` hands a
  // remote -> remote switch a fresh NON-null link - so `isRemote` never flips and the
  // `link !== null`-keyed load effect in `useSpeleoProjectMetadata` never re-runs.
  // The General tab kept project A's values on screen while Save PATCHed project B,
  // overwriting ANOTHER project's server metadata with A's. Reachable today: the
  // held-lock prompt is an independent overlay (App.tsx, outside the `activeDialog`
  // slot) whose "Continue editing" calls `loadProject` for a DIFFERENT project, and
  // the login scan that raises it is fire-and-forget - so it can land over this
  // dialog rather than over the Settings dialog the login was made from.
  //
  // The binding is the open FILE PATH, not `remoteProjectId`: the path is committed
  // in the same synchronous `set()` as the project itself, whereas `remoteProjectId`
  // is nulled by the switch and only re-seeded by the next status poll - it reads
  // "unchanged" for exactly the window this guard exists to cover.
  const openPath = useProjectStore((s) => s.project?.file_path ?? null);
  const boundPath = useRef(openPath);
  const projectChanged = boundPath.current !== openPath;

  const [tab, setTab] = useState<SpeleoProjectTab>(isTab(initialTab) ? initialTab : 'general');

  const panelIdBase = useId();
  const tabId = (tk: SpeleoProjectTab) => `${panelIdBase}-tab-${tk}`;
  const panelId = (tk: SpeleoProjectTab) => `${panelIdBase}-panel-${tk}`;

  useEffect(() => {
    if (!projectChanged) return;
    // Close rather than re-seed. Every tab is stale, not just General, so one rule
    // settles all four at once; and silently re-seeding the form would throw away
    // whatever the user had typed with no notice - the very loss this dialog wires
    // `isDirty` into DialogWrapper to prevent. Those edits cannot be rescued either
    // way (they are edits to a project that is no longer open, and nothing here can
    // still address it), so say what happened instead of leaving a form that looks
    // live but would land on someone else's project.
    closeDialog();
    showToast(
      t('speleodb.projectDialog.projectChanged'),
      TOAST_KEYS.SPELEODB_PROJECT_DIALOG_STALE,
      'warning',
    );
  }, [projectChanged, closeDialog, showToast, t]);

  // The effect above runs after this render commits, so refuse to paint the form (and
  // with it Save, and every other tab's server verb) for the one frame in between.
  if (projectChanged) return null;

  return (
    <DialogWrapper
      title={t('speleodb.projectDialog.title')}
      onClose={closeDialog}
      // Same fixed footprint as Project Settings, so the content pane scrolls
      // within the dialog instead of overflowing the screen.
      fixedSize={{ width: 'min(720px, 95vw)', height: 'clamp(420px, 75vh, 760px)' }}
      // `closeOnBackdropClick={false}` says "do not dismiss this by accident", but it
      // only covers the backdrop - Escape and the X still ran straight through and
      // silently dropped staged General-tab metadata edits. `canSave` is exactly the
      // "there are unsaved edits on the server metadata form" bit the footer's Save
      // button is already gated on, so wiring it here reuses DialogWrapper's built-in
      // confirm overlay instead of inventing a second notion of dirty.
      isDirty={metadata.canSave}
      closeOnBackdropClick={false}
      overflow="hidden"
      bodyClassName="flex flex-col flex-1 min-h-0"
      data-testid="speleodb-project-dialog"
    >
      {!isRemote ? (
        <p className="text-text-secondary p-4" data-testid="speleodb-project-not-remote">
          {t('speleodb.projectDialog.notRemote')}
        </p>
      ) : (
        <div className="flex flex-1 min-h-0">
          <div className="w-44 shrink-0 border-r border-border p-2 space-y-1" role="tablist">
            {TABS.map((tk) => (
              <button
                key={tk}
                id={tabId(tk)}
                role="tab"
                aria-selected={tab === tk}
                aria-controls={panelId(tk)}
                onClick={() => setTab(tk)}
                className={`w-full text-left px-3 py-1.5 rounded text-sm ${tab === tk ? 'bg-accent-10 text-text-primary' : 'text-text-secondary hover:bg-background-50'}`}
                data-testid={`speleodb-project-tab-${tk}`}
              >
                {t(`speleodb.projectDialog.tabs.${tk}`)}
              </button>
            ))}
          </div>

          <div className="flex-1 min-w-0 overflow-auto p-4">
            {tab === 'general' && (
              <div role="tabpanel" id={panelId('general')} aria-labelledby={tabId('general')}>
                <SpeleoDbGeneralTab metadata={metadata} />
              </div>
            )}

            {tab === 'history' && (
              <div role="tabpanel" id={panelId('history')} aria-labelledby={tabId('history')}>
                <SpeleoRevisionHistory />
              </div>
            )}
            {tab === 'admin' && (
              <div role="tabpanel" id={panelId('admin')} aria-labelledby={tabId('admin')}>
                <SpeleoAdminTab />
              </div>
            )}
          </div>
        </div>
      )}

      {/* Always rendered (even with no link, and on the tabs that carry no Save) so
          the dialog chrome does not jump as the panel changes. */}
      <div className="border-t border-border p-3 flex justify-end gap-2 shrink-0">
        <Button variant="secondary" onClick={closeDialog} data-testid="speleodb-project-close">
          {t('common.actions.close')}
        </Button>
        {/* Only the General tab stages its edits; every other tab writes to the
            server on click. Omitted outright for a read-only user, as the in-body
            button was - there is nothing they could save. */}
        {isRemote && tab === 'general' && metadata.canEdit && (
          <Button
            variant="primary"
            disabled={!metadata.canSave}
            onClick={() => void metadata.handleSave()}
            data-testid="speleodb-settings-save"
          >
            {metadata.saving
              ? t('speleodb.projectSettings.saving')
              : t('speleodb.projectSettings.saveButton')}
          </Button>
        )}
      </div>
    </DialogWrapper>
  );
}
