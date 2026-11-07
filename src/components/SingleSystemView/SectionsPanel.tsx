import { useRef, useEffect, useMemo, useState, memo, useCallback } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { useVirtualizerUnmountGuard } from '../common/useVirtualizerUnmountGuard';
import { useTranslation } from 'react-i18next';
import { type Section, type SectionId } from '../../types/project';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import {
  isVirtualKind,
  shotKindFromFlags,
  type SurveyData,
  type SectionMetadata,
} from '../../types/survey-data';
import { useEditingBlocked } from '../../hooks/useEditingBlocked';
import { useShallow } from 'zustand/react/shallow';
import { formatFixed, formatNumber } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { IconButton } from '../common/Button';
import { ExternalLinkIcon, PlusIcon } from '../common/icons';
import { setSource } from '../../utils/telemetry';
import { useLongPress } from '../../hooks';
import { DATA_TABLE_MIN_HEIGHT, DATA_TABLE_MAX_HEIGHT } from './layoutConstants';
import { useGrayscalePaint } from '../../hooks/useGrayscalePaint';

interface SectionsPanelProps {
  sections: Section[];
}

// Focus-mode data-table height as a fraction of the viewport, clamped to the
// data-table panel's shared [min, max] bounds. The post-collapse map re-fit
// lives in SingleSystemView (which stays mounted across focus toggles, unlike
// this panel which collapses away in focus mode).
const SECTION_FOCUS_TABLE_FRACTION = 0.3;

// Delay before an external hover (e.g. skimming geometry on the map) scrolls the
// hovered section's row into view. Debouncing avoids the list thrashing as the
// pointer crosses many shots/stations in quick succession - it settles on the
// section the pointer lands on. Un-hover restore stays immediate.
const HOVER_SCROLL_DEBOUNCE_MS = 120;

// Estimated section-row height (px) seeded into the virtualizer before a row is
// measured. Rows are a single truncated name line plus a stats line, so their
// real height is uniform; the estimate only needs to be close (each mounted row
// is remeasured via measureElement, so scroll math stays pixel-accurate). Kept
// in step with the warnings list estimate in DetailPanel.
const SECTION_ROW_ESTIMATED_HEIGHT = 56;

