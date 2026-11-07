import { useEffect, useLayoutEffect, useState, useCallback, useMemo, useRef, type MouseEvent, type ChangeEvent, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { useProject } from '../../hooks/useProject';
import { useGlobalViewShortcuts } from '../../hooks/useGlobalViewShortcuts';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useGlobalViewStore } from '../../store/globalViewStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { Button } from '../common/Button';
import { AppMenu, type AppMenuItem } from '../common/AppMenu';
import { leadingMenuItems, serviceMenuItems, quitMenuItem, openFromSpeleoDbMenuItem } from '../common/appMenuItems';
import { CloseIcon, DownloadIcon, FilePlusIcon, FolderOpenIcon, ListIcon, MapIcon } from '../common/icons';
import { SpeleoBadgedIcon, RefreshGlyph } from '../common/SpeleoBadgedIcon';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import { MENU_ICON_COLORS } from '../common/menuColors';
import { setProjectActionSource } from '../../utils/telemetry';
import { routeOpenedFile } from '../../utils/openOrImport';
import { openRecentEntry } from './openRecentEntry';
import { PROJECT_EXTENSION, IMPORT_EXTENSIONS } from '../../utils/fileDialogs';
import { LoadingProgress } from '../LoadingProgress';
import SystemCard from './SystemCard';
import { resolveSpeleoLock } from './SpeleoLockBadge';
import GlobalViewMap, { type GlobalViewMapHandle } from './GlobalViewMap';
import MetadataPanel from './MetadataPanel';
import { RecentSortControl } from './RecentSortControl';
import { useRecentSort, sortRecentProjects, defaultDirectionForField, type RecentSortField } from './useRecentSort';
import { RecentDensityToggle } from './RecentDensityToggle';
import { useRecentDensity } from './useRecentDensity';
import SystemTable from './SystemTable';
import MissingSystemCard from './MissingSystemCard';
import RemoteSystemCard from './RemoteSystemCard';
import {
  deriveMissingPaths,
  deriveRemoteOnlyItems,
  buildRecentListItems,
  filterRecentListItems,
} from './recentListItems';
import { canPublishRecent, publishRecentToSpeleoDb } from './publishRecentToSpeleoDb';
import { useOpenRemoteProject } from '../../hooks/useOpenRemoteProject';
import { useSpeleoLocalStates } from '../../hooks/useSpeleoLocalStates';
import { SpeleoAdoptConfirm } from '../Dialogs/SpeleoAdoptConfirm';
import type { ProjectMetadata } from '../../types/project';
import type { SpeleoProjectSummary } from '../../types/ipc.generated';
import { formatNumber } from '../../i18n/format';
import { debugLog } from '../../utils/debugLog';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { formatPhaseLabel } from '../../utils/loadingPhaseLabel';
import { useShallow } from 'zustand/react/shallow';

type ViewTab = 'map' | 'list';

/**
 * Conservative upper bound on the OS double-click interval (Windows' max is
 * 500ms). In the list view we wait this long after a single click before showing
 * the details panel, so a double-click-to-open doesn't flash it - by the time it
 * elapses, the click was clearly a select (not the first half of an open).
 */
const DOUBLE_CLICK_MS = 500;

interface ContextMenuState {
  visible: boolean;
  x: number;
  y: number;
  projectPath: string | null;
}

