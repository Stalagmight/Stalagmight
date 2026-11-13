import { useState, useEffect, useLayoutEffect, useMemo, useCallback, useRef, type CSSProperties, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { type Section, type Shot, type ShotData, type Station, type StationRef } from '../../types/project';
import { useProjectStore } from '../../store/projectStore';
import { useActiveStations } from '../../hooks/useActiveStations';
import { useUIStore, type Selection } from '../../store/uiStore';
import { useEditingBlocked } from '../../hooks/useEditingBlocked';
import { useShallow } from 'zustand/react/shallow';
import { SpreadsheetGrid } from '../common/SpreadsheetGrid';
import { EntityBlob } from '../common/EntityBlob';
import { ChevronDownIcon, ExpandIcon } from '../common/icons';
import type { ProjectWarning } from '../../types/commands';
import { formatShotId, formatStationId } from '../../utils/formatters';
import { getDeleteShotDialog, deriveRangeDeleteParams } from '../../utils/contextMenuHelpers';
import { getNumberFormatLocale } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { useGridLabels } from '../../hooks/useGridLabels';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';
import { ShotColumnsMenu } from '../common/ShotColumnsMenu';
import { ANGLE_FAMILY_KEYS, reorderForAngleMode } from '../Dialogs/newSectionRows';
import { useAnchoredFixedMenu } from '../../hooks/useAnchoredFixedMenu';
import { setSource } from '../../utils/telemetry';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import { invokeIpc } from '../../utils/ipc';
import { buildShotColumns } from './dataTableShotColumns';
import { CREDIT_COLUMN_KEYS, isCreditColumn } from './dataTableCreditColumns';
import { useDataTableCredit } from './useDataTableCredit';
import { buildStationColumns } from './dataTableStationColumns';
import RawTableView from './RawTableView';
import { DetailLink } from '../common/DetailLink';
import { buildShotUpdates, buildShotUpdateBatch, AZIMUTH_AVG_KEY, PITCH_AVG_KEY } from './dataTableEdits';
import { useDataTableViewPersistence } from './useDataTableViewPersistence';
import { OpenInDataButton } from '../DataWorkspace/OpenInDataButton';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { canSplitShot } from '../../utils/shotSplit';
import { nextShotKind } from '../../utils/shotKind';
import {
  type RowEntry,
  type ShotDraft,
  endDraft,
  interleaveRows,
  pruneDrafts,
  insertDraftsAt,
  materializeDraft,
  overflowDraft,
  respawnsBlank,
  draftMissing,
  completeDraftData,
  commitAnchorArgs,
} from './dataTableDrafts';
import type { CellEdit } from '../common/SpreadsheetGrid/types';

interface DataTableProps {
  sections: Section[];
}

/** Chrome height above the docked grid body (the toolbar row), used as the
 *  initial cached offset before the ResizeObserver measures the real value. */
const CHROME_OFFSET_DEFAULT = 28;
/** Floor for the computed grid height so it never collapses below a usable size. */
const MIN_GRID_HEIGHT = 120;

/** Header right-click hide targets: the clicked column's display name and the
 *  key group it hides with (L/R/U/D hide together as LRUD). */
const HEADER_HIDE_GROUPS: Record<string, { nameKey: string; keys: string[] }> = {
  _warnings: { nameKey: 'warnings', keys: ['_warnings'] },
  length: { nameKey: 'length', keys: ['length'] },
  depth_in: { nameKey: 'depthIn', keys: ['depth_in'] },
  depth_out: { nameKey: 'depthOut', keys: ['depth_out'] },
  left: { nameKey: 'lrud', keys: ['left', 'right', 'up', 'down'] },
  right: { nameKey: 'lrud', keys: ['left', 'right', 'up', 'down'] },
  up: { nameKey: 'lrud', keys: ['left', 'right', 'up', 'down'] },
  down: { nameKey: 'lrud', keys: ['left', 'right', 'up', 'down'] },
  kind: { nameKey: 'reference', keys: ['kind'] },
  comment: { nameKey: 'comment', keys: ['comment'] },
  section_id: { nameKey: 'sectionId', keys: ['section_id'] },
  shot_id: { nameKey: 'shotId', keys: ['shot_id'] },
};

/** Stations-view column groups in the eye menu / header hide entries (plain
 *  hide - no angle modes). X/Y toggle together as "Coordinates (m)", lat/long
 *  as the GPS pair. The index pill lives in the gutter, so it never hides. */
const STATION_MENU_COLS: { keys: string[]; nameKey: string }[] = [
  { keys: ['name'], nameKey: 'name' },
  { keys: ['_x', '_y'], nameKey: 'coordsMeters' },
  { keys: ['_lat', '_long'], nameKey: 'coordsGps' },
  { keys: ['_depth'], nameKey: 'depth' },
  { keys: ['_zeroDatum'], nameKey: 'zeroDatum' },
  { keys: ['_connections'], nameKey: 'connections' },
  { keys: ['comment'], nameKey: 'comment' },
];

/** Coordinate and station-name columns hidden by default (matches the backend
 *  serde default), used only as the pre-config-load initial state - once the
 *  persisted config arrives it is authoritative (an empty list there means
 *  "show all"; the backend hides a column added since then once on load). */
const STATION_DEFAULT_HIDDEN = ['_x', '_y', '_lat', '_long', 'name'];

/** Pill-as-trigger picker used in the sub-header: the whole pill (rendered via
 *  {@link renderTrigger}, which embeds the chevron) toggles the dropdown.
 *  Closes on outside click and Escape (and declares data-dropdown-open so the
 *  global Escape guard yields to it). The list anchors centered under the pill. */
function SubHeaderPickerMenu({
  triggerTestId,
  listTestId,
  title,
  options,
  onPick,
  onHoverOption,
  onHoverTrigger,
  renderTrigger,
}: {
  triggerTestId?: string;
  listTestId: string;
  title: string;
  options: { value: string; label: string; disabled?: boolean; dot?: string }[];
  onPick: (value: string) => void;
  onHoverOption?: (value: string | null) => void;
  onHoverTrigger?: (over: boolean) => void;
  renderTrigger: (state: { open: boolean }) => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const toggle = () => setOpen((v) => !v);
  // Position the list with `position:fixed` so it escapes the Dock's
  // overflow-hidden clip box when it flips up (the data-table sub-header sits
  // low in the window); the hook flips/clamps within the viewport.
  const listStyle = useAnchoredFixedMenu(open, triggerRef, listRef, {
    align: 'center',
    gap: 4,
    width: 208, // px; the hook also sets this as the list width (single source of truth)
    maxHeightCap: 256, // matches the old max-h-64 - stay compact + scroll on a tall window
    // The raw-table submenu feeds async rawTables through `options`: it opens on
    // a single disabled "loading" row, then swaps to N rows once the load
    // resolves. Signal that resize so the hook re-places instead of sticking to
    // the stale on-open placement. The length prefix disambiguates the loading
    // placeholder ([{value:''}], length 1) from an empty result ([], length 0) -
    // both would otherwise join to the same '' key.
    remeasureKey: `${options.length}:${options.map((o) => o.value).join('|')}`,
  });
  // Close on outside mousedown / external scroll / resize (shared hook). The
  // list is position:fixed at a captured rect, so any scroll/resize would
  // otherwise leave it floating at a stale spot. Escape is handled separately
  // below.
  useDismissableLayer({
    open,
    onDismiss: () => setOpen(false),
    contains: [triggerRef, listRef],
  });
  useEffect(() => {
    if (!open) return undefined;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('keydown', onKey);
    };
  }, [open]);
  return (
    // `min-w-0` down the whole trigger chain: the pill lives in a grid track that
    // is allowed to shrink, and every box between that track and the truncating
    // label has to be allowed to shrink with it (#429).
    <div className="relative min-w-0" onMouseDown={(e) => e.stopPropagation()}>
      <button
        ref={triggerRef}
        type="button"
        onClick={toggle}
        onMouseEnter={onHoverTrigger ? () => onHoverTrigger(true) : undefined}
        onMouseLeave={onHoverTrigger ? () => onHoverTrigger(false) : undefined}
        className="flex min-w-0 max-w-full cursor-pointer items-center"
        title={title}
        data-testid={triggerTestId}
      >
        {renderTrigger({ open })}
      </button>
      {open && (
        <div
          ref={listRef}
          data-dropdown-open
          className="overflow-auto rounded border border-border bg-panel p-1 text-xs shadow-lg"
          style={listStyle}
          data-testid={listTestId}
          onMouseLeave={() => onHoverOption?.(null)}
        >
          {options.map((o) => (
            <button
              key={o.value}
              data-testid={`${listTestId}-option-${encodeURIComponent(o.value)}`}
              disabled={o.disabled}
              className="flex w-full items-center gap-1.5 rounded px-2 py-0.5 text-left hover:bg-accent-10 disabled:opacity-50"
              onMouseEnter={() => onHoverOption?.(o.value)}
              onClick={() => {
                onHoverOption?.(null);
                setOpen(false);
                onPick(o.value);
              }}
            >
              {o.dot !== undefined && (
                <span
                  className="h-2 w-2 shrink-0 rounded-full"
                  style={{ backgroundColor: o.dot }}
                  aria-hidden="true"
                />
              )}
              <span className="truncate">{o.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * The middle cell of {@link SubHeaderRow}: holds the centered pill, and shrinks
 * to nothing before it will push on the controls either side of it.
 *
 * Centering here is a property of the ROW, not of this box - the two side columns
 * are equal `1fr`, so the middle track lands on the header's true midpoint. The
 * previous cut instead spanned the full width with `absolute inset-x-0` and a
 * `pointer-events-none` band, which centered just as well but took no part in the
 * layout: at 1024x640 the pill simply grew over the "Shots" button and its
 * `pointer-events-auto` wrapper swallowed the clicks (#429).
 *
 * Still no CSS `transform` anywhere on this path. A transformed ancestor would
 * make the picker's `position:fixed` list resolve against this box instead of the
 * viewport, re-introducing the Dock-clip that fix escapes (#162).
 */
function CenteredPillBand({ children }: { children: ReactNode }) {
  return <div className="flex min-w-0 justify-center">{children}</div>;
}

/**
 * The data table's sub-header row: `title | centered pill | controls`.
 *
 * A three-column grid rather than a flex row, because the pill has to center on
 * the full width while the side groups keep their natural size. The outer columns
 * are `1fr` with their default `auto` minimum, so they never shrink below their
 * content; the middle one is `minmax(0, auto)`, so it is the one that gives way
 * and the pill truncates instead of overlapping (#429).
 */
function SubHeaderRow({ children }: { children: ReactNode }) {
  return (
    <div
      className="relative grid grid-cols-[1fr_minmax(0,auto)_1fr] items-center gap-3 px-3 py-1 border-b border-border shrink-0 text-xs"
      data-testid="data-table-sub-header"
    >
      {children}
    </div>
  );
}

/** The chevron that sits INSIDE the sub-header pill, dim until the menu opens. */
function PillChevron({ open, testId }: { open: boolean; testId: string }) {
  return (
    <span className={`ml-0.5 ${open ? 'text-text' : 'text-text-secondary'}`} data-testid={testId}>
      <ChevronDownIcon size={12} />
    </span>
  );
}

/** Raw-table name pill: neutral gray sibling of the section EntityBlob, with
 *  the same syntactic-level gate (plain text below 'full'). `trailing` (the
 *  picker chevron) renders inside the pill. */
function RawTablePill({ name, trailing }: { name: string; trailing?: ReactNode }) {
  const level = useUIStore((s) => s.syntacticLevel);
  if (!syntacticLevelAtLeast(level, 'full')) {
    return (
      <span
        className="inline-flex min-w-0 max-w-full items-center font-medium"
        data-testid="data-table-raw-name"
      >
        <span className="truncate">{name}</span>
        {trailing}
      </span>
    );
  }
  return (
    <span
      className="sh-pill inline-flex min-w-0 max-w-full items-center gap-1 align-middle rounded-full px-2 py-0.5 text-xs leading-none text-text-primary tabular-nums"
      style={{ '--o': 'var(--cc-text-secondary)' } as CSSProperties}
      data-testid="data-table-raw-name"
    >
      <span className="truncate">{name}</span>
      {trailing}
    </span>
  );
}

export default function DataTable({ sections }: DataTableProps) {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  const [selectedSectionId, setSelectedSectionId] = useState<number | null>(
    sections[0]?.id ?? null
  );
  const [tableView, setTableView] = useState<'shots' | 'stations'>('shots');
  // Track key for scrolling to a row (set when external selection changes)
  const [scrollToKey, setScrollToKey] = useState<number | null>(null);
  const [stationScrollToKey, setStationScrollToKey] = useState<number | null>(null);
  // Track previous selection to only react to actual selection changes
  const prevSelectionRef = useRef<Selection | null>(null);
  // Flag to skip scroll when selection originated from within the table
  const internalSelectionRef = useRef(false);

  const { updateShot, setShotKind, warnings, isShotFrozen, updateStationComment, executeBatch, createShot, isDirty } = useProjectStore();
  // Coordinate columns follow the active geometry (adjusted in the read-only
  // closure-adjusted view); row refs/comments stay measured for editing.
  const activeStations = useActiveStations();
  // Closure-adjusted view, Plan/Export mode, and a locked remote SpeleoDB project are all
  // read-only: cell editing and the reference toggle are disabled.
  const editingBlocked = useEditingBlocked();
  const {
    selection,
    setSelection,
    setHoveredShot,
    hoveredShot,
    triggerPanToShot,
    dataTableHeight,
    dataTableFullScreen,
    rawTableSelection,
    setRawTableSelection,
    enterDataTableFullScreen,
    announceSelection,
    setHoveredSectionId,
    openDialog,
    triggerCenterOnStation,
    setHoveredStation,
    hoveredStation,
    showToast,
    triggerResolveWarning,
  } = useUIStore(useShallow((s) => ({
    selection: s.selection,
    setSelection: s.setSelection,
    setHoveredShot: s.setHoveredShot,
    hoveredShot: s.hoveredShot,
    triggerPanToShot: s.triggerPanToShot,
    triggerResolveWarning: s.triggerResolveWarning,
    dataTableHeight: s.dataTableHeight,
    dataTableFullScreen: s.dataTableFullScreen,
    rawTableSelection: s.rawTableSelection,
    setRawTableSelection: s.setRawTableSelection,
    enterDataTableFullScreen: s.enterDataTableFullScreen,
    announceSelection: s.announceSelection,
    setHoveredSectionId: s.setHoveredSectionId,
    openDialog: s.openDialog,
    triggerCenterOnStation: s.triggerCenterOnStation,
    setHoveredStation: s.setHoveredStation,
    hoveredStation: s.hoveredStation,
    showToast: s.showToast,
  })));

  // Create a map of shot warnings for quick lookup
  const shotWarningsMap = useMemo(() => {
    const map = new Map<string, ProjectWarning[]>();
    for (const warning of warnings) {
      const key = `${warning.sectionId}:${warning.shotId}`;
      if (!map.has(key)) {
        map.set(key, []);
      }
      map.get(key)!.push(warning);
    }
    return map;
  }, [warnings]);

  const getShotWarnings = (sectionId: number, shotId: number): ProjectWarning[] => {
    return shotWarningsMap.get(`${sectionId}:${shotId}`) ?? [];
  };

  // Update selected section when sections change (e.g., first load or section deleted)
  useEffect(() => {
    if (sections.length === 0) {
      // No sections - clear selection
      setSelectedSectionId(null);
      return;
    }

    if (selectedSectionId === null) {
      // No selection yet - select first section
      const firstSection = sections[0];
      if (firstSection) {
        setSelectedSectionId(firstSection.id);
      }
      return;
    }

    // Check if currently selected section still exists
    const sectionExists = sections.some((s) => s.id === selectedSectionId);
    if (!sectionExists) {
      // Section was deleted - switch to first remaining section
      const firstSection = sections[0];
      setSelectedSectionId(firstSection?.id ?? null);
    }
  }, [sections, selectedSectionId]);

  // Sync external selection (from Map, Sections Panel) to DataTable
  // Only react to actual selection changes (not sections data changes)
  useEffect(() => {
    // Check if selection actually changed
    const prevSelection = prevSelectionRef.current;
    const selectionChanged =
      selection !== prevSelection &&
      (selection === null ||
        prevSelection === null ||
        selection.type !== prevSelection.type ||
        selection.sectionId !== prevSelection.sectionId ||
        selection.shotId !== prevSelection.shotId ||
        selection.stationRef?.section_id !== prevSelection.stationRef?.section_id ||
        selection.stationRef?.station_index !== prevSelection.stationRef?.station_index);

    const referenceChanged = selection !== prevSelection;
    prevSelectionRef.current = selection;

    if (!selectionChanged) {
      // A table-internal re-dispatch of the SAME selection (e.g. keyboard
      // column moves re-selecting the active shot) still consumed its event -
      // a stale flag here would swallow the next genuinely external selection.
      if (referenceChanged && internalSelectionRef.current) {
        internalSelectionRef.current = false;
      }
      return;
    }

    // Skip scroll/view-switch for selections made from within the table
    if (internalSelectionRef.current) {
      internalSelectionRef.current = false;
      return;
    }

    if (!selection) {
      // No selection - clear scroll target but keep current dropdown
      setScrollToKey(null);
      return;
    }

    if (selection.type === 'shot' && selection.sectionId !== undefined && selection.shotId !== undefined) {
      // Shot selected externally - switch to shots view, switch dropdown, and scroll to row
      if (selection.sectionId !== selectedSectionId) {
        setSelectedSectionId(selection.sectionId);
      }
      if (tableView !== 'shots') setTableView('shots');
      setScrollToKey(selection.shotId);
    } else if (selection.type === 'station' && selection.stationRef) {
      const { section_id, station_index } = selection.stationRef;
      const section = sections.find((s) => s.id === section_id);
      if (section) {
        // Switch section if needed
        if (section_id !== selectedSectionId) {
          setSelectedSectionId(section_id);
        }
        // Switch to stations view and scroll to station
        if (tableView !== 'stations') setTableView('stations');
        setStationScrollToKey(station_index);
      }
    } else if (selection.type === 'section' && selection.sectionId !== undefined) {
      // Section selected externally - switch dropdown, clear row highlight
      if (selection.sectionId !== selectedSectionId) {
        setSelectedSectionId(selection.sectionId);
      }
      setScrollToKey(null);
    }
  }, [selection, selectedSectionId, sections, tableView]);

  // Clear scrollToKey after it's been consumed (to allow re-scrolling to same row)
  useEffect(() => {
    if (scrollToKey === null) return;
    const timer = setTimeout(() => setScrollToKey(null), 100);
    return () => clearTimeout(timer);
  }, [scrollToKey]);

  useEffect(() => {
    if (stationScrollToKey === null) return;
    const timer = setTimeout(() => setStationScrollToKey(null), 100);
    return () => clearTimeout(timer);
  }, [stationScrollToKey]);

  const selectedSection = sections.find((s) => s.id === selectedSectionId);

  // Handle shot deletion - clear selection if selected shot no longer exists
  useEffect(() => {
    if (!selection || selection.type !== 'shot') return;
    if (selection.sectionId !== selectedSectionId) return;
    if (!selectedSection) return;

    // Check if the selected shot still exists in this section
    const shotExists = selectedSection.shots.some((s) => s.id === selection.shotId);
    if (!shotExists) {
      // Shot was deleted - clear selection
      setSelection(null);
    }
  }, [selection, selectedSection, selectedSectionId, setSelection]);

  // Announce shot selection changes to screen readers
  // Uses a ref to track previous selection and only announce actual changes
  const prevAnnouncedSelectionRef = useRef<{ sectionId: number; shotId: number } | null>(null);
  useEffect(() => {
    // Only announce shot selections
    if (!selection || selection.type !== 'shot') {
      prevAnnouncedSelectionRef.current = null;
      return;
    }
    if (selection.sectionId === undefined || selection.shotId === undefined) return;

    // Check if this is actually a new selection
    const prev = prevAnnouncedSelectionRef.current;
    if (prev && prev.sectionId === selection.sectionId && prev.shotId === selection.shotId) {
      return; // Same selection, don't re-announce
    }

    // Find the section for the selection (may differ from selectedSection during transitions)
    const section = sections.find((s) => s.id === selection.sectionId);
    if (!section) return;

    // Find shot position (1-indexed) for announcement
    const shotIndex = section.shots.findIndex((s) => s.id === selection.shotId);
    if (shotIndex === -1) return;

    const shotPosition = shotIndex + 1;
    const message = t('dataTable.announcement.shotSelected', {
      shotId: formatShotId(selection.sectionId, shotPosition),
    });
    announceSelection(message);

    // Update ref with new selection
    prevAnnouncedSelectionRef.current = { sectionId: selection.sectionId, shotId: selection.shotId };
  }, [selection, sections, announceSelection, t]);

  // Shot cell-editing, selection, pan, hover and context-menu are now handled by
  // SpreadsheetGrid via the onShot* handlers defined below.

  // Clicking the header's section pill selects the section app-wide (map/panels
  // follow), mirroring the section pills elsewhere in the app.
  // Track last hovered section ID for ref-based cleanup (matches SectionsPanel pattern)
  const lastHoveredSectionIdRef = useRef<number | null>(null);

  const handleSectionHover = useCallback(
    (value: string | null) => {
      if (value !== null) {
        const sectionId = Number(value);
        if (!Number.isFinite(sectionId)) {
          lastHoveredSectionIdRef.current = null;
          setHoveredSectionId(null);
          return;
        }
        lastHoveredSectionIdRef.current = sectionId;
        setHoveredSectionId(sectionId);
      } else {
        lastHoveredSectionIdRef.current = null;
        setHoveredSectionId(null);
      }
    },
    [setHoveredSectionId]
  );

  // Clear hoveredSectionId on unmount if it was set by this component
  useEffect(() => {
    return () => {
      if (lastHoveredSectionIdRef.current !== null) {
        const currentHoveredId = useUIStore.getState().hoveredSectionId;
        if (currentHoveredId === lastHoveredSectionIdRef.current) {
          useUIStore.getState().setHoveredSectionId(null);
        }
      }
    };
  }, []);

  // --- Station view data ---

  const sectionStations = useMemo((): Station[] => {
    if (tableView !== 'stations' || selectedSectionId === null) return [];
    // X/Y/Z columns follow the active geometry so they match the map; the row
    // refs/comments are unchanged, so comment editing still targets measured data.
    return activeStations.filter((s) => s.station_ref.section_id === selectedSectionId);
  }, [activeStations, selectedSectionId, tableView]);

  const handleViewChange = useCallback((value: 'shots' | 'stations') => {
    setTableView(value);
  }, []);


  // --- Shots view (SpreadsheetGrid) ----------------------------------------
  // The effective number locale (never the 'locale'/'system' sentinel) so the
  // grid parses numbers with the same separators it displays them in.
  const numberLocale = getNumberFormatLocale();

  const selectShotRow = useCallback(
    (shot: Shot) => {
      if (!selectedSection) return;
      internalSelectionRef.current = true;
      setSource('data-table');
      setSelection({ type: 'shot', sectionId: selectedSection.id, shotId: shot.id });
    },
    [selectedSection, setSelection]
  );

  // Single edit → one update_shot (one undo). Values are already coerced by the
  // column; the virtual azimuth-average expands to in/out via buildShotUpdates.
  // Column order: drag a selected column header to move it. Empty =
  // declaration order. Persisted with the rest of the view state below.
  const [shotColumnOrder, setShotColumnOrder] = useState<string[]>([]);

  // Column visibility. Azimuth/inclination are tri-state -
  // the average column, the in/out leg pair, or neither; the raw ID columns
  // start hidden; L/R/U/D toggle as one LRUD group.
  const [azimuthMode, setAzimuthMode] = useState<'avg' | 'inout' | 'off'>('avg');
  const [inclinationMode, setInclinationMode] = useState<'avg' | 'inout' | 'off'>('off');
  const [hiddenShotCols, setHiddenShotCols] = useState<ReadonlySet<string>>(
    () => new Set(['section_id', 'shot_id', ...CREDIT_COLUMN_KEYS])
  );
  // The stations view keeps its own order/visibility (no angle modes there);
  // persisted alongside the shot view state below.
  const [stationColumnOrder, setStationColumnOrder] = useState<string[]>([]);
  const [hiddenStationCols, setHiddenStationCols] = useState<ReadonlySet<string>>(
    () => new Set(STATION_DEFAULT_HIDDEN)
  );
  // Manual column widths, keyed by column key. Empty = every column keeps its
  // declared width. The grid commits these on resize release, not per mousemove.
  const [shotColumnWidths, setShotColumnWidths] = useState<Record<string, number>>({});
  const [stationColumnWidths, setStationColumnWidths] = useState<Record<string, number>>({});
  // Right-click on a column header: hide it / switch the angle family's mode.
  const [headerMenu, setHeaderMenu] = useState<{ x: number; y: number; colKey: string } | null>(null);
  const headerMenuRef = useRef<HTMLDivElement | null>(null);
  // The panel root, handed to the cursor-pinned menus below as their anchor
  // scope: they are fixed siblings of the grid, so a grid scroll strands them
  // without being an ancestor of anything in their `contains`.
  const panelRef = useRef<HTMLDivElement | null>(null);
  // Flip the menu above/left of the cursor when it would cross a viewport edge.
  useLayoutEffect(() => {
    const el = headerMenuRef.current;
    if (!el || !headerMenu) return;
    const r = el.getBoundingClientRect();
    let { x, y } = headerMenu;
    if (y + r.height > window.innerHeight) y = Math.max(4, y - r.height);
    if (x + r.width > window.innerWidth) x = Math.max(4, x - r.width);
    if (x !== headerMenu.x || y !== headerMenu.y) setHeaderMenu({ ...headerMenu, x, y });
  }, [headerMenu]);
  // Close on outside mousedown / external scroll / resize (shared hook): the
  // menu is position:fixed at captured cursor coords, so any scroll/resize would
  // otherwise leave it floating at a stale spot. Escape stays separate below.
  useDismissableLayer({
    open: headerMenu !== null,
    onDismiss: () => setHeaderMenu(null),
    contains: [headerMenuRef],
    anchorScope: panelRef,
    survivesUnrelatedScroll: true,
  });
  useEffect(() => {
    if (!headerMenu) return undefined;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setHeaderMenu(null);
    };
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('keydown', onKey);
    };
  }, [headerMenu]);

  const shotHiddenColumns = useMemo(() => {
    const hidden = [...hiddenShotCols];
    if (azimuthMode !== 'avg') hidden.push(AZIMUTH_AVG_KEY);
    if (azimuthMode !== 'inout') hidden.push('azimuth_in', 'azimuth_out');
    if (inclinationMode !== 'avg') hidden.push(PITCH_AVG_KEY);
    if (inclinationMode !== 'inout') hidden.push('pitch_in', 'pitch_out');
    return hidden;
  }, [hiddenShotCols, azimuthMode, inclinationMode]);

  // Toggle a column (or a whole group, e.g. LRUD): visible iff none of its
  // keys are hidden, so a mixed group resolves to "show all" first.
  const toggleShotCols = useCallback((keys: string[]) => {
    setHiddenShotCols((prev) => {
      const next = new Set(prev);
      const visible = keys.every((k) => !prev.has(k));
      for (const k of keys) {
        if (visible) next.add(k);
        else next.delete(k);
      }
      return next;
    });
  }, []);
  const toggleStationCols = useCallback((keys: string[]) => {
    setHiddenStationCols((prev) => {
      const next = new Set(prev);
      const visible = keys.every((k) => !prev.has(k));
      for (const k of keys) {
        if (visible) next.add(k);
        else next.delete(k);
      }
      return next;
    });
  }, []);

  const stationHiddenColumns = useMemo(() => [...hiddenStationCols], [hiddenStationCols]);


  // Persist the view state (order, visibility, angle modes, widths) in the app
  // config; see useDataTableViewPersistence for the seed/debounce/flush details.
  useDataTableViewPersistence(
    {
      shotColumnOrder,
      hiddenShotCols,
      azimuthMode,
      inclinationMode,
      stationColumnOrder,
      hiddenStationCols,
      shotColumnWidths,
      stationColumnWidths,
    },
    {
      setShotColumnOrder,
      setHiddenShotCols,
      setAzimuthMode,
      setInclinationMode,
      setStationColumnOrder,
      setHiddenStationCols,
      setShotColumnWidths,
      setStationColumnWidths,
    }
  );


  // --- Draft "new shot" rows --------------------------------------------------
  // The grid always shows one empty editable row at the end (the end draft);
  // "Insert shot above/below" adds more drafts anchored to a neighbouring shot.
  // A draft's values live here until length, both depths and an azimuth are
  // present, then it commits as a real shot via create_shot at its anchor (a
  // missing azimuth leg duplicates the other; inclination stays optional).
  const [drafts, setDrafts] = useState<ShotDraft[]>(() => [endDraft()]);
  // Draft keys come from a monotonic counter and are NEVER reused (incl. the
  // end draft, which gets a fresh key on every reset): the grid keys per-cell
  // state (missing-cell marks) by row identity, and a reused key would leak a
  // stale mark onto an unrelated later draft.
  const draftKeyRef = useRef(-2);
  useEffect(() => {
    setDrafts([endDraft()]);
    draftKeyRef.current = -2;
  }, [selectedSectionId]);
  // Drop drafts whose anchor shot was deleted (the end draft always survives).
  useEffect(() => {
    if (!selectedSection) return;
    setDrafts((prev) => pruneDrafts(prev, selectedSection.shots));
  }, [selectedSection]);

  // The grid's row list: shots interleaved with draft rows (drafts hidden in
  // the read-only closure-adjusted view). EVERY grid row index maps through
  // here - never into the shots array directly.
  const rowEntries = useMemo(
    (): RowEntry[] =>
      selectedSection ? interleaveRows(selectedSection.shots, editingBlocked ? [] : drafts) : [],
    [selectedSection, editingBlocked, drafts]
  );
  const shotIndexAt = useCallback(
    (gridRowIndex: number): number | null => {
      const entry = rowEntries[gridRowIndex];
      return entry?.kind === 'shot' ? entry.shotIndex : null;
    },
    [rowEntries]
  );

  // Rows handed to the grid, materializing each draft as a Shot-shaped row
  // whose id is the (negative) draft key.
  const gridShotRows = useMemo(
    () => rowEntries.map((e) => (e.kind === 'shot' ? e.shot : materializeDraft(e.draft))),
    [rowEntries]
  );

  // The grid's name-box content, lifted into the condensed panel header:
  // "<column> <row>" for a single cell, "3R × 2C" for a range.
  const [gridSel, setGridSel] = useState<{
    rows: number;
    cols: number;
    activeRow: number;
    activeColKey: string;
  } | null>(null);
  // Surveyors / Explorers / Survey date: columns and their popover editor.
  const credit = useDataTableCredit({
    t,
    sectionId: selectedSectionId,
    rows: gridShotRows,
    shotIndexAt,
    editingBlocked,
  });


  const applyDraftEdit = useCallback((key: number, colKey: string, value: unknown) => {
    setDrafts((prev) =>
      prev.map((draft) => {
        if (draft.key !== key) return draft;
        const d: Record<string, unknown> = { ...draft.data };
        // `undefined` means "drop the field" (a required cell cleared back to
        // missing) - delete the key so completeDraftData/draftMissing treat it
        // as absent and a stale value can't survive into the committed shot. A
        // `null` (cleared optional, e.g. inclination) is kept as an explicit
        // clear. The avg columns fan out to both legs.
        const put = (k: string) => {
          if (value === undefined) delete d[k];
          else d[k] = value;
        };
        if (colKey === AZIMUTH_AVG_KEY) {
          put('azimuth_in');
          put('azimuth_out');
        } else if (colKey === PITCH_AVG_KEY) {
          put('pitch_in');
          put('pitch_out');
        } else {
          put(colKey);
        }
        return { ...draft, data: d as Partial<ShotData> };
      })
    );
  }, []);

  // A required cell cleared to "missing" on a draft row: drop the field so a
  // later auto-commit can't resurrect the value the user just blanked. Edits on
  // real shots keep their model value (nothing to do here).
  const onShotClearMissing = useCallback(
    (rowIndex: number, colKey: string) => {
      if (editingBlocked) return;
      const entry = rowEntries[rowIndex];
      if (entry?.kind === 'draft') applyDraftEdit(entry.draft.key, colKey, undefined);
    },
    [editingBlocked, rowEntries, applyDraftEdit]
  );

  // Discard the draft rows in a grid-row range; the end draft resets to empty
  // instead of disappearing (it is always present).
  const discardDraftRows = useCallback(
    (fromRow: number, toRow: number) => {
      const keys = new Set<number>();
      for (const entry of rowEntries.slice(fromRow, toRow + 1)) {
        if (entry.kind === 'draft') keys.add(entry.draft.key);
      }
      if (keys.size === 0) return;
      setDrafts((prev) =>
        prev.flatMap((d) =>
          keys.has(d.key) ? (respawnsBlank(d) ? [endDraft(draftKeyRef.current--)] : []) : [d]
        )
      );
    },
    [rowEntries]
  );

  // Each draft's still-missing required fields: visible columns tint their
  // draft cell red; hidden ones tint the columns (eye) button and their
  // dropdown row instead, until shown or filled. Inserted drafts surface them
  // immediately; the end draft only once it holds a real value - clearing an
  // optional cell on it leaves `comment: null` behind (see applyDraftEdit) and
  // must not turn the row red.
  const draftMissingByKey = useMemo(() => {
    const map = new Map<number, ReturnType<typeof draftMissing>>();
    for (const d of drafts) map.set(d.key, draftMissing(d, hiddenShotCols, azimuthMode));
    return map;
  }, [drafts, hiddenShotCols, azimuthMode]);
  const draftHiddenProblems = useMemo(() => {
    const union = new Set<string>();
    for (const m of draftMissingByKey.values()) for (const k of m.hiddenProblems) union.add(k);
    return union;
  }, [draftMissingByKey]);

  const shotCellClassName = useCallback(
    (shot: Shot, _rowIndex: number, colKey: string) =>
      shot.id < 0 && draftMissingByKey.get(shot.id)?.cells.has(colKey) ? 'sg-cell-missing' : undefined,
    [draftMissingByKey]
  );

  // Commit a draft once its required fields are present - one at a time; the
  // refresh after create_shot re-runs the effect for the next complete one.
  const draftCommitting = useRef(false);
  useEffect(() => {
    if (!selectedSection || draftCommitting.current) return;
    for (const draft of drafts) {
      const data = completeDraftData(draft);
      if (data === null) continue;
      const { afterShotId, beforeShotId } = commitAnchorArgs(draft, selectedSection.shots);
      draftCommitting.current = true;
      setSource('data-table');
      void createShot(selectedSection.id, data, afterShotId, beforeShotId)
        .then(() =>
          setDrafts((prev) =>
            prev.flatMap((d) =>
              d.key === draft.key ? (respawnsBlank(d) ? [endDraft(draftKeyRef.current--)] : []) : [d]
            )
          )
        )
        .finally(() => {
          draftCommitting.current = false;
        });
      return;
    }
  }, [drafts, selectedSection, createShot]);


  const onShotEdit = useCallback(
    (rowIndex: number, colKey: string, value: unknown) => {
      if (!selectedSection || editingBlocked) return;
      const entry = rowEntries[rowIndex];
      if (!entry) return;
      if (entry.kind === 'draft') {
        applyDraftEdit(entry.draft.key, colKey, value);
        return;
      }
      selectShotRow(entry.shot);
      if (isCreditColumn(colKey)) return credit.applyCell(entry.shot, colKey, value);
      const updates = buildShotUpdates(entry.shot, [{ colKey, value }]);
      if (Object.keys(updates).length > 0) void updateShot(selectedSection.id, entry.shot.id, updates);
    },
    [selectedSection, editingBlocked, rowEntries, selectShotRow, updateShot, applyDraftEdit, credit]
  );

  // Multi-cell paste/fill → one execute_batch (one undo).
  /** Blank row the grid parses and validates pasted values against before this
   *  host has created them. Read, never written. */
  const shotGrowRows = useMemo(
    () => ({ growRowTemplate: () => materializeDraft(endDraft(-1)) }),
    []
  );

  const onShotBatchEdit = useCallback(
    (edits: CellEdit[]) => {
      if (!selectedSection || editingBlocked) return;
      setSource('data-table');
      // Edits landing on draft rows (paste/fill into them) feed the drafts;
      // the rest are remapped to shots-array indices and commit as one batch.
      // Rows past the end are a paste that ran off the single trailing draft.
      // Give each one its OWN draft, appended in one update, before any edit is
      // applied - otherwise the grid clips them and only the first new shot
      // survives.
      const overflowRows = [...new Set(edits.map((e) => e.row))]
        .filter((row) => row >= rowEntries.length)
        .sort((a, b) => a - b);
      const overflowKeys = new Map<number, number>();
      if (overflowRows.length > 0) {
        // Overflow rows, NOT canonical trailing blanks: end-anchored so the
        // pasted rows stay in order, but each disappears once committed rather
        // than leaving one empty row per pasted row.
        const fresh = overflowRows.map(() => overflowDraft(draftKeyRef.current--));
        overflowRows.forEach((row, i) => overflowKeys.set(row, fresh[i]!.key));
        setDrafts((prev) => [...prev, ...fresh]);
      }

      const shotEdits: CellEdit[] = [];
      for (const e of edits) {
        const overflowKey = overflowKeys.get(e.row);
        if (overflowKey !== undefined) {
          applyDraftEdit(overflowKey, e.colKey, e.value);
          continue;
        }
        const entry = rowEntries[e.row];
        if (!entry) continue;
        if (entry.kind === 'draft') applyDraftEdit(entry.draft.key, e.colKey, e.value);
        else shotEdits.push({ ...e, row: entry.shotIndex });
      }
      const commands = buildShotUpdateBatch(shotEdits, selectedSection.shots, selectedSection.id);
      if (commands.length > 0) void executeBatch(commands);
    },
    [selectedSection, editingBlocked, rowEntries, executeBatch, applyDraftEdit]
  );

  const onShotActiveCellChange = useCallback(
    (rowIndex: number) => {
      const entry = rowEntries[rowIndex];
      if (entry?.kind === 'shot') selectShotRow(entry.shot);
    },
    [rowEntries, selectShotRow]
  );

  const onShotRowHover = useCallback(
    (rowIndex: number | null) => {
      if (rowIndex === null || !selectedSection) {
        setHoveredShot(null);
        return;
      }
      const entry = rowEntries[rowIndex];
      setHoveredShot(
        entry?.kind === 'shot' ? { sectionId: selectedSection.id, shotId: entry.shot.id } : null
      );
    },
    [rowEntries, selectedSection, setHoveredShot]
  );

  // Map → grid hover tint (the reverse of onShotRowHover; purely visual, so the
  // pair can't feed back into each other).
  const shotHoveredRow = useMemo(() => {
    if (hoveredShot?.sectionId !== selectedSectionId) return null;
    const idx = rowEntries.findIndex((e) => e.kind === 'shot' && e.shot.id === hoveredShot.shotId);
    return idx >= 0 ? idx : null;
  }, [hoveredShot, rowEntries, selectedSectionId]);

  const shotRowClassName = useCallback(
    (shot: Shot, rowIndex: number) => {
      if (shot.id < 0) return 'sg-row-draft';
      const shotIndex = shotIndexAt(rowIndex);
      return selectedSectionId !== null && shotIndex !== null && isShotFrozen(selectedSectionId, shotIndex)
        ? 'sg-row-frozen'
        : undefined;
    },
    [selectedSectionId, isShotFrozen, shotIndexAt]
  );

  // "Insert N shots above/below" → blank draft rows anchored at the boundary;
  // they commit (and turn real) once filled.
  const onShotInsertRows = useCallback(
    (beforeIndex: number, count: number) => {
      if (!selectedSection || editingBlocked) return;
      const next = insertDraftsAt(selectedSection.shots, drafts, beforeIndex, count, () => draftKeyRef.current--);
      if (next) setDrafts(next);
    },
    [selectedSection, editingBlocked, drafts]
  );

  // "Remove draft row" appears when the selection covers draft rows only -
  // discarding a draft never asks for confirmation (nothing is stored yet).
  // "Insert Station..." appears for a single selected real shot row long
  // enough to split (#59) - the data table's live equivalent of the map
  // ContextMenu's shot-menu item; kept as the same literal (untranslated)
  // label so the two entry points read identically.
  const shotExtraMenuItems = useCallback(
    ({ fromRow, toRow }: { fromRow: number; toRow: number }) => {
      if (editingBlocked) return [];
      const entries = rowEntries.slice(fromRow, toRow + 1);
      if (entries.length === 0) return [];

      if (entries.every((e) => e.kind === 'draft')) {
        return [
          {
            label: t('dataTable.removeDraft', { count: entries.length }),
            onClick: () => discardDraftRows(fromRow, toRow),
          },
        ];
      }

      const only = entries.length === 1 ? entries[0] : undefined;
      if (
        selectedSection &&
        only?.kind === 'shot' &&
        canSplitShot(only.shot.length)
      ) {
        const { shot } = only;
        const sectionId = selectedSection.id;
        const sectionName = selectedSection.name ?? t('sections.defaultName', { id: sectionId });
        return [
          {
            label: 'Insert Station...',
            onClick: () => {
              setSource('data-table');
              openDialog('insert-station', {
                sectionId,
                shotId: shot.id,
                shotLength: shot.length,
                sectionName,
              });
            },
          },
        ];
      }

      return [];
    },
    [rowEntries, editingBlocked, selectedSection, t, discardDraftRows, openDialog]
  );

  // Surface partially-applied pastes (only the non-zero parts).
  const onShotPasteReport = useCallback(
    ({ written, clipped, skipped }: { written: number; clipped: number; skipped: number }) => {
      const parts: string[] = [];
      if (written > 0) parts.push(t('dataTable.pasteReport.written', { count: written }));
      if (clipped > 0) parts.push(t('dataTable.pasteReport.clipped', { count: clipped }));
      if (skipped > 0) parts.push(t('dataTable.pasteReport.skipped', { count: skipped }));
      if (parts.length > 0) {
        const kind = clipped > 0 || skipped > 0 ? 'warning' : 'success';
        showToast(parts.join(' · '), TOAST_KEYS.DATA_TABLE_PASTE, kind);
      }
    },
    [t, showToast]
  );

  // Delete on a gutter-selected range (or the context menu's "Delete rows"):
  // draft rows in the range are discarded outright (the end draft resets, no
  // confirm - nothing is stored yet). A single real shot routes through the
  // shared position-aware helper (split-vs-shift choice for mid shots), matching
  // the detail panel / map. A contiguous run of >1 real shot opens the
  // bulk-delete flow (or the section-delete confirm when the whole section is
  // selected).
  /** Returns false because this NEVER deletes here: it opens a confirm and the
   *  rows are still on screen until the user answers. The grid collapses the
   *  selection only on a delete that happened, so cancelling the dialog no
   *  longer leaves the rows with their selection gone. */
  const onShotDeleteRows = useCallback(
    (fromRow: number, toRow: number): boolean => {
      if (!selectedSection || editingBlocked) return false;
      setSource('data-table');
      const inRange = rowEntries
        .slice(fromRow, toRow + 1)
        .flatMap((e) => (e.kind === 'shot' ? [{ shot: e.shot, shotIndex: e.shotIndex }] : []));
      discardDraftRows(fromRow, toRow);
      // Draft rows only: those ARE gone, so the collapse is right here.
      if (inRange.length === 0) return true;

      const total = selectedSection.shots.length;
      const sectionName =
        selectedSection.name ?? t('sections.defaultName', { id: selectedSection.id });

      if (inRange.length === 1) {
        const only = inRange[0]!;
        const { dialog, data } = getDeleteShotDialog(
          selectedSection.id,
          only.shot.id,
          only.shotIndex === 0,
          only.shotIndex === total - 1,
          total,
          sectionName
        );
        openDialog(dialog, data);
        return false;
      }

      const firstIdx = inRange[0]!.shotIndex;
      const lastIdx = inRange[inRange.length - 1]!.shotIndex;
      // Defensive: a discarded draft must not have split the real-shot run (the
      // backend deletes the positional closure [start..end], so a gap would
      // remove unselected shots). Drafts only trail the real shots, so this is
      // unreachable in practice.
      if (lastIdx - firstIdx + 1 !== inRange.length) {
        showToast(t('dataTable.multiDeleteUnsupported'), TOAST_KEYS.DATA_TABLE_DELETE, 'warning');
        return false;
      }

      const { kind } = deriveRangeDeleteParams(firstIdx, lastIdx, total);
      if (kind === 'whole') {
        // Deleting every shot removes the section - reuse the section confirm.
        openDialog('confirm-delete', {
          type: 'section',
          sectionId: selectedSection.id,
          sectionName,
        });
        return false;
      }

      openDialog('bulk-delete', {
        sectionId: selectedSection.id,
        startShotId: inRange[0]!.shot.id,
        endShotId: inRange[inRange.length - 1]!.shot.id,
        shotCount: inRange.length,
        kind,
        sectionName,
        startStationIndex: firstIdx,
        endStationIndex: lastIdx + 1,
      });
      // Never deleted here - a dialog is open and the rows are still on screen.
      return false;
    },
    [selectedSection, editingBlocked, rowEntries, discardDraftRows, showToast, t, openDialog]
  );

  const handleCycleShotKind = useCallback(
    (shot: Shot, direction: 1 | -1) => {
      if (!selectedSection || editingBlocked) return;
      const next = nextShotKind(shot.kind, direction);
      // A draft row's id is the draft key: no backend identity yet, so the
      // cycle only touches local state. Otherwise use the same command the
      // context menu and detail panel use - before #27 this path went through
      // the generic `update_shot` while those two used `set_shot_reference`.
      if (shot.id < 0) applyDraftEdit(shot.id, 'kind', next);
      else void setShotKind(selectedSection.id, shot.id, next);
    },
    [selectedSection, editingBlocked, setShotKind, applyDraftEdit]
  );

  const handlePanToShot = useCallback(
    (shot: Shot) => {
      if (selectedSection) {
        setSource('data-table');
        triggerPanToShot(selectedSection.id, shot.id);
      }
    },
    [selectedSection, triggerPanToShot]
  );

  const baseShotColumns = useMemo(
    () =>
      buildShotColumns({
        t,
        precision,
        sectionId: selectedSectionId,
        geometryReadOnly: editingBlocked,
        isShotFrozen,
        getShotWarnings,
        shotIndexAt,
        onCycleShotKind: handleCycleShotKind,
        onResolveWarning: (shotId: number) => {
          if (selectedSectionId === null) return;
          // triggerResolveWarning opens the Warnings panel (and exits
          // section-focus / fullscreen) so the resolve view is actually shown.
          triggerResolveWarning(selectedSectionId, shotId);
        },
      }),
    // getShotWarnings reads shotWarningsMap (derived from `warnings`); rebuild when
    // warnings change so frozen/warning icons stay current.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [t, precision, selectedSectionId, editingBlocked, isShotFrozen, warnings, shotIndexAt, handleCycleShotKind, triggerResolveWarning]
  );
  // The credit columns sit just before the comment, which absorbs the leftover width.
  const shotColumns = useMemo(() => {
    const at = baseShotColumns.findIndex((c) => c.key === 'comment');
    return [...baseShotColumns.slice(0, at), ...credit.columns, ...baseShotColumns.slice(at)];
  }, [baseShotColumns, credit.columns]);

  // Localized grid chrome; rowNoun makes the context menus speak in shots
  // (or stations), not rows.
  const gridLabels = useGridLabels('dataTable.shotNoun');
  const stationGridLabels = useGridLabels('dataTable.stationNoun');

  const shotScrollToRow = useMemo(() => {
    if (scrollToKey === null) return null;
    const idx = rowEntries.findIndex((e) => e.kind === 'shot' && e.shot.id === scrollToKey);
    return idx >= 0 ? idx : null;
  }, [scrollToKey, rowEntries]);

  const selectionIndicator = useMemo(() => {
    if (!gridSel || tableView !== 'shots' || !selectedSection) return null;
    if (gridSel.rows > 1 || gridSel.cols > 1) return `${gridSel.rows}R × ${gridSel.cols}C`;
    const header = shotColumns.find((c) => c.key === gridSel.activeColKey)?.header ?? gridSel.activeColKey;
    return `${header} ${gridSel.activeRow + 1}`;
  }, [gridSel, tableView, selectedSection, shotColumns]);

  // Gutter shows the shot's ID pill (replaces the old ID column); double-click
  // pans the map to the shot, as it did on the column's pill.
  const shotRowLabel = useCallback(
    (shot: Shot, rowIndex: number) => {
      // Draft rows earn their pill only once they become real shots.
      if (shot.id < 0) return <span className="opacity-50">+</span>;
      const shotIndex = shotIndexAt(rowIndex);
      return selectedSectionId !== null && shotIndex !== null ? (
        <span
          onDoubleClick={() => handlePanToShot(shot)}
          data-testid={`data-table-shot-row-label-${selectedSectionId}-${shot.id}`}
        >
          <EntityBlob entity="shot">{formatShotId(selectedSectionId, shotIndex + 1)}</EntityBlob>
        </span>
      ) : (
        rowIndex + 1
      );
    },
    [selectedSectionId, handlePanToShot, shotIndexAt]
  );

  // --- Stations view (SpreadsheetGrid) --------------------------------------
  // Stations are derived from shots, so only the comment is editable - a
  // single edit per commit (set_station_comment has no batch form, so the
  // grid gets no onBatchEdit).
  const onStationEdit = useCallback(
    (rowIndex: number, colKey: string, value: unknown) => {
      if (editingBlocked || colKey !== 'comment') return;
      const station = sectionStations[rowIndex];
      if (!station) return;
      setSource('data-table');
      const comment = typeof value === 'string' && value.trim() !== '' ? value : null;
      void updateStationComment(station.station_ref, comment);
    },
    [editingBlocked, sectionStations, updateStationComment]
  );

  const onStationActiveCellChange = useCallback(
    (rowIndex: number) => {
      const station = sectionStations[rowIndex];
      if (!station) return;
      internalSelectionRef.current = true;
      setSource('data-table');
      setSelection({ type: 'station', stationRef: station.station_ref });
    },
    [sectionStations, setSelection]
  );

  const onStationRowHover = useCallback(
    (rowIndex: number | null) => {
      const station = rowIndex !== null ? sectionStations[rowIndex] : undefined;
      setHoveredStation(station ? station.station_ref : null);
    },
    [sectionStations, setHoveredStation]
  );

  // Map → grid hover tint (mirrors shotHoveredRow).
  const stationHoveredRow = useMemo(() => {
    if (hoveredStation?.section_id !== selectedSectionId) return null;
    const idx = sectionStations.findIndex(
      (s) => s.station_ref.station_index === hoveredStation.station_index
    );
    return idx >= 0 ? idx : null;
  }, [hoveredStation, sectionStations, selectedSectionId]);

  const stationScrollToRow = useMemo(() => {
    if (stationScrollToKey === null) return null;
    const idx = sectionStations.findIndex(
      (s) => s.station_ref.station_index === stationScrollToKey
    );
    return idx >= 0 ? idx : null;
  }, [stationScrollToKey, sectionStations]);

  // GPS (zero-datum) cell click: a station that already is - or is merged
  // with - a zero datum opens the dialog in plain edit mode; the rest open in
  // add-zero-datum mode with the GPS input enabled and focused.
  const handleGpsCellClick = useCallback(
    (station: Station) => {
      setSource('data-table');
      const mergedWithDatum = station.merged_with.some((ref) =>
        activeStations.some(
          (s) =>
            s.station_ref.section_id === ref.section_id &&
            s.station_ref.station_index === ref.station_index &&
            s.is_zero_datum
        )
      );
      if (station.is_zero_datum || mergedWithDatum) {
        openDialog('edit-station', { stationRef: station.station_ref });
      } else {
        openDialog('edit-station', {
          stationRef: station.station_ref,
          forceEnableZeroDatum: true,
          focusGpsInput: true,
        });
      }
    },
    [activeStations, openDialog]
  );

  // Merged/closure picker opened from a connections cell: lists the linked
  // stations as Detail-panel-style links (click selects, hover highlights).
  const [connMenu, setConnMenu] = useState<{ x: number; y: number; station: Station } | null>(null);
  const connMenuRef = useRef<HTMLDivElement | null>(null);
  useLayoutEffect(() => {
    const el = connMenuRef.current;
    if (!el || !connMenu) return;
    const r = el.getBoundingClientRect();
    let { x, y } = connMenu;
    if (y + r.height > window.innerHeight) y = Math.max(4, y - r.height);
    if (x + r.width > window.innerWidth) x = Math.max(4, x - r.width);
    if (x !== connMenu.x || y !== connMenu.y) setConnMenu({ ...connMenu, x, y });
  }, [connMenu]);
  // Close on outside mousedown / external scroll / resize (shared hook): the
  // menu is position:fixed at captured cursor coords, so scroll/resize would
  // otherwise leave it floating at a stale spot. Escape stays separate below.
  useDismissableLayer({
    open: connMenu !== null,
    onDismiss: () => setConnMenu(null),
    contains: [connMenuRef],
    anchorScope: panelRef,
    survivesUnrelatedScroll: true,
  });
  useEffect(() => {
    if (!connMenu) return undefined;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setConnMenu(null);
    };
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('keydown', onKey);
    };
  }, [connMenu]);
  const handleConnectionsCellClick = useCallback(
    (station: Station, pos: { x: number; y: number }) => {
      setConnMenu({ ...pos, station });
    },
    []
  );
  const selectLinkedStation = useCallback(
    (ref: StationRef) => {
      setConnMenu(null);
      setHoveredStation(null);
      setSource('data-table');
      setSelection({ type: 'station', stationRef: ref });
    },
    [setSelection, setHoveredStation]
  );

  // The section a connected station belongs to: its name (falling back to
  // "Section N") for the pill, and a click that navigates to that section.
  const getSectionName = useCallback(
    (sectionId: number) =>
      sections.find((s) => s.id === sectionId)?.name ?? t('sections.defaultName', { id: sectionId }),
    [sections, t]
  );
  const selectLinkedSection = useCallback(
    (sectionId: number) => {
      setConnMenu(null);
      setHoveredSectionId(null);
      setSource('data-table');
      setSelection({ type: 'section', sectionId });
    },
    [setSelection, setHoveredSectionId]
  );

  // Same anchor rule as the map (MapCanvas): the first zero datum's GPS.
  const project = useProjectStore((s) => s.project);
  const stationGpsOrigin = useMemo(() => {
    const zeroDatum = project?.zero_datums?.[0];
    return zeroDatum?.gps
      ? { latitude: zeroDatum.gps.latitude, longitude: zeroDatum.gps.longitude }
      : null;
  }, [project?.zero_datums]);

  const stationColumns = useMemo(
    () =>
      buildStationColumns({
        t,
        precision,
        geometryReadOnly: editingBlocked,
        gpsOrigin: stationGpsOrigin,
        onGpsCellClick: handleGpsCellClick,
        onConnectionsCellClick: handleConnectionsCellClick,
      }),
    [t, precision, editingBlocked, stationGpsOrigin, handleGpsCellClick, handleConnectionsCellClick]
  );

  // Gutter shows the station's ID pill (the old index column); double-click
  // centers the map on the station, as the row double-click used to.
  const stationRowLabel = useCallback(
    (station: Station) => (
      <span
        onDoubleClick={() => {
          setSource('data-table');
          triggerCenterOnStation(station.station_ref);
        }}
        data-testid={`data-table-station-row-label-${station.station_ref.section_id}-${station.station_ref.station_index}`}
      >
        <EntityBlob entity="station">
          {formatStationId(station.station_ref.section_id, station.station_ref.station_index)}
        </EntityBlob>
      </span>
    ),
    [triggerCenterOnStation]
  );

  // The grid must fit the panel body EXACTLY: taller and the wrapper clips it
  // (occluded horizontal scrollbar) or grows a second scrollbar. Measure the
  // body instead of hand-summing chrome heights; the observer also tracks the
  // panel's resize drag and its open/close height transition.
  const [rawTables, setRawTables] = useState<string[] | 'unavailable' | null>(null);
  const lastRawTableRef = useRef<string | null>(null);
  const fetchRawTableNames = useCallback(() => {
    invokeIpc<string[]>('get_raw_table_names', undefined, {
      purpose: 'list raw sqlite tables for the data viewer',
    })
      .then((names) => setRawTables(Array.isArray(names) ? names : 'unavailable'))
      .catch(() => setRawTables('unavailable'));
  }, []);
  useEffect(() => {
    if (dataTableFullScreen) fetchRawTableNames();
  }, [dataTableFullScreen, fetchRawTableNames]);
  // Re-list after a save (dirty → clean): the first save of a never-saved
  // project creates the database, so a list that came back "unavailable" must
  // refresh rather than stay stuck until the user exits and re-enters fullscreen.
  const prevDirtyRawRef = useRef(isDirty);
  useEffect(() => {
    const wasDirty = prevDirtyRawRef.current;
    prevDirtyRawRef.current = isDirty;
    if (dataTableFullScreen && wasDirty && !isDirty) fetchRawTableNames();
  }, [isDirty, dataTableFullScreen, fetchRawTableNames]);
  useEffect(() => {
    if (rawTableSelection) lastRawTableRef.current = rawTableSelection;
    // '' = raw mode entered from the toolbar picker with no table chosen yet.
    if (rawTableSelection === '' && Array.isArray(rawTables) && rawTables.length > 0) {
      const last = lastRawTableRef.current;
      setRawTableSelection(last !== null && rawTables.includes(last) ? last : rawTables[0]!);
    }
  }, [rawTableSelection, rawTables, setRawTableSelection]);

  const gridBodyRef = useRef<HTMLDivElement | null>(null);
  // Fullscreen grid height: the body fills the window independently of
  // dataTableHeight, so it must be measured.
  const [fullScreenGridHeight, setFullScreenGridHeight] = useState<number | null>(null);
  // Chrome height above the grid body (the toolbar row). Cached so the DOCKED grid
  // height can be derived directly from the live dataTableHeight - one render per
  // resize tick, with NO measure→setState→re-render round-trip - while staying exact
  // (the chrome is constant during a vertical drag). The ResizeObserver refreshes it
  // only when it actually changes (toolbar wrap), guarded against sub-pixel churn so
  // a drag doesn't re-render the table via this state.
  const [chromeOffset, setChromeOffset] = useState(CHROME_OFFSET_DEFAULT);
  const dataTableHeightRef = useRef(dataTableHeight);
  dataTableHeightRef.current = dataTableHeight;
  const dataTableFullScreenRef = useRef(dataTableFullScreen);
  dataTableFullScreenRef.current = dataTableFullScreen;
  useEffect(() => {
    const el = gridBodyRef.current;
    if (!el) return undefined;
    const measure = () => {
      const h = el.clientHeight;
      if (dataTableFullScreenRef.current) {
        setFullScreenGridHeight(h);
      } else {
        const offset = Math.max(0, dataTableHeightRef.current - h);
        setChromeOffset((prev) => (Math.abs(prev - offset) > 1 ? offset : prev));
      }
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, []);
  // Entering/leaving fullscreen changes the body height drastically; measure it
  // synchronously so the fullscreen grid is sized in the same paint (no trailing).
  useLayoutEffect(() => {
    const el = gridBodyRef.current;
    if (el && dataTableFullScreen) setFullScreenGridHeight(el.clientHeight);
  }, [dataTableFullScreen]);
  const gridHeight = dataTableFullScreen
    ? fullScreenGridHeight ?? Math.max(MIN_GRID_HEIGHT, dataTableHeight - CHROME_OFFSET_DEFAULT)
    : Math.max(MIN_GRID_HEIGHT, dataTableHeight - chromeOffset);

  // Unconditional hide (header right-click) - unlike the menu checkbox toggle.
  const hideShotCols = useCallback((keys: string[]) => {
    setHiddenShotCols((prev) => new Set([...prev, ...keys]));
  }, []);
  const hideStationCols = useCallback((keys: string[]) => {
    setHiddenStationCols((prev) => new Set([...prev, ...keys]));
  }, []);

  // Switch an angle family's mode, putting the newly shown columns in the slot
  // of the ones they replace (Avg ↔ In+Out swap in place). Transitions through
  // Off keep every slot - hidden columns remember their position in the order.
  const changeAngleMode = useCallback(
    (family: 'azimuth' | 'inclination', next: 'avg' | 'inout' | 'off') => {
      const current = family === 'azimuth' ? azimuthMode : inclinationMode;
      if (next === current) return;
      if (current !== 'off' && next !== 'off') {
        setShotColumnOrder((prev) =>
          reorderForAngleMode(
            prev,
            shotColumns.map((c) => c.key),
            ANGLE_FAMILY_KEYS[family][current],
            ANGLE_FAMILY_KEYS[family][next]
          )
        );
      }
      (family === 'azimuth' ? setAzimuthMode : setInclinationMode)(next);
    },
    [azimuthMode, inclinationMode, shotColumns]
  );

  // Items of the header right-click menu: angle-family columns get a mode
  // switch and Off; everything else gets a plain hide. Station columns only
  // ever hide (no angle modes there).
  const headerMenuItems = useMemo(() => {
    if (!headerMenu) return [];
    const key = headerMenu.colKey;
    if (tableView === 'stations') {
      const entry = STATION_MENU_COLS.find((c) => c.keys.includes(key));
      const name = entry
        ? t(`dataTable.stationColumnsMenu.${entry.nameKey}`)
        : stationColumns.find((c) => c.key === key)?.header ?? key;
      return [
        {
          label: t('dataTable.headerMenu.hide', { name }),
          onClick: () => hideStationCols(entry ? entry.keys : [key]),
        },
      ];
    }
    const family = (['azimuth', 'inclination'] as const).find((f) => {
      const fam = ANGLE_FAMILY_KEYS[f];
      return (fam.avg as readonly string[]).includes(key) || (fam.inout as readonly string[]).includes(key);
    });
    const items: { label: string; onClick: () => void }[] = [];
    if (family) {
      const onAvg = (ANGLE_FAMILY_KEYS[family].avg as readonly string[]).includes(key);
      items.push({
        label: onAvg ? t('dataTable.headerMenu.showInOut') : t('dataTable.headerMenu.showAverage'),
        onClick: () => changeAngleMode(family, onAvg ? 'inout' : 'avg'),
      });
      items.push({
        label: t('dataTable.headerMenu.hide', { name: t(`dataTable.columnsMenu.${family}`) }),
        onClick: () => changeAngleMode(family, 'off'),
      });
    } else {
      const group = HEADER_HIDE_GROUPS[key] ?? { nameKey: null, keys: [key] };
      const name = group.nameKey
        ? t(`dataTable.columnsMenu.${group.nameKey}`)
        : shotColumns.find((c) => c.key === key)?.header ?? key;
      items.push({
        label: t('dataTable.headerMenu.hide', { name }),
        onClick: () => hideShotCols(group.keys),
      });
    }
    return items;
  }, [headerMenu, tableView, t, changeAngleMode, hideShotCols, shotColumns, stationColumns, hideStationCols]);

  // The column-visibility menu serves whichever view is active, so it reads and
  // writes that view's hidden set. The rows themselves live in ShotColumnsMenu,
  // shared with the new-section dialog and the shot editor; the stations view
  // has a different column domain, so it passes its own rows instead.
  const menuHiddenCols = tableView === 'shots' ? hiddenShotCols : hiddenStationCols;
  const menuToggleCols = tableView === 'shots' ? toggleShotCols : toggleStationCols;
  const stationMenuRows = useMemo(
    () =>
      STATION_MENU_COLS.map(({ keys, nameKey }) => ({
        keys,
        label: t(`dataTable.stationColumnsMenu.${nameKey}`),
      })),
    [t]
  );

  return (
    <div
      ref={panelRef}
      // Fill the Dock's panel body (it owns the size + the open/close slide). A
      // self height + `transition-all` here animated the height over 300ms on
      // every resize tick, so the table visibly lagged the dragged panel edge.
      className="flex flex-col h-full bg-panel border-t border-border"
      data-testid="data-table"
      data-fullscreen={dataTableFullScreen ? 'true' : 'false'}
    >
      {rawTableSelection !== null ? (
        /* Raw-data sub-header: title | raw table name | "as of last save" hint
           (the viewer shows last-saved state, so flag pending edits). */
        <SubHeaderRow>
          <span className="truncate font-medium" data-testid="data-table-title">
            {t('dataTable.title')}
          </span>
          <CenteredPillBand>
            <SubHeaderPickerMenu
              triggerTestId="data-table-raw-table-pill"
              listTestId="data-table-raw-table-menu-list"
              title={t('dataTable.rawTableMenu')}
              options={
                Array.isArray(rawTables)
                  ? rawTables.map((name) => ({ value: name, label: name }))
                  : [
                      {
                        value: '',
                        label:
                          rawTables === 'unavailable'
                            ? t('dataTable.rawData.unavailable')
                            : t('dataTable.rawData.loading'),
                        disabled: true,
                      },
                    ]
              }
              onPick={setRawTableSelection}
              renderTrigger={({ open }) => {
                const chevron = <PillChevron open={open} testId="data-table-raw-table-menu" />;
                return rawTableSelection !== '' ? (
                  <RawTablePill name={rawTableSelection} trailing={chevron} />
                ) : (
                  chevron
                );
              }}
            />
          </CenteredPillBand>
          <div className="flex min-w-0 items-center justify-end">
            {isDirty && (
              <span className="truncate text-text-secondary" data-testid="data-table-raw-dirty-hint">
                {t('dataTable.rawData.asOfLastSave')}
              </span>
            )}
          </div>
        </SubHeaderRow>
      ) : (
      /* Condensed header: title + selection indicator | section pill | view toggle + collapse */
      <SubHeaderRow>
        <div className="flex min-w-0 items-center gap-3">
          <span className="truncate font-medium" data-testid="data-table-title">
            {t('dataTable.title')}
          </span>
          {selectionIndicator && (
            <span
              className="truncate text-text-secondary tabular-nums"
              data-testid="data-table-selection-indicator"
            >
              {selectionIndicator}
            </span>
          )}
        </div>
        <CenteredPillBand>
          {selectedSection && (
            <SubHeaderPickerMenu
              triggerTestId="data-table-section-pill"
              listTestId="data-table-section-menu-list"
              title={t('dataTable.sectionMenu')}
              options={sections.map((s) => ({
                value: String(s.id),
                label: s.name ?? t('sections.defaultName', { id: s.id }),
                dot: s.color,
              }))}
              onPick={(value) => {
                const id = Number(value);
                setSelectedSectionId(id);
                setSource('data-table');
                setSelection({ type: 'section', sectionId: id });
              }}
              onHoverOption={handleSectionHover}
              onHoverTrigger={(over) => handleSectionHover(over ? String(selectedSection.id) : null)}
              renderTrigger={({ open }) => (
                <EntityBlob entity="section" className="min-w-0 max-w-full">
                  <span className="truncate">
                    {selectedSection.name ?? t('sections.defaultName', { id: selectedSection.id })}
                  </span>
                  <PillChevron open={open} testId="data-table-section-menu" />
                </EntityBlob>
              )}
            />
          )}
        </CenteredPillBand>
        {/* Controls column. `shrink-0` on the segmented control is load-bearing:
            its `overflow-hidden` (the rounded clip) drops its automatic minimum
            size to zero, which would let this column - and so the "Shots" label -
            collapse instead of the pill. */}
        <div className="flex items-center justify-end gap-3">
          <div
            className="flex shrink-0 items-center rounded border border-border overflow-hidden"
            role="radiogroup"
            aria-label={t('dataTable.title')}
            data-testid="data-table-view-select"
          >
            {(['shots', 'stations'] as const).map((view) => (
              <button
                key={view}
                role="radio"
                aria-checked={tableView === view}
                className={`whitespace-nowrap px-2 py-0.5 ${
                  tableView === view ? 'bg-accent-20 text-text' : 'text-text-secondary hover:bg-accent-10'
                }`}
                onClick={() => handleViewChange(view)}
                data-testid={`data-table-view-${view}`}
              >
                {view === 'shots' ? t('dataTable.viewShots') : t('dataTable.viewStations')}
              </button>
            ))}
          </div>
          <ShotColumnsMenu
            hidden={menuHiddenCols}
            onToggle={menuToggleCols}
            buttonTestId="data-table-columns-menu"
            dropdownTestId="data-table-columns-dropdown"
            anchorScope={panelRef}
            {...(tableView === 'shots'
              ? {
                  extraRows: { warnings: true, sectionId: true, shotId: true, credit: true },
                  angleModes: {
                    azimuth: {
                      mode: azimuthMode,
                      onPick: (m) => changeAngleMode('azimuth', m),
                    },
                    inclination: {
                      mode: inclinationMode,
                      onPick: (m) => changeAngleMode('inclination', m),
                    },
                  },
                  problems: draftHiddenProblems,
                }
              : { rowsOverride: stationMenuRows })}
          />
          <OpenInDataButton sectionId={selectedSectionId} />
          {!dataTableFullScreen && (
            <button
              onClick={enterDataTableFullScreen}
              className="text-text-secondary hover:text-text"
              title={t('dataTable.fullScreen.enter')}
              data-testid="data-table-expand-button"
            >
              <ExpandIcon size={14} />
            </button>
          )}
        </div>
      </SubHeaderRow>
      )}

      {/* Table. Both grids scroll internally and are sized to this body exactly,
          so the wrapper must never scroll alongside them (double scrollbars). */}
      <div ref={gridBodyRef} className="flex-1 overflow-hidden" onKeyDownCapture={credit.onKeyDownCapture}>
          {credit.editor}
          {rawTableSelection !== null ? (
            rawTableSelection === '' ? (
              <div className="flex items-center justify-center h-full text-text-secondary">
                {rawTables === 'unavailable'
                  ? t('dataTable.rawData.unavailable')
                  : t('dataTable.rawData.loading')}
              </div>
            ) : (
              <RawTableView tableName={rawTableSelection} height={gridHeight} />
            )
          ) : selectedSection ? (
            tableView === 'shots' ? (
              <SpreadsheetGrid<Shot>
                key={selectedSectionId ?? 'none'}
                columns={shotColumns}
                rows={gridShotRows}
                rowKey={(s) => s.id}
                rowTestId={(s) => `shot-row-${selectedSection.id}-${s.id}`}
                cellTestId={(s, _rowIndex, colKey) => `shot-cell-${selectedSection.id}-${s.id}-${colKey}`}
                locale={numberLocale}
                height={gridHeight}
                onEdit={onShotEdit}
                onBatchEdit={onShotBatchEdit}
                {...(editingBlocked ? {} : { pasteGrowsRows: shotGrowRows })}
                onClearMissing={onShotClearMissing}
                onActiveCellChange={onShotActiveCellChange}
                onRowHover={onShotRowHover}
                onPasteReport={onShotPasteReport}
                selectedRow={shotScrollToRow}
                hoveredRow={shotHoveredRow}
                rowClassName={shotRowClassName}
                cellClassName={shotCellClassName}
                rowLabel={shotRowLabel}
                labels={gridLabels}
                gutterWidth={72}
                onSelectionChange={setGridSel}
                onHeaderContextMenu={(colKey, pos) => setHeaderMenu({ ...pos, colKey })}
                {...(editingBlocked ? {} : { onInsertRows: onShotInsertRows })}
                onDeleteRows={onShotDeleteRows}
                extraContextMenuItems={shotExtraMenuItems}
                hiddenColumns={shotHiddenColumns}
                columnOrder={shotColumnOrder}
                onColumnOrderChange={setShotColumnOrder}
                columnWidths={shotColumnWidths}
                onColumnWidthsChange={setShotColumnWidths}
              />
            ) : sectionStations.length === 0 ? (
              <div className="flex items-center justify-center h-full text-text-secondary">
                {t('dataTable.stationsEmpty')}
              </div>
            ) : (
              <SpreadsheetGrid<Station>
                key={`stations-${selectedSectionId ?? 'none'}`}
                columns={stationColumns}
                rows={sectionStations}
                rowKey={(s) => s.station_ref.station_index}
                rowTestId={(s) => `station-row-${s.station_ref.section_id}-${s.station_ref.station_index}`}
                cellTestId={(s, _rowIndex, colKey) =>
                  `station-cell-${s.station_ref.section_id}-${s.station_ref.station_index}-${colKey}`
                }
                locale={numberLocale}
                height={gridHeight}
                onEdit={onStationEdit}
                onActiveCellChange={onStationActiveCellChange}
                onRowHover={onStationRowHover}
                selectedRow={stationScrollToRow}
                hoveredRow={stationHoveredRow}
                rowLabel={stationRowLabel}
                labels={stationGridLabels}
                gutterWidth={72}
                onHeaderContextMenu={(colKey, pos) => setHeaderMenu({ ...pos, colKey })}
                hiddenColumns={stationHiddenColumns}
                columnOrder={stationColumnOrder}
                onColumnOrderChange={setStationColumnOrder}
                columnWidths={stationColumnWidths}
                onColumnWidthsChange={setStationColumnWidths}
              />
            )
          ) : (
            <div className="flex items-center justify-center h-full text-text-secondary">
              {t('dataTable.selectSection')}
            </div>
          )}
      </div>
      {connMenu && (
        <div
          ref={connMenuRef}
          data-dropdown-open
          className="fixed z-50 min-w-56 max-w-md w-max overflow-hidden rounded border border-border bg-panel p-2 text-xs shadow-lg"
          style={{ left: connMenu.x, top: connMenu.y }}
          onMouseDown={(e) => e.stopPropagation()}
          data-testid="data-table-conn-menu"
        >
          {connMenu.station.merged_with.length > 0 && (
            <>
              <div className="px-1 pb-1 text-text-secondary">{t('dataTable.connMenu.merged')}</div>
              <ul className="space-y-0.5 pb-1">
                {connMenu.station.merged_with.map((ref) => (
                  <li
                    key={`m-${ref.section_id}-${ref.station_index}`}
                    className="flex items-center gap-1.5"
                  >
                    <DetailLink
                      type="section"
                      target={{ sectionId: ref.section_id }}
                      displayText={getSectionName(ref.section_id)}
                      pillLabel={getSectionName(ref.section_id)}
                      testId={`conn-merged-section-${ref.section_id}-${ref.station_index}-link`}
                      onSelect={() => selectLinkedSection(ref.section_id)}
                      onHoverStart={() => setHoveredSectionId(ref.section_id)}
                      onHoverEnd={() => setHoveredSectionId(null)}
                    />
                    <DetailLink
                      type="station"
                      target={{ sectionId: ref.section_id, position: ref.station_index }}
                      displayText={formatStationId(ref.section_id, ref.station_index)}
                      onSelect={() => selectLinkedStation(ref)}
                      onHoverStart={() => setHoveredStation(ref)}
                      onHoverEnd={() => setHoveredStation(null)}
                    />
                  </li>
                ))}
              </ul>
            </>
          )}
          {connMenu.station.closures_with && connMenu.station.closures_with.length > 0 && (
            <>
              <div className="px-1 pb-1 text-text-secondary">{t('dataTable.connMenu.closures')}</div>
              <ul className="space-y-0.5">
                {connMenu.station.closures_with.map((c) => (
                  <li
                    key={`c-${String(c.closure_id)}-${c.station_ref.section_id}-${c.station_ref.station_index}`}
                    className="flex items-center gap-1.5"
                  >
                    <DetailLink
                      type="section"
                      target={{ sectionId: c.station_ref.section_id }}
                      displayText={getSectionName(c.station_ref.section_id)}
                      pillLabel={getSectionName(c.station_ref.section_id)}
                      testId={`conn-closure-section-${String(c.closure_id)}-${c.station_ref.section_id}-${c.station_ref.station_index}-link`}
                      onSelect={() => selectLinkedSection(c.station_ref.section_id)}
                      onHoverStart={() => setHoveredSectionId(c.station_ref.section_id)}
                      onHoverEnd={() => setHoveredSectionId(null)}
                    />
                    <DetailLink
                      type="station"
                      target={{ sectionId: c.station_ref.section_id, position: c.station_ref.station_index }}
                      displayText={formatStationId(c.station_ref.section_id, c.station_ref.station_index)}
                      onSelect={() => selectLinkedStation(c.station_ref)}
                      onHoverStart={() => setHoveredStation(c.station_ref)}
                      onHoverEnd={() => setHoveredStation(null)}
                    />
                  </li>
                ))}
              </ul>
            </>
          )}
        </div>
      )}
      {headerMenu && (
        <div
          ref={headerMenuRef}
          className="fixed z-50 w-44 rounded border border-border bg-panel p-1 text-xs shadow-lg"
          style={{ left: headerMenu.x, top: headerMenu.y }}
          onMouseDown={(e) => e.stopPropagation()}
          data-dropdown-open
          data-testid="data-table-header-menu"
        >
          {headerMenuItems.map((item, index) => (
            <button
              key={item.label}
              data-testid={`data-table-header-menu-item-${index}`}
              className="block w-full rounded px-2 py-0.5 text-left hover:bg-accent-10"
              onClick={() => {
                item.onClick();
                setHeaderMenu(null);
              }}
            >
              {item.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
