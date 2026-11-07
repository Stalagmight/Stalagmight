import { useEffect, useCallback, useState, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { getCurrentWebview, type DragDropEvent } from '@tauri-apps/api/webview';
import { useProjectStore, selectNewerFileReadOnly } from './store/projectStore';
import { LoadRepairsBanner } from './components/common/LoadRepairsBanner';
import { InternalErrorBanner } from './components/common/InternalErrorBanner';
import { useInternalErrorEvent } from './hooks/useInternalErrorEvent';
import { SETTABLE_UNREADABLE_SETTINGS } from './store/unreadableSettings';
import { isMoveCarryRefusal } from './store/moveCarry';
import { useUIStore } from './store/uiStore';
import { useSpeleoStore } from './store/speleoStore';
import { useGasPlanStore, selectIsPlanDirty } from './store/gasPlanStore';
import { useShallow } from 'zustand/react/shallow';
import { invokeIpc, getErrorMessage } from './utils/ipc';
import { isEditGateCode } from './utils/editErrors';
import { errorCode } from './utils/commandError';
import { useEditBlockedReason, editBlockedReasonNow } from './hooks/useEditingBlocked';
import {
  saveFailureOf,
  surfaceSaveFailure,
  shouldShowAutosaveBanner,
  completeImportToSingleView,
  finishImportWithoutOrphanSuggestions,
} from './utils/saveFeedback';
import { extractFilename } from './utils/path';
import { routeOpenedFile } from './utils/openOrImport';
import { useLaunchWithFile } from './utils/launchWithFile';
import { guardQuit } from './utils/quit';
import { isDroppableFilePath } from './utils/fileDialogs';
import { useBeforeUnload, useLoadingProgress, useSpeleoPrePushSave, useTheme, useSyntacticHighlighting, useNetworkStatus, useUiScale } from './hooks';
import { useOnboarding } from './hooks/useOnboarding';
import { registerOnboardingRestart } from './hooks/useOnboardingRestart';
import { ORIENTATION_FLOW_ID } from './onboarding/flows';
import i18n from './i18n';
import { setNumberFormatLocale, setDateFormatLocale } from './i18n/format';
import GlobalView from './components/GlobalView/GlobalView';
import SingleSystemView from './components/SingleSystemView/SingleSystemView';
import SplashScreen from './components/SplashScreen/SplashScreen';
import OnboardingOverlay from './components/Onboarding/OnboardingOverlay';
import { ImportDialog, ExportDialog, NewProjectDialog, NewProjectForImportDialog, DmpSelectionDialog, DmpNewImportDialog, ZeroDatumSelectionDialog, ZeroDatumConfigDialog, TmlImportDialog, CompassImportDialog, WallsImportDialog, GpxSelectionDialog, GpxPlacementDialog, ConfirmDialog, ReverseSectionDialog, DeleteShotChoiceDialog, BulkDeleteDialog, MergeDialog, ClosureDialog, MergeProjectsDialog, DrawShotDialog, EditSectionDialog, EditShotDialog, EditStationDialog, UnmergeStationsDialog, RemoveClosureDialog, UnsavedChangesDialog, ErrorDialog, ImportBlockedDialog, SavePermissionDialog, OverwriteConfirmDialog, DeleteProjectDialog, SectionAdjustDialog, SettingsDialog, ProjectSettingsDialog, SplitSectionDialog, InsertStationDialog, CorruptionRecoveryDialog, DebugDialog, SwapCommentsDialog, OrphanPlacementDialog, KeyboardShortcutsDialog, TelemetryOptInDialog, OpenFromSpeleoDbDialog, PublishToSpeleoDbDialog, SpeleoDoneDialog, SpeleoDivergenceDialog, SpeleoForceEditDialog, SpeleoRestoreDialog, SpeleoUnwindDialog, SpeleoUnwoundPushDialog, SpeleoDiscardChangesDialog, SpeleoRevertDialog, SpeleoCloseDialog, SpeleoQuitDialog, SpeleoNoPermissionDialog, HeldLockResumeDialog, UnrecognisedShotKindDialog, SpeleoUnpushedLocalDialog, SpeleoManagedFileDialog, SpeleoDbProjectDialog, SpeleoStorageChoiceDialog, SpeleoDumpPickerDialog, ImportReceiptDialog, ImportAsNewProjectDialog, LaunchOpenConfirmDialog, RewrittenPathOverwritePrompt } from './components/Dialogs';
import { initTelemetry, flushTelemetry, setProjectActionSource } from './utils/telemetry';
import { ContextMenu, ToastStack } from './components/common';
import { MemoryProbe } from './components/MemoryProbe/MemoryProbe';
import type { DmpSelectionConfig } from './components/Dialogs/DmpSelectionDialog';
import type { ZeroDatumConfigResult } from './components/Dialogs/ZeroDatumConfigDialog';
import type { OrphanPlacementSuggestion } from './types/commands';
import { TOAST_KEYS } from './utils/toastKeys';
import { ZERO_DATUM_CARRY_KEY, readCarriedZeroDatumConfig, carryZeroDatumConfig } from './utils/zeroDatumCarry';

function App() {
  const { t } = useTranslation();
  const { project, stations, loading, reloadConfigInOrder, config, isDirty, setConfig } = useProjectStore(
    useShallow((s) => ({
      project: s.project,
      stations: s.stations,
      loading: s.loading,
      reloadConfigInOrder: s.reloadConfigInOrder,
      config: s.config,
      isDirty: s.isDirty,
      setConfig: s.setConfig,
    })),
  );
  const integrityIssues = useProjectStore((s) => s.integrityIssues);
  // #576 D39(a): why Repair (a survey mutation) is unavailable right now, if it is.
  const repairBlocked = useEditBlockedReason();
  const integrityDismissed = useProjectStore((s) => s.integrityDismissed);
  const integritySaveBlocked = useProjectStore((s) => s.integritySaveBlocked);
  const checkProjectIntegrity = useProjectStore((s) => s.checkProjectIntegrity);
  const repairProjectIntegrity = useProjectStore((s) => s.repairProjectIntegrity);
  const dismissIntegrityIssues = useProjectStore((s) => s.dismissIntegrityIssues);
  const integrityDialogOpen = useProjectStore((s) => s.integrityDialogOpen);
  const openIntegrityDialog = useProjectStore((s) => s.openIntegrityDialog);
  const closeIntegrityDialog = useProjectStore((s) => s.closeIntegrityDialog);
  const historyDiscarded = useProjectStore((s) => s.historyDiscarded);
  const historyDiscardedDismissed = useProjectStore((s) => s.historyDiscardedDismissed);
  const checkHistoryDiscarded = useProjectStore((s) => s.checkHistoryDiscarded);
  const dismissHistoryDiscarded = useProjectStore((s) => s.dismissHistoryDiscarded);
  const newerFileReadOnly = useProjectStore(selectNewerFileReadOnly);
  const speleoLinkUnreadable = useProjectStore((s) => s.speleoLinkUnreadable);
  const unreadableProjectSettings = useProjectStore((s) => s.unreadableProjectSettings);
  const autosaveFailure = useProjectStore((s) => s.autosaveFailure);
  const autosaveFailureKind = useProjectStore((s) => s.autosaveFailureKind);
  const clearAutosaveFailure = useProjectStore((s) => s.clearAutosaveFailure);
  const saveProject = useProjectStore((s) => s.saveProject);
  const [repairingIntegrity, setRepairingIntegrity] = useState(false);
  const {
    view,
    setView,
    activeDialog,
    dialogData,
    dmpImportWizardPhase,
    placementMode,
    closeDialog,
    openDialog,
    initPanels,
    triggerFitToBounds,
    selectionAnnouncement,
  } = useUIStore(useShallow((s) => ({
    view: s.view,
    setView: s.setView,
    activeDialog: s.activeDialog,
    dialogData: s.dialogData,
    dmpImportWizardPhase: s.dmpImportWizardPhase,
    placementMode: s.placementMode,
    closeDialog: s.closeDialog,
    openDialog: s.openDialog,
    initPanels: s.initPanels,
    triggerFitToBounds: s.triggerFitToBounds,
    selectionAnnouncement: s.selectionAnnouncement,
  })));

  // #198/fix 3: whenever a SpeleoDB hand-off verb (start/update/push/done, from the
  // status button OR a close dialog) reports a divergence, surface the resolution
  // dialog from ONE place - so no entry point can leave the user on a crimson button
  // with no next step. App is the only always-mounted host (the status button
  // unmounts in Plan/Export). SpeleoDivergenceDialog clears `divergence` on
  // resolve/cancel.
  const speleoDivergence = useSpeleoStore((s) => s.divergence);
  useEffect(() => {
    // Fire only on the transition to a pending divergence - deps intentionally OMIT
    // `activeDialog`. Re-running on every active-dialog change let an unresolved
    // divergence re-open ON TOP of an unrelated dialog the user had since navigated
    // to (e.g. Settings), trapping them (N1). If they leave it unresolved the crimson
    // status button still routes them back. No re-open loop: `openDialog` doesn't
    // touch `divergence`, so this only runs when the divergence itself changes.
    if (speleoDivergence !== null) {
      openDialog('speleodb-divergence');
    }
  }, [speleoDivergence, openDialog]);

  // #201 (B1): the held-lock resume/release prompt is an INDEPENDENT overlay (see
  // `heldLockPromptOpen`), not the single `activeDialog` slot, so a login done from
  // inside another dialog (e.g. the Settings account panel) layers the prompt OVER
  // it instead of replacing it - the underlying dialog stays mounted and intact.
  // The store opens it only on prompt-worthy scans (login / first app-open with held
  // locks); routine badge refreshes never do.
  const heldLockPromptOpen = useSpeleoStore((s) => s.heldLockPromptOpen);

  // The credential-storage question is raised by the same event, from the same
  // place, and so gets the same treatment: an independent overlay, because the
  // login that raises it is usually made from inside the Settings account panel.
  const storageChoiceOpen = useSpeleoStore((s) => s.storageChoiceOpen);
  const dumpPickerOpen = useSpeleoStore((s) => s.dumpPickerOpen);

  // Apply theme from config (resolves auto mode, sets data-theme attribute)
  useTheme();

  // Apply syntactic-highlighting colors (sets --sh-* vars + data-sh-level)
  useSyntacticHighlighting();

  // Listen for loading progress events from backend
  useLoadingProgress();

  // [#576 RV4-R-S6] What a failed Done / Push's pre-push save could not keep
  useSpeleoPrePushSave();

  // Event-driven network status orchestrator (tile transport events + offline polling)
  useNetworkStatus();

  // [#576 A11] The backend's one-shot internal-error notice, for InternalErrorBanner
  useInternalErrorEvent();

  // Whole-UI zoom via Ctrl+= / Ctrl+- / Ctrl+0 (persisted)
  useUiScale();

  // Splash screen state - visible until config loads, then fades out and unmounts
  const [showSplash, setShowSplash] = useState(true);
  const [splashMounted, setSplashMounted] = useState(true);

  // Files the OS handed us (#424): double-click / "Open with" / a second launch
  // forwarded by the running instance. Routed through the same open/import
  // router as every other entry point, but not before config has loaded AND the
  // splash has actually finished unmounting - not just started fading. Readiness
  // tied to `config` alone fired while the splash was still on screen (it unmounts
  // on `onTransitionEnd`, one render tick later), so a cold launch with an argv
  // file could raise `launch-open-confirm` / an import dialog while the splash was
  // still visible (#445 review). `splashMounted` is the same signal onboarding's
  // auto-start already waits on (`splashVisible` below), for the same reason.
  useLaunchWithFile(config != null && !splashMounted);

  // Track whether we're hovering with a valid import file (DMP/TML/TMLU)
  const [isDraggingImport, setIsDraggingImport] = useState(false);

  // Determine if import dialog is open and has placed sections
  // This is used to show a confirmation when closing the app
  const isImportInProgress = useMemo(() => {
    // Also block during DMP selection if we have a parsed file
    if (activeDialog === 'dmp-selection' && dialogData.dmpFile) {
      return true;
    }
    // Block during the DMP placing wizard - it holds un-finalized, in-memory
    // imported/placed sections that an OS window-close would otherwise drop (#110).
    if (activeDialog === 'dmp-import-new' && dialogData.dmpFile) {
      return true;
    }
    // Block during GPX selection/placement dialog
    if ((activeDialog === 'gpx-selection' || activeDialog === 'gpx-placement') && dialogData.gpxFile) {
      return true;
    }
    // Block during new project for import dialog
    if (activeDialog === 'new-project-for-import' && (dialogData.dmpFile || dialogData.tmlFile || dialogData.gpxFile || dialogData.compassFile || dialogData.wallsFile)) {
      return true;
    }
    return false;
  }, [activeDialog, dialogData]);

  // Guard the app close during an import, OR with unsaved dive-plan edits (#244).
  // The gas store persists across mode switches now, so a dirty (loaded + diverged)
  // plan stays dirty in any mode - closing loses those edits regardless of the current
  // mode, so the close guard is correct without an appMode check.
  // The OS window X lands here as `tauri://close-requested`, so this is also the
  // only seam that can put it through the app's own exit guards (#270): without
  // `guardQuit` the X destroyed the window whenever neither of the two states
  // above was set, silently dropping unsaved-to-disk work and unpushed SpeleoDB
  // edits that the menu Quit has always prompted for. X and Quit now share one
  // guard.
  const planDirty = useGasPlanStore(selectIsPlanDirty);
  useBeforeUnload(
    isImportInProgress || planDirty,
    isImportInProgress
      ? 'Import is in progress. Discard all import data and close?'
      : 'You have unsaved changes to this dive plan. Discard and close?',
    undefined,
    guardQuit
  );

  useEffect(() => {
    // Load config on startup - in the config chain [#576 NF-W8FE-4], so a settings
    // save made while it is in flight cannot be overtaken by its answer.
    void reloadConfigInOrder();

    // Flush telemetry on page unload (best-effort)
    const handleUnload = () => { void flushTelemetry(); };
    window.addEventListener('beforeunload', handleUnload);
    return () => window.removeEventListener('beforeunload', handleUnload);
  }, [reloadConfigInOrder]);

  // Initialize telemetry once config is loaded (only collects if user opted in)
  useEffect(() => {
    if (config != null) {
      void initTelemetry(config.general.telemetry_enabled);
    }
  }, [config?.general.telemetry_enabled]);

  // After a project loads, check its structural integrity so corruption already
  // on disk (older versions / external edits) surfaces a warn + repair banner (#17).
  useEffect(() => {
    if (project?.file_path && !loading.isLoading) {
      void checkProjectIntegrity();
    }
  }, [project?.file_path, loading.isLoading, checkProjectIntegrity]);

  // [#473] ...and whether that load could read the project's edit history at all.
  // Same key as the integrity check, for the same reason: both describe the file
  // that just finished loading, and asking before it settles would describe the
  // previous one.
  //
  // The backend can also reopen a project WITHOUT a frontend load - the SpeleoDB
  // replace/pull/revision paths close and reopen internally - and neither dep here
  // changes when it does. Those routes are covered, but NOT all by this effect:
  //   - The adopt and update flows finish with a real `loadProject`
  //     (`useSpeleoActions.ts`), which toggles `isLoading` and re-fires this effect.
  //   - The in-place REVISION swap does not, and cannot: it reopens different bytes at
  //     the SAME path, and `loadProject` would short-circuit on the already-open
  //     project. It re-asks through `applySpeleoTreeReplacement` instead, which is the
  //     chokepoint every in-place tree replacement passes through.
  //   - The adopt reopen (`speleodb_pull.rs`, right after the staged file is renamed
  //     over the destination) is NEW bytes, not a rollback - it is covered because
  //     adoption ends in a full `loadProject`, not because the bytes are unchanged.
  //   - The ROLLBACK reopens are byte-identical and owe nothing, and there are FOUR of
  //     them, not one: `speleodb_cache.rs` x3 (after the staged download is deleted,
  //     `dest` never having been touched) and `speleodb_pull.rs` x2 - the post-promote
  //     arm that reopens `previous` when the adopt failed to open, and
  //     `adopt_abandon_staging`, which runs only before or instead of the promote. An
  //     earlier version of this comment said "only `speleodb_cache.rs`" and silently
  //     omitted the latter two, which is the same shape of enumeration error that let
  //     the `discardChanges` route go unnoticed.
  // A future path that reopens DIFFERENT bytes with no frontend load must re-ask, and
  // the tree-replacement seam is where it should do it.
  useEffect(() => {
    if (project?.file_path && !loading.isLoading) {
      void checkHistoryDiscarded();
    }
  }, [project?.file_path, loading.isLoading, checkHistoryDiscarded]);

  // Hide splash once config has loaded (or immediately if splash is disabled)
  useEffect(() => {
    if (config != null) {
      if (!config.general.splash_screen) {
        // Splash disabled: skip animation, unmount immediately
        setShowSplash(false);
        setSplashMounted(false);
      } else {
        setShowSplash(false);
      }
    }
  }, [config]);

  // Sync i18n language with config locale setting
  useEffect(() => {
    const locale = config?.general?.locale;
    if (!locale) return;
    if (locale === 'system') {
      void i18n.changeLanguage();
      return;
    }
    if (i18n.language !== locale) {
      void i18n.changeLanguage(locale);
    }
  }, [config?.general?.locale]);

  // Sync number format locale with config
  useEffect(() => {
    setNumberFormatLocale(config?.general?.number_format ?? 'locale');
  }, [config?.general?.number_format]);

  // Sync date format locale with config
  useEffect(() => {
    setDateFormatLocale(config?.general?.date_format ?? 'locale');
  }, [config?.general?.date_format]);

  // Initialize panel sizes from config ONCE at startup. Config reloads happen
  // live (saving any panel/view preference re-fetches it); re-initializing on
  // each would snap user-dragged panel sizes back to their persisted values.
  const panelSizesInitialized = useRef(false);
  useEffect(() => {
    if (config?.panels && !panelSizesInitialized.current) {
      panelSizesInitialized.current = true;
      initPanels(config.panels);
    }
  }, [config, initPanels]);

  // Update window title based on project state
  useEffect(() => {
    const updateTitle = async () => {
      let title = 'Stalagmight';
      if (project) {
        if (project.file_path) {
          // Extract filename from path
          const filename = extractFilename(project.file_path);
          title = `Stalagmight - ${filename}`;
        } else {
          title = `Stalagmight - ${project.name} (unsaved)`;
        }
        if (isDirty) {
          title = `${title} *`;
        }
      }
      try {
        await getCurrentWindow().setTitle(title);
      } catch {
        // Fallback for dev mode or if Tauri API unavailable
        document.title = title;
      }
    };
    updateTitle();
  }, [project, project?.file_path, project?.name, isDirty]);

  /**
   * Handle file drop events for importable files (DMP/TML/TMLU/Compass/GPX)
   * and `.stalx` projects. Delegates to the shared open/import router (same as
   * the toolbar Import button, Ctrl+I, and native file-open), which dispatches
   * by extension: a `.stalx` is routed as a plain OPEN (with its own
   * unsaved-changes/SpeleoDB guards), everything else as an import, applying
   * the read-only/closure-adjusted and "close the current project first"
   * guards.
   */
  const handleDroppedFile = useCallback(async (filePath: string) => {
    // Tag the entry point so the routed import command (import_sections /
    // create_project_full / execute*Import) is attributed to drag-drop, like the
    // toolbar / menu / keyboard paths all set their source. routeOpenedFile clears
    // it on every terminal/abandon path.
    setProjectActionSource('drag-drop');
    // 'Open Failed', not 'Import Failed': since F11 the drop route also covers
    // a dropped `.stalx` (a plain OPEN, not an import). Most `.stalx` open
    // failures are handled inside `loadProject` with their own dialog title,
    // but an unexpected throw falls into `routeOpenedFile`'s own catch and
    // uses this title - same neutral choice `launchWithFile.ts` already made
    // for the OS-launch channel, which faces the identical dual-purpose
    // ambiguity.
    await routeOpenedFile(filePath, 'Open Failed');
  }, []);

  // Set up drag-drop event listener for importable files (DMP/TML/TMLU/Compass/GPX)
  // and `.stalx` projects.
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    const setupDragDrop = async () => {
      try {
        const webview = getCurrentWebview();
        unlisten = await webview.onDragDropEvent((event) => {
          const payload = event.payload as DragDropEvent;

          if (payload.type === 'enter') {
            const hasDroppableFile = payload.paths?.some((path: string) => isDroppableFilePath(path));
            setIsDraggingImport(hasDroppableFile ?? false);
          } else if (payload.type === 'drop') {
            setIsDraggingImport(false);
            const droppedPath = payload.paths?.find((path: string) => isDroppableFilePath(path));
            if (droppedPath) {
              handleDroppedFile(droppedPath);
            }
          } else if (payload.type === 'leave') {
            setIsDraggingImport(false);
          }
        });
      } catch (e) {
        console.warn('Drag-drop not available:', e);
      }
    };

    setupDragDrop();

    return () => {
      if (unlisten) {
        unlisten();
      }
    };
  }, [handleDroppedFile]);

  const persistCompletedOnboardingFlows = useCallback((flowIds: string[]) => {
    const normalized = Array.from(
      new Set(flowIds.map((flowId) => flowId.trim()).filter((flowId) => flowId.length > 0))
    );
    const currentConfig = useProjectStore.getState().config;
    if (!currentConfig) {
      return;
    }

    const currentCompleted = currentConfig.general.onboarding_completed_flows ?? [];
    const unchanged =
      normalized.length === currentCompleted.length &&
      normalized.every((flowId, idx) => flowId === currentCompleted[idx]);
    if (unchanged) {
      return;
    }

    // [#576 D27-32(c)] This one setting, not the whole store snapshot.
    void useProjectStore
      .getState()
      .patchConfig([{ path: ['general', 'onboarding_completed_flows'], value: normalized }]);
  }, []);

  // The one-shot rule applies to the ORIENTATION tour only (#331). It used to
  // gate the shared `enabled` switch, which also suppressed the contextual
  // DMP/placement guides - so a surveyor who imported their first DMP in week
  // three never saw its guide. Those repeat-suppress per flow via
  // `onboarding_completed_flows` instead.
  //
  // The decision is held in session state so that burning the flag (below) does
  // not tear down the tour currently on screen, and is re-taken when "Restart
  // onboarding" clears the flag mid-session.
  const [orientationConsumed, setOrientationConsumed] = useState(false);
  const orientationDecidedRef = useRef(false);
  const firstLaunchBurnedRef = useRef(false);
  useEffect(() => {
    if (!config) {
      return;
    }
    if (!config.general.onboarding_first_launch_done) {
      // Fresh install, or re-armed from Settings.
      orientationDecidedRef.current = true;
      firstLaunchBurnedRef.current = false;
      setOrientationConsumed(false);
      return;
    }
    if (!orientationDecidedRef.current) {
      orientationDecidedRef.current = true;
      setOrientationConsumed(true);
    }
  }, [config]);

  // Memoized on the primitive values it is built from, not recreated fresh
  // every App render: an unstable reference here re-ran `useOnboarding`'s
  // effects on every App render (e.g. on every `loading-progress` event
  // during an import) instead of only on a real change to one of these
  // values. That churn is only ever wasted work, not the source of the
  // #368 thrash itself - a real, sustained `shouldStart` transition (a
  // project load, a dialog) still reaches these effects either way, new
  // object or not. The fix for THAT is `useOnboarding`'s remembered-step
  // resume; this memo only keeps unrelated re-renders from mattering.
  const onboardingContext = useMemo(
    () => ({
      view,
      hasProject: project !== null,
      activeDialog,
      dmpImportWizardPhase,
      placementModeActive: placementMode !== null,
      loadingProject: loading.isLoading,
      splashVisible: splashMounted,
      configLoaded: config !== null,
      orientationConsumed,
    }),
    [
      view,
      project !== null,
      activeDialog,
      dmpImportWizardPhase,
      placementMode !== null,
      loading.isLoading,
      splashMounted,
      config !== null,
      orientationConsumed,
    ]
  );

  const onboarding = useOnboarding(onboardingContext, {
    enabled: config?.general?.onboarding_enabled ?? true,
    completedFlowIds: config?.general?.onboarding_completed_flows ?? [],
    onCompletedFlowIdsChange: persistCompletedOnboardingFlows,
  });

  // Lets Settings > General > Restart onboarding reach `clearRememberedProgress`
  // without prop-drilling it through SettingsDialog and every settings
  // category (#368) - see useOnboardingRestart.ts for why the config write
  // Restart stages cannot be relied on to signal this on its own.
  useEffect(() => {
    registerOnboardingRestart(onboarding.clearRememberedProgress);
    return () => registerOnboardingRestart(null);
  }, [onboarding.clearRememberedProgress]);

  // Burn the flag once the orientation tour is actually on screen - not when
  // config loads (a first launch has no project, so the tour cannot start yet
  // and the one chance would be spent on a session that showed nothing), and
  // not on any other flow (they are not what the flag governs). The config is
  // re-read from the store rather than closed over, so this merges against the
  // live value instead of the snapshot the effect was scheduled with.
  useEffect(() => {
    if (onboarding.activeFlowId !== ORIENTATION_FLOW_ID || firstLaunchBurnedRef.current) {
      return;
    }
    const store = useProjectStore.getState();
    const currentConfig = store.config;
    if (!currentConfig || currentConfig.general.onboarding_first_launch_done) {
      return;
    }
    firstLaunchBurnedRef.current = true;
    const nextConfig = {
      ...currentConfig,
      general: { ...currentConfig.general, onboarding_first_launch_done: true },
    };
    store.setConfig(nextConfig);
    void store.patchConfig([{ path: ['general', 'onboarding_first_launch_done'], value: true }]);
  }, [onboarding.activeFlowId]);

  const disableOnboardingGuides = useCallback(() => {
    const store = useProjectStore.getState();
    const currentConfig = store.config;
    if (!currentConfig) {
      return;
    }
    if (!currentConfig.general.onboarding_enabled) {
      return;
    }
    const nextConfig = {
      ...currentConfig,
      general: {
        ...currentConfig.general,
        onboarding_enabled: false,
      },
    };
    store.setConfig(nextConfig);
    void store.patchConfig([{ path: ['general', 'onboarding_enabled'], value: false }]);
  }, []);

  const handleRepairIntegrity = async () => {
    // #576 D39(a): Repair mutates the survey, so the read-only contract applies. The
    // button is disabled while editing is blocked; re-checked here for a dialog left
    // open across a mode switch.
    const blocked = editBlockedReasonNow();
    if (blocked !== null) {
      useUIStore
        .getState()
        .showToast(t(`integrityRepair.blocked.${blocked}`), TOAST_KEYS.EDIT_DISABLED, 'warning');
      return;
    }
    setRepairingIntegrity(true);
    try {
      const { fixed, remaining } = await repairProjectIntegrity();
      const issueWord = t('integrityRepair.toasts.issue', { count: fixed });
      const message =
        remaining > 0
          ? t('integrityRepair.toasts.partial', {
              fixed,
              remaining,
              issueWord,
              needWord: t('integrityRepair.toasts.needWord', { count: remaining }),
            })
          : t('integrityRepair.toasts.full', { fixed, issueWord });
      useUIStore
        .getState()
        .showToast(message, TOAST_KEYS.INTEGRITY_REPAIR, remaining > 0 ? 'warning' : 'success');
      // Close the dialog only when fully clean. Read the AUTHORITATIVE store state
      // (the same integrityIssues the banner and the dialog render-gate use) rather
      // than the point-in-time `remaining` - a command or undo that interleaved
      // during the repair's awaits could have changed the real issue set, and
      // repairProjectIntegrity awaits its post-repair checkProjectIntegrity before
      // resolving, so integrityIssues is already fresh here.
      if (useProjectStore.getState().integrityIssues.length === 0) {
        closeIntegrityDialog();
      }
    } catch (e) {
      // A move-carry refusal (GHOST-2) is not a failed repair and is its own notice;
      // nor is a read-only refusal (#576 D39(a)), which the store already showed as
      // the read-only notice.
      if (isMoveCarryRefusal(e) || isEditGateCode(errorCode(e))) return;
      // repairProjectIntegrity re-throws on a real failure (e.g. the IPC rejected
      // during teardown) rather than reporting a fake no-op success. Surface it and
      // keep the dialog open - integrityIssues is untouched, so the banner/save-block
      // stay correct and the user can retry.
      useUIStore
        .getState()
        .showToast(
          t('integrityRepair.toasts.failed'),
          TOAST_KEYS.INTEGRITY_REPAIR,
          'error',
        );
    } finally {
      setRepairingIntegrity(false);
    }
  };

  return (
    <div className="h-full bg-background text-text-primary flex flex-col relative" style={{ paddingTop: 'env(safe-area-inset-top)' }}>
      {/* [#576 A11] Internal-error banner: the backend recovered from a crash and the
          user must save and restart. First, as the most severe ongoing condition, and
          shown with or without a project open (see the component). */}
      <InternalErrorBanner />

      {/* Integrity warning banner (corruption found on load/save) */}
      {integrityIssues.length > 0 && (integritySaveBlocked || !integrityDismissed) && !loading.isLoading && (
        <div
          className="bg-warning-10 border-b border-warning-20 px-4 py-2 flex items-center gap-3 text-sm"
          role="alert"
          data-testid="integrity-banner"
        >
          <span className="text-warning font-medium flex-1 min-w-0">
            This project has {integrityIssues.length} integrity{' '}
            {integrityIssues.length === 1 ? 'issue' : 'issues'} that may affect your survey.
            {integritySaveBlocked && ' Saving is paused until you repair them.'}
          </span>
          <button
            onClick={() => openIntegrityDialog()}
            className="px-3 py-1 rounded bg-accent text-white text-xs"
            data-testid="integrity-review-button"
          >
            Review &amp; Repair
          </button>
          {/* Dismiss is offered only for non-blocking (detect-only) issues. When
              saving is blocked, the banner stays so the Repair entry point can't
              be hidden on a project that can't be saved. */}
          {!integritySaveBlocked && (
            <button
              onClick={dismissIntegrityIssues}
              className="px-2 py-1 text-text-secondary hover:text-text text-xs"
              data-testid="integrity-dismiss-button"
            >
              Dismiss
            </button>
          )}
        </div>
      )}

      {/* [#576 B6-D1] Newer-version banner. A project saved by a newer Stalagmight is
          open read-only (the backend refuses every edit and save, so this build can
          never rewrite it at its older schema). An ongoing condition, so a banner
          (docs/notifications.md), and no Dismiss: it lasts exactly as long as the
          project is open, and it is the only place the user learns why nothing on
          screen is editable. No action button either - the remedy is outside the
          app. */}
      {newerFileReadOnly && project && !loading.isLoading && (
        <div
          className="bg-warning-10 border-b border-warning-20 px-4 py-2 flex items-center gap-3 text-sm"
          role="status"
          data-testid="newer-file-banner"
        >
          <span className="text-warning font-medium flex-1 min-w-0">
            {t('banners.newerFile.lead')}{' '}
            <span className="text-text-secondary font-normal">{t('banners.newerFile.detail')}</span>
          </span>
        </div>
      )}

      {/* [#576 B35(b) review] Unreadable-link banner. The file's SpeleoDB link row
          could not be read, so the project opened as a local one - but the backend
          refuses every save over this file, because the rebuilt file would lose the
          row. Without this, the first autosave failed with a message about a link the
          user was never shown. An ongoing condition (banner, docs/notifications.md)
          that lasts until a Save As moves the session to another file, so no
          Dismiss, like the newer-file banner above; the remedy is in the message. */}
      {speleoLinkUnreadable && project && !loading.isLoading && (
        <div
          className="bg-warning-10 border-b border-warning-20 px-4 py-2 flex items-center gap-3 text-sm"
          role="status"
          data-testid="speleo-link-unreadable-banner"
        >
          <span className="text-warning font-medium flex-1 min-w-0">
            {t('banners.speleoLinkUnreadable.lead')}{' '}
            <span className="text-text-secondary font-normal">
              {t('banners.speleoLinkUnreadable.detail')}
            </span>
          </span>
        </div>
      )}

      {/* [#576 B15] Unread-settings banner. Stored per-project settings the backend
          could not read on open are held as defaults, and every save is refused so
          the file keeps its stored values. An ongoing condition (until the user sets
          them again in Project Settings, or reopens), so a banner
          (docs/notifications.md) that names WHICH settings, and no Dismiss: it is the
          one place the user learns why saving is off and how to turn it back on. */}
      {unreadableProjectSettings.length > 0 && project && !loading.isLoading && (
        <div
          className="bg-warning-10 border-b border-warning-20 px-4 py-2 flex items-center gap-3 text-sm"
          role="status"
          data-testid="settings-unreadable-banner"
        >
          <span className="text-warning font-medium flex-1 min-w-0">
            {t('banners.settingsUnreadable.lead', {
              names: unreadableProjectSettings
                .map((key) => t(`banners.settingsUnreadable.names.${key}`, { defaultValue: key }))
                .join(', '),
            })}{' '}
            <span className="text-text-secondary font-normal">
              {t('banners.settingsUnreadable.detail')}
              {/* [#576 B15 follow-up] Every key the open reports today has a control
                  in Project Settings (NF-W3-2 added the north reference's); one that
                  has none clears only on reopen. */}
              {unreadableProjectSettings.some(
                (key) => !(SETTABLE_UNREADABLE_SETTINGS as readonly string[]).includes(key),
              ) && <> {t('banners.settingsUnreadable.reopenNote')}</>}
            </span>
          </span>
        </div>
      )}

      {/* [#473] Discarded-history banner. A history table the backend could not read
          is replaced by an empty stack so the project still opens - the right call -
          but that silently costs the user every undo AND redo entry they had, which
          docs/notifications.md names as a "silent state change" and forbids. Banner
          rather than toast because it is an ongoing condition (the project has no
          history for as long as it is open), and it carries no action button: there
          is nothing the user can do about it, only something they must know. */}
      {historyDiscarded && !historyDiscardedDismissed && project && !loading.isLoading && (
        <div
          className="bg-warning-10 border-b border-warning-20 px-4 py-2 flex items-center gap-3 text-sm"
          role="status"
          data-testid="history-discarded-banner"
        >
          <span className="text-warning font-medium flex-1 min-w-0">
            {t('banners.historyDiscarded.lead')}{' '}
            <span className="text-text-secondary font-normal">
              {t('banners.historyDiscarded.detail')}
            </span>
          </span>
          <button
            onClick={dismissHistoryDiscarded}
            className="px-2 py-1 text-text-secondary hover:text-text text-xs"
            data-testid="history-discarded-dismiss-button"
          >
            {t('banners.historyDiscarded.dismiss')}
          </button>
        </div>
      )}

      {/* [#576 B35(h)] What opening the project reset or dropped (see the component). */}
      <LoadRepairsBanner />

      {/* Autosave-failure banner (docs/notifications.md: an ongoing "your latest
          changes are not on disk" condition is banner-class, not modal and not a
          repeating toast). When saving is integrity-blocked the integrity banner
          above already owns the "saving is paused" story, so this one yields -
          except for a failure Repair cannot fix; see shouldShowAutosaveBanner. */}
      {shouldShowAutosaveBanner({
        autosaveFailure,
        autosaveFailureKind,
        integritySaveBlocked,
        integrityIssueCount: integrityIssues.length,
      }) && project && !loading.isLoading && (
        <div
          className="bg-warning-10 border-b border-warning-20 px-4 py-2 flex items-center gap-3 text-sm"
          role="status"
          data-testid="autosave-banner"
        >
          <span className="text-warning font-medium flex-1 min-w-0">
            {t('banners.autosave.lead')} {autosaveFailure}
          </span>
          <button
            onClick={() => {
              void (async () => {
                const result = await saveProject().catch((e: unknown) => {
                  surfaceSaveFailure(saveFailureOf(e, t('banners.autosave.genericFailure')), openDialog, useProjectStore.getState(), {
                    skipIntegrityRouting: true,
                  });
                  return null;
                });
                // A manual retry that still fails gets the full user-initiated
                // treatment (permission guidance / Review & Repair / error dialog).
                if (result && !result.success) {
                  surfaceSaveFailure(result, openDialog, useProjectStore.getState());
                }
              })();
            }}
            className="px-3 py-1 rounded bg-accent text-white text-xs"
            data-testid="autosave-save-button"
          >
            {t('banners.autosave.saveNow')}
          </button>
          <button
            onClick={clearAutosaveFailure}
            className="px-2 py-1 text-text-secondary hover:text-text text-xs"
            data-testid="autosave-dismiss-button"
          >
            {t('banners.autosave.dismiss')}
          </button>
        </div>
      )}

      {/* Integrity details + repair dialog */}
      {integrityDialogOpen && integrityIssues.length > 0 && (
        <div
          className="fixed inset-0 z-[1000] bg-overlay flex items-center justify-center p-4"
          role="dialog"
          aria-modal="true"
          aria-labelledby="integrity-dialog-title"
          data-testid="integrity-dialog"
          onClick={() => !repairingIntegrity && closeIntegrityDialog()}
        >
          <div
            className="bg-panel border border-border rounded-lg shadow-xl w-full max-w-xl max-h-[80vh] flex flex-col"
            data-testid="integrity-dialog-panel"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="px-4 py-3 border-b border-border">
              <h2 id="integrity-dialog-title" className="font-medium text-text-primary">
                Project integrity ({integrityIssues.length}{' '}
                {integrityIssues.length === 1 ? 'issue' : 'issues'})
              </h2>
            </div>
            <div className="px-4 py-3 overflow-auto">
              <p className="text-sm text-text-secondary mb-3">
                The following problems were found in this project. Repairing drops invalid
                references and realigns drifted station merges; valid survey data is kept. Save the
                project afterwards to make the fix permanent.
              </p>
              <ul
                className="list-disc list-inside space-y-1 text-sm text-text-primary"
                data-testid="integrity-dialog-list"
              >
                {integrityIssues.map((issue, i) => (
                  <li key={i} className="break-words">{issue}</li>
                ))}
              </ul>
            </div>
            <div className="px-4 py-3 border-t border-border flex justify-end gap-2">
              {repairBlocked !== null && (
                <span
                  id="integrity-repair-blocked"
                  className="mr-auto self-center text-xs text-text-secondary"
                  data-testid="integrity-dialog-repair-blocked"
                >
                  {t(`integrityRepair.blocked.${repairBlocked}`)}
                </span>
              )}
              <button
                onClick={() => closeIntegrityDialog()}
                disabled={repairingIntegrity}
                className="px-3 py-1.5 rounded border border-border text-sm hover:bg-background disabled:opacity-50"
                data-testid="integrity-dialog-cancel"
              >
                Cancel
              </button>
              <button
                onClick={handleRepairIntegrity}
                disabled={repairingIntegrity || repairBlocked !== null}
                title={repairBlocked !== null ? t(`integrityRepair.blocked.${repairBlocked}`) : undefined}
                aria-describedby={repairBlocked !== null ? 'integrity-repair-blocked' : undefined}
                className="px-3 py-1.5 rounded bg-accent text-white text-sm disabled:opacity-50"
                data-testid="integrity-dialog-repair"
              >
                {repairingIntegrity ? 'Repairing…' : 'Repair'}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Splash screen overlay */}
      {splashMounted && (
        <SplashScreen
          visible={showSplash}
          onTransitionEnd={() => setSplashMounted(false)}
        />
      )}

      {/* Drop indicator overlay */}
      {isDraggingImport && (
        <div
          className="absolute inset-0 z-50 bg-accent-20 border-4 border-dashed border-accent flex items-center justify-center pointer-events-none"
          data-testid="import-drop-indicator"
        >
          <div className="bg-panel p-6 rounded-lg shadow-xl text-center">
            <div className="text-4xl mb-2">📥</div>
            <div className="text-lg font-semibold">{t('app.dropOverlay.title')}</div>
            <div className="text-sm text-text-secondary mt-1">
              {project ? t('app.dropOverlay.existingProject') : t('app.dropOverlay.newProject')}
            </div>
          </div>
        </div>
      )}

      {view === 'global' || !project ? (
        <GlobalView />
      ) : (
        <SingleSystemView />
      )}

      <MemoryProbe />

      {/* Dialogs */}
      {activeDialog === 'import' && <ImportDialog />}
      {activeDialog === 'export' && <ExportDialog />}
      {activeDialog === 'new-project' && <NewProjectDialog />}
      {activeDialog === 'open-from-speleodb' && <OpenFromSpeleoDbDialog />}
      {activeDialog === 'publish-to-speleodb' && <PublishToSpeleoDbDialog />}
      {activeDialog === 'speleodb-done' && <SpeleoDoneDialog />}
      {activeDialog === 'speleodb-divergence' && <SpeleoDivergenceDialog />}
      {activeDialog === 'speleodb-force-edit' && <SpeleoForceEditDialog />}
      {activeDialog === 'speleodb-restore' && <SpeleoRestoreDialog />}
      {/* [#303] The Unwind confirmation, raised from the undo history dropdown's
          pinned entry - the only trigger for it. */}
      {activeDialog === 'speleodb-unwind' && <SpeleoUnwindDialog />}
      {activeDialog === 'speleodb-unwound-push' && <SpeleoUnwoundPushDialog />}
      {activeDialog === 'speleodb-discard-changes' && <SpeleoDiscardChangesDialog />}
      {activeDialog === 'speleodb-revert' && <SpeleoRevertDialog />}
      {activeDialog === 'speleodb-close' && <SpeleoCloseDialog />}
      {activeDialog === 'speleodb-quit' && <SpeleoQuitDialog />}
      {activeDialog === 'speleodb-no-permission' && <SpeleoNoPermissionDialog />}
      {activeDialog === 'speleodb-unpushed-local' && <SpeleoUnpushedLocalDialog />}
      {activeDialog === 'speleodb-managed-file' && <SpeleoManagedFileDialog />}
      {activeDialog === 'speleodb-project' && <SpeleoDbProjectDialog />}
      {activeDialog === 'new-project-for-import' && <NewProjectForImportDialog />}
      {activeDialog === 'dmp-selection' && dialogData.dmpFile ? (
        <DmpSelectionDialog
          dmpFile={dialogData.dmpFile as import('./types/commands').DmpFile}
          filePath={dialogData.filePath as string}
          onCancel={closeDialog}
          onContinue={(selectionCfg) => {
            // Continue to the new workflow
            const hasZeroDatums = (project?.zero_datums?.length ?? 0) > 0;
            openDialog(hasZeroDatums ? 'dmp-import-new' : 'zero-datum-selection', {
              dmpFile: dialogData.dmpFile,
              filePath: dialogData.filePath,
              selectionConfig: selectionCfg,
            });
          }}
        />
      ) : null}
      {activeDialog === 'dmp-import-new' && dialogData.dmpFile && dialogData.selectionConfig ? (
        <DmpNewImportDialog
          dmpFile={dialogData.dmpFile as import('./types/commands').DmpFile}
          selectionConfig={dialogData.selectionConfig as DmpSelectionConfig}
          project={project}
          stations={stations}
          zeroDatumConfig={dialogData.zeroDatumConfig as ZeroDatumConfigResult | undefined}
          savedManualPlacingState={dialogData.savedManualPlacingState as import('./components/Dialogs/DmpNewImportDialog').SavedManualPlacingState | undefined}
          autoStartManualPlacement={Boolean(dialogData.autoStartManualPlacement)}
          onCancel={closeDialog}
          onBack={() => {
            if (dialogData.zeroDatumConfig) {
              // Came through zero datum flow - go back to zero datum config.
              //
              // The config the user already entered rides along (#394 F10): the
              // wizard withdrew what it imported before calling this, so the
              // forward pass re-imports the anchor from scratch, and asking for
              // the GPS/name/shot edits a second time is the difference between
              // "step back and adjust one field" and "start over". Consumed by
              // ZeroDatumConfigDialog as its initial values.
              openDialog('zero-datum-config', {
                dmpFile: dialogData.dmpFile,
                filePath: dialogData.filePath,
                selectionConfig: dialogData.selectionConfig,
                zeroDatumSectionIndex: (dialogData.zeroDatumConfig as ZeroDatumConfigResult).sectionIndex,
                [ZERO_DATUM_CARRY_KEY]: dialogData.zeroDatumConfig,
              });
            } else {
              openDialog('dmp-selection', {
                dmpFile: dialogData.dmpFile,
                filePath: dialogData.filePath,
              });
            }
          }}
          onComplete={async (newSectionIds?: number[], receipt?: import('./types/ipc/ImportReceipt').ImportReceipt) => {
            // Persist the freshly-imported project before anything else (#92) -
            // including before handing off to orphan placement - so the import
            // can never be lost. If the save failed, an error/permission dialog is
            // now showing; leave it up instead of dismissing it via closeDialog.
            if (!(await useProjectStore.getState().saveAfterImport())) {
              return;
            }
            const importUi = {
              closeDialog,
              setView,
              triggerFitToBounds,
              openDialog,
              showToast: useUIStore.getState().showToast,
            };
            // The suggest call is the ONLY thing wrapped in try/catch here
            // (non-fatal - a failed lookup just skips the suggestion step);
            // `finishImportWithoutOrphanSuggestions` sits outside it and
            // completes the import exactly once, so a throw partway through
            // IT can never fall through to a duplicate completion.
            if (newSectionIds?.length) {
              let suggestions: OrphanPlacementSuggestion[] = [];
              try {
                suggestions = await invokeIpc(
                  'suggest_orphan_placements',
                  { newlyImportedSectionIds: newSectionIds },
                  { purpose: 'suggest orphan placements after DMP import' }
                );
              } catch (e) {
                // Skipped, but not silently (#576 NF-AN4-N4): the import has
                // landed, so it completes, and the user is told the step that
                // offers to attach unanchored sections did not run, and why.
                const reason = getErrorMessage(e);
                console.warn('[dmpImport] orphan placement lookup failed:', reason);
                useUIStore.getState().showToast(
                  t('dmpWizard.toasts.orphanLookupFailed', { reason }),
                  TOAST_KEYS.DMP_ORPHAN_LOOKUP_FAILED,
                  'warning',
                );
              }
              if (suggestions.length > 0) {
                openDialog('orphan-placement', { suggestions, receipt });
                return;
              }
              await finishImportWithoutOrphanSuggestions('dmp-import-new', importUi, newSectionIds, receipt);
              return;
            }
            // The shared exit, not a hand-rolled copy of it (#477). The inline
            // version this replaces did the same four things in the same order
            // and omitted the fifth - `setImportContext(null)` - so a completed
            // DMP import left its context set while every other importer
            // (TML/Compass/Walls/GPX, and both failure paths via
            // `surfaceImportFailure`) dropped it. The sibling branch had already
            // been fixed for exactly that reason: `OrphanPlacementDialog.finish`
            // clears it explicitly and points at #435 for this duplication.
            // Runs AFTER `saveAfterImport` above, which reads the context.
            completeImportToSingleView('dmp-import-new', importUi, receipt);
          }}
        />
      ) : null}
      {activeDialog === 'zero-datum-selection' && dialogData.dmpFile && dialogData.selectionConfig ? (
        <ZeroDatumSelectionDialog
          dmpFile={dialogData.dmpFile as import('./types/commands').DmpFile}
          selectionConfig={dialogData.selectionConfig as DmpSelectionConfig}
          filePath={dialogData.filePath as string}
          onCancel={closeDialog}
          onBack={() => {
            openDialog('dmp-selection', {
              dmpFile: dialogData.dmpFile,
              filePath: dialogData.filePath,
            });
          }}
          onSkip={() => {
            openDialog('dmp-import-new', {
              dmpFile: dialogData.dmpFile,
              filePath: dialogData.filePath,
              selectionConfig: dialogData.selectionConfig,
            });
          }}
          onSelectSection={(sectionIndex) => {
            // Re-seed the carry (#534) only when the re-picked section matches
            // the one it was entered for - `carryZeroDatumConfig` owns that
            // decision so App.tsx and the routing tests share one expression.
            openDialog('zero-datum-config', {
              dmpFile: dialogData.dmpFile,
              filePath: dialogData.filePath,
              selectionConfig: dialogData.selectionConfig,
              zeroDatumSectionIndex: sectionIndex,
              [ZERO_DATUM_CARRY_KEY]: carryZeroDatumConfig(
                readCarriedZeroDatumConfig(dialogData),
                sectionIndex
              ),
            });
          }}
        />
      ) : null}
      {activeDialog === 'zero-datum-config' && dialogData.dmpFile && dialogData.selectionConfig ? (
        <ZeroDatumConfigDialog
          dmpFile={dialogData.dmpFile as import('./types/commands').DmpFile}
          selectionConfig={dialogData.selectionConfig as DmpSelectionConfig}
          sectionIndex={dialogData.zeroDatumSectionIndex as number}
          filePath={dialogData.filePath as string}
          // Set by the wizard's Back handler above (one hop), OR relayed
          // through zero-datum-selection's own Back/forward pass when the
          // same section is re-picked (#534). A first visit, or a different
          // section pick, has no previous answers and the dialog falls back
          // to its defaults.
          initialConfig={readCarriedZeroDatumConfig(dialogData)}
          onCancel={closeDialog}
          onBack={(current) => {
            // Hand back the dialog's LIVE state, not just what it opened with
            // (#534) - a first-visit Back (no prior carry at all) still has
            // something worth restoring on a same-section forward pass.
            openDialog('zero-datum-selection', {
              dmpFile: dialogData.dmpFile,
              filePath: dialogData.filePath,
              selectionConfig: dialogData.selectionConfig,
              [ZERO_DATUM_CARRY_KEY]: current,
            });
          }}
          onConfirm={(result) => {
            openDialog('dmp-import-new', {
              dmpFile: dialogData.dmpFile,
              filePath: dialogData.filePath,
              selectionConfig: dialogData.selectionConfig,
              zeroDatumConfig: result,
            });
          }}
        />
      ) : null}
      {activeDialog === 'tml-import' && dialogData.tmlFile ? (
        <TmlImportDialog
          tmlFile={dialogData.tmlFile as import('./types/commands').TmlFile}
          filePath={dialogData.filePath as string}
        />
      ) : null}
      {activeDialog === 'compass-import' && dialogData.compassFile ? (
        <CompassImportDialog
          compassFile={dialogData.compassFile as import('./types/commands').CompassFile}
          filePath={dialogData.filePath as string}
        />
      ) : null}
      {activeDialog === 'walls-import' && dialogData.wallsFile ? (
        <WallsImportDialog
          wallsFile={dialogData.wallsFile as import('./types/commands').WallsFile}
          filePath={dialogData.filePath as string}
        />
      ) : null}
      {activeDialog === 'gpx-selection' && dialogData.gpxFile ? (
        <GpxSelectionDialog
          gpxFile={dialogData.gpxFile as import('./types/commands').GpxFile}
          filePath={dialogData.filePath as string}
        />
      ) : null}
      {activeDialog === 'gpx-placement' && dialogData.gpxFile ? (
        <GpxPlacementDialog />
      ) : null}
      {activeDialog === 'confirm-delete' && <ConfirmDialog />}
      {activeDialog === 'reverse-section' && <ReverseSectionDialog />}
      {activeDialog === 'delete-shot-choice' && <DeleteShotChoiceDialog />}
      {activeDialog === 'bulk-delete' && <BulkDeleteDialog />}
      {activeDialog === 'merge' && <MergeDialog />}
      {activeDialog === 'closure' && <ClosureDialog />}
      {activeDialog === 'merge-projects' && <MergeProjectsDialog />}
      {activeDialog === 'draw-shot' && <DrawShotDialog />}
      {activeDialog === 'edit-section' && <EditSectionDialog />}
      {activeDialog === 'edit-shot' && <EditShotDialog />}
      {activeDialog === 'edit-station' && <EditStationDialog />}
      {activeDialog === 'unmerge-stations' && <UnmergeStationsDialog />}
      {activeDialog === 'remove-closure' && <RemoveClosureDialog />}
      {activeDialog === 'unsaved-changes' && <UnsavedChangesDialog />}
      {activeDialog === 'error' && <ErrorDialog />}
      {activeDialog === 'import-blocked' && <ImportBlockedDialog />}
      {activeDialog === 'save-permission' && <SavePermissionDialog />}
      {activeDialog === 'confirm-overwrite' && <OverwriteConfirmDialog />}
      {activeDialog === 'delete-project' && <DeleteProjectDialog />}
      {activeDialog === 'context-menu' && <ContextMenu />}
      {activeDialog === 'section-adjust' && <SectionAdjustDialog />}
      {activeDialog === 'settings' && <SettingsDialog />}
      {activeDialog === 'project-settings' && <ProjectSettingsDialog />}
      {activeDialog === 'split-section' && <SplitSectionDialog />}
      {activeDialog === 'insert-station' && <InsertStationDialog />}
      {activeDialog === 'corruption-recovery' && <CorruptionRecoveryDialog />}
      {activeDialog === 'debug' && <DebugDialog />}
      {activeDialog === 'swap-comments' && <SwapCommentsDialog />}
      {activeDialog === 'unrecognised-shot-kind' && <UnrecognisedShotKindDialog />}
      {activeDialog === 'orphan-placement' && (
        <OrphanPlacementDialog
          suggestions={(dialogData.suggestions ?? []) as OrphanPlacementSuggestion[]}
          receipt={dialogData.receipt as import('./types/ipc/ImportReceipt').ImportReceipt | undefined}
        />
      )}
      {activeDialog === 'keyboard-shortcuts' && <KeyboardShortcutsDialog />}
      {activeDialog === 'import-receipt' && <ImportReceiptDialog />}
      {activeDialog === 'import-as-new-project' && <ImportAsNewProjectDialog />}
      {activeDialog === 'launch-open-confirm' && <LaunchOpenConfirmDialog />}
      {/* Not an `activeDialog`: layered over the dialog that is waiting for its answer (#576 D35). */}
      <RewrittenPathOverwritePrompt />

      {onboarding.activeStep && (
        <OnboardingOverlay
          step={onboarding.activeStep}
          stepIndex={onboarding.stepIndex}
          stepCount={onboarding.stepCount}
          canGoBack={onboarding.canGoBack}
          onBack={onboarding.back}
          onNext={onboarding.next}
          onSkipCurrent={onboarding.skip}
          onSkipAll={disableOnboardingGuides}
        />
      )}

      {/* Telemetry opt-in (shown once on first launch when feature is available, deferred during onboarding) */}
      {config && !onboarding.activeStep && (
        <TelemetryOptInDialog config={config} onConfigChanged={setConfig} />
      )}

      {/* Held-lock resume/release prompt (#201): an INDEPENDENT top-layer overlay
          (rendered last so its portal sits above any open dialog), so a login done
          from inside another dialog - e.g. the Settings account panel - layers it
          OVER that dialog instead of replacing it, leaving Settings intact. */}
      {heldLockPromptOpen && <HeldLockResumeDialog />}

      {/* "Where should your API key go?" - raised by a login whose keychain write
          failed. Also an independent top-layer overlay, and for the same reason:
          the login is usually made from the Settings account panel, which must
          survive the question with its other staged edits intact. */}
      {storageChoiceOpen && <SpeleoStorageChoiceDialog />}

      {/* [#258] "Which file is the survey?" - raised when a SpeleoDB dump holds
          more than one openable file. Independent and mounted LAST so it layers
          over whichever surface asked: this question comes from the Open-from-
          SpeleoDB browser, GlobalView, the refused-open prompt and the revision
          history alike, and three of those already hold the single dialog slot. */}
      {dumpPickerOpen && <SpeleoDumpPickerDialog />}

      {/* Toast stack (portals itself for z-index reliability). Mounted
          unconditionally: its screen-reader live regions must exist before their
          content changes, or the first toast of a run goes unannounced. */}
      <ToastStack />

      {/* ARIA live region for selection announcements (mounted permanently) */}
      <div
        id="selection-announcer"
        role="status"
        aria-live="polite"
        aria-atomic="true"
        className="sr-only"
      >
        {selectionAnnouncement}
      </div>
    </div>
  );
}

export default App;