export default function GlobalView() {
  // [perf] Mark GlobalView mount + first effect so we can see when the
  // post-back UI is actually live (paired with [perf:close]).
  const _mountTimeRef = useRef(performance.now());
  useEffect(() => {
    const ms = performance.now() - _mountTimeRef.current;
    debugLog(`[perf:globalview-mount] firstEffect=${ms.toFixed(0)}ms after construction`);
    void import('../../utils/memoryProbe').then(({ recordPerfTiming }) => {
      recordPerfTiming('GlobalView mount', ms);
    });
  }, []);

  const {
    recentProjects,
    handleNewProject,
    handleOpenOrImport,
  } = useProject();

  const {
    loading,
    loadRecentProjects,
    loadProject,
    resetLoading,
    cancelLoading,
    removeFromRecentProjects,
    config,
  } = useProjectStore(
    useShallow((s) => ({
      loading: s.loading,
      loadRecentProjects: s.loadRecentProjects,
      loadProject: s.loadProject,
      resetLoading: s.resetLoading,
      cancelLoading: s.cancelLoading,
      removeFromRecentProjects: s.removeFromRecentProjects,
      config: s.config,
    })),
  );

  // Note: useLoadingProgress() is called in App.tsx (always mounted) to avoid duplicate listeners
  const openDialog = useUIStore((s) => s.openDialog);
  const showToast = useUIStore((s) => s.showToast);
  const syntacticLevel = useUIStore((s) => s.syntacticLevel);
  // Toolbar buttons show icons at the 'toolbar_only' level and up (menu items
  // use the stricter 'icons' level - handled inside AppMenu).
  const showToolbarIcons = syntacticLevelAtLeast(syntacticLevel, 'toolbar_only');
  const { t } = useTranslation();
  const [activeTab, setActiveTab] = useState<ViewTab>('list');
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(null);
  const [multiSelectedIds, setMultiSelectedIds] = useState<Set<string>>(new Set());
  const [metadataPanelCollapsed, setMetadataPanelCollapsed] = useState(false);
  // Search filter state (owned here, passed down to GlobalViewMap)
  const [searchTerm, setSearchTerm] = useState('');
  const searchInputRef = useRef<HTMLInputElement>(null);
  // Ref to GlobalViewMap for spatial navigation
  const mapRef = useRef<GlobalViewMapHandle>(null);
  const [contextMenu, setContextMenu] = useState<ContextMenuState>({
    visible: false,
    x: 0,
    y: 0,
    projectPath: null,
  });
  const contextMenuRef = useRef<HTMLDivElement>(null);
  // State for zoom to system trigger (used by context menu "Zoom to System" action)
  const [zoomToSystem, setZoomToSystem] = useState<string | null>(null);

  useEffect(() => {
    loadRecentProjects();
  }, [loadRecentProjects]);

  // #201 (B1): the home screen is the app-open landing, so scan SpeleoDB lock state
  // here - one GET resolves every accessible project's holder. Populates the recents
  // lock badges and, if this user holds any locks, raises the resume/release prompt
  // (App owns the once-per-session guard). Best-effort + a no-op when logged out.
  const scanLocksOnOpen = useSpeleoStore((s) => s.scanLocksOnOpen);
  const remoteLocks = useSpeleoStore((s) => s.remoteLocks);
  const remoteProjects = useSpeleoStore((s) => s.remoteProjects);
  const remoteSummaries = useSpeleoStore((s) => s.remoteSummaries);
  const speleoEmail = useSpeleoStore((s) => s.session?.userEmail ?? null);
  const speleoLoggedIn = useSpeleoStore((s) => s.session?.loggedIn ?? false);

  // #218: pull + open a purely-remote SpeleoDB project from a GlobalView row, via the
  // shared flow. Errors surface as toasts; an Ariane (.tml-only) project routes to the
  // shared adopt confirm (rendered inline below). No onBeforeLoad: GlobalView is not a
  // dialog, so loadProject's LoadingState takes over the view directly.
  const { openRemote, adoptRemote } = useOpenRemoteProject({
    // [#258] See OpenFromSpeleoDbDialog: an Ariane entry chosen in the picker still
    // has to reach its confirmation, and the retry answers to nobody.
    onAdoptAfterPick: (summary) => setPendingAdopt(summary),
  });
  const [remoteBusy, setRemoteBusy] = useState(false);
  const [pendingAdopt, setPendingAdopt] = useState<SpeleoProjectSummary | null>(null);
  // Synchronous re-entrancy guard: `remoteBusy` state only reflects on the next render,
  // so a second row click (or a click on a different remote row) before then would slip
  // past the visual disable and issue a concurrent open. The ref blocks it immediately.
  const remoteBusyRef = useRef(false);
  const handleOpenRemote = useCallback(
    async (summary: SpeleoProjectSummary) => {
      if (remoteBusyRef.current) return;
      remoteBusyRef.current = true;
      setRemoteBusy(true);
      try {
        const outcome = await openRemote(summary);
        if (outcome.status === 'adopt') setPendingAdopt(summary);
        else if (outcome.status === 'error') {
          showToast(outcome.message, TOAST_KEYS.SPELEODB_OPEN_REMOTE_ERROR, 'error');
        }
      } finally {
        remoteBusyRef.current = false;
        setRemoteBusy(false);
      }
    },
    [openRemote, showToast],
  );
  const handleConfirmAdopt = useCallback(async () => {
    if (!pendingAdopt || remoteBusyRef.current) return;
    const summary = pendingAdopt;
    setPendingAdopt(null);
    remoteBusyRef.current = true;
    setRemoteBusy(true);
    try {
      const outcome = await adoptRemote(summary);
      if (outcome.status === 'error') showToast(outcome.message, TOAST_KEYS.SPELEODB_OPEN_REMOTE_ERROR, 'error');
    } finally {
      remoteBusyRef.current = false;
      setRemoteBusy(false);
    }
  }, [pendingAdopt, adoptRemote, showToast]);
  useEffect(() => {
    void scanLocksOnOpen();
  }, [scanLocksOnOpen]);

  // (The manual re-scan button's handler lives further down, next to the
  // local-state hook it also has to refresh - see handleRefreshLocks.)

  // Close on outside mousedown / external scroll / resize (shared hook, same as
  // Select/AppMenu). Escape is handled in the global-view keyboard handler.
  useDismissableLayer({
    open: contextMenu.visible,
    onDismiss: () => setContextMenu((prev) => ({ ...prev, visible: false })),
    contains: [contextMenuRef],
  });

  // Note: Context menu escape is handled in the main keyboard handler below

  // Access global view store for lazy loading and selection
  const { geometryData, geometryStates, selectedSystemIds } = useGlobalViewStore(
    useShallow((s) => ({
      geometryData: s.geometryData,
      geometryStates: s.geometryStates,
      selectedSystemIds: s.selectedSystemIds,
    })),
  );

  // Sync store selection to local state when map Ctrl+click changes store
  // This ensures keyboard shortcuts work with map multi-selection
  useEffect(() => {
    // Only sync when there's an actual change to avoid loops
    const storeSet = selectedSystemIds;
    const localSet = multiSelectedIds;

    // Check if sets are different
    const areDifferent = storeSet.size !== localSet.size ||
      [...storeSet].some(id => !localSet.has(id));

    if (areDifferent) {
      setMultiSelectedIds(new Set(storeSet));
      // Update primary selection based on store
      if (storeSet.size === 0) {
        // Store cleared - clear primary selection too
        setSelectedProjectId(null);
      } else if (!storeSet.has(selectedProjectId ?? '')) {
        // Primary selection not in store - update to first in store
        const firstId = [...storeSet][0];
        if (firstId) {
          setSelectedProjectId(firstId);
        }
      }
    }
  }, [selectedSystemIds]); // eslint-disable-line react-hooks/exhaustive-deps -- one-way store-to-local sync: the local selection is only compared against

  // Check if there are any GPS-positioned projects (null check only, no sentinel)
  const gpsProjectCount = useMemo(
    () => recentProjects.filter((p) => p.gps_center !== null).length,
    [recentProjects]
  );
  const hasGpsProjects = gpsProjectCount > 0;

  // Handle project selection (single click)
  const handleProjectSelect = useCallback((filePath: string, ctrlKey?: boolean) => {
    if (ctrlKey) {
      // Ctrl+click: toggle selection in multi-select mode. The SET is committed
      // through a pure functional updater so two clicks in one React batch can't
      // each read a stale `multiSelectedIds` and drop the other's change.
      const wasSelected = multiSelectedIds.has(filePath);
      setMultiSelectedIds((prev) => {
        const newSet = new Set(prev);
        if (newSet.has(filePath)) {
          newSet.delete(filePath);
        } else {
          newSet.add(filePath);
        }
        return newSet;
      });
      // Primary selection for the metadata panel - kept OUT of the updater above
      // so that updater stays pure (no setState side-effect; clean under
      // StrictMode's double-invoke). On an ADD, the clicked project becomes
      // primary; on a DESELECT, promote the last surviving member (or clear).
      // Derived from the pre-click set; a same-batch double-click would only skew
      // the primary pointer, which self-heals on the next selection sync.
      if (wasSelected) {
        const survivors = [...multiSelectedIds].filter((id) => id !== filePath);
        setSelectedProjectId(survivors.length > 0 ? survivors[survivors.length - 1]! : null);
      } else {
        setSelectedProjectId(filePath);
      }
    } else {
      // Regular click: single select, clear multi-select
      setSelectedProjectId(filePath);
      setMultiSelectedIds(new Set([filePath]));
    }
  }, [multiSelectedIds]);

  // Handle right-click context menu (from list view)
  const handleContextMenu = useCallback((e: MouseEvent, filePath: string) => {
    e.preventDefault();
    // If right-clicked project isn't selected, select it
    if (!multiSelectedIds.has(filePath)) {
      setSelectedProjectId(filePath);
      setMultiSelectedIds(new Set([filePath]));
    }
    setContextMenu({
      visible: true,
      x: e.clientX,
      y: e.clientY,
      projectPath: filePath,
    });
  }, [multiSelectedIds]);

  // Handle context menu from map (called with coordinates instead of MouseEvent)
  const handleMapContextMenu = useCallback((filePath: string, x: number, y: number) => {
    // If right-clicked project isn't selected, select it
    if (!multiSelectedIds.has(filePath)) {
      setSelectedProjectId(filePath);
      setMultiSelectedIds(new Set([filePath]));
    }
    setContextMenu({
      visible: true,
      x,
      y,
      projectPath: filePath,
    });
  }, [multiSelectedIds]);

  // Handle project open (double-click on map, list row, context menu, or from
  // metadata panel). Routed through the shared open/import router (#450) for
  // consistency and defense-in-depth - the SpeleoDB hand-back / unsaved-
  // changes / swap-confirm guards it applies are currently unreachable here
  // (all three setView('global') callers - useProject.ts, UnsavedChangesDialog.tsx
  // and TmlImportDialog.tsx's discard path - already close the project first, so
  // GlobalView never renders with one open; Ctrl+I's picker choice now leans on
  // that same invariant, see fileShortcutActions.ts), but sharing the router also
  // gets recents the `allow: 'project-only'`
  // extension gate below for free, which IS load-bearing: a persisted recents
  // entry is untrusted input and must not reach the import parsers.
  //
  // Two recents-adjacent opens deliberately stay outside this path:
  // - handleRetryLoading (below) re-drives `loadProject` directly on the SAME
  //   path a previous call already routed and validated; re-routing would gate
  //   nothing new.
  // - handlePublishToSpeleoDb (below) calls `loadProject` via an injected
  //   callback so the publish flow can await the load before opening its own
  //   dialog; the router's dialogs would fight that sequencing.
  const handleProjectOpen = useCallback(async (filePath: string) => {
    // Recents entries come from persisted config (a hand-edited or legacy
    // entry could carry any extension), so this restricts routing to `.stalx`
    // - it must not reach the import parsers, mirroring the OS-launch route's
    // `LAUNCHABLE_EXTENSIONS` allowlist (launch.rs). Every recents open call
    // site (map, list/table, context menu, keyboard shortcut) funnels through
    // this one function, so the gate lives here rather than at each call site.
    await routeOpenedFile(filePath, 'Open Failed', { allow: 'project-only' });
  }, []);

  // Source-tagged open variants for telemetry attribution (the recent list vs
  // the recent map are distinct entry points feeding the same loadProject).
  // Delegates to `openRecentEntry` (attributes, THEN routes with the
  // `.stalx`-only gate) rather than duplicating that ordering here - see
  // openRecentEntry.ts.
  const handleOpenFromList = useCallback((filePath: string) => {
    return openRecentEntry(filePath, 'recent-list');
  }, []);
  const handleOpenFromMap = useCallback((filePath: string) => {
    return openRecentEntry(filePath, 'recent-map');
  }, []);

  // Handle cancel during loading
  const handleCancelLoading = useCallback(async () => {
    await cancelLoading();
  }, [cancelLoading]);

  // Handle retry after error
  const handleRetryLoading = useCallback(async () => {
    if (loading.projectPath) {
      // Retry of a recent-projects open; attribute it the same way.
      setProjectActionSource('recent-list');
      await loadProject(loading.projectPath);
    }
  }, [loading.projectPath, loadProject]);

  // Handle dismiss error (reset loading state)
  const handleDismissError = useCallback(() => {
    resetLoading();
  }, [resetLoading]);

  // Handle remove from recent (from metadata panel)
  const handleRemoveFromRecent = useCallback(async (filePath: string) => {
    await removeFromRecentProjects(filePath);
    // Clear selection if the removed project was selected
    if (selectedProjectId === filePath) {
      setSelectedProjectId(null);
    }
    // Remove from multi-selection
    setMultiSelectedIds((prev) => {
      const newSet = new Set(prev);
      newSet.delete(filePath);
      return newSet;
    });
  }, [removeFromRecentProjects, selectedProjectId]);

  // Prune selection when a project leaves the recents list (e.g. after a delete,
  // or any external removal): otherwise selectedProjectId / multiSelectedIds dangle
  // on a path that's gone, and Enter/Delete would act on it. The valid set is the
  // loaded recents plus config "missing" paths, so selecting a missing entry sticks.
  useEffect(() => {
    const valid = new Set<string>([
      ...recentProjects.map((p) => p.file_path),
      ...(config?.general?.recent_projects ?? []),
    ]);
    setSelectedProjectId((prev) => (prev && !valid.has(prev) ? null : prev));
    setMultiSelectedIds((prev) => {
      let changed = false;
      const next = new Set<string>();
      for (const id of prev) {
        if (valid.has(id)) next.add(id);
        else changed = true;
      }
      return changed ? next : prev;
    });
    // Also prune the map-selection store. The store→local sync effect above
    // copies selectedSystemIds back into multiSelectedIds on the next map
    // ctrl-click, so a deleted path left in the store gets resurrected into the
    // selection (status-bar count + Enter/Delete then point at a gone file).
    // Read fresh via getState() (not the render-closure value, which isn't a
    // dep here) and only write when something was actually removed.
    const gv = useGlobalViewStore.getState();
    const prunedSystems = new Set([...gv.selectedSystemIds].filter((id) => valid.has(id)));
    if (prunedSystems.size !== gv.selectedSystemIds.size) {
      gv.setSelectedSystems(prunedSystems);
    }
  }, [recentProjects, config]);

  // Register GlobalView keyboard shortcuts via hook
  useGlobalViewShortcuts({
    selectedProjectId,
    multiSelectedIds,
    activeTab,
    recentProjects,
    loading,
    contextMenu,
    mapRef,
    setSelectedProjectId,
    setMultiSelectedIds,
    setContextMenu,
    handleProjectOpen,
    handleRemoveFromRecent,
  });

  // Handle merge projects
  const handleMergeProjects = useCallback(() => {
    setContextMenu((prev) => ({ ...prev, visible: false }));
    const selectedProjects: ProjectMetadata[] = recentProjects.filter((p) =>
      multiSelectedIds.has(p.file_path)
    );
    if (selectedProjects.length >= 2) {
      openDialog('merge-projects', { selectedProjects });
    }
  }, [multiSelectedIds, recentProjects, openDialog]);

  // Switch between map and list views. Search is list-only, so clear the term
  // when entering the map (the map shows all projects unfiltered).
  const handleSwitchTab = useCallback((tab: ViewTab) => {
    setActiveTab(tab);
    if (tab === 'map') {
      setSearchTerm('');
    }
  }, []);

  // Handle zoom to system (context menu action)
  const handleZoomToSystem = useCallback(() => {
    if (contextMenu.projectPath) {
      // Toggle to map tab if not already there
      if (activeTab !== 'map') {
        handleSwitchTab('map');
      }
      // Trigger zoom - use a counter pattern to ensure re-trigger even for same path
      setZoomToSystem(contextMenu.projectPath);
    }
    setContextMenu((prev) => ({ ...prev, visible: false }));
  }, [contextMenu.projectPath, activeTab, handleSwitchTab]);

  // Callback for GlobalViewMap to signal zoom was handled
  const handleZoomHandled = useCallback(() => {
    setZoomToSystem(null);
  }, []);

  // Handle show in folder (context menu action)
  const handleShowInFolder = useCallback(async () => {
    if (contextMenu.projectPath) {
      try {
        await revealItemInDir(contextMenu.projectPath);
      } catch (err) {
        console.error('Failed to reveal in folder:', err);
      }
    }
    setContextMenu((prev) => ({ ...prev, visible: false }));
  }, [contextMenu.projectPath]);

  // Publish a LOCAL recent to SpeleoDB (context-menu twin of the in-project
  // hamburger item). The backend publishes the OPEN project and the dialog
  // prefills from it at mount, so this OPENS the project first and only raises
  // the dialog once that load has settled - see publishRecentToSpeleoDb.ts. A
  // failed open raises nothing: loadProject already surfaced the failure (the
  // loading-error panel / permission / corruption dialog), exactly as the plain
  // context-menu Open does.
  const handlePublishToSpeleoDb = useCallback(async () => {
    const path = contextMenu.projectPath;
    setContextMenu((prev) => ({ ...prev, visible: false }));
    if (!path) return;
    await publishRecentToSpeleoDb(path, {
      loadProject,
      openDialog,
      markSource: () => setProjectActionSource('context-menu'),
    });
  }, [contextMenu.projectPath, loadProject, openDialog]);

  // Handle search input changes (instant filtering)
  const handleSearchChange = useCallback((e: ChangeEvent<HTMLInputElement>) => {
    setSearchTerm(e.target.value);
  }, []);

  // Handle clearing search (X button)
  const handleClearSearch = useCallback(() => {
    setSearchTerm('');
    searchInputRef.current?.focus();
  }, []);

  // Handle search input keyboard events (Escape to clear)
  const handleSearchKeyDown = useCallback((e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Escape') {
      setSearchTerm('');
      // Keep focus on input after clearing
    }
  }, []);

  // Get the selected project metadata
  const selectedProject = useMemo(() => {
    if (!selectedProjectId) return null;
    return recentProjects.find(p => p.file_path === selectedProjectId) || null;
  }, [selectedProjectId, recentProjects]);

  // Selected projects array for multi-selection support. Declared before the
  // latch effect below so it can treat a surviving multi-selection as a live
  // selection - the MetadataPanel renders the aggregate from this array even when
  // there is no primary selectedProject.
  const selectedProjects = useMemo(() => {
    return recentProjects.filter(p => multiSelectedIds.has(p.file_path));
  }, [recentProjects, multiSelectedIds]);

  // Latch gating the list-view details panel. It starts hidden and opens only
  // once a selection is confirmed; while a selection holds it STAYS open -
  // switching between projects just updates its content (never re-hide/flicker
  // it). The first reveal is gated so a double-click-to-open doesn't flash it:
  // immediate in the map view (no open-vs-select ambiguity), and debounced by the
  // double-click window in the list view. Clears only when NOTHING is selected
  // (no primary AND no multi-selection survivors - e.g. the selected project was
  // removed/pruned) so the list view never strands the empty "Select a project"
  // panel and the next selection is debounced again. (Also resets when GlobalView
  // remounts, e.g. after returning from a project.)
  const [detailsOpen, setDetailsOpen] = useState(false);
  useEffect(() => {
    if (!selectedProjectId && selectedProjects.length === 0) {
      setDetailsOpen(false); // nothing selected - re-hide and re-arm the debounce
      return;
    }
    if (detailsOpen) return; // already open - leave it; content follows the selection
    // Past here, something is selected (primary OR a surviving multi-selection),
    // so reveal the panel. A multi-selection with no primary (e.g. the prune
    // effect nulled the primary while other selections remain) still reveals -
    // the panel renders the aggregate from selectedProjects.
    if (activeTab === 'map') {
      setDetailsOpen(true);
      return;
    }
    const timer = window.setTimeout(() => setDetailsOpen(true), DOUBLE_CLICK_MS);
    return () => window.clearTimeout(timer);
  }, [selectedProjectId, selectedProjects.length, activeTab, detailsOpen]);

  // Get selected projects count
  const selectedCount = multiSelectedIds.size;

  // Recent-projects sort preference (persisted) and the sorted list for the
  // list view. The map view is intentionally unaffected by this.
  const recentSort = useRecentSort();
  const sortedProjects = useMemo(
    () => sortRecentProjects(recentProjects, recentSort.field, recentSort.direction),
    [recentProjects, recentSort.field, recentSort.direction]
  );

  // List view layout density (cards vs compact table), persisted.
  const [density, setDensity] = useRecentDensity();

  // Clicking a table column header sorts by it: toggle direction if already
  // active, otherwise switch fields with a sensible default direction.
  const handleHeaderSort = useCallback(
    (field: RecentSortField) => {
      if (recentSort.field === field) {
        recentSort.toggleDirection();
      } else {
        recentSort.setField(field);
        recentSort.setDirection(defaultDirectionForField(field));
      }
    },
    [recentSort]
  );

  // Recent paths the backend kept in config but couldn't load (file moved,
  // deleted, or unreadable) are surfaced as "unavailable" rows so the user can
  // see and remove them, rather than having them silently vanish.
  const recentPaths = config?.general.recent_projects ?? [];
  const missingPaths = useMemo(
    () => deriveMissingPaths(recentPaths, recentProjects),
    [recentPaths, recentProjects]
  );
  // #218: purely-remote SpeleoDB projects (accessible on the server, not in local
  // recents) - excluding any already linked to a local recent so they aren't listed
  // twice. Empty when logged out / before the scan.
  const remoteItems = useMemo(
    () => deriveRemoteOnlyItems(remoteSummaries, recentProjects),
    [remoteSummaries, recentProjects]
  );
  // Combined, then filtered by the search term (name / location / path).
  const listItems = useMemo(
    () => buildRecentListItems(sortedProjects, missingPaths, remoteItems),
    [sortedProjects, missingPaths, remoteItems]
  );
  const visibleItems = useMemo(
    () => filterRecentListItems(listItems, searchTerm),
    [listItems, searchTerm]
  );
  // #270: which of these projects hold work that never reached SpeleoDB. ONE local
  // IPC call for the whole list (never per row), keyed on the id SET of everything
  // this screen can list - deliberately NOT the search-filtered subset, so typing in
  // the box does not re-issue it. A failure yields no accents (see the hook).
  const localStateIds = useMemo(
    () => [
      ...recentProjects
        .map((p) => p.speleodb_project_id)
        .filter((id): id is string => typeof id === 'string' && id.length > 0),
      ...Object.keys(remoteSummaries),
    ],
    [recentProjects, remoteSummaries]
  );
  const { states: localStates, reload: reloadLocalStates } = useSpeleoLocalStates(localStateIds);

  // Manual SpeleoDB re-scan (the header refresh button, #201): re-reads lock state
  // for the recents badges. `prompt`-less, so it never re-raises the held-lock
  // dialog (that is reserved for login / app-open).
  //
  // The local (un-pushed) states are refreshed alongside the locks, or the button
  // would only be half a refresh: the accent that marks work existing solely on
  // this computer is LOCAL state, which the lock scan does not touch, and the hook
  // keys its fetch on the id SET - so over an unchanged recents list nothing
  // re-issued it and the accent stayed at whatever it was when GlobalView mounted.
  const [refreshingLocks, setRefreshingLocks] = useState(false);
  const handleRefreshLocks = useCallback(async () => {
    setRefreshingLocks(true);
    reloadLocalStates();
    try {
      await scanLocksOnOpen();
    } finally {
      setRefreshingLocks(false);
    }
  }, [scanLocksOnOpen, reloadLocalStates]);

  const isSearching = searchTerm.trim().length > 0;
  // Gate the tabs and content area on the combined count (loaded + missing) so
  // an all-missing recent list still renders its removable rows instead of the
  // EmptyState - otherwise the user can never clear those entries. (The metadata
  // panel stays gated on loaded projects, since missing rows aren't selectable.)
  const hasRecentItems = listItems.length > 0;

  // Progressive header condensation: when the grid header can't fit without
  // overflowing (it never wraps - the buttons are nowrap), drop text in a fixed
  // priority order to reclaim width:
  //   1. the "Stalagmight" brand
  //   2. the New Project / Open+Import button labels (icon only)
  //   3. the Map / List toggle labels (icon only)
  // Mirrors the SingleSystemView Toolbar's collapse (reset-on-resize + step-up in a
  // layout effect so every step settles before paint), but the fit test here is a
  // plain grid overflow rather than a centered-carousel collision.
  const HEADER_CONDENSE_MAX = 3;
  const [headerCondense, setHeaderCondense] = useState(0);
  const headerCondenseRef = useRef(0);
  const [headerMeasureWidth, setHeaderMeasureWidth] = useState(0);
  const headerRef = useRef<HTMLElement>(null);
  const setHeaderCondenseBoth = useCallback((n: number) => {
    headerCondenseRef.current = n;
    setHeaderCondense(n);
  }, []);

  // Re-run the fit on a window/panel resize.
  useEffect(() => {
    const header = headerRef.current;
    if (!header) return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (w != null) setHeaderMeasureWidth(w);
    });
    ro.observe(header);
    return () => ro.disconnect();
  }, []);

  // Reset to the full layout whenever the available width OR the natural content
  // width changes, so the step-up pass re-collapses from scratch (this is how the
  // header EXPANDS again when space is freed). Declared before the step-up effect.
  useLayoutEffect(() => {
    if (headerCondenseRef.current !== 0) setHeaderCondenseBoth(0);
  }, [
    setHeaderCondenseBoth,
    headerMeasureWidth,
    showToolbarIcons,
    hasRecentItems,
    speleoLoggedIn,
    activeTab,
    recentProjects.length,
    gpsProjectCount,
    hasGpsProjects,
    t,
  ]);

  // Step-up: while the header overflows its width, drop one more label (priority
  // order above). The guard skips the stale render right after a reset.
  useLayoutEffect(() => {
    if (headerCondense !== headerCondenseRef.current) return;
    const header = headerRef.current;
    // Not laid out / zero-size test env: skip until real geometry exists.
    if (!header || header.clientWidth === 0) return;
    const overflowing = header.scrollWidth > header.clientWidth + 1;
    if (overflowing && headerCondenseRef.current < HEADER_CONDENSE_MAX) {
      setHeaderCondenseBoth(headerCondenseRef.current + 1);
    }
  });

  const brandCondensed = headerCondense >= 1;
  const actionsCondensed = headerCondense >= 2;
  const tabsCondensed = headerCondense >= 3;

  // Hamburger menu contents. The New/Open/Settings/Debug/Quit rows are all
  // shared with the in-project menu (appMenuItems) - one door (#407), so the
  // two hamburgers cannot drift. There is deliberately NO Import row here
  // (#508): with no project open there is nothing to import INTO, so Import
  // and Open would be two rows for one behavior - "Open / Import" above already
  // accepts every importable format and routes it by extension. Import earns
  // its own row only in-project, where it means the one thing Open does not do
  // (add mNemo data to the open project). Memoized so AppMenu's items-dependent
  // callbacks don't churn on every keystroke-driven re-render.
  const menuItems: AppMenuItem[] = useMemo(
    () => [
      ...leadingMenuItems(t, { onNewProject: handleNewProject }, 'hamburger'),
      // "Open from SpeleoDB" needs a signed-in session (the browser lists the
      // caller's remote projects), so hide it when logged out.
      ...(speleoLoggedIn
        ? [openFromSpeleoDbMenuItem(t, { onOpen: () => openDialog('open-from-speleodb') })]
        : []),
      { id: 'app-menu-sep-1', label: '', separator: true },
      ...serviceMenuItems(t, { onSettings: () => openDialog('settings'), onDebug: () => openDialog('debug') }),
      { id: 'app-menu-sep-2', label: '', separator: true },
      quitMenuItem(t),
    ],
    [t, handleNewProject, openDialog, speleoLoggedIn],
  );

  return (
    <div data-testid="global-view" className="h-full flex flex-col">
      {/* Header - left: menu + primary actions, center: title, right: Map|List toggle */}
      <header ref={headerRef} className="grid grid-cols-[1fr_auto_1fr] items-center px-4 py-3 border-b border-border">
        <div className="flex items-center gap-3 justify-self-start">
          <AppMenu items={menuItems} aria-label={t('globalView.menu.aria')} />
          <Button
            data-testid="new-project-button"
            className="whitespace-nowrap"
            onClick={() => { setProjectActionSource('toolbar'); handleNewProject(); }}
            // The icon is forced on when condensed (icon-only) so the button is never empty.
            icon={showToolbarIcons || actionsCondensed ? <span style={{ color: MENU_ICON_COLORS.new }}><FilePlusIcon size={16} /></span> : undefined}
            aria-label={actionsCondensed ? t('globalView.actions.newProject') : undefined}
            title={actionsCondensed ? t('globalView.actions.newProject') : undefined}
          >
            {actionsCondensed ? null : t('globalView.actions.newProject')}
          </Button>
          <Button
            data-testid="global-open-import-button"
            className="whitespace-nowrap"
            onClick={() => { setProjectActionSource('toolbar'); handleOpenOrImport(); }}
            icon={showToolbarIcons || actionsCondensed ? <span style={{ color: MENU_ICON_COLORS.open }}><FolderOpenIcon size={16} /></span> : undefined}
            aria-label={actionsCondensed ? t('globalView.actions.openOrImport') : undefined}
            title={actionsCondensed ? t('globalView.actions.openOrImport') : undefined}
          >
            {actionsCondensed ? null : t('globalView.actions.openOrImport')}
          </Button>
        </div>

        {/* Keep the <h1> element (grid needs its 3rd/center child to stay aligned)
            but drop its text at the first condensation stage. */}
        <h1 className="text-xl font-bold justify-self-center whitespace-nowrap">
          {brandCondensed ? '' : 'Stalagmight'}
        </h1>

        <div className="flex items-center gap-2 justify-self-end">
          {hasRecentItems && (
            <div
              className="inline-flex rounded border border-border overflow-hidden"
              role="group"
              aria-label={t('globalView.viewToggle.aria')}
            >
              <button
                type="button"
                aria-pressed={activeTab === 'map'}
                aria-label={tabsCondensed ? t('globalView.tabs.map') : undefined}
                title={tabsCondensed ? t('globalView.tabs.map') : undefined}
                data-testid="globalview-view-toggle-map"
                onClick={() => handleSwitchTab('map')}
                className={`flex items-center gap-1.5 whitespace-nowrap px-3 py-1.5 text-sm font-medium transition-colors ${
                  activeTab === 'map'
                    ? 'bg-accent text-accent-on'
                    : 'text-text-secondary hover:text-text hover:bg-panel'
                }`}
              >
                {(showToolbarIcons || tabsCondensed) && <MapIcon size={16} />}
                {!tabsCondensed && (
                  <>
                    {t('globalView.tabs.map')}
                    {hasGpsProjects ? ` (${formatNumber(gpsProjectCount)})` : ''}
                  </>
                )}
              </button>
              <button
                type="button"
                aria-pressed={activeTab === 'list'}
                aria-label={tabsCondensed ? t('globalView.tabs.list') : undefined}
                title={tabsCondensed ? t('globalView.tabs.list') : undefined}
                data-testid="globalview-view-toggle-list"
                onClick={() => handleSwitchTab('list')}
                className={`flex items-center gap-1.5 whitespace-nowrap px-3 py-1.5 text-sm font-medium border-l border-border transition-colors ${
                  activeTab === 'list'
                    ? 'bg-accent text-accent-on'
                    : 'text-text-secondary hover:text-text hover:bg-panel'
                }`}
              >
                {(showToolbarIcons || tabsCondensed) && <ListIcon size={16} />}
                {!tabsCondensed && <>{t('globalView.tabs.list')} ({formatNumber(recentProjects.length)})</>}
              </button>
            </div>
          )}
          {/* SpeleoDB re-scan, far right of the view toggle - only when logged in
              (#201). Refreshes the recents lock badges without re-prompting. */}
          {speleoLoggedIn && (
            <button
              type="button"
              onClick={() => void handleRefreshLocks()}
              disabled={refreshingLocks}
              title={t('globalView.speleoRefresh.title')}
              aria-label={t('globalView.speleoRefresh.aria')}
              data-testid="globalview-speleo-refresh"
              className="flex items-center justify-center w-8 h-8 rounded text-text-secondary hover:text-text hover:bg-panel transition-colors disabled:opacity-50"
            >
              <SpeleoBadgedIcon
                badge={<RefreshGlyph size={10} />}
                badgeColor="text-accent"
                size={18}
                testIdPrefix="globalview-speleo-refresh-icon"
              />
            </button>
          )}
        </div>
      </header>

      {/* Loading Modal Overlay */}
      {loading.isLoading && (
        <div className="fixed inset-0 bg-overlay flex items-center justify-center z-50">
          <LoadingProgress
            title={
              loading.error
                ? t('globalView.loading.failedTitle')
                : t('globalView.loading.loadingTitle', {
                  name: loading.projectName ?? t('globalView.loading.projectFallback'),
                })
            }
            phase={formatPhaseLabel(t, loading.phase, loading.phaseIndex, loading.phaseCount)}
            overallProgress={loading.overallProgress}
            onCancel={loading.error ? handleDismissError : handleCancelLoading}
            {...(loading.phaseProgress !== undefined && { phaseProgress: loading.phaseProgress })}
            {...(loading.phaseCurrent !== undefined && { phaseCurrent: loading.phaseCurrent })}
            {...(loading.phaseTotal !== undefined && { phaseTotal: loading.phaseTotal })}
            {...(loading.error ? { error: loading.error } : {})}
            {...(loading.error && loading.projectPath ? { onRetry: handleRetryLoading } : {})}
          />
        </div>
      )}

      {/* Content */}
      <main className="flex-1 overflow-hidden flex">
        {/* Main content area - disabled during loading */}
        <div className={`flex-1 overflow-hidden ${loading.isLoading ? 'pointer-events-none opacity-50' : ''}`}>
          {!hasRecentItems ? (
            <div className="h-full overflow-auto p-6">
              <EmptyState
                onOpenOrImport={() => { setProjectActionSource('empty-state'); handleOpenOrImport(); }}
                onNew={() => { setProjectActionSource('empty-state'); handleNewProject(); }}
              />
            </div>
          ) : activeTab === 'map' ? (
            // Search is list-only now; the map always shows all projects (searchTerm="").
            <GlobalViewMap
              ref={mapRef}
              projects={recentProjects}
              selectedProjectId={selectedProjectId}
              onProjectSelect={(path) => handleProjectSelect(path, false)}
              onProjectOpen={handleOpenFromMap}
              geometryData={geometryData}
              geometryStates={geometryStates}
              searchTerm=""
              onContextMenu={handleMapContextMenu}
              zoomToSystem={zoomToSystem}
              onZoomHandled={handleZoomHandled}
            />
          ) : (
            // Top padding lives on the inner content (not the scroll container) so
            // the table's sticky header pins flush to the top - container
            // padding-top would otherwise leave a gap the rows scroll through.
            <div className="h-full overflow-auto px-6 pb-6">
              <div className="flex items-center justify-between gap-4 mb-4 pt-6">
                <h2 className="text-lg font-medium">{t('globalView.recent.title')}</h2>
                <div className="flex items-center gap-3">
                  {selectedCount >= 2 && (
                    <Button
                      onClick={handleMergeProjects}
                      data-testid="merge-projects-button"
                      aria-label={t('globalView.actions.mergeProjectsAria', {
                        count: selectedCount,
                        countFormatted: formatNumber(selectedCount),
                      })}
                    >
                      {t('globalView.actions.mergeProjects', {
                        count: selectedCount,
                        countFormatted: formatNumber(selectedCount),
                      })}
                    </Button>
                  )}
                  <RecentSortControl
                    field={recentSort.field}
                    direction={recentSort.direction}
                    onFieldChange={recentSort.setField}
                    onToggleDirection={recentSort.toggleDirection}
                  />
                  <RecentDensityToggle density={density} onChange={setDensity} />
                </div>
              </div>
              <div className="flex items-start justify-between gap-4 mb-4" data-testid="globalview-list-status-row">
                <p className="text-xs text-text-secondary" data-testid="globalview-list-status">
                  {isSearching ? (
                    t('globalView.recent.countFiltered', {
                      shown: formatNumber(visibleItems.length),
                      total: formatNumber(listItems.length),
                    })
                  ) : (
                    <>
                      {t('globalView.recent.hintPrefix')}{' '}
                      {t('globalView.recent.hintFindPrefix')}{' '}
                      <button
                        type="button"
                        onClick={() => { setProjectActionSource('recent-list'); handleOpenOrImport(); }}
                        className="text-accent hover:underline"
                        data-testid="globalview-hint-open-link"
                      >
                        {t('globalView.recent.hintFindLink')}
                      </button>
                      {t('globalView.recent.hintFindSuffix')}
                    </>
                  )}
                </p>

                {/* Search - list-only; the map shows all projects. */}
                <div className="relative shrink-0">
                  <input
                    ref={searchInputRef}
                    type="text"
                    value={searchTerm}
                    onChange={handleSearchChange}
                    onKeyDown={handleSearchKeyDown}
                    placeholder={t('globalView.search.placeholder')}
                    className="bg-panel border border-border rounded px-2 py-1 text-sm w-48 pr-7 focus:outline-none focus-visible:ring-1 focus-visible:ring-accent"
                    data-testid="globalview-search-input"
                    aria-label={t('globalView.search.aria')}
                  />
                  {searchTerm && (
                    <button
                      onClick={handleClearSearch}
                      className="absolute right-1 top-1/2 -translate-y-1/2 text-text-secondary hover:text-text p-1"
                      title={t('globalView.search.clearTitle')}
                      aria-label={t('globalView.search.clearAria')}
                      data-testid="globalview-search-clear"
                    >
                      <CloseIcon size={14} />
                    </button>
                  )}
                </div>
              </div>

              {visibleItems.length === 0 ? (
                <div
                  className="flex flex-col items-center justify-center text-center py-16 text-text-secondary"
                  data-testid="globalview-list-empty"
                >
                  <p className="text-sm mb-2">
                    {t('globalView.recent.noResults', { term: searchTerm })}
                  </p>
                  <button
                    type="button"
                    onClick={handleClearSearch}
                    className="text-sm text-accent hover:underline"
                    data-testid="globalview-list-clear-search"
                  >
                    {t('globalView.search.clearTitle')}
                  </button>
                </div>
              ) : density === 'table' ? (
                <SystemTable
                  items={visibleItems}
                  selectedProjectId={selectedProjectId}
                  multiSelectedIds={multiSelectedIds}
                  sortField={recentSort.field}
                  sortDirection={recentSort.direction}
                  remoteLocks={remoteLocks}
                  remoteProjects={remoteProjects}
                  myEmail={speleoEmail}
                  localStates={localStates}
                  onSort={handleHeaderSort}
                  onSelect={handleProjectSelect}
                  onOpen={handleOpenFromList}
                  onContextMenu={handleContextMenu}
                  onRemoveMissing={handleRemoveFromRecent}
                  onOpenRemote={(summary) => void handleOpenRemote(summary)}
                  busy={remoteBusy}
                />
              ) : (
                <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
                  {visibleItems.map((item) =>
                    item.kind === 'available' ? (
                      <SystemCard
                        key={item.filePath}
                        name={item.project.name}
                        location={item.project.location}
                        filePath={item.filePath}
                        modifiedAt={item.project.modified_at}
                        sectionCount={item.project.section_count}
                        totalLength={item.project.total_length}
                        isSelected={multiSelectedIds.has(item.filePath)}
                        speleoLock={resolveSpeleoLock(item.project, remoteLocks, speleoEmail, remoteProjects, localStates)}
                        onSelect={(ctrlKey) => handleProjectSelect(item.filePath, ctrlKey)}
                        onContextMenu={(e) => handleContextMenu(e, item.filePath)}
                        onOpen={handleOpenFromList}
                      />
                    ) : item.kind === 'remote' ? (
                      <RemoteSystemCard
                        key={`remote-${item.projectId}`}
                        summary={item.summary}
                        lock={resolveSpeleoLock(
                          { is_remote: true, speleodb_project_id: item.projectId },
                          remoteLocks,
                          speleoEmail,
                          remoteProjects,
                          localStates,
                        )}
                        busy={remoteBusy}
                        onOpen={(summary) => void handleOpenRemote(summary)}
                      />
                    ) : (
                      <MissingSystemCard
                        key={item.filePath}
                        name={item.name}
                        filePath={item.filePath}
                        onRemove={handleRemoveFromRecent}
                      />
                    )
                  )}
                </div>
              )}
            </div>
          )}
        </div>

        {/* Metadata Panel. Map view: always shown (lives alongside the map), incl.
            the empty "Select a project" state. List view: hidden until the first
            selection is confirmed past the double-click window (so it doesn't flash
            on a double-click-to-open), then it follows the selection - and requires
            a live selection (primary OR a surviving multi-selection), so it never
            shows the empty state in the list. */}
        {recentProjects.length > 0 && (activeTab === 'map' || (detailsOpen && (selectedProject !== null || selectedProjects.length > 0))) && (
          <MetadataPanel
            project={selectedProject}
            selectedProjects={selectedProjects}
            onOpenProject={handleOpenFromList}
            onRemoveFromRecent={handleRemoveFromRecent}
            collapsed={metadataPanelCollapsed}
            onCollapseChange={setMetadataPanelCollapsed}
          />
        )}
      </main>

      {/* Status bar */}
      <footer className="px-4 py-2 border-t border-border text-sm text-text-secondary flex justify-between">
        <span>
          {t('globalView.status.systems', {
            count: recentProjects.length,
            countFormatted: formatNumber(recentProjects.length),
          })}
          {selectedCount > 1 && ` | ${t('globalView.status.selected', {
            count: selectedCount,
            countFormatted: formatNumber(selectedCount),
          })}`}
        </span>
        <span>{t('globalView.status.ready')}</span>
      </footer>

      {/* Context Menu */}
      {contextMenu.visible && (
        <div
          ref={contextMenuRef}
          className="fixed z-50 bg-panel border border-border rounded-lg shadow-xl py-1 min-w-[180px]"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          role="menu"
          aria-label={t('globalView.contextMenu.aria')}
          data-testid="project-context-menu"
        >
          <button
            className="w-full px-4 py-2 text-left text-sm hover:bg-background-50 flex items-center gap-2"
            onClick={() => {
              if (contextMenu.projectPath) {
                setProjectActionSource('context-menu');
                handleProjectOpen(contextMenu.projectPath);
              }
              setContextMenu((prev) => ({ ...prev, visible: false }));
            }}
            role="menuitem"
            data-testid="context-menu-open"
          >
            {t('globalView.contextMenu.open')}
          </button>
          <button
            className="w-full px-4 py-2 text-left text-sm hover:bg-background-50 flex items-center gap-2"
            onClick={handleZoomToSystem}
            role="menuitem"
            data-testid="context-menu-zoom"
          >
            {t('globalView.contextMenu.zoom')}
          </button>
          <button
            className="w-full px-4 py-2 text-left text-sm hover:bg-background-50 flex items-center gap-2"
            onClick={handleShowInFolder}
            role="menuitem"
            data-testid="context-menu-show-in-folder"
          >
            {t('globalView.contextMenu.showInFolder')}
          </button>
          {/* Publish to SpeleoDB - only for a LOCAL recent, only while signed in,
              only on a single selection (publishing is per project). Same label
              and same dialog as the in-project hamburger item. */}
          {canPublishRecent(
            recentProjects.find((p) => p.file_path === contextMenu.projectPath),
            speleoLoggedIn,
            selectedCount,
          ) && (
            <button
              className="w-full px-4 py-2 text-left text-sm hover:bg-background-50 flex items-center gap-2"
              onClick={() => { void handlePublishToSpeleoDb(); }}
              role="menuitem"
              data-testid="context-menu-publish-speleodb"
            >
              {t('speleodb.publish.menuItem')}
            </button>
          )}
          {selectedCount >= 2 && (
            <button
              className="w-full px-4 py-2 text-left text-sm hover:bg-background-50 flex items-center gap-2"
              onClick={handleMergeProjects}
              role="menuitem"
              data-testid="context-menu-merge"
            >
              {t('globalView.contextMenu.mergeProjects', {
                count: selectedCount,
                countFormatted: formatNumber(selectedCount),
              })}
            </button>
          )}
          <div className="border-t border-border my-1" />
          <button
            className="w-full px-4 py-2 text-left text-sm hover:bg-background-50 flex items-center gap-2"
            onClick={async () => {
              // Handle multi-selection remove with count
              try {
                if (selectedCount > 1) {
                  // Remove all selected projects sequentially
                  for (const id of multiSelectedIds) {
                    await handleRemoveFromRecent(id);
                  }
                } else if (contextMenu.projectPath) {
                  await handleRemoveFromRecent(contextMenu.projectPath);
                }
              } catch (err) {
                console.error('Failed to remove from recent:', err);
              }
              setContextMenu((prev) => ({ ...prev, visible: false }));
            }}
            role="menuitem"
            data-testid="context-menu-remove"
          >
            {selectedCount > 1
              ? t('globalView.contextMenu.removeMultiple', {
                count: selectedCount,
                countFormatted: formatNumber(selectedCount),
              })
              : t('globalView.contextMenu.remove')}
          </button>
          {selectedCount <= 1 && contextMenu.projectPath && (() => {
            // A remote (SpeleoDB) recent is only a local cache copy, so both the label
            // and the confirm dialog frame the delete as "local copy" (matches the
            // details-panel button).
            const ctxProj = recentProjects.find((p) => p.file_path === contextMenu.projectPath);
            return (
            <>
              <div className="border-t border-border my-1" />
              <button
                className="w-full px-4 py-2 text-left text-sm hover:bg-background-50 text-error flex items-center gap-2"
                onClick={() => {
                  openDialog('delete-project', {
                    path: contextMenu.projectPath,
                    name: ctxProj?.name,
                    isRemote: ctxProj?.is_remote,
                  });
                  setContextMenu((prev) => ({ ...prev, visible: false }));
                }}
                role="menuitem"
                data-testid="context-menu-delete"
              >
                {t(ctxProj?.is_remote ? 'globalView.contextMenu.deleteLocalCopy' : 'globalView.contextMenu.delete')}
              </button>
            </>
            );
          })()}
        </div>
      )}

      {/* #218: an Ariane (.tml-only) remote project routes Open to the shared adopt
          confirm; confirming pulls + adopts it via the same flow as the browser. */}
      {pendingAdopt && (
        <SpeleoAdoptConfirm
          projectName={pendingAdopt.name}
          busy={remoteBusy}
          onCancel={() => setPendingAdopt(null)}
          onConfirm={() => void handleConfirmAdopt()}
        />
      )}
    </div>
  );
}

