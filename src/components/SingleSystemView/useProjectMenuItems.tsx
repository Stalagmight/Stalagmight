import { useSpeleoStore } from '../../store/speleoStore';
/**
 * Builds the in-project hamburger (AppMenu) items for SingleSystemView.
 *
 * Reuses the shared GlobalView rows (New/Open/Settings/Debug/Quit) from
 * appMenuItems and adds the project-specific actions (Save, Save As, Save and
 * Close, Close Project, Export, Project Settings, Project Details). Item ids
 * reuse the GlobalView/legacy test-ids for matching actions so existing e2e
 * selectors/helpers keep working.
 */
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useProject } from '../../hooks/useProject';
import { useUIStore } from '../../store/uiStore';
import { setSource } from '../../utils/telemetry';
import { MENU_ICON_COLORS } from '../common/menuColors';
import {
  leadingMenuItems,
  serviceMenuItems,
  quitMenuItem,
  importMenuItem,
  mnemoImportMenuItem,
} from '../common/appMenuItems';
import type { AppMenuItem } from '../common/AppMenu';
import {
  SaveIcon,
  CloseIcon,
  ExportIcon,
  SlidersIcon,
  InfoIcon,
  DatabaseIcon,
  LayersIcon,
  MapIcon,
  TargetIcon,
} from '../common/icons';

interface UseProjectMenuItemsArgs {
  /** The SingleSystemView-wrapped save (cancels autosave, then saves). */
  onSave: (opts?: { closeAfterSave?: boolean }) => Promise<{ success: boolean } | null | undefined>;
  /** A save (auto or manual) failed - flag the Save row with a dark-red bg. */
  saveFailed?: boolean;
}