/**
 * Which section-list index owns the list's default keyboard Tab stop while no row
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

export default function SectionsPanel({ sections }: SectionsPanelProps) {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  const { selection, multiSelection, setSelection, addToSelection, toggleSelection, clearSelection, hoveredSectionId, hoveredShot, hoveredStation, setHoveredSectionId, openDialog, enterSectionFocus, triggerZoomToSection } = useUIStore(
    useShallow((s) => ({
      selection: s.selection,
      multiSelection: s.multiSelection,
      setSelection: s.setSelection,
      addToSelection: s.addToSelection,
      toggleSelection: s.toggleSelection,
      clearSelection: s.clearSelection,
      hoveredSectionId: s.hoveredSectionId,
      hoveredShot: s.hoveredShot,
      hoveredStation: s.hoveredStation,
      setHoveredSectionId: s.setHoveredSectionId,
      openDialog: s.openDialog,
      enterSectionFocus: s.enterSectionFocus,
      triggerZoomToSection: s.triggerZoomToSection,
    }))
  );

  // The closure-adjusted view, Plan/Export mode, and a locked remote SpeleoDB
  // project are all read-only - section add/delete are disabled there.
  const editingBlocked = useEditingBlocked();

  // The row to highlight comes from any of: an explicit section hover (panel row /
  // DetailPanel section link), or - for map hovers - the section that owns the
  // currently hovered shot or station. Deriving in the panel keeps the map-driven
  // highlight out of `hoveredSectionId`, which is also consumed by the map's
  // section-wide highlighter.
  const effectiveHoveredSectionId: SectionId | null =
    hoveredSectionId ?? hoveredShot?.sectionId ?? hoveredStation?.section_id ?? null;

  // Identity of the hovered GEOMETRY, not just its section - moving between two
  // shots/stations of the SAME section still changes this, so the debounced
  // scroll re-arms and only fires once the pointer truly settles (mirrors the
  // precedence in effectiveHoveredSectionId above).
  const hoverKey: string | null =
    hoveredSectionId != null
      ? `section:${hoveredSectionId}`
      : hoveredShot
        ? `shot:${hoveredShot.sectionId}:${hoveredShot.shotId}`
        : hoveredStation
          ? `station:${hoveredStation.section_id}:${hoveredStation.station_index}`
          : null;

  // The section that owns the current selection - a directly selected section, or
  // the section of a selected shot/station (selected from the map, data table, or
  // details panel). The matching row is highlighted and scrolled into view so the
  // Sections panel always reflects what's selected elsewhere. Same mapping used by
  // SingleSystemView / the data table.
  const selectedSectionId: SectionId | null =
    selection?.type === 'section' || selection?.type === 'shot'
      ? selection.sectionId ?? null
      : selection?.type === 'station'
        ? selection.stationRef?.section_id ?? null
        : null;

  // Every directly-selected section, so a shift/ctrl-extended selection actually
  // looks like one. `selectedSectionId` above stays the single "owning" section
  // (it also covers a selected shot/station and drives the scroll-into-view).
  const selectedSectionIds = useMemo(
    () =>
      new Set(
        multiSelection
          .filter((s) => s.type === 'section' && s.sectionId !== undefined)
          .map((s) => s.sectionId as SectionId)
      ),
    [multiSelection]
  );

  // Track the last hovered section ID set by this panel (for cleanup on unmount)
  const lastHoveredIdRef = useRef<SectionId | null>(null);

  // Scroll container for the section list, plus the scroll position captured
  // before an *external* hover (map / data table / details panel) auto-scrolled
  // the hovered row into view. Restored once nothing is hovered again so the
  // panel doesn't drift while the user skims other surfaces. `null` means there
  // is no external-hover scroll to undo.
  const listContainerRef = useRef<HTMLDivElement | null>(null);
  const preHoverScrollTopRef = useRef<number | null>(null);
  // Pending debounced hover-scroll timer (see HOVER_SCROLL_DEBOUNCE_MS).
  const hoverScrollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Keyboard accessibility: track focused index and refs for roving tabindex
  const [focusedIndex, setFocusedIndex] = useState<number>(-1);
  const optionRefs = useRef<(HTMLLIElement | null)[]>([]);
  // Index whose row we want to focus once the virtualizer has mounted it. A
  // keyboard jump (Home/End, or a fast arrow-step past the overscan window) can
  // target a row that isn't rendered yet; scrollToIndex mounts it, then the
  // row's ref callback focuses it. `null` means no focus is pending.
  const pendingFocusIndexRef = useRef<number | null>(null);

  // Flat survey-data arrays back the per-row stats (length / reference count).
  // Subscribing here rebuilds the id -> metadata lookup below whenever the data
  // changes, without any component re-render caused by reading it per row.
  const surveyData = useProjectStore((s) => s.surveyData);

  // O(1) section-id -> metadata lookup, built once per surveyData change. This
  // replaces the former per-row `surveyData.sections.find(...)`, which made a
  // panel open O(n^2) (~2x10^8 comparisons at 14k sections). Each row now reads
  // its shot range straight from the flat arrays via this map.
  const sectionMetaById = useMemo(() => {
    const map = new Map<SectionId, SectionMetadata>();
    if (surveyData) {
      for (const meta of surveyData.sections) map.set(meta.id, meta);
    }
    return map;
  }, [surveyData]);

  // section-id -> list index, so the hover / selection reveal effects resolve a
  // row's virtual index in O(1) instead of scanning `sections` each time.
  const sectionIndexById = useMemo(() => {
    const map = new Map<SectionId, number>();
    sections.forEach((s, i) => map.set(s.id, i));
    return map;
  }, [sections]);

  // Virtualize the list: only the visible window (plus overscan) mounts, so a
  // 14k-section project no longer builds ~14k <li> subtrees on open. Same
  // scroll idiom as SpreadsheetGrid / DetailPanel's warnings list. The scroll
  // element is the `sections-list` container, whose scrollTop the hover
  // save/restore logic and the scroll e2e specs read and write directly.
  const rowVirtualizer = useVirtualizer({
    count: sections.length,
    getScrollElement: () => listContainerRef.current,
    estimateSize: () => SECTION_ROW_ESTIMATED_HEIGHT,
    overscan: 8,
  });
  // Rows are measured, so a reveal still pending when the panel unmounts (a
  // selection, then Plan mode) would throw from its frame (#576 NF-W8E3-1).
  useVirtualizerUnmountGuard(rowVirtualizer);

  // Reveal a row by index, then move keyboard focus to it. When the target is
  // already mounted (adjacent step within the overscan window) it focuses
  // immediately; otherwise scrollToIndex mounts it and its ref callback focuses
  // it once attached (see `pendingFocusIndexRef`).
  const focusRow = useCallback((index: number) => {
    rowVirtualizer.scrollToIndex(index, { align: 'auto' });
    const el = optionRefs.current[index];
    if (el) {
      pendingFocusIndexRef.current = null;
      el.focus();
    } else {
      pendingFocusIndexRef.current = index;
    }
  }, [rowVirtualizer]);

  // Clear hoveredSectionId on unmount if it was set by this panel
  useEffect(() => {
    return () => {
      if (lastHoveredIdRef.current !== null) {
        // Only clear if the store still has the ID we set
        const currentHoveredId = useUIStore.getState().hoveredSectionId;
        if (currentHoveredId === lastHoveredIdRef.current) {
          useUIStore.getState().setHoveredSectionId(null);
        }
      }
    };
  }, []);

  // Scroll the hovered section's row into view when it's off-screen.
  // block: 'nearest' is a no-op when the row is already visible, so this
  // is silent for panel-internal hovers and only acts on external (map) hovers.
  //
  // For external hovers (map / data table / details panel - the panel itself did
  // not set the hover, so `lastHoveredIdRef` is null) we remember the scroll
  // position before the first auto-scroll and restore it once nothing is hovered
  // anywhere, so skimming other surfaces doesn't permanently move the list.
  useEffect(() => {
    const container = listContainerRef.current;
    if (effectiveHoveredSectionId === null) {
      // Nothing hovered anywhere - undo any external-hover auto-scroll. Immediate
      // (the prior effect's cleanup already dropped any pending hover-scroll).
      if (preHoverScrollTopRef.current !== null && container) {
        container.scrollTop = preHoverScrollTopRef.current;
      }
      preHoverScrollTopRef.current = null;
      return;
    }
    const idx = sectionIndexById.get(effectiveHoveredSectionId) ?? -1;
    if (idx < 0) return;
    // Panel-internal row hovers (lastHoveredIdRef set) are the user already
    // interacting with the list - don't capture/restore for those.
    const isExternalHover = lastHoveredIdRef.current === null;
    // Debounce the reveal: skimming geometry on the map changes the hovered
    // section many times in quick succession; only scroll once it settles.
    hoverScrollTimerRef.current = setTimeout(() => {
      hoverScrollTimerRef.current = null;
      // Skip while a DATA-TABLE dropdown is open (e.g. the section picker, whose
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
    // Keyed on hoverKey (the geometry), not effectiveHoveredSectionId, so hovering
    // a different shot/station within the same section also re-arms the debounce.
  }, [hoverKey, effectiveHoveredSectionId, sectionIndexById, rowVirtualizer]);

  // Scroll the selected section's row into view when the selection changes (a
  // shot/station/section selected from the map, data table, or details panel).
  // Unlike the hover scroll, this is a persistent reveal - there's nothing to
  // restore - so it's kept separate from the hover save/restore bookkeeping.
  // Keyed on the full `selection` (not just the owning section id): selecting
  // another shot of the SAME section (e.g. clicking rows in the data table)
  // must re-reveal the section row even though its id didn't change.
  const prevSelectionRef = useRef<typeof selection>(null);
  useEffect(() => {
    // `sections` edits also re-run this effect; only a fresh setSelection call
    // (every call stores a new object, even for the same target) is a
    // user-driven selection event.
    const selectionEvent = prevSelectionRef.current !== selection;
    prevSelectionRef.current = selection;
    if (selectedSectionId === null) return;
    const idx = sectionIndexById.get(selectedSectionId) ?? -1;
    if (idx < 0) return;
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
    // the already-selected section. If it happened during an active external
    // hover (hover a shot, then click to select it), drop the pre-hover restore
    // so un-hovering stays on the selection rather than snapping back to where
    // the list was before the hover.
    if (selectionEvent) {
      preHoverScrollTopRef.current = null;
    }
  }, [selection, selectedSectionId, sectionIndexById, rowVirtualizer]);

  const handleSectionClick = useCallback((section: Section, e?: React.MouseEvent) => {
    // Only handle left mouse button for mouse events
    if (e && e.button !== 0) return;
    setSource('sections-panel');
    const sel = { type: 'section' as const, sectionId: section.id };
    // Shift/ctrl extend the selection, matching the map's station and shot
    // clicks (useMapTools). This is the only affordance that produces a
    // multi-section selection, so it is what makes the context menu's
    // "Reverse N Sections" (#24) reachable at all.
    if (e?.shiftKey) {
      addToSelection(sel);
      return;
    }
    if (e?.ctrlKey || e?.metaKey) {
      toggleSelection(sel);
      return;
    }
    // Single click only selects - it must NOT move the map (reverts #64's
    // single-click-centers behaviour). Double-click zooms to the section, which
    // is what frames it - see handleSectionDoubleClick.
    setSelection(sel);
  }, [setSelection, addToSelection, toggleSelection]);

  // Shared focus-entry: select the section and drop into focus mode. Used by
  // double-click and keyboard Enter so focus is reachable without a mouse.
  const enterFocusFor = useCallback((section: Section) => {
    const focusHeight = Math.round(
      Math.min(
        Math.max(window.innerHeight * SECTION_FOCUS_TABLE_FRACTION, DATA_TABLE_MIN_HEIGHT),
        DATA_TABLE_MAX_HEIGHT,
      ),
    );
    enterSectionFocus(section.id, focusHeight);
  }, [enterSectionFocus]);

  const handleKeyDown = useCallback((e: React.KeyboardEvent, currentIndex: number) => {
    // Only handle keyboard events when the list item itself is focused,
    // not when focus is on nested controls (like the delete button)
    if (e.target !== e.currentTarget) return;

    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        e.stopPropagation();
        if (currentIndex < sections.length - 1) {
          const nextIndex = currentIndex + 1;
          setFocusedIndex(nextIndex);
          focusRow(nextIndex);
        }
        break;
      case 'ArrowUp':
        e.preventDefault();
        e.stopPropagation();
        if (currentIndex > 0) {
          const prevIndex = currentIndex - 1;
          setFocusedIndex(prevIndex);
          focusRow(prevIndex);
        }
        break;
      case 'Home':
        e.preventDefault();
        e.stopPropagation();
        setFocusedIndex(0);
        focusRow(0);
        break;
      case 'End':
        e.preventDefault();
        e.stopPropagation();
        setFocusedIndex(sections.length - 1);
        focusRow(sections.length - 1);
        break;
      case 'Enter':
        // Enter = activate (keyboard equivalent of double-click): select + zoom
        // to the section. Focus mode is reserved for the dedicated focus button.
        e.preventDefault();
        e.stopPropagation();
        if (sections[currentIndex]) {
          handleSectionClick(sections[currentIndex]);
          triggerZoomToSection(sections[currentIndex].id);
        }
        break;
      case ' ':
        // Space = select only (the keyboard equivalent of single-click).
        e.preventDefault();
        e.stopPropagation();
        if (sections[currentIndex]) {
          handleSectionClick(sections[currentIndex]);
        }
        break;
      case 'Escape':
        e.preventDefault();
        e.stopPropagation();
        clearSelection();
        break;
    }
  }, [sections, handleSectionClick, triggerZoomToSection, clearSelection, focusRow]);

  const handleContainerBlur = (e: React.FocusEvent<HTMLUListElement>) => {
    // CRITICAL: Only clear when focus leaves the listbox ENTIRELY
    // Using relatedTarget to check if new focus target is within container
    const next = e.relatedTarget;
    if (!next || !(next instanceof Node) || !e.currentTarget.contains(next)) {
      setFocusedIndex(-1);
      lastHoveredIdRef.current = null;
      setHoveredSectionId(null);
    }
  };

  const handleSectionMouseEnter = useCallback((sectionId: SectionId) => {
    lastHoveredIdRef.current = sectionId;
    setHoveredSectionId(sectionId);
  }, [setHoveredSectionId]);

  const handleSectionMouseLeave = useCallback(() => {
    // Always clear hover on mouse leave - the mouse is no longer over the element
    // Keyboard focus is handled separately via the onFocus handler
    lastHoveredIdRef.current = null;
    setHoveredSectionId(null);
  }, [setHoveredSectionId]);

  const handleFocusButtonMouseDown = useCallback((e: React.MouseEvent) => {
    // Stop propagation so the row's own mouse-down handling doesn't double-fire.
    e.stopPropagation();
  }, []);

  const handleSectionDoubleClick = useCallback((section: Section, e: React.MouseEvent) => {
    // Only handle left mouse button
    if (e.button !== 0) return;
    // Don't trigger if double-clicking nested controls (like the focus button)
    if (e.target !== e.currentTarget && (e.target as HTMLElement).closest('button')) return;
    // Select, then zoom/frame the section on the map - without entering focus
    // mode (that's what the dedicated per-row focus button is for).
    handleSectionClick(section, e);
    triggerZoomToSection(section.id);
  }, [handleSectionClick, triggerZoomToSection]);

  const handleFocusButtonDoubleClick = useCallback((e: React.MouseEvent) => {
    // The button enters focus on a single click; swallow the double-click so the
    // row's double-click handler doesn't run a second entry.
    e.stopPropagation();
  }, []);

  // Dedicated focus-mode entry button (replaces the old per-row delete control;
  // section deletion now lives in the right-click / long-press context menu).
  // Single click selects the section and enters focus mode - the only focus
  // entry on the row since double-click was retasked to zoom.
  const handleEnterFocusClick = useCallback((section: Section, e: React.MouseEvent) => {
    e.stopPropagation();
    if (e.button !== 0) return;
    handleSectionClick(section);
    enterFocusFor(section);
  }, [handleSectionClick, enterFocusFor]);

  const handleSectionFocus = useCallback((sectionId: SectionId, index: number) => {
    setFocusedIndex(index);
    lastHoveredIdRef.current = sectionId;
    setHoveredSectionId(sectionId);
  }, [setHoveredSectionId]);

  const handleAddSection = () => {
    if (editingBlocked) return;
    setSource('sections-panel');
    openDialog('import');
  };

  const openSectionContextMenu = useCallback((sectionId: number, x: number, y: number) => {
    setSource('sections-panel');
    openDialog('context-menu', {
      target: { type: 'section', sectionId },
      position: { x, y },
      // The menu is pinned to these coordinates and rendered from the dialog
      // layer, so nothing about its own DOM says the list can move under it.
      // Naming the scroller here is what lets it close when this list scrolls,
      // while a scroll anywhere else - the map, the data table - leaves it be.
      scrollScope: '[data-testid="sections-list"]',
    });
  }, [openDialog]);

  const handleSectionContextMenu = useCallback((e: React.MouseEvent, sectionId: number) => {
    e.preventDefault();
    e.stopPropagation();
    openSectionContextMenu(sectionId, e.clientX, e.clientY);
  }, [openSectionContextMenu]);

  return (
    <div className="h-full flex flex-col bg-panel" data-testid="sections-panel">
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-border">
        <h2 className="font-medium">{t('sections.title')}</h2>
        <IconButton
          onClick={handleAddSection}
          icon={<PlusIcon size={14} />}
          size="sm"
          variant="ghost"
          tooltip={t('sections.actions.add')}
          aria-label={t('sections.actions.add')}
          data-testid="sections-add-button"
          disabled={editingBlocked}
        />
      </div>

      {/* Section list */}
      <div ref={listContainerRef} className="flex-1 overflow-auto" data-testid="sections-list">
        {sections.length === 0 ? (
          <div className="p-4 text-center text-text-secondary text-sm">
            {t('sections.empty.message')}
            <br />
            <button
              onClick={handleAddSection}
              disabled={editingBlocked}
              className="text-accent hover:underline mt-2 inline-block disabled:opacity-50 disabled:cursor-not-allowed disabled:no-underline"
              data-testid="sections-empty-add"
            >
              {t('sections.empty.action')}
            </button>
          </div>
        ) : (
          <ul
            role="listbox"
            aria-label={t('sections.ariaList')}
            className="py-1"
            onBlur={handleContainerBlur}
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
                  selectedSectionId !== null ? (sectionIndexById.get(selectedSectionId) ?? -1) : -1,
                );
                return virtualItems.map((virtualRow) => {
                const index = virtualRow.index;
                const section = sections[index]!;
                return (
                  <SectionRow
                    key={section.id}
                    section={section}
                    index={index}
                    meta={sectionMetaById.get(section.id)}
                    surveyData={surveyData}
                    isSelected={selectedSectionId === section.id || selectedSectionIds.has(section.id)}
                    isHovered={effectiveHoveredSectionId === section.id}
                    isFocused={focusedIndex === index}
                    isDefaultFocus={index === defaultTabIndex}
                    precision={precision.length}
                    virtualStart={virtualRow.start}
                    measureElement={rowVirtualizer.measureElement}
                    pendingFocusIndexRef={pendingFocusIndexRef}
                    onMouseDown={handleSectionClick}
                    onDoubleClick={handleSectionDoubleClick}
                    onContextMenu={handleSectionContextMenu}
                    onMouseEnter={handleSectionMouseEnter}
                    onMouseLeave={handleSectionMouseLeave}
                    onFocus={handleSectionFocus}
                    onKeyDown={handleKeyDown}
                    onEnterFocus={handleEnterFocusClick}
                    onEnterFocusMouseDown={handleFocusButtonMouseDown}
                    onEnterFocusDoubleClick={handleFocusButtonDoubleClick}
                    onLongPress={openSectionContextMenu}
                    optionRefs={optionRefs}
                  />
                );
                });
              })()}
            </div>
          </ul>
        )}
      </div>
    </div>
  );
}

