import { useRef, useEffect, useMemo, useState, useCallback } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { useTranslation } from 'react-i18next';
import { type Line, type LineId } from '../../types/project';
import { useUIStore, type Selection } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { type LineMetadata } from '../../types/survey-data';
import {
  useEditingBlocked,
  isEditingBlockedNow,
  useEditingBlockedReason,
  editingBlockedReasonNow,
} from '../../hooks/useEditingBlocked';
import { useShallow } from 'zustand/react/shallow';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { IconButton } from '../common/Button';
import { FolderPlusIcon, PlusIcon } from '../common/icons';
import { setSource } from '../../utils/telemetry';
import { getErrorMessage } from '../../utils/ipc';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { DATA_TABLE_MIN_HEIGHT, DATA_TABLE_MAX_HEIGHT } from './layoutConstants';
import { UNGROUPED_GROUP_ID } from '../../store/sectionsViewSlice';
import { createSectionWithRename } from '../common/caveSectionMenuItems';
import { calculateLineStats, GroupHeaderRow, LineRow } from './SectionsPanelRows';
import {
  buildSectionTree,
  dropChangesSomething,
  linesToCarry,
  sectionNameProblem,
  treeArrowAction,
  treePositions,
  type GroupId,
  type TreeRow,
} from './sectionsTree';
import { LineDragGhost, useLineDrag } from './useLineDrag';

// The row helpers moved to SectionsPanelRows.tsx; re-exported for their tests.
export { calculateLineStats, hexColorFor } from './SectionsPanelRows';

interface SectionsPanelProps {
  lines: Line[];
}

type GroupRow = Extract<TreeRow, { kind: 'group' }>;

// Focus-mode data-table height as a fraction of the viewport, clamped to the
// data-table panel's shared [min, max] bounds. The post-collapse map re-fit
// lives in SingleSystemView (which stays mounted across focus toggles, unlike
// this panel which collapses away in focus mode).
const LINE_FOCUS_TABLE_FRACTION = 0.3;

// Delay before an external hover (e.g. skimming geometry on the map) scrolls the
// hovered line's row into view. Debouncing avoids the list thrashing as the
// pointer crosses many shots/stations in quick succession - it settles on the
// line the pointer lands on. Un-hover restore stays immediate.
const HOVER_SCROLL_DEBOUNCE_MS = 120;

// Estimated row heights (px) seeded into the virtualizer before a row is
// measured: a line row is a single truncated name line plus a stats line, a
// group header one line. The estimate only needs to be close (each mounted row
// is remeasured via measureElement, so scroll math stays pixel-accurate). The
// line estimate is kept in step with the warnings list estimate in DetailPanel.
const LINE_ROW_ESTIMATED_HEIGHT = 56;
const GROUP_ROW_ESTIMATED_HEIGHT = 34;

/**
 * Which row index owns the list's default keyboard Tab stop while no row
 * is focused (`-1` = none: a row is focused, or nothing is mounted). The list is
 * virtualized, so the Tab stop MUST land on a currently MOUNTED row -- otherwise,
 * once an external selection / hover reveal / manual scroll moves the virtual
 * window past index 0 (unmounting it), every mounted row would be `tabIndex={-1}`
 * and keyboard users could not Tab into the list at its current scroll position.
 * Prefer the selected row when it is inside the mounted window, else the first
 * (lowest-index) mounted row. A focused row keeps the Tab stop via its own
 * `isFocused` path, so this returns `-1` when a MOUNTED row is focused. But a
 * focused index whose row scrolled out of the virtual window (unmounting it,
 * which does not reliably fire `blur`) must NOT suppress the default Tab stop -
 * otherwise no mounted row is tabbable and keyboard users are trapped out.
 */
export function rovingDefaultTabIndex(
  focusedIndex: number,
  mountedIndices: readonly number[],
  selectedIndex: number,
): number {
  if (focusedIndex !== -1 && mountedIndices.includes(focusedIndex)) return -1;
  if (mountedIndices.length === 0) return -1;
  if (selectedIndex !== -1 && mountedIndices.includes(selectedIndex)) return selectedIndex;
  let first = mountedIndices[0]!;
  for (const i of mountedIndices) if (i < first) first = i;
  return first;
}

/**
 * The Sections panel: the project's lines, grouped under their Sections
 * (#327) once the project has one, with an "Ungrouped" group for the rest.
 * A tree (role=tree) over one virtualized, flattened list of rows.
 *
 * A group header is not a selectable map item: a click only focuses it (the
 * map, Details and the data table have nothing to show for a Section, and
 * selecting all its lines would silently replace the user's selection and
 * draw a hidden Section). Shift- or Ctrl-click adds all its lines to the
 * selection instead; double-click (or Enter) frames them on the map.
 */
