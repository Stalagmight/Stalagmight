/**
 * Shared AppMenu rows used by BOTH the GlobalView hamburger and the in-project
 * hamburger (useProjectMenuItems), so the common New / Open / Settings / Debug /
 * Quit entries are defined exactly once instead of being hand-maintained twice
 * (they had already drifted - e.g. the quit handler). Item ids match the
 * legacy/e2e selectors so existing tests keep working.
 */
import type { TFunction } from 'i18next';
import {
  FilePlusIcon,
  FolderOpenIcon,
  DownloadIcon,
  SettingsIcon,
  BugIcon,
  QuitIcon,
} from './icons';
import { SpeleoBadgedIcon } from './SpeleoBadgedIcon';
import { MENU_ICON_COLORS } from './menuColors';
import { setProjectActionSource, type SourceName } from '../../utils/telemetry';
import { quitApp } from '../../utils/quit';
import { pickAndRoute } from '../../utils/openOrImport';
import { openOrImportFilters, inProjectImportFilters } from '../../utils/fileDialogs';
import type { AppMenuItem } from './AppMenu';

const ICON_SIZE = 18;

/**
 * The IN-PROJECT hamburger's "Import" row (#508) - adds mNemo data or a `.stalx`
 * (#674) to the open project, and nothing else.
 *
 * Two shapes died here on purpose. The pre-#407 version was five per-format
 * rows; #407 collapsed them into a one-row submenu offering the SAME unified
 * filters as Open, in BOTH hamburgers. That left the recents screen with two
 * rows doing one thing, and left in-project Import offering formats it cannot
 * actually import into an open project (every non-DMP format only ever reaches
 * the "close this project and import as new?" prompt - which is Open's job).
 * So: the recents hamburger drops the row entirely (Open / Import already does
 * everything there), and this row keeps only the additive `.dmp` case, with the
 * submenu flattened away now that structural parity between the two hamburgers
 * is moot.
 *
 * No `editingBlocked` disable: the router owns that decision and rejects the
 * DMP-into-read-only-project case with a tailored, fix-it message (an
 * import-blocked prompt for the causes one click can fix).
 */
export function importMenuItem(t: TFunction): AppMenuItem {
  return {
    // Id kept from the submenu parent for e2e/test-id continuity.
    id: 'global-import-menu-button',
    label: t('globalView.actions.import'),
    icon: <DownloadIcon size={ICON_SIZE} />,
    iconColor: MENU_ICON_COLORS.import,
    action: 'import_dmp',
    onClick: () => {
      setProjectActionSource('hamburger');
      void pickAndRoute(inProjectImportFilters(), 'Import Failed', { importDoor: true });
    },
  };
}

/** New + Open rows. `source` is the telemetry entry-point tag (e.g. 'hamburger'). */
export function leadingMenuItems(
  t: TFunction,
  actions: { onNewProject: () => void },
  source: SourceName,
): AppMenuItem[] {
  return [
    {
      id: 'app-menu-new-project',
      label: t('globalView.actions.newProject'),
      icon: <FilePlusIcon size={ICON_SIZE} />,
      iconColor: MENU_ICON_COLORS.new,
      action: 'new_file',
      onClick: () => {
        setProjectActionSource(source);
        actions.onNewProject();
      },
    },
    {
      // The one door (#407): the hamburger Open row is the same unified
      // Open/Import as the toolbar button and Ctrl+O - every file the app
      // supports, routed by extension through routeOpenedFile. Id kept for
      // e2e/test-id continuity.
      id: 'global-open-project-button',
      label: t('globalView.actions.openOrImport'),
      icon: <FolderOpenIcon size={ICON_SIZE} />,
      iconColor: MENU_ICON_COLORS.open,
      action: 'open_file',
      onClick: () => {
        setProjectActionSource(source);
        void pickAndRoute(openOrImportFilters(), 'Open Failed');
      },
    },
  ];
}

/**
 * "Open from SpeleoDB" row (#203) - opens the remote project browser. Shared by
 * the GlobalView hamburger and the in-project menu so both reach the same
 * production browser (the dev spike entry it replaces was GlobalView-only).
 */
export function openFromSpeleoDbMenuItem(
  t: TFunction,
  actions: { onOpen: () => void },
): AppMenuItem {
  return {
    id: 'open-from-speleodb-button',
    label: t('globalView.actions.openFromSpeleoDb'),
    icon: (
      <SpeleoBadgedIcon
        badge={<FolderOpenIcon size={11} />}
        badgeColor="text-blue-400"
        size={ICON_SIZE}
        testIdPrefix="open-from-speleodb-button-icon"
      />
    ),
    onClick: actions.onOpen,
  };
}

/**
 * Settings row plus the Debug submenu. Debug Logs is the submenu's first row in
 * BOTH hamburgers, so it sits in the same place whether or not a project is open;
 * the in-project menu appends its overlay toggles (`debugExtras`) below it.
 */
export function serviceMenuItems(
  t: TFunction,
  actions: { onSettings: () => void; onDebug: () => void },
  debugExtras: AppMenuItem[] = [],
): AppMenuItem[] {
  return [
    {
      id: 'settings-button',
      label: t('globalView.actions.settings'),
      icon: <SettingsIcon size={ICON_SIZE} />,
      iconColor: MENU_ICON_COLORS.settings,
      onClick: actions.onSettings,
    },
    {
      // Ids kept from the in-project Debug submenu and the former top-level
      // Debug Logs row, for e2e/test-id continuity.
      id: 'debug-menu',
      label: t('singleView.menu.debug'),
      icon: <BugIcon size={ICON_SIZE} />,
      iconColor: MENU_ICON_COLORS.debug,
      submenu: [
        {
          id: 'global-debug-button',
          label: t('globalView.actions.debugLogs'),
          icon: <BugIcon size={ICON_SIZE} />,
          iconColor: MENU_ICON_COLORS.debug,
          onClick: actions.onDebug,
        },
        ...(debugExtras.length > 0
          ? [{ id: 'debug-menu-sep', label: '', separator: true }, ...debugExtras]
          : []),
      ],
    },
  ];
}

/** Quit row - shares the single close + error-handling contract via quitApp. */
export function quitMenuItem(t: TFunction): AppMenuItem {
  return {
    id: 'global-quit-button',
    label: t('globalView.actions.quit'),
    icon: <QuitIcon size={ICON_SIZE} />,
    iconColor: MENU_ICON_COLORS.quit,
    action: 'quit',
    onClick: quitApp,
  };
}