/**
 * Compute total length + reference-shot count for the section row.
 * Called once per *visible* (virtualized) section row by `SectionRow`.
 *
 * `meta` is resolved by the panel from a `Map<sectionId, SectionMetadata>`
 * built once per surveyData change, replacing the former per-row
 * `surveyData.sections.find(...)` that made a panel open O(n^2). Given the
 * metadata, the shot range is read straight from the `shotLength` / `shotFlags`
 * typed arrays - it never touches `section.shots`, so the lazy Shot
 * materialization stays cold (the Phase-A.5 memory optimization).
 *
 * Falls back to the legacy `section.shots` walk only on the unusual path where
 * `surveyData` isn't populated yet (e.g. legacy JSON refresh in-flight). That
 * branch stays cold in the common path because `meta` is present.
 */
function calculateSectionStats(
  section: Section,
  meta: SectionMetadata | undefined,
  surveyData: SurveyData | null,
): {
  shotCount: number;
  length: number;
  referenceCount: number;
} {
  if (surveyData && meta) {
    let length = 0;
    let referenceCount = 0;
    const end = meta.shotOffset + meta.shotCount;
    for (let g = meta.shotOffset; g < end; g++) {
      length += surveyData.shotLength[g]!;
      // Counts both virtual kinds: the panel shows a single "virtual shots"
      // tally, and #27's Jump/Reference split is surfaced in the table, not here.
      if (isVirtualKind(shotKindFromFlags(surveyData.shotFlags[g]!))) {
        referenceCount++;
      }
    }
    return { shotCount: meta.shotCount, length, referenceCount };
  }
  // Legacy fallback - only hit during the JSON refresh window where
  // `surveyData` may briefly be a stale legacy-converted copy. Same
  // behavior as before, just kept for safety.
  let length = 0;
  let referenceCount = 0;
  for (const shot of section.shots) {
    length += shot.length;
    if (isVirtualKind(shot.kind)) referenceCount++;
  }
  return {
    shotCount: section.shots.length,
    length,
    referenceCount,
  };
}