export default function SectionsPanel({ lines }: SectionsPanelProps) {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  const { selection, multiSelection, setSelection, addToSelection, toggleSelection, clearSelection, selectAll, hoveredLineId, hoveredShot, hoveredStation, setHoveredLineId, openDialog, enterLineFocus, triggerZoomToLine, triggerZoomToLines, showToast, hiddenSections, setSectionHidden, collapsedSections, setSectionCollapsed, pendingSectionRename, requestSectionRename } = useUIStore(
    useShallow((s) => ({
      selection: s.selection,
      multiSelection: s.multiSelection,
      setSelection: s.setSelection,
      addToSelection: s.addToSelection,
      toggleSelection: s.toggleSelection,
      clearSelection: s.clearSelection,
      selectAll: s.selectAll,
      hoveredLineId: s.hoveredLineId,
      hoveredShot: s.hoveredShot,
      hoveredStation: s.hoveredStation,
      setHoveredLineId: s.setHoveredLineId,
      openDialog: s.openDialog,
      enterLineFocus: s.enterLineFocus,
      triggerZoomToLine: s.triggerZoomToLine,
      triggerZoomToLines: s.triggerZoomToLines,
      showToast: s.showToast,
      hiddenSections: s.hiddenSections,
      setSectionHidden: s.setSectionHidden,
      collapsedSections: s.collapsedSections,
      setSectionCollapsed: s.setSectionCollapsed,
      pendingSectionRename: s.pendingSectionRename,
      requestSectionRename: s.requestSectionRename,
    }))
  );

  // The closure-adjusted view, Plan/Export mode, and a locked remote SpeleoDB
  // project are all read-only - line add/delete are disabled there.
  const editingBlocked = useEditingBlocked();
  // Why the color dot is inert: a read-only SpeleoDB copy is the project, the
  // closure-adjusted view (or Plan/Export) is the view.
  const editingBlockedReason = useEditingBlockedReason();
  const colorBlockedText = t(
    editingBlockedReason === 'project' ? 'sections.actions.colorReadOnlyProject' : 'sections.actions.colorReadOnly',
  );

  // The row to highlight comes from any of: an explicit line hover (panel row /
  // DetailPanel line link), or - for map hovers - the line that owns the
  // currently hovered shot or station. Deriving in the panel keeps the map-driven
  // highlight out of `hoveredLineId`, which is also consumed by the map's
  // line-wide highlighter.
  const effectiveHoveredLineId: LineId | null =
    hoveredLineId ?? hoveredShot?.lineId ?? hoveredStation?.line_id ?? null;

  // Identity of the hovered GEOMETRY, not just its line - moving between two
  // shots/stations of the SAME line still changes this, so the debounced
  // scroll re-arms and only fires once the pointer truly settles (mirrors the
  // precedence in effectiveHoveredLineId above).
  const hoverKey: string | null =
    hoveredLineId != null
      ? `line:${hoveredLineId}`
      : hoveredShot
        ? `shot:${hoveredShot.lineId}:${hoveredShot.shotId}`
        : hoveredStation
          ? `station:${hoveredStation.line_id}:${hoveredStation.station_index}`
          : null;

  // The line that owns the current selection - a directly selected line, or
  // the line of a selected shot/station (selected from the map, data table, or
  // details panel). The matching row is highlighted and scrolled into view so the
  // Sections panel always reflects what's selected elsewhere. Same mapping used by
  // SingleSystemView / the data table.
  const selectedLineId: LineId | null =
    selection?.type === 'line' || selection?.type === 'shot'
      ? selection.lineId ?? null
      : selection?.type === 'station'
        ? selection.stationRef?.line_id ?? null
        : null;

  // Every directly-selected line, so a shift/ctrl-extended selection actually
  // looks like one. `selectedLineId` above stays the single "owning" line
  // (it also covers a selected shot/station and drives the scroll-into-view).
  const selectedLineIds = useMemo(
    () =>
      new Set(
        multiSelection
          .filter((s) => s.type === 'line' && s.lineId !== undefined)
          .map((s) => s.lineId as LineId)
      ),
    [multiSelection]
  );

  // --- The tree -------------------------------------------------------------
  const project = useProjectStore((s) => s.project);
  const sections = useMemo(() => project?.cave_sections ?? [], [project?.cave_sections]);
  // Collapse state is per project, for the session (keyed like the store's
  // other per-project state, by created_at).
  const projectKey = project?.created_at ?? '';
  const collapsedList = collapsedSections[projectKey];
  const tree = useMemo(
    () => buildSectionTree(lines, sections, new Set(collapsedList ?? []), new Set(hiddenSections)),
    [lines, sections, collapsedList, hiddenSections],
  );
  const rows = tree.rows;
  const positions = useMemo(() => treePositions(rows), [rows]);
  const rowIndexByKey = useMemo(() => new Map(rows.map((r, i) => [r.key, i])), [rows]);
  const lineOrder = useMemo(() => lines.map((l) => l.id), [lines]);

  // Track the last hovered line ID set by this panel (for cleanup on unmount)
  const lastHoveredIdRef = useRef<LineId | null>(null);

  // Scroll container for the line list, plus the scroll position captured
  // before an *external* hover (map / data table / details panel) auto-scrolled
  // the hovered row into view. Restored once nothing is hovered again so the
  // panel doesn't drift while the user skims other surfaces. `null` means there
  // is no external-hover scroll to undo.
  const listContainerRef = useRef<HTMLDivElement | null>(null);
  const preHoverScrollTopRef = useRef<number | null>(null);
  // Pending debounced hover-scroll timer (see HOVER_SCROLL_DEBOUNCE_MS).
  const hoverScrollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Keyboard accessibility: the focused row, by KEY (a collapse or a move
  // shifts every index below it), and refs for the roving tabindex.
  const [focusedKey, setFocusedKey] = useState<string | null>(null);
  const focusedIndex = focusedKey === null ? -1 : (rowIndexByKey.get(focusedKey) ?? -1);
  const optionRefs = useRef<(HTMLLIElement | null)[]>([]);
  // Index whose row we want to focus once the virtualizer has mounted it. A
  // keyboard jump (Home/End, or a fast arrow-step past the overscan window) can
  // target a row that isn't rendered yet; scrollToIndex mounts it, then the
  // row's ref callback focuses it. `null` means no focus is pending.
  const pendingFocusIndexRef = useRef<number | null>(null);
  // Set by a header Shift/Ctrl-click: the selection it makes must not expand
  // the header's collapsed group to reveal the selected line.
  const suppressRevealRef = useRef(false);
  // A plain press on a row of a multi-line selection, waiting for its click
  // (it may instead start dragging the whole selection).
  const deferredClickRef = useRef<LineId | null>(null);

  // Flat survey-data arrays back the per-row stats (length / reference count).
  // Subscribing here rebuilds the id -> metadata lookup below whenever the data
  // changes, without any component re-render caused by reading it per row.
  const surveyData = useProjectStore((s) => s.surveyData);
  const updateLine = useProjectStore((s) => s.updateLine);
  const updateCaveSection = useProjectStore((s) => s.updateCaveSection);
  const moveLinesToCaveSection = useProjectStore((s) => s.moveLinesToCaveSection);

  // O(1) line-id -> metadata lookup, built once per surveyData change. This
  // replaces the former per-row `surveyData.lines.find(...)`, which made a
  // panel open O(n^2) (~2x10^8 comparisons at 14k lines). Each row now reads
  // its shot range straight from the flat arrays via this map.
  const lineMetaById = useMemo(() => {
    const map = new Map<LineId, LineMetadata>();
    if (surveyData) {
      for (const meta of surveyData.lines) map.set(meta.id, meta);
    }
    return map;
  }, [surveyData]);

  // Each group's total length, for its header. Only once the list is a tree.
  const groupLength = useMemo(() => {
    const out = new Map<GroupId, number>();
    if (!tree.grouped) return out;
    for (const line of lines) {
      const group = tree.groupOfLine.get(line.id) ?? UNGROUPED_GROUP_ID;
      const { length } = calculateLineStats(line, lineMetaById.get(line.id), surveyData);
      out.set(group, (out.get(group) ?? 0) + length);
    }
    return out;
  }, [tree, lines, lineMetaById, surveyData]);

  // Virtualize the list: only the visible window (plus overscan) mounts, so a
  // 14k-line project no longer builds ~14k <li> subtrees on open. Same
  // scroll idiom as SpreadsheetGrid / DetailPanel's warnings list. The scroll
  // element is the `sections-list` container, whose scrollTop the hover
  // save/restore logic and the scroll e2e specs read and write directly.
  const rowVirtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => listContainerRef.current,
    estimateSize: (i) => (rows[i]?.kind === 'group' ? GROUP_ROW_ESTIMATED_HEIGHT : LINE_ROW_ESTIMATED_HEIGHT),
    // Measurements follow the row, not its slot, across a collapse.
    getItemKey: (i) => rows[i]?.key ?? i,
    overscan: 8,
  });

  // Reveal a row by index, then move keyboard focus to it. When the target is
  // already mounted (adjacent step within the overscan window) it focuses
  // immediately; otherwise scrollToIndex mounts it and its ref callback focuses
  // it once attached (see `pendingFocusIndexRef`).
  const focusRow = useCallback((index: number) => {
    const row = rows[index];
    if (!row) return;
    setFocusedKey(row.key);
    rowVirtualizer.scrollToIndex(index, { align: 'auto' });
    const el = optionRefs.current[index];
    if (el && el.isConnected) {
      pendingFocusIndexRef.current = null;
      el.focus();
    } else {
      pendingFocusIndexRef.current = index;
    }
  }, [rows, rowVirtualizer]);

  // Clear hoveredLineId on unmount if it was set by this panel
  useEffect(() => {
    return () => {
      if (lastHoveredIdRef.current !== null) {
        // Only clear if the store still has the ID we set
        const currentHoveredId = useUIStore.getState().hoveredLineId;
        if (currentHoveredId === lastHoveredIdRef.current) {
          useUIStore.getState().setHoveredLineId(null);
        }
      }
    };
  }, []);

  // Scroll the hovered line's row into view when it's off-screen (its group's
  // header while the group is collapsed).
  // block: 'nearest' is a no-op when the row is already visible, so this
  // is silent for panel-internal hovers and only acts on external (map) hovers.
  //
  // For external hovers (map / data table / details panel - the panel itself did
  // not set the hover, so `lastHoveredIdRef` is null) we remember the scroll
  // position before the first auto-scroll and restore it once nothing is hovered
  // anywhere, so skimming other surfaces doesn't permanently move the list.
  useEffect(() => {
    const container = listContainerRef.current;
    if (effectiveHoveredLineId === null) {
      // Nothing hovered anywhere - undo any external-hover auto-scroll. Immediate
      // (the prior effect's cleanup already dropped any pending hover-scroll).
      if (preHoverScrollTopRef.current !== null && container) {
        container.scrollTop = preHoverScrollTopRef.current;
      }
      preHoverScrollTopRef.current = null;
      return;
    }
    const idx = tree.rowOfLine.get(effectiveHoveredLineId) ?? -1;
    if (idx < 0) return;
    // Panel-internal row hovers (lastHoveredIdRef set) are the user already
    // interacting with the list - don't capture/restore for those.
    const isExternalHover = lastHoveredIdRef.current === null;
    // Debounce the reveal: skimming geometry on the map changes the hovered
    // line many times in quick succession; only scroll once it settles.
    hoverScrollTimerRef.current = setTimeout(() => {
      hoverScrollTimerRef.current = null;
      // Skip while a DATA-TABLE dropdown is open (e.g. the line picker, whose
      // own option hovers drive this scroll): the move is invisible behind the
      // dropdown and the resulting scroll event would trip its outside-scroll
      // guard and close it. Scoped to the data table - NOT a global
      // `[data-dropdown-open]` - so an unrelated open menu/Select elsewhere in
      // the app doesn't suppress a legitimate map-hover reveal. (position:fixed
      // dropdowns stay DOM descendants of the data table, so this still matches
      // them.) Checked at fire time - the hover can be set before the dropdown
      // opens.
      if (document.querySelector('[data-testid="data-table"] [data-dropdown-open]')) return;
      // Capture the pre-hover scroll position right before we actually move it -
      // NOT when the timer was armed. A skipped scroll (guard above, or a
      // non-external hover) then leaves preHoverScrollTopRef null, so a later
      // un-hover can't restore the list to a position this hover never moved it
      // from (which would undo an unrelated manual scroll). Guarded on `=== null`
      // so only the FIRST reveal in a skim sequence sets the baseline.
      if (isExternalHover && preHoverScrollTopRef.current === null && container) {
        preHoverScrollTopRef.current = container.scrollTop;
      }
      // scrollToIndex with align:'auto' is a no-op when the row is already
      // visible (matching the previous scrollIntoView({ block: 'nearest' })),
      // and reveals off-screen rows that virtualization hasn't mounted.
      rowVirtualizer.scrollToIndex(idx, { align: 'auto' });
    }, HOVER_SCROLL_DEBOUNCE_MS);
    return () => {
      if (hoverScrollTimerRef.current !== null) {
        clearTimeout(hoverScrollTimerRef.current);
        hoverScrollTimerRef.current = null;
      }
    };
    // Keyed on hoverKey (the geometry), not effectiveHoveredLineId, so hovering
    // a different shot/station within the same line also re-arms the debounce.
  }, [hoverKey, effectiveHoveredLineId, tree, rowVirtualizer]);

  // Scroll the selected line's row into view when the selection changes (a
  // shot/station/line selected from the map, data table, or details panel).
  // Unlike the hover scroll, this is a persistent reveal - there's nothing to
  // restore - so it's kept separate from the hover save/restore bookkeeping.
  // Keyed on the full `selection` (not just the owning line id): selecting
  // another shot of the SAME line (e.g. clicking rows in the data table)
  // must re-reveal the line row even though its id didn't change. A selection
  // event on a line of a collapsed group expands the group, so the selection
  // is always visible; a collapse the user makes afterwards stands.
  const prevSelectionRef = useRef<typeof selection>(null);
  useEffect(() => {
    // `lines` edits also re-run this effect; only a fresh setSelection call
    // (every call stores a new object, even for the same target) is a
    // user-driven selection event.
    const selectionEvent = prevSelectionRef.current !== selection;
    prevSelectionRef.current = selection;
    const suppressReveal = suppressRevealRef.current;
    suppressRevealRef.current = false;
    if (selectedLineId === null) return;
    const idx = tree.rowOfLine.get(selectedLineId) ?? -1;
    if (idx < 0) return;
    const group = tree.groupOfLine.get(selectedLineId);
    if (selectionEvent && !suppressReveal && group !== undefined && rows[idx]?.kind === 'group') {
      setSectionCollapsed(projectKey, group, false);
      return; // re-runs with the line's own row once the group is open
    }
    // A selection reveal wins over a pending (debounced) hover scroll - cancel it
    // so it can't snap the list back to the hovered row 50ms later.
    if (hoverScrollTimerRef.current !== null) {
      clearTimeout(hoverScrollTimerRef.current);
      hoverScrollTimerRef.current = null;
    }
    // Mounts the owning row (if virtualization has it unmounted) and brings it
    // into view; align:'auto' leaves an already-visible row untouched.
    rowVirtualizer.scrollToIndex(idx, { align: 'auto' });
    // EVERY selection event re-anchors the panel here - including re-selecting
    // the already-selected line. If it happened during an active external
    // hover (hover a shot, then click to select it), drop the pre-hover restore
    // so un-hovering stays on the selection rather than snapping back to where
    // the list was before the hover.
    if (selectionEvent) {
      preHoverScrollTopRef.current = null;
    }
  }, [selection, selectedLineId, tree, rows, rowVirtualizer, projectKey, setSectionCollapsed]);

  // --- Selection ------------------------------------------------------------

  /** Select a line the way a mouse-down does: Shift adds, Ctrl/Cmd toggles,
   *  a plain press selects only it. Also the keyboard's Space / Enter. */
  const selectLine = useCallback((line: Line, e?: React.MouseEvent) => {
    // Only handle left mouse button for mouse events
    if (e && e.button !== 0) return;
    setSource('sections-panel');
    const sel = { type: 'line' as const, lineId: line.id };
    // Shift/ctrl extend the selection, matching the map's station and shot
    // clicks (useMapTools). This is the only affordance that produces a
    // multi-line selection, so it is what makes the context menu's
    // "Reverse N Lines" (#24) reachable at all.
    if (e?.shiftKey) {
      addToSelection(sel);
      return;
    }
    if (e?.ctrlKey || e?.metaKey) {
      toggleSelection(sel);
      return;
    }
    // Single click only selects - it must NOT move the map (reverts #64's
    // single-click-centers behaviour). Double-click zooms to the line, which
    // is what frames it - see handleLineDoubleClick.
    setSelection(sel);
  }, [setSelection, addToSelection, toggleSelection]);

  const handleLineMouseDown = useCallback((line: Line, e?: React.MouseEvent) => {
    deferredClickRef.current = null;
    // A plain press on a row of a selection of several lines may be the start
    // of dragging all of them onto a Section: narrow the selection to this line
    // only if it turns out to be a click (handleLineClick).
    if (
      e && e.button === 0 && !e.shiftKey && !e.ctrlKey && !e.metaKey &&
      tree.grouped && !editingBlocked && selectedLineIds.size > 1 && selectedLineIds.has(line.id)
    ) {
      deferredClickRef.current = line.id;
      return;
    }
    selectLine(line, e);
  }, [selectLine, tree.grouped, editingBlocked, selectedLineIds]);

  // --- Drag lines onto a Section -------------------------------------------

  const blockedToast = useCallback(() => {
    showToast(
      t(editingBlockedReasonNow() === 'project' ? 'caveSections.readOnlyProject' : 'caveSections.readOnly'),
      TOAST_KEYS.EDIT_DISABLED,
      'warning',
    );
  }, [showToast, t]);

  const lineDrag = useLineDrag({
    enabled: tree.grouped && !editingBlocked,
    listRef: listContainerRef,
    carry: (lineId) => linesToCarry(lineId, selectedLineIds, lineOrder),
    canDrop: (lineIds, group) => dropChangesSomething(lineIds, group, tree.groupOfLine),
    onDrop: (lineIds, group) => {
      // Re-checked at the drop: the mode can change while the button is down.
      if (isEditingBlockedNow()) {
        blockedToast();
        return;
      }
      setSource('sections-panel');
      void moveLinesToCaveSection(lineIds, group === UNGROUPED_GROUP_ID ? null : group);
    },
  });
  const dropTarget = lineDrag.drag?.target ?? null;
  const { consumeDropClick, onPointerDown: dragPointerDown } = lineDrag;
  const handleDragPointerDown = useCallback(
    (line: Line, e: React.PointerEvent) => dragPointerDown(line.id, e),
    [dragPointerDown],
  );

  const handleLineClick = useCallback((line: Line, e: React.MouseEvent) => {
    if (deferredClickRef.current !== line.id) return;
    deferredClickRef.current = null;
    if (consumeDropClick()) return;
    selectLine(line, e);
  }, [selectLine, consumeDropClick]);

  // Shared focus-entry: select the line and drop into focus mode. Used by
  // double-click and keyboard Enter so focus is reachable without a mouse.
  const enterFocusFor = useCallback((line: Line) => {
    const focusHeight = Math.round(
      Math.min(
        Math.max(window.innerHeight * LINE_FOCUS_TABLE_FRACTION, DATA_TABLE_MIN_HEIGHT),
        DATA_TABLE_MAX_HEIGHT,
      ),
    );
    enterLineFocus(line.id, focusHeight);
  }, [enterLineFocus]);

  // --- Group headers ----------------------------------------------------------

  const [renamingId, setRenamingId] = useState<number | null>(null);
  const [renameError, setRenameError] = useState<string | null>(null);

  const startRename = useCallback((sectionId: number) => {
    if (isEditingBlockedNow()) {
      blockedToast();
      return;
    }
    setRenameError(null);
    setRenamingId(sectionId);
    const idx = tree.rowOfGroup.get(sectionId);
    if (idx !== undefined) rowVirtualizer.scrollToIndex(idx, { align: 'auto' });
  }, [blockedToast, tree, rowVirtualizer]);

  // "New Section" (here or in a line's menu) and the header menu's Rename ask
  // for the inline rename through the store, once the Section is listed.
  useEffect(() => {
    if (pendingSectionRename === null || !tree.rowOfGroup.has(pendingSectionRename)) return;
    requestSectionRename(null);
    startRename(pendingSectionRename);
  }, [pendingSectionRename, tree, requestSectionRename, startRename]);

  const closeRename = useCallback((sectionId: number, refocus: boolean) => {
    setRenamingId(null);
    setRenameError(null);
    if (!refocus) return;
    const idx = tree.rowOfGroup.get(sectionId);
    if (idx !== undefined) requestAnimationFrame(() => focusRow(idx));
  }, [tree, focusRow]);

  const commitRename = useCallback((sectionId: number, value: string, viaBlur: boolean): boolean => {
    const section = sections.find((s) => s.id === sectionId);
    if (!section) {
      closeRename(sectionId, false);
      return true;
    }
    // Re-checked at commit time: the mode can change while the field is open.
    if (isEditingBlockedNow()) {
      blockedToast();
      closeRename(sectionId, !viaBlur);
      return true;
    }
    const problem = sectionNameProblem(value, sections, sectionId);
    if (problem) {
      // Leaving the field with a name that cannot be used keeps the old one;
      // Enter says why and keeps the field open.
      if (viaBlur) {
        closeRename(sectionId, false);
        return true;
      }
      setRenameError(
        problem.kind === 'taken'
          ? t('caveSections.rename.taken', { name: problem.name })
          : t(problem.kind === 'empty' ? 'caveSections.rename.empty' : 'caveSections.rename.tooLong'),
      );
      return false;
    }
    const name = value.trim();
    closeRename(sectionId, !viaBlur);
    if (name !== section.name) {
      setSource('sections-panel');
      void updateCaveSection(sectionId, { name });
    }
    return true;
  }, [sections, closeRename, blockedToast, updateCaveSection, t]);

  const toggleCollapsed = useCallback((row: GroupRow) => {
    setSectionCollapsed(projectKey, row.group, !row.collapsed);
  }, [setSectionCollapsed, projectKey]);

  const handleGroupClick = useCallback((row: GroupRow, e: React.MouseEvent) => {
    if (e.button !== 0) return;
    setFocusedKey(row.key);
    if (!(e.shiftKey || e.ctrlKey || e.metaKey) || row.lineIds.length === 0) return;
    // Shift/Ctrl-click adds every line of the group to the selection.
    setSource('sections-panel');
    const have = new Set(selectedLineIds);
    const added: Selection[] = row.lineIds.filter((id) => !have.has(id)).map((id) => ({ type: 'line', lineId: id }));
    suppressRevealRef.current = true;
    selectAll([...multiSelection, ...added]);
  }, [selectAll, multiSelection, selectedLineIds]);

  const handleGroupDoubleClick = useCallback((row: GroupRow, e: React.MouseEvent) => {
    if (e.button !== 0) return;
    if (e.target !== e.currentTarget && (e.target as HTMLElement).closest('button, input')) return;
    setSource('sections-panel');
    triggerZoomToLines(row.lineIds);
  }, [triggerZoomToLines]);

  const openGroupContextMenu = useCallback((row: GroupRow, x: number, y: number) => {
    if (!row.section) return;
    setSource('sections-panel');
    openDialog('context-menu', {
      target: { type: 'cave-section', sectionId: row.section.id },
      position: { x, y },
      scrollScope: '[data-testid="sections-list"]',
    });
  }, [openDialog]);

  const handleSectionColorChange = useCallback(async (sectionId: number, color: string) => {
    if (isEditingBlockedNow()) {
      blockedToast();
      return false;
    }
    setSource('sections-panel');
    try {
      return (await updateCaveSection(sectionId, { color })).success;
    } catch (e) {
      showToast(getErrorMessage(e, t('editGate.commandFailed')), TOAST_KEYS.COMMAND_FAILED, 'error');
      return false;
    }
  }, [updateCaveSection, showToast, t, blockedToast]);

  const handleGroupFocus = useCallback((index: number) => {
    const row = rows[index];
    if (row) setFocusedKey(row.key);
  }, [rows]);

  // --- Keyboard ---------------------------------------------------------------

  const openLineContextMenu = useCallback((lineId: number, x: number, y: number) => {
    setSource('sections-panel');
    openDialog('context-menu', {
      target: { type: 'line', lineId },
      position: { x, y },
      // The menu is pinned to these coordinates and rendered from the dialog
      // layer, so nothing about its own DOM says the list can move under it.
      // Naming the scroller here is what lets it close when this list scrolls,
      // while a scroll anywhere else - the map, the data table - leaves it be.
      scrollScope: '[data-testid="sections-list"]',
    });
  }, [openDialog]);

  const handleKeyDown = useCallback((e: React.KeyboardEvent, currentIndex: number) => {
    // Only handle keyboard events when the row itself is focused, not when
    // focus is on nested controls (like the rename field)
    if (e.target !== e.currentTarget) return;
    // A context menu opened over the list keeps the focus on its row; its keys
    // (Escape closes it) are the menu's.
    if (useUIStore.getState().activeDialog === 'context-menu') return;
    const row = rows[currentIndex];
    if (!row) return;
    const handled = () => {
      e.preventDefault();
      e.stopPropagation();
    };

    switch (e.key) {
      case 'ArrowDown':
        handled();
        if (currentIndex < rows.length - 1) focusRow(currentIndex + 1);
        break;
      case 'ArrowUp':
        handled();
        if (currentIndex > 0) focusRow(currentIndex - 1);
        break;
      case 'Home':
        handled();
        focusRow(0);
        break;
      case 'End':
        handled();
        focusRow(rows.length - 1);
        break;
      case 'ArrowLeft':
      case 'ArrowRight': {
        const action = treeArrowAction(rows, currentIndex, e.key);
        if (action.type === 'none') break;
        handled();
        if (action.type === 'focus') focusRow(action.index);
        else setSectionCollapsed(projectKey, action.group, action.type === 'collapse');
        break;
      }
      case 'Enter':
        // Enter = activate (keyboard equivalent of double-click): select + zoom
        // to the line, or frame a group's lines. Focus mode is reserved for the
        // dedicated focus button.
        handled();
        if (row.kind === 'line') {
          selectLine(row.line);
          triggerZoomToLine(row.line.id);
        } else {
          triggerZoomToLines(row.lineIds);
        }
        break;
      case ' ':
        // Space = select a line only (the keyboard equivalent of single-click);
        // on a header it opens or closes the group (its chevron).
        handled();
        if (row.kind === 'line') selectLine(row.line);
        else toggleCollapsed(row);
        break;
      case 'F2':
        if (row.kind === 'group' && row.section) {
          handled();
          startRename(row.section.id);
        }
        break;
      case 'ContextMenu':
      case 'F10': {
        if (e.key === 'F10' && !e.shiftKey) break;
        handled();
        const rect = e.currentTarget.getBoundingClientRect();
        if (row.kind === 'line') openLineContextMenu(row.line.id, rect.left + 24, rect.bottom);
        else openGroupContextMenu(row, rect.left + 24, rect.bottom);
        break;
      }
      case 'Escape':
        handled();
        clearSelection();
        break;
    }
  }, [rows, focusRow, selectLine, triggerZoomToLine, triggerZoomToLines, clearSelection, setSectionCollapsed, projectKey, toggleCollapsed, startRename, openLineContextMenu, openGroupContextMenu]);

  const handleContainerBlur = (e: React.FocusEvent<HTMLUListElement>) => {
    // CRITICAL: Only clear when focus leaves the tree ENTIRELY
    // Using relatedTarget to check if new focus target is within container
    const next = e.relatedTarget;
    if (!next || !(next instanceof Node) || !e.currentTarget.contains(next)) {
      setFocusedKey(null);
      lastHoveredIdRef.current = null;
      setHoveredLineId(null);
    }
  };

  const handleLineMouseEnter = useCallback((lineId: LineId) => {
    lastHoveredIdRef.current = lineId;
    setHoveredLineId(lineId);
  }, [setHoveredLineId]);

  const handleLineMouseLeave = useCallback(() => {
    // Always clear hover on mouse leave - the mouse is no longer over the element
    // Keyboard focus is handled separately via the onFocus handler
    lastHoveredIdRef.current = null;
    setHoveredLineId(null);
  }, [setHoveredLineId]);

  // The row's color dot commits through the same update_line command as the
  // Edit Line dialog. Re-checked at commit time: the mode can change while the
  // system picker is open, and the closure-adjusted view and Plan/Export have no
  // server-side backstop. A rejected CommandResult is already toasted by
  // executeCommand; a transport rejection is only refreshed and re-thrown, and
  // the caller owns telling the user (docs/notifications.md).
  // A click on the dot while editing is blocked, or a pick confirmed after it
  // became blocked, says why instead of doing nothing.
  // Read at call time: the mode can change while the system picker is open.
  const handleLineColorBlocked = useCallback(() => {
    showToast(
      t(editingBlockedReasonNow() === 'project' ? 'sections.actions.colorReadOnlyProject' : 'sections.actions.colorReadOnly'),
      TOAST_KEYS.EDIT_DISABLED,
      'warning',
    );
  }, [showToast, t]);

  const handleLineColorChange = useCallback(async (lineId: LineId, color: string) => {
    if (isEditingBlockedNow()) {
      handleLineColorBlocked();
      return false;
    }
    setSource('sections-panel');
    try {
      return (await updateLine(lineId, { color })).success;
    } catch (e) {
      showToast(getErrorMessage(e, t('editGate.commandFailed')), TOAST_KEYS.COMMAND_FAILED, 'error');
      return false;
    }
  }, [updateLine, showToast, t, handleLineColorBlocked]);

  const handleControlMouseDown = useCallback((e: React.MouseEvent) => {
    // Stop propagation so the row's own mouse-down handling doesn't double-fire.
    e.stopPropagation();
  }, []);

  const handleLineDoubleClick = useCallback((line: Line, e: React.MouseEvent) => {
    // Only handle left mouse button
    if (e.button !== 0) return;
    // Don't trigger if double-clicking nested controls (like the focus button)
    if (e.target !== e.currentTarget && (e.target as HTMLElement).closest('button')) return;
    // Select, then zoom/frame the line on the map - without entering focus
    // mode (that's what the dedicated per-row focus button is for).
    selectLine(line, e);
    triggerZoomToLine(line.id);
  }, [selectLine, triggerZoomToLine]);

  const handleControlDoubleClick = useCallback((e: React.MouseEvent) => {
    // The button enters focus on a single click; swallow the double-click so the
    // row's double-click handler doesn't run a second entry.
    e.stopPropagation();
  }, []);

  // Dedicated focus-mode entry button (replaces the old per-row delete control;
  // line deletion now lives in the right-click / long-press context menu).
  // Single click selects the line and enters focus mode - the only focus
  // entry on the row since double-click was retasked to zoom.
  const handleEnterFocusClick = useCallback((line: Line, e: React.MouseEvent) => {
    e.stopPropagation();
    if (e.button !== 0) return;
    selectLine(line);
    enterFocusFor(line);
  }, [selectLine, enterFocusFor]);

  const handleLineFocus = useCallback((lineId: LineId, index: number) => {
    const row = rows[index];
    if (row) setFocusedKey(row.key);
    lastHoveredIdRef.current = lineId;
    setHoveredLineId(lineId);
  }, [rows, setHoveredLineId]);

  const handleAddLine = () => {
    if (editingBlocked) return;
    setSource('sections-panel');
    openDialog('import');
  };

  const handleNewSection = () => {
    if (isEditingBlockedNow()) {
      blockedToast();
      return;
    }
    setSource('sections-panel');
    void createSectionWithRename([], t);
  };

  const handleLineContextMenu = useCallback((e: React.MouseEvent, lineId: number) => {
    e.preventDefault();
    e.stopPropagation();
    openLineContextMenu(lineId, e.clientX, e.clientY);
  }, [openLineContextMenu]);

  const dropTargetName =
    dropTarget === null
      ? null
      : dropTarget === UNGROUPED_GROUP_ID
        ? t('caveSections.ungrouped')
        : (sections.find((s) => s.id === dropTarget)?.name ?? null);

  return (
    <div className="h-full flex flex-col bg-panel" data-testid="sections-panel">
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-border">
        <h2 className="font-medium">{t('sections.title')}</h2>
        <div className="flex items-center gap-1">
          <IconButton
            onClick={handleNewSection}
            icon={<FolderPlusIcon size={14} />}
            size="sm"
            variant="ghost"
            tooltip={editingBlocked ? t('caveSections.readOnly') : t('caveSections.actions.new')}
            aria-label={t('caveSections.actions.new')}
            data-testid="sections-new-section-button"
            disabled={editingBlocked}
          />
          <IconButton
            onClick={handleAddLine}
            icon={<PlusIcon size={14} />}
            size="sm"
            variant="ghost"
            tooltip={t('sections.actions.add')}
            aria-label={t('sections.actions.add')}
            data-testid="sections-add-button"
            disabled={editingBlocked}
          />
        </div>
      </div>

      {/* Line tree */}
      <div ref={listContainerRef} className="flex-1 overflow-auto select-none" data-testid="sections-list">
        {rows.length === 0 ? (
          <div className="p-4 text-center text-text-secondary text-sm">
            {t('sections.empty.message')}
            <br />
            <button
              onClick={handleAddLine}
              disabled={editingBlocked}
              className="text-accent hover:underline mt-2 inline-block disabled:opacity-50 disabled:cursor-not-allowed disabled:no-underline"
              data-testid="sections-empty-add"
            >
              {t('sections.empty.action')}
            </button>
          </div>
        ) : (
          <ul
            role="tree"
            aria-label={t('caveSections.ariaTree')}
            aria-multiselectable="true"
            className="py-1"
            onBlur={handleContainerBlur}
            data-testid="sections-tree"
          >
            {/* Virtualization sizer: holds the full scroll height while only the
                visible window of rows is mounted (absolutely positioned within).
                Kept inside the `py-1` <ul> so the list's top/bottom breathing
                room is unchanged. */}
            <div style={{ height: `${rowVirtualizer.getTotalSize()}px`, position: 'relative' }}>
              {(() => {
                const virtualItems = rowVirtualizer.getVirtualItems();
                // Roving-tabindex owner: a MOUNTED row keeps the default Tab stop
                // so the list stays keyboard-reachable at its current scroll
                // position (index 0 is often unmounted after a reveal / scroll).
                const defaultTabIndex = rovingDefaultTabIndex(
                  focusedIndex,
                  virtualItems.map((v) => v.index),
                  selectedLineId !== null ? (tree.rowOfLine.get(selectedLineId) ?? -1) : -1,
                );
                return virtualItems.map((virtualRow) => {
                  const index = virtualRow.index;
                  const row = rows[index]!;
                  const position = positions[index]!;
                  const common = {
                    index,
                    posinset: position.posinset,
                    setsize: position.setsize,
                    isFocused: focusedIndex === index,
                    isDefaultFocus: index === defaultTabIndex,
                    virtualStart: virtualRow.start,
                    measureElement: rowVirtualizer.measureElement,
                    pendingFocusIndexRef,
                    optionRefs,
                    onKeyDown: handleKeyDown,
                    colorEditable: !editingBlocked,
                    onColorBlocked: handleLineColorBlocked,
                    colorBlockedText,
                    onControlMouseDown: handleControlMouseDown,
                    onControlDoubleClick: handleControlDoubleClick,
                  };
                  if (row.kind === 'group') {
                    return (
                      <GroupHeaderRow
                        key={row.key}
                        {...common}
                        row={row}
                        length={groupLength.get(row.group) ?? 0}
                        precision={precision.length}
                        isDropTarget={dropTarget === row.group}
                        renaming={row.section !== null && renamingId === row.section.id}
                        renameError={renameError}
                        onRenameCommit={(value, viaBlur) => commitRename(row.section!.id, value, viaBlur)}
                        onRenameCancel={() => closeRename(row.section!.id, true)}
                        onRenameInput={() => setRenameError(null)}
                        onToggleCollapsed={toggleCollapsed}
                        onToggleHidden={setSectionHidden}
                        onClick={handleGroupClick}
                        onDoubleClick={handleGroupDoubleClick}
                        onContextMenu={openGroupContextMenu}
                        onFocus={handleGroupFocus}
                        onColorChange={handleSectionColorChange}
                      />
                    );
                  }
                  const line = row.line;
                  return (
                    <LineRow
                      key={row.key}
                      {...common}
                      line={line}
                      level={row.level}
                      dropGroup={row.group}
                      dimmed={row.hidden}
                      inDropTarget={dropTarget !== null && row.group === dropTarget}
                      meta={lineMetaById.get(line.id)}
                      surveyData={surveyData}
                      isSelected={selectedLineId === line.id || selectedLineIds.has(line.id)}
                      isHovered={effectiveHoveredLineId === line.id}
                      precision={precision.length}
                      onMouseDown={handleLineMouseDown}
                      onClick={handleLineClick}
                      onDragPointerDown={handleDragPointerDown}
                      onDoubleClick={handleLineDoubleClick}
                      onContextMenu={handleLineContextMenu}
                      onMouseEnter={handleLineMouseEnter}
                      onMouseLeave={handleLineMouseLeave}
                      onFocus={handleLineFocus}
                      onEnterFocus={handleEnterFocusClick}
                      onColorChange={handleLineColorChange}
                      onLongPress={openLineContextMenu}
                    />
                  );
                });
              })()}
            </div>
          </ul>
        )}
      </div>
      {lineDrag.drag && <LineDragGhost drag={lineDrag.drag} targetName={dropTargetName} />}
    </div>
  );
}