export function useProjectMenuItems({ onSave, saveFailed = false }: UseProjectMenuItemsArgs): AppMenuItem[] {
  const { t } = useTranslation();
  const {
    handleNewProject,
    handleSaveProjectAs,
    handleCloseProject,
  } = useProject();
  const openDialog = useUIStore((s) => s.openDialog);
  const showDevStats = useUIStore((s) => s.showDevStats);
  const showMemoryStats = useUIStore((s) => s.showMemoryStats);
  const showMapStats = useUIStore((s) => s.showMapStats);
  const showZoomStats = useUIStore((s) => s.showZoomStats);
  const toggleDisplay = useUIStore((s) => s.toggleDisplay);

  const speleoLink = useSpeleoStore((s) => s.link);
  return useMemo(() => {
    const sz = 18;

    const saveAndClose = async () => {
      // Pass the close intent so a deferred Save-As (the overwrite-confirm dialog)
      // still closes the project after it saves. A direct save returns the result
      // and we close here; a save routed to the overwrite dialog returns null and
      // that dialog finishes the close once the user confirms.
      const result = await onSave({ closeAfterSave: true });
      // Only leave the project if the save actually succeeded (a cancelled
      // Save As returns null; an integrity/permission block returns success:false).
      // handleCloseProject now owns the held-lock close guard (fix 1), so the T2d
      // "push & release before closing" prompt fires here too.
      if (result && result.success) {
        await handleCloseProject();
      }
    };

    // "Project Details" = clear the selection so the Details panel shows
    // <ProjectStats>, then reveal the panel. revealDetailPanel (not
    // toggleDetailPanel) so a hover-mode user's reveal preference isn't silently
    // overwritten with 'pin' just to view stats.
    const showProjectDetails = () => {
      const ui = useUIStore.getState();
      ui.clearSelection();
      ui.revealDetailPanel();
    };

    return [
      ...leadingMenuItems(t, { onNewProject: handleNewProject }, 'hamburger'),
      { id: 'project-menu-sep-1', label: '', separator: true },
      {
        id: 'toolbar-save-button',
        label: t('common.actions.save'),
        icon: <SaveIcon size={sz} />,
        iconColor: MENU_ICON_COLORS.save,
        dangerBg: saveFailed,
        action: 'save',
        // Tag the save with the toolbar/menu entry point (the old toolbar Save
        // button did this; the menu must too, or save_project loses its source).
        onClick: () => { setSource('toolbar'); void onSave(); },
      },
      {
        id: 'project-menu-save-as',
        // A SpeleoDB-managed project has one working copy (the cache file), so an
        // explicit destination writes a detached UNLINKED copy and the session
        // stays put - the label says so. The action is unchanged: the backend
        // decides copy-vs-relocate from the link, not from which label was shown.
        label: speleoLink ? t('singleView.menu.saveACopy') : t('singleView.menu.saveAs'),
        icon: <SaveIcon size={sz} />,
        iconColor: MENU_ICON_COLORS.save,
        action: 'save_as',
        onClick: () => { void handleSaveProjectAs(); },
      },
      {
        id: 'project-menu-save-and-close',
        label: t('singleView.menu.saveAndClose'),
        icon: <SaveIcon size={sz} />,
        iconColor: MENU_ICON_COLORS.save,
        onClick: () => { void saveAndClose(); },
      },
      { id: 'project-menu-sep-2', label: '', separator: true },
      // Import means "add mNemo data to THIS project" (#508), so it offers a
      // .dmp-only picker - the one format that is additive to an open project.
      // Switching projects stays on the Open row above, which keeps the full
      // unified picker and its replacement guards. No editingBlocked disable:
      // the router rejects exactly the additive DMP case, with a tailored
      // message naming the read-only cause.
      importMenuItem(t),
      ...mnemoImportMenuItem(t, { onOpen: () => openDialog('mnemo-import') }),
      {
        id: 'toolbar-export-button',
        label: t('common.actions.export'),
        icon: <ExportIcon size={sz} />,
        iconColor: MENU_ICON_COLORS.export,
        action: 'export',
        onClick: () => { setSource('toolbar'); openDialog('export'); },
      },
      { id: 'project-menu-sep-3', label: '', separator: true },
      {
        id: 'project-menu-project-settings',
        label: t('singleView.menu.projectSettings'),
        icon: <SlidersIcon size={sz} />,
        iconColor: MENU_ICON_COLORS.projectSettings,
        onClick: () => openDialog('project-settings'),
      },
      {
        id: 'project-menu-project-details',
        label: t('singleView.menu.projectDetails'),
        icon: <InfoIcon size={sz} />,
        iconColor: MENU_ICON_COLORS.projectDetails,
        onClick: showProjectDetails,
      },
      { id: 'project-menu-sep-4', label: '', separator: true },
      ...serviceMenuItems(t, { onSettings: () => openDialog('settings'), onDebug: () => openDialog('debug') }, [
        {
          id: 'display-toggle-showDevStats',
          label: t('singleView.menu.devStats'),
          icon: <DatabaseIcon size={sz} />,
          iconColor: MENU_ICON_COLORS.debug,
          checked: showDevStats,
          onClick: () => toggleDisplay('showDevStats'),
        },
        {
          id: 'display-toggle-showMemoryStats',
          label: t('singleView.menu.memoryStats'),
          icon: <LayersIcon size={sz} />,
          iconColor: MENU_ICON_COLORS.debug,
          checked: showMemoryStats,
          onClick: () => toggleDisplay('showMemoryStats'),
        },
        {
          id: 'display-toggle-showMapStats',
          label: t('singleView.menu.mapStats'),
          icon: <MapIcon size={sz} />,
          iconColor: MENU_ICON_COLORS.debug,
          checked: showMapStats,
          onClick: () => toggleDisplay('showMapStats'),
        },
        {
          id: 'display-toggle-showZoomStats',
          label: t('singleView.menu.zoomStats'),
          icon: <TargetIcon size={sz} />,
          iconColor: MENU_ICON_COLORS.debug,
          checked: showZoomStats,
          onClick: () => toggleDisplay('showZoomStats'),
        },
      ]),
      { id: 'project-menu-sep-5', label: '', separator: true },
      {
        id: 'back-button',
        label: t('singleView.menu.closeProject'),
        icon: <CloseIcon size={sz} />,
        iconColor: MENU_ICON_COLORS.close,
        // handleCloseProject now routes a lock-held close through the speledb-close
        // guard itself (fix 1).
        onClick: () => { void handleCloseProject(); },
      },
      quitMenuItem(t),
    ];
  }, [t, openDialog, showDevStats, showMemoryStats, showMapStats, showZoomStats, toggleDisplay, onSave, saveFailed, handleNewProject, handleSaveProjectAs, handleCloseProject, speleoLink]);
}
