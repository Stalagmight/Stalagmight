import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useLatest } from '../../hooks/useLatest';
import { useKeyboardShortcuts } from '../../hooks/useKeyboardShortcuts';
import { useOnboardingStep } from '../../hooks/useOnboardingStep';
import { useProject } from '../../hooks/useProject';
import { useAutoSave, type AutoSaveStatus } from '../../hooks/useAutoSave';
import { useToastBottomClearance } from '../../hooks/useToastBottomClearance';
import { useParseIndicatorClearance } from '../../hooks/useParseIndicatorClearance';
import { useProjectStore } from '../../store/projectStore';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { PANEL_FIELDS, useUIStore, type PanelId, type PanelMode } from '../../store/uiStore';
import type { SectionId } from '../../types/project';
import { useShallow } from 'zustand/react/shallow';
import { ErrorBoundary } from '../common/ErrorBoundary';
import { OfflineIndicator } from '../common/OfflineIndicator';
import { MapAttribution } from '../Map/MapAttribution';
import Toolbar from './Toolbar';
import { SpeleoBanner } from './SpeleoBanner';
import SectionsPanel from './SectionsPanel';
import DetailPanel from './DetailPanel';
import { GasPlanPanel } from './GasPlanPanel';
import { PlanTablesPanel } from './PlanTablesPanel';
import { ExportPanel } from './ExportPanel';
import ExportPreview from '../Dialogs/ExportPreview';
import { useExportStore } from '../../store/exportStore';
import DataTable from './DataTable';
import MapCanvas from '../Map/MapCanvas';
import {
  PANEL_SIZE_BOUNDS,
  DETAIL_REVEAL_AUTOCLOSE_MS,
  MAP_MIN_WIDTH,
  MAP_MIN_HEIGHT,
  DATA_DOCK_TAB_INSET,
  DATA_DOCK_TAB_FALLBACK_INSET,
} from './layoutConstants';
import { useSidePanelSqueeze, useDataPanelSqueeze, resolveResize } from './sidePanelSqueeze';
import { ChevronDownIcon, ChevronLeftIcon, ChevronRightIcon, ChevronUpIcon } from '../common/icons';
import { usePanelReveal } from './usePanelReveal';
import { Dock, BAR } from './Dock';
import { dmpImportFilters } from '../../utils/fileDialogs';
import { pickAndRoute } from '../../utils/openOrImport';
import { useProjectMenuItems } from './useProjectMenuItems';
import { setSource, withTrackingSuppressed, withPersistSuppressed, setProjectActionSource } from '../../utils/telemetry';
import { useTranslation } from 'react-i18next';
import { formatFixed, formatNumber } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { useGrayscaleTokens } from '../../hooks/useGrayscaleTokens';
import { GrayscalePaintContext } from '../../hooks/useGrayscalePaint';

// #50: after double-click collapses the side panels, the map grows. fitToSection
// reads the container size at call time, so re-fit once the layout has reflowed
// and MapLibre's ResizeObserver has resized the map (a couple of frames). This
// lives here rather than in SectionsPanel, which unmounts when it collapses.
const SECTION_FOCUS_REFIT_DELAY_MS = 180;

// Top offsets (px) for the side opener tabs, aligning each tab with its panel's
// header/content divider. These track the panel header layout - revisit if the
// header height or the top-left map controls change. (The webview UI zoom scales
// these uniformly with the header, so they stay aligned under zoom.)
const SECTIONS_TAB_TOP = 40; // just below the top-left "Fit to view" button (bottom ≈ 32px)
const DETAILS_TAB_TOP = 79; // aligned to the Details panel's header/content divider

// Opener-tab arrow glyphs. Module-level constant elements (immutable, depend on
// nothing) so their identity is stable across renders - required for the memoized
// <Dock> to skip reconciliation during a sibling's resize drag (a fresh element
// each render would otherwise bust the memo every ~16ms).
const CHEVRON_RIGHT = <ChevronRightIcon size={14} />;
const CHEVRON_LEFT = <ChevronLeftIcon size={14} />;
const CHEVRON_UP = <ChevronUpIcon size={14} />;
const CHEVRON_DOWN = <ChevronDownIcon size={14} />;