interface EmptyStateProps {
  onOpenOrImport: () => void;
  onNew: () => void;
}

/**
 * Every format chip's label is derived from the canonical extension lists in
 * `fileDialogs.ts` (`PROJECT_EXTENSION` / `IMPORT_EXTENSIONS`), never a
 * hand-typed string, so a new extension added to one of those groups shows up
 * here automatically instead of silently missing from the welcome screen. The
 * per-group arrays are reversed only for the display order the approved mock
 * asks for (".dat / .mak", ".srv / .wpj") - the underlying extension set is
 * untouched.
 */
const FORMAT_CHIPS: { id: string; label: string; native?: boolean }[] = [
  { id: 'stalx', label: `.${PROJECT_EXTENSION}`, native: true },
  ...IMPORT_EXTENSIONS.tml.map((ext) => ({ id: ext, label: `.${ext}` })),
  ...IMPORT_EXTENSIONS.dmp.map((ext) => ({ id: ext, label: `.${ext}` })),
  { id: 'compass', label: [...IMPORT_EXTENSIONS.compass].reverse().map((ext) => `.${ext}`).join(' / ') },
  { id: 'walls', label: [...IMPORT_EXTENSIONS.walls].reverse().map((ext) => `.${ext}`).join(' / ') },
  ...IMPORT_EXTENSIONS.gpx.map((ext) => ({ id: ext, label: `.${ext}` })),
];