interface SectionRowProps {
  section: Section;
  index: number;
  /** Flat-array metadata for this section (id -> SectionMetadata), resolved by
   *  the panel from its memoized map. `undefined` only during the legacy JSON
   *  refresh window; the stats then fall back to the `section.shots` walk. */
  meta: SectionMetadata | undefined;
  surveyData: SurveyData | null;
  isSelected: boolean;
  isHovered: boolean;
  isFocused: boolean;
  isDefaultFocus: boolean;
  precision: number;
  /** Virtualizer offset (px) of this row from the top of the sizer. */
  virtualStart: number;
  /** Virtualizer measure callback; attached to the row so its real height feeds
   *  back into the scroll math (rows are uniform, but this stays pixel-exact). */
  measureElement: (node: Element | null) => void;
  /** Set by keyboard nav to a not-yet-mounted target index; the row focuses
   *  itself once its ref attaches. */
  pendingFocusIndexRef: React.MutableRefObject<number | null>;
  onMouseDown: (section: Section, e?: React.MouseEvent) => void;
  onDoubleClick: (section: Section, e: React.MouseEvent) => void;
  onContextMenu: (e: React.MouseEvent, sectionId: number) => void;
  onMouseEnter: (sectionId: SectionId) => void;
  onMouseLeave: () => void;
  onFocus: (sectionId: SectionId, index: number) => void;
  onKeyDown: (e: React.KeyboardEvent, index: number) => void;
  onEnterFocus: (section: Section, e: React.MouseEvent) => void;
  onEnterFocusMouseDown: (e: React.MouseEvent) => void;
  onEnterFocusDoubleClick: (e: React.MouseEvent) => void;
  onLongPress: (sectionId: number, x: number, y: number) => void;
  optionRefs: React.MutableRefObject<(HTMLLIElement | null)[]>;
}