export default function SingleSystemView() {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  useKeyboardShortcuts();
  // [#576 W70] Toasts keep their gap above the status bar instead of covering it.
  const statusBarToastClearance = useToastBottomClearance('project-view-status-bar');
  // [#576 W71] The import parse indicator floats above the imagery attribution too.
  const attributionParseIndicatorClearance = useParseIndicatorClearance('project-view-map-attribution');

  const {
    project,
    stats,
    handleSaveProject: rawSave,
    canUndo,
    canRedo,
    undo,
    redo,
  } = useProject();

  const isDirty = useProjectStore((s) => s.isDirty);
  const autoSave = useAutoSave();
  const saveFailed = autoSave.status === 'error';
  // Red wash in the status bar's bottom-right corner only when a save failed -
  // spans the full bar height and fades right→left across ~18% of its width.
  const statusBarGradient = saveFailed
    ? 'linear-gradient(to left, rgba(248, 113, 113, 0.30), transparent 18%)'
    : undefined;
  const placementModeActive = useUIStore((s) => s.placementMode !== null);
  // Top-level workspace mode (#38, the workspace-mode reframe). Survey shows the editable data
  // table + detail panel; Plan swaps the right Dock for the gas setup sidebar and the
  // bottom Dock for the tabbed plan tables; Export swaps the right Dock for the export
  // stub and hides the bottom table. The carousel owns enter/exit; setView()/setColorMode
  // reset it. The left Sections Dock is survey-only (Plan/Export are read-only).
  const appMode = useUIStore((s) => s.appMode);
  // R9/R10 (#395): the bottom Cylinders/DPV/Segments/Profile panel belongs to the full rig, which
  // only exists once the diver has answered "Where does this dive start?".
  const gasOriginChosen = useGasPlanStore((s) => s.originChosen);
  // Drives the imagery attribution overlay below; MapCanvas adds and removes the
  // satellite source off this same flag.
  const showSatellite = useUIStore((s) => s.showSatellite);
  // Right inset (px) putting the imagery attribution bar flush against the left
  // edge of the Data dock's tab.
  //
  // Measured rather than computed. The tab's own `tabInset` is relative to the
  // Data overlay, which is portaled to Main with its own left/right offsets, so
  // it does not line up with this column's right edge; and the tab's width moves
  // with its contents (label when closed, pin + close icons when open) and with
  // webview text-zoom. Arithmetic on either left a visible gap.
  const mapAreaRef = useRef<HTMLDivElement>(null);
  const dataTabLeftRef = useRef<number | null>(null);
  const [attributionRight, setAttributionRight] = useState(DATA_DOCK_TAB_FALLBACK_INSET);

  const syncAttributionInset = useCallback(() => {
    const host = mapAreaRef.current;
    const tabLeft = dataTabLeftRef.current;
    if (!host || tabLeft === null) return;
    // Both edges in viewport coordinates, so the overlay's own offsets cancel.
    setAttributionRight(Math.max(0, Math.round(host.getBoundingClientRect().right - tabLeft)));
  }, []);

  const handleDataTabBounds = useCallback((rect: DOMRect) => {
    dataTabLeftRef.current = rect.left;
    syncAttributionInset();
  }, [syncAttributionInset]);

  // The tab observer above fires when the TAB changes; this covers the other
  // half - the map column itself resizing as side panels open or drag.
  useEffect(() => {
    const host = mapAreaRef.current;
    if (!host || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(syncAttributionInset);
    observer.observe(host);
    return () => observer.disconnect();
  }, [syncAttributionInset]);

  // Pin the bottom Dock open the moment the origin is chosen in Plan mode (it is
  // hidden entirely before that - see the Dock render condition below). Inlined
  // setState (not a toggle action) because a mode-forced open is not a user
  // action: it must never persist via savePanelLayout, or the user's real
  // closed-panel preference gets silently overwritten on disk. setAppMode used to
  // do this unconditionally on Plan entry (#241); staged entry moved it here.
  useEffect(() => {
    if (appMode === 'plan' && gasOriginChosen) {
      useUIStore.setState({ dataTableOpen: true, dataTableMode: 'pin' });
    }
  }, [appMode, gasOriginChosen]);

  // Export mode shows a large WYSIWYG export preview over the map area; it
  // mirrors the inline ExportControls options via the shared export store.
  const exportPreviewType = useExportStore((s) => s.exportType);
  const exportImageOptions = useExportStore((s) => s.imageOptions);

  // Depend on the stable cancelPending useCallback (not the whole autoSave object,
  // which is a fresh object each render) so handleSaveProject keeps a stable
  // identity - otherwise useProjectMenuItems' useMemo rebuilds the menu array on
  // every render (incl. ~60×/s during a resize drag).
  const cancelPending = autoSave.cancelPending;
  const handleSaveProject = useCallback(async (opts?: { closeAfterSave?: boolean }) => {
    cancelPending();
    return rawSave(opts);
  }, [cancelPending, rawSave]);

  // Toolbar "Import" button - `.dmp` only (#508), matching the hamburger Import
  // row and Ctrl+I: with a project open, Import means adding mNemo data to it.
  // The other survey formats would only ever reach the "import as new project?"
  // confirm, and a `.stalx` is a plain open - both belong to the hamburger's
  // Open / Import row, which keeps the full unified picker. Still routed through
  // the shared router, so the read-only guard and its fix-it prompt apply.
  const handleImportDmp = useCallback(async () => {
    setProjectActionSource('toolbar');
    await pickAndRoute(dmpImportFilters(), 'Import Failed');
  }, []);

  // Hamburger menu items (file/project actions). Built here so it can use the
  // autosave-aware wrapped save.
  const menuItems = useProjectMenuItems({ onSave: handleSaveProject, saveFailed });

  const {
    sectionsPanelWidth,
    detailPanelWidth,
    dataTableHeight,
    dataTableFullScreen,
    focusedSectionId,
    exitSectionFocus,
    setFocusedSection,
    selection,
    triggerZoomToSection,
    triggerFitToBounds,
    setSectionsPanelWidth,
    setDetailPanelWidth,
    setDataTableHeight,
    savePanelSizes,
  } = useUIStore(useShallow((s) => ({
    sectionsPanelWidth: s.sectionsPanelWidth,
    detailPanelWidth: s.detailPanelWidth,
    dataTableHeight: s.dataTableHeight,
    dataTableFullScreen: s.dataTableFullScreen,
    focusedSectionId: s.focusedSectionId,
    exitSectionFocus: s.exitSectionFocus,
    setFocusedSection: s.setFocusedSection,
    selection: s.selection,
    triggerZoomToSection: s.triggerZoomToSection,
    triggerFitToBounds: s.triggerFitToBounds,
    setSectionsPanelWidth: s.setSectionsPanelWidth,
    setDetailPanelWidth: s.setDetailPanelWidth,
    setDataTableHeight: s.setDataTableHeight,
    savePanelSizes: s.savePanelSizes,
  })));

  // The Data panel's overlay is portaled to this Main element so it can span the
  // full width (past the center column's side insets). It stops at a pinned-open
  // side panel's edge; otherwise it fills to the window edge when the Data panel
  // is open (covering the gap left by a compressed side bar), or sits between the
  // closed bars (BAR inset) when the Data panel is itself closed. It is also the
  // element the side-panel squeeze measures (its width IS the row that has to fit
  // both panels plus the map).
  const [mainEl, setMainEl] = useState<HTMLDivElement | null>(null);

  // #260: viewport-driven squeeze. Which panels currently reflow the map is read
  // straight from the PERSISTED mode/open (a hover-floating panel overlays the map
  // and costs no layout width), so this is computable before the reveal hooks run
  // and there is no circular dependency between "am I collapsed" and "am I open".
  const sectionsPinnedOpen = useUIStore((s) => s.sectionsPanelMode === 'pin' && s.sectionsPanelOpen);
  const detailPinnedOpen = useUIStore((s) => s.detailPanelMode === 'pin' && s.detailPanelOpen);
  // Sections is survey-only; Plan/Export don't render it at all, so it costs
  // nothing there (not even an opener bar).
  const sectionsPresent = appMode === 'survey';
  const squeezeSections = useMemo(
    () => ({
      width: sectionsPanelWidth,
      min: PANEL_SIZE_BOUNDS.sections.min,
      present: sectionsPresent,
      pinnedOpen: sectionsPinnedOpen,
    }),
    [sectionsPanelWidth, sectionsPresent, sectionsPinnedOpen],
  );
  const squeezeDetail = useMemo(
    () => ({
      width: detailPanelWidth,
      min: PANEL_SIZE_BOUNDS.detail.min,
      present: true,
      pinnedOpen: detailPinnedOpen,
    }),
    [detailPanelWidth, detailPinnedOpen],
  );
  const squeeze = useSidePanelSqueeze(mainEl, {
    mapMinWidth: MAP_MIN_WIDTH,
    barWidth: BAR,
    sections: squeezeSections,
    detail: squeezeDetail,
  });
  // Display-only widths: these feed the Docks' rendered size, NEVER the store or
  // savePanelSizes (see sidePanelSqueeze).
  const sectionsDisplayWidth = squeeze.sections.width;
  const detailDisplayWidth = squeeze.detail.width;

  // The same squeeze on the VERTICAL axis (#260). The original fix covered only
  // the width, but `dataTableHeight` has the identical shape: persisted in pixels
  // up to 500, against a Main box that shrinks with the UI zoom, with a
  // `flex-shrink-0` spacer that beats the `flex-1` map wrapper outright - so at
  // 250% zoom (Main ~330 CSS px) the map collapsed to 0px, both side opener bars
  // got height-0 overlays, and the Data tab was clipped above Main's top, leaving
  // no way to close the panel again without zooming back out.
  const dataPinnedOpen = useUIStore((s) => s.dataTableMode === 'pin' && s.dataTableOpen);
  const dataSqueeze = useDataPanelSqueeze(mainEl, {
    height: dataTableHeight,
    min: PANEL_SIZE_BOUNDS['data-table'].min,
    mapMinHeight: MAP_MIN_HEIGHT,
    // Fullscreen is a deliberate, explicit "cover everything" state, so it is
    // exempt: there is no map row to protect while it is on.
    pinnedOpen: dataPinnedOpen && !dataTableFullScreen,
  });
  // Display-only height, used everywhere `dataTableHeight` fed layout: the Dock's
  // own size AND the two side Docks' `closedBottomInset`, which were both passing
  // the unclamped persisted value.
  const dataDisplayHeight = dataSqueeze.height;

  // Two-mode (hover/pin) reveal state for each panel. The store holds the
  // persisted mode/open; these hooks add transient hover visibility + handlers.
  // `forcedClosed` layers the transient viewport collapse on top without touching
  // either persisted field.
  const sectionsReveal = usePanelReveal('sections', { forcedClosed: squeeze.sections.collapsed });
  const detailReveal = usePanelReveal('detail', { forcedClosed: squeeze.detail.collapsed });
  const dataReveal = usePanelReveal('data-table', { forcedClosed: dataSqueeze.collapsed });

  // #260: a drag on a SQUEEZED panel is measured against the squeezed rendering,
  // so a size above the clamp is unreachable and must not be persisted - see
  // `resolveResize`, which states the rule next to the display-only one it is the
  // other half of. It CLAMPS rather than refusing: the drag mutates its own inputs,
  // so a shrink-then-grow could end on an unreachable number and a refusal there
  // discarded the reachable part of the same drag too.
  //
  // Read through a ref rather than deps: these props feed useResizeHandle, which
  // re-binds its listeners whenever their identity changes, and the display size
  // moves ~60x/s during an unsqueezed drag.
  const squeezeRef = useLatest({
    sections: { display: sectionsDisplayWidth, user: sectionsPanelWidth },
    detail: { display: detailDisplayWidth, user: detailPanelWidth },
    data: { display: dataDisplayHeight, user: dataTableHeight },
  });
  /** The size to write for `size`, clamped to what this drag could reach on screen. */
  const resolveSize = useCallback((axis: 'sections' | 'detail' | 'data', size: number) => {
    const { display, user } = squeezeRef.current[axis];
    return resolveResize(display, user, size);
  }, [squeezeRef]);

  // Stable onResizeEnd handlers per Dock so handleProps identity stays stable
  // through a drag (these props feed useResizeHandle, which re-binds listeners
  // when they change). Inline arrows would be re-created every render - including
  // the ~60×/s store-width updates during the drag itself.
  const applySectionsWidth = useCallback(
    (size: number) => {
      setSectionsPanelWidth(resolveSize('sections', size));
    },
    [resolveSize, setSectionsPanelWidth],
  );
  const applyDetailWidth = useCallback(
    (size: number) => {
      setDetailPanelWidth(resolveSize('detail', size));
    },
    [resolveSize, setDetailPanelWidth],
  );
  const applyDataHeight = useCallback(
    (size: number) => {
      setDataTableHeight(resolveSize('data', size));
    },
    [resolveSize, setDataTableHeight],
  );
  const handleSectionsResizeEnd = useCallback(
    (size: number) => {
      savePanelSizes('sections', resolveSize('sections', size));
    },
    [resolveSize, savePanelSizes],
  );
  const handleDetailResizeEnd = useCallback(
    (size: number) => {
      savePanelSizes('detail', resolveSize('detail', size));
    },
    [resolveSize, savePanelSizes],
  );
  // Don't persist the temporary focus-mode height (#50): exit restores the prior
  // height only in memory, so a focus-time drag must not leak to config.
  const handleDataResizeEnd = useCallback(
    (size: number) => {
      if (focusedSectionId === null) savePanelSizes('data-table', resolveSize('data', size));
    },
    [resolveSize, savePanelSizes, focusedSectionId],
  );

  // Programmatic Details reveal (jump-to-warning from the data table). The store
  // bumps detailRevealNonce ONLY in hover mode (pin mode is revealed by opening
  // the panel in the store directly), so float it open like a hover and arm an
  // auto-close - the reveal isn't driven by a pointer over the panel, so without
  // this it could float open indefinitely. Lives here, not in usePanelReveal, so
  // only the Details panel carries the subscription (the others never need it).
  const detailRevealNonce = useUIStore((s) => s.detailRevealNonce);
  const detailRevealSeen = useRef(detailRevealNonce);
  const { onRegionEnter: revealDetail, scheduleClose: scheduleDetailClose } = detailReveal;
  useEffect(() => {
    if (detailRevealNonce === detailRevealSeen.current) return;
    detailRevealSeen.current = detailRevealNonce;
    revealDetail();
    scheduleDetailClose(DETAIL_REVEAL_AUTOCLOSE_MS);
  }, [detailRevealNonce, revealDetail, scheduleDetailClose]);

  // Which panel (if any) is being resized - at most one at a time, so a single
  // enum replaces three parallel booleans. Drives each panel's edge that follows a
  // sibling so it can track live (no transition lag): the side bars follow the Data
  // panel's height, and the Data overlay's left/right follow the side panels' widths.
  const [resizingPanel, setResizingPanel] = useState<PanelId | null>(null);
  // Stable per-panel callback for a Dock's onResizingChange. Clears only when the
  // panel that set the flag reports it ended, so a stray end-notification from a
  // different Dock can't wipe the active one. Stable identity (the Docks are
  // memoized) so it isn't re-created on the ~60×/s resize-tick re-renders.
  const setPanelResizing = useCallback(
    (panel: PanelId, resizing: boolean) =>
      setResizingPanel((prev) => (resizing ? panel : prev === panel ? null : prev)),
    [],
  );
  const onSectionsResizing = useCallback((r: boolean) => setPanelResizing('sections', r), [setPanelResizing]);
  const onDataResizing = useCallback((r: boolean) => setPanelResizing('data-table', r), [setPanelResizing]);
  const onDetailResizing = useCallback((r: boolean) => setPanelResizing('detail', r), [setPanelResizing]);
  const dataResizing = resizingPanel === 'data-table';
  const sidePanelResizing = resizingPanel === 'sections' || resizingPanel === 'detail';
  // Stop the Data overlay at ANY open side panel's edge - pinned OR hover-floating
  // (both render the full panel at its persisted width); only fill to the window
  // edge when the side is minimized and the Data panel is open, else sit between
  // the closed bars. (isOpen, not isPinnedOpen, or a floating side panel overlaps
  // the Data panel and hides its tab.)
  // Outside Survey the Sections Dock is not rendered at all (see the appMode gate below), so there
  // is neither a panel nor a closed bar on the left to stop at - the bottom panel must reach the
  // window edge. `sectionsReveal.isOpen` still reports the PERSISTED survey-mode state, which is
  // exactly why Plan's cylinders bar used to sit indented by a panel that was not there (#298).
  // Inside Survey it is the display width, i.e. the width each Dock is actually RENDERING at in
  // its current state - squeezed while it reflows the map, and the user's own width (capped at
  // Main, never the raw persisted value) while it floats over it - so the Data overlay stops at
  // the panel edge the user can see rather than at a width nothing on screen has. Note a
  // viewport-collapsed panel still reports that same width: it is what the panel renders at if
  // the user floats it open, and while it is NOT open neither branch below reads it.
  const dataOverlayLeft =
    appMode !== 'survey'
      ? 0
      : sectionsReveal.isOpen
        ? sectionsDisplayWidth
        : dataReveal.isOpen
          ? 0
          : BAR;
  const dataOverlayRight = detailReveal.isOpen
    ? detailDisplayWidth
    : dataReveal.isOpen
      ? 0
      : BAR;

  // #50: name shown in place of the project name while focused on one section.
  // A single lookup drives both existence (the auto-exit effect) and the name.
  // Memoized: SingleSystemView re-renders ~60×/s during a panel-resize drag (it
  // subscribes to the panel width/height being mutated each tick), so the O(n)
  // .find() must not re-scan project.sections every frame when focus is active.
  const focusedSection = useMemo(
    () =>
      focusedSectionId !== null
        ? project?.sections.find((s) => s.id === focusedSectionId) ?? null
        : null,
    [focusedSectionId, project?.sections],
  );
  const focusedSectionExists = focusedSection !== null;
  const focusedSectionName =
    focusedSectionId !== null
      ? focusedSection?.name ?? t('sections.defaultName', { id: focusedSectionId })
      : null;

  // The section that owns the current selection (section / shot / station) - the
  // same mapping the data table uses to pick its section.
  const selectedSectionId: SectionId | undefined =
    selection?.type === 'section' || selection?.type === 'shot'
      ? selection.sectionId
      : selection?.type === 'station'
        ? selection.stationRef?.section_id
        : undefined;

  // #50: adjust the map once the layout settles, after the side panels collapse
  // (entry) or return (exit). ENTRY (null -> id) frames the focused section in the
  // enlarged map; EXIT (id -> null) returns to a neutral whole-map overview
  // (fit-to-bounds) rather than staying zoomed into the section. NOT on a follow
  // (id -> id'), so clicking another section never re-zooms - consistent with
  // single-click no longer moving the map. Lives here (not in SectionsPanel,
  // which unmounts on collapse) so the timer survives the panel mount/unmount.
  const prevFocusedRef = useRef<SectionId | null>(null);
  const focusedSectionIdRef = useLatest<SectionId | null>(focusedSectionId);
  const refitTimerRef = useRef<number | null>(null);
  useEffect(() => {
    const prev = prevFocusedRef.current;
    prevFocusedRef.current = focusedSectionId;
    const transition =
      prev === null && focusedSectionId !== null
        ? 'enter'
        : prev !== null && focusedSectionId === null
          ? 'exit'
          : null; // follow / no transition
    // A follow (id -> id') deliberately doesn't re-frame the map, and crucially
    // must NOT cancel a still-pending ENTER re-fit either: the entry timer reads
    // the latest focused id at fire time, so a quick follow during the settle
    // window still frames the section the user landed on instead of dropping the
    // zoom. So leave any pending timer untouched on a follow.
    if (transition === null) return;
    if (refitTimerRef.current !== null) window.clearTimeout(refitTimerRef.current);
    refitTimerRef.current = window.setTimeout(() => {
      refitTimerRef.current = null;
      setSource('sections-panel');
      const id = focusedSectionIdRef.current;
      if (transition === 'enter' && id !== null) {
        triggerZoomToSection(id);
      } else {
        triggerFitToBounds();
      }
    }, SECTION_FOCUS_REFIT_DELAY_MS);
  }, [focusedSectionId, triggerZoomToSection, triggerFitToBounds, focusedSectionIdRef]);
  // The effect above no longer returns a per-change cleanup (so a follow can't
  // cancel an in-flight entry zoom); clear any pending re-fit on unmount instead.
  useEffect(
    () => () => {
      if (refitTimerRef.current !== null) window.clearTimeout(refitTimerRef.current);
    },
    [],
  );

  // #50: while focused, follow the selected section so the toolbar heading tracks
  // the data table (clicking another section on the map updates both).
  useEffect(() => {
    if (
      focusedSectionId !== null &&
      selectedSectionId !== undefined &&
      selectedSectionId !== focusedSectionId
    ) {
      setFocusedSection(selectedSectionId);
    }
  }, [focusedSectionId, selectedSectionId, setFocusedSection]);

  // #50: if the focused section disappears (e.g. deleted while focused), exit
  // focus mode rather than stay in the collapsed layout showing a stale name.
  useEffect(() => {
    if (focusedSectionId !== null && project && !focusedSectionExists) {
      exitSectionFocus();
    }
  }, [focusedSectionId, focusedSectionExists, project, exitSectionFocus]);

  // Fullscreen data table: the overlay's top edge tracks the live toolbar
  // height (it can change with wrapping/zoom), published as a CSS var on the
  // root so the fixed-position wrapper can reference it. useLayoutEffect so the
  // var is set before the overlay's first paint.
  const rootRef = useRef<HTMLDivElement | null>(null);
  const projectLoaded = project != null;
  useLayoutEffect(() => {
    if (!projectLoaded) return undefined;
    const root = rootRef.current;
    const toolbar = root?.querySelector<HTMLElement>('[data-testid="toolbar"]');
    const statusBar = root?.querySelector<HTMLElement>('[data-testid="status-bar"]');
    if (!root || !toolbar) return undefined;
    // Expose the chrome heights so floating menus (e.g. data-table dropdowns) can
    // stay clear of the top toolbar and the bottom status bar when they flip.
    const apply = () => {
      root.style.setProperty('--toolbar-h', `${toolbar.offsetHeight}px`);
      if (statusBar) root.style.setProperty('--statusbar-h', `${statusBar.offsetHeight}px`);
    };
    apply();
    const ro = new ResizeObserver(apply);
    ro.observe(toolbar);
    if (statusBar) ro.observe(statusBar);
    return () => ro.disconnect();
  }, [projectLoaded]);

  // Snapshot of a panel's persisted {mode, open} captured when onboarding forced
  // it open, so exiting the step restores the user's exact preference (including
  // hover mode) instead of leaving it pinned. Null when onboarding isn't holding it.
  type PanelSnapshot = { mode: PanelMode; open: boolean };
  const sectionsOnboardingSnapRef = useRef<PanelSnapshot | null>(null);
  const detailOnboardingSnapRef = useRef<PanelSnapshot | null>(null);
  const dataTableOnboardingSnapRef = useRef<PanelSnapshot | null>(null);

  // The active onboarding step (DOM-published by OnboardingOverlay), observed once
  // via the shared hook. Only this view's flow drives the panel reveals here.
  const onboarding = useOnboardingStep();
  const singleViewOnboardingStepId =
    onboarding.flowId === 'workspace-basics' ? onboarding.stepId : null;

  // Reveal the right panel during late SURVEY WORKSPACE steps (avoids a visible
  // tooltip jump when entering a panel step), without clobbering the user's
  // persisted panel mode: pin-open on entry, restore the snapshot on exit. Driven
  // by the observed step id, so it runs only when the step actually changes.
  useEffect(() => {
    // Onboarding's transient panel toggles must produce NO telemetry AND NO
    // config write (the user's saved layout must survive the temporary reveal),
    // so suppress both concerns together - withTrackingSuppressed gates telemetry
    // and withPersistSuppressed gates savePanelLayout's persist.
    const withOnboardingSuppressed = (fn: () => void) =>
      withTrackingSuppressed(() => withPersistSuppressed(fn));

    const syncOnboardingPanel = (
      panel: PanelId,
      shouldShow: boolean,
      snapRef: { current: PanelSnapshot | null },
    ) => {
      const ui = useUIStore.getState();
      const mode = ui[PANEL_FIELDS[panel].mode];
      const open = ui[PANEL_FIELDS[panel].open];
      const pinnedOpen = mode === 'pin' && open;

      if (shouldShow) {
        // Reveal it (pin-open) and remember the prior state - unless it's already
        // pinned-open, in which case there's nothing to reveal or restore.
        if (snapRef.current === null && !pinnedOpen) {
          snapRef.current = { mode, open };
          withOnboardingSuppressed(() => ui.setPanelMode(panel, 'pin'));
        }
      } else if (snapRef.current !== null) {
        const snap = snapRef.current;
        snapRef.current = null;
        // Only restore if the panel is still in the pinned-open state onboarding
        // forced it into. If the user changed it during the step (e.g. un-pinned or
        // closed it), that's a deliberate choice - honor it instead of reverting to
        // the pre-step snapshot.
        if (pinnedOpen) {
          withOnboardingSuppressed(() => {
            // setPanelMode forces open = (mode === 'pin'); fix up open if it differed
            // (e.g. the user had pinned-but-closed).
            ui.setPanelMode(panel, snap.mode);
            if (snap.open !== (snap.mode === 'pin')) ui.setPanelOpen(panel, snap.open);
          });
        }
      }
    };

    // One merged "docks" step (#331) spotlights all three panels at once, so all
    // three reveal together instead of one per step.
    const onDocksStep = singleViewOnboardingStepId === 'docks';
    syncOnboardingPanel('sections', onDocksStep, sectionsOnboardingSnapRef);
    syncOnboardingPanel('detail', onDocksStep, detailOnboardingSnapRef);
    syncOnboardingPanel('data-table', onDocksStep, dataTableOnboardingSnapRef);

    // Restore on unmount too, not just on the next step. The reveal is a pinned
    // override of the user's saved layout, and unmounting mid-step (project
    // closed from the app menu while the tour is up) would otherwise strand all
    // three panels pinned open with the real layout lost.
    return () => {
      syncOnboardingPanel('sections', false, sectionsOnboardingSnapRef);
      syncOnboardingPanel('detail', false, detailOnboardingSnapRef);
      syncOnboardingPanel('data-table', false, dataTableOnboardingSnapRef);
    };
  }, [singleViewOnboardingStepId]);

  // Shared panel content - each rendered in exactly ONE place (its Dock), so the
  // heavy instance is never duplicated/remounted across pin/unpin/hover. Memoized
  // so a panel-resize re-render (which fires ~60×/s as the store width/height
  // updates during a drag) doesn't reconcile the heavy DataTable / Sections /
  // Details subtrees: the Docks receive the SAME element reference and React bails
  // out of the child render. Dimmed during placement (dim wraps content only,
  // never the tab/handle).
  // The gray is drawn without a CSS filter (#576 NF-W13FE-CSS-D1, #605: on WebKitGTK a
  // filtered subtree repaints through a GPU promise image, which a memory release can
  // deadlock): every colour token re-declared gray on the wrapper (useGrayscaleTokens, as
  // the toolbar does), and the colours that are no token (a section's own colour on its
  // swatch) gray themselves through GrayscalePaintContext / useGrayscalePaint. The text
  // colour the panels inherit was resolved on App's root from the coloured token, so the
  // wrapper re-declares it (`text-text-primary`) to resolve against the gray one. The wrapper
  // stays `pointer-events-none`: the dimmed panels are not interactive while placing.
  const dimCls = placementModeActive ? 'pointer-events-none select-none opacity-45 text-text-primary' : '';
  const dimStyle = useGrayscaleTokens(placementModeActive);
  const sections = project?.sections;
  const sectionsContent = useMemo(
    () =>
      sections ? (
        <div className={`h-full ${dimCls}`} style={dimStyle}>
          <GrayscalePaintContext.Provider value={placementModeActive}>
            <ErrorBoundary panelName="Sections">
              <SectionsPanel sections={sections} />
            </ErrorBoundary>
          </GrayscalePaintContext.Provider>
        </div>
      ) : null,
    [sections, dimCls, dimStyle, placementModeActive],
  );
  const detailContent = useMemo(
    () => (
      <div
        className={`h-full ${dimCls}`}
        style={dimStyle}
        // A programmatic hover-mode reveal (jump-to-warning) arms a timed auto-close
        // that otherwise only cancels on pointer-enter. Treat keyboard focus / typing
        // inside the panel as engagement too (onRegionEnter clears the grace), so a
        // keyboard-only user resolving a warning doesn't have it close mid-task.
        onFocusCapture={revealDetail}
        onKeyDownCapture={revealDetail}
      >
        <GrayscalePaintContext.Provider value={placementModeActive}>
          <ErrorBoundary panelName="Details">
            {appMode === 'plan' ? (
              <GasPlanPanel />
            ) : appMode === 'export' ? (
              <ExportPanel />
            ) : (
              <DetailPanel />
            )}
          </ErrorBoundary>
        </GrayscalePaintContext.Provider>
      </div>
    ),
    [dimCls, dimStyle, placementModeActive, revealDetail, appMode],
  );
  const dataInner = useMemo(
    () =>
      appMode === 'plan' ? (
        gasOriginChosen ? (
          <div className="h-full">
            <ErrorBoundary panelName="Plan Tables">
              <PlanTablesPanel />
            </ErrorBoundary>
          </div>
        ) : null
      ) : sections ? (
        <div className={`h-full ${dimCls}`} style={dimStyle}>
          <GrayscalePaintContext.Provider value={placementModeActive}>
            <ErrorBoundary panelName="Data Table">
              <DataTable sections={sections} />
            </ErrorBoundary>
          </GrayscalePaintContext.Provider>
        </div>
      ) : null,
    [appMode, gasOriginChosen, sections, dimCls, dimStyle, placementModeActive],
  );

  // Memoized on `t` (changes only on locale switch): each Dock's `label` and
  // `tabLabels` props must keep a stable identity so the memoized <Dock> can skip
  // reconciliation during a sibling's resize drag - a fresh object per render
  // would bust the memo every tick.
  const panelLabels = useMemo(() => {
    const name = {
      sections: t('singleView.panels.sectionsName'),
      detail: t('singleView.panels.detailName'),
      // In Plan mode the bottom Dock holds the Cylinders/DPV/Segments/Profile tables,
      // so its tab says "Gear" rather than "Data" (#395).
      data: appMode === 'plan' ? t('singleView.panels.gearName') : t('singleView.panels.dataName'),
    };
    const labels = (n: string) => ({
      open: t('singleView.panels.open', { name: n }),
      pin: t('singleView.panels.pin'),
      unpin: t('singleView.panels.unpin'),
      close: t('singleView.panels.close', { name: n }),
    });
    return {
      name,
      sections: labels(name.sections),
      detail: labels(name.detail),
      data: labels(name.data),
    };
  }, [t, appMode]);

  if (!project) {
    return (
      <div className="h-full flex items-center justify-center">
        <div className="text-text-secondary">{t('singleView.empty')}</div>
      </div>
    );
  }

  return (
    <div ref={rootRef} className="h-full flex flex-col" data-testid="single-system-view">
      {/* Page heading for screen readers. The toolbar center now shows the
          active tool name (#51), so the project/focused-section name - the real
          page heading - lives here, visually hidden but still in the heading
          outline (follows the focused section in focus mode, per #50). */}
      <h1 className="sr-only" data-testid="page-heading">
        {focusedSectionName ?? project.name}
      </h1>
      {/* Toolbar */}
      <Toolbar
        menuItems={menuItems}
        saveFailed={saveFailed}
        onImport={handleImportDmp}
        onUndo={undo}
        onRedo={redo}
        canUndo={canUndo}
        canRedo={canRedo}
        placementModeActive={placementModeActive}
        dataTableFullScreen={dataTableFullScreen}
      />
      {/* SpeleoDB status banner (#200 seam) - renders only when the store has a
          banner (push-unconfirmed / concurrent-session); otherwise nothing. */}
      <SpeleoBanner />

      {/* Main content - the `relative` here is the positioning context for the
          side Docks' overlays + tabs. Each Dock renders a flow spacer (the only
          thing that reflows the map) plus a single absolute panel instance. */}
      <div ref={setMainEl} className="relative flex-1 flex overflow-hidden">
        {/* Sections (left) - survey-only; Plan/Export show the read-only closure map
            with no section-editing affordances. */}
        {appMode === 'survey' && (
        <Dock
          reveal={sectionsReveal}
          edge="left"
          // Display width: squeezed down when the viewport can't afford the user's
          // own width without starving the map (#260). The store keeps the user's
          // value, so zooming back out restores it.
          size={sectionsDisplayWidth}
          minSize={PANEL_SIZE_BOUNDS.sections.min}
          maxSize={PANEL_SIZE_BOUNDS.sections.max}
          onResize={applySectionsWidth}
          onResizeEnd={handleSectionsResizeEnd}
          label={panelLabels.name.sections}
          openArrow={CHEVRON_RIGHT}
          closeArrow={CHEVRON_LEFT}
          testId="sections"
          tabLabels={panelLabels.sections}
          tabTop={SECTIONS_TAB_TOP}
          // When the Data panel is open (pinned OR hover-floating), compress this
          // bar (if minimized) so it stops at the Data panel's top instead of
          // running alongside it.
          closedBottomInset={dataReveal.isOpen ? dataDisplayHeight : 0}
          suspendSiblingTransition={dataResizing}
          onResizingChange={onSectionsResizing}
        >
          {sectionsContent}
        </Dock>
        )}

        {/* Map + data table. `min-w-0` lets this column shrink when both side
            panels dock (otherwise flex refuses and push behavior breaks). */}
        <div className="relative flex-1 flex flex-col min-w-0 overflow-hidden">
          <div ref={mapAreaRef} className="flex-1 relative">
            <ErrorBoundary panelName="Map">
              <MapCanvas />
            </ErrorBoundary>
            {/* Offline indicator - positioned in top-right of map area */}
            <OfflineIndicator className="absolute top-2 right-14 z-10" />
            {/* Esri requires the imagery's source names to stay visible for as
                long as the imagery is drawn, so this tracks the satellite layer
                rather than the map. Mounted here rather than inside MapCanvas,
                which is map chrome's usual home in this view - see the
                OfflineIndicator above.

                Sits on the map's bottom edge, flush against the left edge of the
                Data dock's tab, so the tab is to its right and neither obstructs
                the other (Esri's terms forbid covering the names, and the tab
                drew over them before this inset existed). The ScaleControl holds
                the other end of the same edge, bottom-left. */}
            <MapAttribution
              visible={showSatellite}
              className="absolute bottom-0 z-10"
              style={{ right: attributionRight }}
              elementRef={attributionParseIndicatorClearance}
            />
            {/* Export mode: large WYSIWYG preview of the image export, overlaid
                on the (still-mounted) map. Only for the image export type -
                data exports keep the map visible. */}
            {appMode === 'export' && exportPreviewType === 'image' && (
              <div
                className="absolute inset-0 z-20 bg-background flex items-center justify-center p-6"
                data-testid="export-mode-preview"
              >
                <ErrorBoundary panelName="Export Preview">
                  <ExportPreview options={exportImageOptions} fill />
                </ErrorBoundary>
              </div>
            )}
          </div>

          {/* Data (bottom) - inside the center column so docked side panels push
              it (and its tab) for free. Fullscreen renders without reparenting. In
              Plan mode it holds the tabbed plan tables ("Gear"); Export hides it
              (map only), and Plan hides it too until an origin is chosen - the
              staged entry screen is the question card plus the map, nothing else
              (#395). */}
          {appMode !== 'export' && !(appMode === 'plan' && !gasOriginChosen) && (
          <Dock
            reveal={dataReveal}
            edge="bottom"
            size={dataDisplayHeight}
            minSize={PANEL_SIZE_BOUNDS['data-table'].min}
            maxSize={PANEL_SIZE_BOUNDS['data-table'].max}
            onResize={applyDataHeight}
            onResizeEnd={handleDataResizeEnd}
            label={panelLabels.name.data}
            openArrow={CHEVRON_UP}
            closeArrow={CHEVRON_DOWN}
            testId="data"
            tabLabels={panelLabels.data}
            // Keep the Data tab clear of the Details tab on the right (it can
            // ride up to the same height when the Data panel is tall). Shared
            // with the imagery attribution bar, which butts against this tab.
            tabInset={DATA_DOCK_TAB_INSET}
            onTabBoundsChange={handleDataTabBounds}
            fullscreen={dataTableFullScreen}
            // Portal the overlay to Main so it can span the full width (the
            // reflow spacer stays in the center column to push the map up).
            overlayContainer={mainEl}
            overlayLeft={dataOverlayLeft}
            overlayRight={dataOverlayRight}
            onResizingChange={onDataResizing}
            suspendSiblingTransition={sidePanelResizing}
          >
            {dataInner}
          </Dock>
          )}
        </div>

        {/* Details (right) */}
        <Dock
          reveal={detailReveal}
          edge="right"
          // See the Sections Dock: display-only squeeze, never persisted.
          size={detailDisplayWidth}
          minSize={PANEL_SIZE_BOUNDS.detail.min}
          maxSize={PANEL_SIZE_BOUNDS.detail.max}
          onResize={applyDetailWidth}
          onResizeEnd={handleDetailResizeEnd}
          label={panelLabels.name.detail}
          openArrow={CHEVRON_LEFT}
          closeArrow={CHEVRON_RIGHT}
          testId="detail"
          tabLabels={panelLabels.detail}
          tabTop={DETAILS_TAB_TOP}
          closedBottomInset={dataReveal.isOpen ? dataDisplayHeight : 0}
          suspendSiblingTransition={dataResizing}
          onResizingChange={onDetailResizing}
        >
          {detailContent}
        </Dock>
      </div>

      {/* Status bar. It sits just below the Data panel, so treat it as part of
          the Data hover region: keep an already-open (hover) Data panel open
          while the pointer rests here, but don't open it from closed. */}
      <footer
        ref={statusBarToastClearance}
        onMouseEnter={() => {
          if (dataReveal.isFloatingOpen) dataReveal.onRegionEnter();
        }}
        onMouseLeave={dataReveal.onRegionLeave}
        className="px-4 py-2 border-t border-border text-sm text-text-secondary flex justify-between"
        style={{ backgroundImage: statusBarGradient }}
        data-testid="status-bar"
      >
        <span>
          {t('singleView.status.summary', {
            sections: formatNumber(stats?.section_count ?? 0),
            shots: formatNumber(stats?.shot_count ?? 0),
            length: formatLength(stats?.total_length ?? 0, precision.length),
          })}
          {focusedSectionName !== null && (
            <span data-testid="status-focused-section" className="ml-2 text-accent">
              {' | '}
              {t('singleView.status.focusedSection', { name: focusedSectionName })}
            </span>
          )}
        </span>
        <SaveStatus
          status={autoSave.status}
          countdown={autoSave.countdown}
          isDirty={isDirty}
          onSave={handleSaveProject}
        />
      </footer>
    </div>
  );
}

function SaveStatus({
  status,
  countdown,
  isDirty,
  onSave,
}: {
  status: AutoSaveStatus;
  countdown: number;
  isDirty: boolean;
  onSave: () => void;
}) {
  const { t } = useTranslation();

  let text: string;
  let className = '';

  switch (status) {
    case 'saving':
      text = t('singleView.status.saving');
      break;
    case 'saved':
      text = t('singleView.status.saved');
      break;
    case 'pending':
      text = t('singleView.status.autoSaveCountdown', { seconds: countdown });
      break;
    case 'error':
      text = t('singleView.status.saveError');
      className = 'text-red-400';
      break;
    case 'idle':
    default:
      text = isDirty
        ? t('singleView.status.unsavedChanges')
        : t('singleView.status.saved');
      if (isDirty) className = 'text-yellow-400';
      break;
  }

  // On a failed save, the status doubles as a retry - clicking it runs the same
  // save as the burger-menu Save button.
  if (status === 'error') {
    return (
      <button
        type="button"
        data-testid="save-status"
        onClick={() => onSave()}
        title={t('common.actions.save')}
        className={`${className} cursor-pointer hover:underline`}
      >
        {text}
      </button>
    );
  }

  return (
    <span data-testid="save-status" className={className}>
      {text}
    </span>
  );
}

function formatLength(meters: number, lengthPrecision: number): string {
  if (meters >= 1000) {
    return `${formatFixed(meters / 1000, undefined, lengthPrecision)} km`;
  }
  return `${formatFixed(meters, undefined, lengthPrecision)} m`;
}