function EmptyState({ onOpenOrImport, onNew }: EmptyStateProps) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col items-center justify-center h-full gap-6">
      <div className="text-center max-w-lg">
        <h2 className="text-2xl font-bold mb-2">{t('globalView.empty.title')}</h2>
        <p className="text-text-secondary">{t('globalView.empty.description')}</p>
      </div>

      <button
        type="button"
        onClick={onOpenOrImport}
        data-testid="empty-state-open-project"
        className="w-full max-w-xl flex flex-col items-center gap-4 rounded-lg border-2 border-dashed border-border p-10 text-center transition-colors hover:border-accent hover:bg-accent-10"
      >
        <DownloadIcon size={40} />
        <span className="text-lg font-semibold">{t('globalView.empty.dropZone.cta')}</span>
        <div className="flex flex-wrap items-center justify-center gap-2">
          {FORMAT_CHIPS.map((chip) => (
            <span
              key={chip.id}
              data-testid={`empty-state-format-chip-${chip.id}`}
              className={`rounded-full border px-2.5 py-1 text-xs font-medium ${
                chip.native
                  ? 'border-accent bg-accent-10 text-accent'
                  : 'border-border bg-panel text-text-secondary'
              }`}
            >
              {chip.label}
            </span>
          ))}
        </div>
        <p className="max-w-md text-xs text-text-secondary">
          {t('globalView.empty.dropZone.promise')}
        </p>
      </button>

      <button
        type="button"
        onClick={onNew}
        data-testid="empty-state-new-project"
        className="flex items-center gap-2 text-sm font-medium text-text-secondary transition-colors hover:text-text"
      >
        <FilePlusIcon size={16} />
        {t('globalView.empty.newProject')}
      </button>
    </div>
  );
}