const SectionRow = memo(function SectionRow({
  section,
  index,
  meta,
  surveyData,
  isSelected,
  isHovered,
  isFocused,
  isDefaultFocus,
  precision,
  virtualStart,
  measureElement,
  pendingFocusIndexRef,
  onMouseDown,
  onDoubleClick,
  onContextMenu,
  onMouseEnter,
  onMouseLeave,
  onFocus,
  onKeyDown,
  onEnterFocus,
  onEnterFocusMouseDown,
  onEnterFocusDoubleClick,
  onLongPress,
  optionRefs,
}: SectionRowProps) {
  const { t } = useTranslation();
  const paint = useGrayscalePaint();
  const stats = useMemo(
    () => calculateSectionStats(section, meta, surveyData),
    [section, meta, surveyData],
  );

  const handleLongPress = useCallback(
    (pos: { x: number; y: number }) => {
      onLongPress(section.id, pos.x, pos.y);
    },
    [onLongPress, section.id],
  );

  const longPressHandlers = useLongPress(handleLongPress);

  // One ref serves three roles: the panel's roving-tabindex/optionRefs registry,
  // the virtualizer's height measurement (needs the `data-index` attribute set
  // below), and focusing a keyboard target the moment it mounts.
  const refCallback = useCallback((el: HTMLLIElement | null) => {
    optionRefs.current[index] = el;
    measureElement(el);
    if (el && pendingFocusIndexRef.current === index) {
      pendingFocusIndexRef.current = null;
      el.focus();
    }
  }, [optionRefs, index, measureElement, pendingFocusIndexRef]);

  return (
    <li
      ref={refCallback}
      data-index={index}
      role="option"
      aria-selected={isSelected}
      tabIndex={isFocused ? 0 : (isDefaultFocus ? 0 : -1)}
      data-testid={`section-row-${section.id}`}
      title={t('sections.actions.zoom')}
      style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${virtualStart}px)` }}
      className={`px-3 py-2 border-l-2 border-transparent cursor-pointer group transition-colors outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset ${
        isSelected
          ? 'bg-accent-20 ring-2 ring-accent ring-inset'
          : (isHovered ? 'bg-accent-10 border-accent' : 'hover:bg-accent-10')
      }`}
      onMouseDown={(e) => onMouseDown(section, e)}
      onDoubleClick={(e) => onDoubleClick(section, e)}
      onContextMenu={(e) => onContextMenu(e, section.id)}
      {...longPressHandlers}
      onMouseEnter={() => onMouseEnter(section.id)}
      onMouseLeave={onMouseLeave}
      onFocus={() => onFocus(section.id, index)}
      onKeyDown={(e) => onKeyDown(e, index)}
    >
      <div
        className="flex items-center gap-2"
        data-testid={`section-item-${section.id}`}
      >
        <span
          className="w-3 h-3 rounded-full flex-shrink-0"
          style={{ backgroundColor: paint(section.color) }}
        />
        <span className="flex-1 truncate">
          {section.name ?? t('sections.defaultName', { id: section.id })}
        </span>
        <IconButton
          onClick={(e) => onEnterFocus(section, e)}
          onMouseDown={onEnterFocusMouseDown}
          onDoubleClick={onEnterFocusDoubleClick}
          icon={<ExternalLinkIcon size={12} />}
          size="sm"
          variant="ghost"
          tooltip={t('sections.actions.focus')}
          aria-label={t('sections.actions.focus')}
          className="opacity-0 group-hover:opacity-100 text-text-secondary hover:text-accent"
          tabIndex={-1}
          data-testid={`section-focus-${section.id}`}
        />
      </div>
      <div className="text-xs text-text-secondary mt-1 ml-5">
        {t('sections.stats', {
          shots: formatNumber(stats.shotCount),
          length: `${formatFixed(stats.length, undefined, precision)}${t('common.units.meters')}`,
        })}
      </div>
    </li>
  );
});
