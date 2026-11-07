import { useState, useRef, useEffect, useLayoutEffect, useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore, type ToolType } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { Button, IconButton } from '../common/Button';
import {
  ChevronDownIcon,
  DownloadIcon,
  ShrinkIcon,
  UndoIcon,
  RedoIcon,
} from '../common/icons';
import { AppMenu, type AppMenuItem } from '../common/AppMenu';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';
import { holdsWriteLock } from '../../utils/quit';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import { ToolGlyph, toolAccentVar } from '../common/toolIcons';
import type { CSSProperties } from 'react';
import DisplayMenu from './DisplayMenu';
import { ModeCarousel } from './ModeCarousel';
import TableSelectDropdown from './TableSelectDropdown';
import { SpeleoStatusButton } from './SpeleoStatusButton';
import { useSpeleoStore } from '../../store/speleoStore';
import { useEditingBlocked } from '../../hooks/useEditingBlocked';
import { useGrayscaleTokens } from '../../hooks/useGrayscaleTokens';
import UndoHistoryDropdown from './UndoHistoryDropdown';
import { TOOL_GROUPS, isNavigationTool, type ToolGroupDef } from '../../utils/toolGroups';
import { isMutatingTool, navigationToolsEnabled } from '../../utils/toolUtils';
import { setSource } from '../../utils/telemetry';
import { defaultConfig } from '../../types/config';

interface ToolbarProps {
  /** Items for the left-side hamburger (AppMenu): file/project actions. */
  menuItems: AppMenuItem[];
  /** A save failed - tint the hamburger icon red. */
  saveFailed?: boolean;
  onImport: () => void;
  onUndo: () => void;
  onRedo: () => void;
  canUndo: boolean;
  canRedo: boolean;
  placementModeActive: boolean;
  /** Reduced variant while the data table is fullscreen: the left slot shows the
   *  "Back to Map" button instead of the Import button; the hamburger + Undo/Redo
   *  stay, and the map tools give way to the table picker. */
  dataTableFullScreen: boolean;
}

/** Tool metadata for standalone tools */
interface StandaloneTool {
  kind: 'standalone';
  id: ToolType;
  shortcut: string;
}

/** Tool group reference for grouped tools */
interface GroupRef {
  kind: 'group';
  groupId: string;
}

/** Separator between tool clusters */
interface Separator {
  kind: 'separator';
}

export type ToolbarItem = StandaloneTool | GroupRef | Separator;

/**
 * Get the display shortcut string for a tool from the shortcuts config.
 * Returns the first binding or empty string if none.
 */
function getShortcutForTool(tool: ToolType, shortcuts: Record<string, string[]>): string {
  // Map tool type to shortcut config key
  const keyMap: Record<ToolType, string> = {
    select: 'select',
    pan: 'pan',
    zoom: 'zoom',
    merge: 'merge',
    measure: 'measure',
    move_station: 'move_station',
    move_segment: 'move_segment',
    move_shot: 'move_shot',
    comment: 'comment_tool',
    move_comment: 'move_comment_tool',
  };
  const configKey = keyMap[tool];
  const bindings = shortcuts[configKey];
  if (Array.isArray(bindings) && bindings.length > 0) {
    return bindings[0] ?? '';
  }
  return '';
}

/** Translation keys for tool labels */
const TOOL_LABEL_KEYS: Record<ToolType, string> = {
  select: 'toolbar.tools.select',
  pan: 'toolbar.tools.pan',
  zoom: 'toolbar.tools.zoom',
  merge: 'toolbar.tools.merge',
  measure: 'toolbar.tools.measure',
  move_station: 'toolbar.tools.moveStation',
  move_segment: 'toolbar.tools.moveSegment',
  move_shot: 'toolbar.tools.moveShot',
  comment: 'toolbar.tools.comment',
  move_comment: 'toolbar.tools.moveComment',
};

const TOOL_GROUP_LABEL_KEYS: Record<string, string> = {
  navigate: 'toolbar.groups.navigate',
  comment: 'toolbar.groups.comment',
  move: 'toolbar.groups.move',
};

/**
 * Toolbar layout definition (#51): navigation tools, a divider, the action
 * tools, a divider, then the standalone measure tool.
 * [Select Pan/Zoom] | [Merge Comment Move] | [Measure]
 */
export const TOOLBAR_ITEMS: ToolbarItem[] = [
  { kind: 'standalone', id: 'select', shortcut: 'S' },
  { kind: 'group', groupId: 'navigate' },
  { kind: 'separator' },
  { kind: 'standalone', id: 'merge', shortcut: 'M' },
  { kind: 'group', groupId: 'comment' },
  { kind: 'group', groupId: 'move' },
  { kind: 'separator' },
  { kind: 'standalone', id: 'measure', shortcut: 'R' },
];

/**
 * Drop every tool `keepTool` rejects from a toolbar layout, then tidy the
 * separators the removals left behind.
 *
 * A GROUP survives if ANY of its members is kept - the group button itself picks
 * the subtool, so a half-kept group is still a usable button.
 *
 * The separator cleanup is the subtle half (#241): removing a whole cluster
 * leaves two ADJACENT separators, and dropping every separator with a separator
 * neighbour deletes both (each sees the other), rendering a toolbar with no
 * divider at all. So: collapse runs of separators to one, and drop any leading or
 * trailing separator.
 *
 * Composable on purpose - the read-only and hide-navigation filters both run
 * through here, in either order, and each pass re-tidies what the previous left.
 */
export function filterToolbarItems(
  items: ToolbarItem[],
  keepTool: (tool: ToolType) => boolean
): ToolbarItem[] {
  const kept = items.filter((item) => {
    if (item.kind === 'separator') return true;
    if (item.kind === 'standalone') return keepTool(item.id);
    const group = TOOL_GROUPS.find((g) => g.id === item.groupId);
    return group ? group.tools.some((tool) => keepTool(tool)) : false;
  });
  const collapsed: ToolbarItem[] = [];
  for (const item of kept) {
    if (item.kind === 'separator') {
      const last = collapsed[collapsed.length - 1];
      if (!last || last.kind === 'separator') continue;
    }
    collapsed.push(item);
  }
  if (collapsed[collapsed.length - 1]?.kind === 'separator') collapsed.pop();
  return collapsed;
}

// Plan/Export are read-only workspaces (#38): the mutating tools (merge, comment,
// move) make no sense there, so they're HIDDEN - not merely disabled - leaving only
// the non-mutating Select / Pan / Zoom / Measure. Filtered once from TOOLBAR_ITEMS so
// it stays in sync, with separators left dangling by the removals cleaned up.
export const READ_ONLY_TOOLBAR_ITEMS: ToolbarItem[] = filterToolbarItems(
  TOOLBAR_ITEMS,
  (tool) => !isMutatingTool(tool)
);

/** What one condense stage turns off, and the classes that stage implies. */
export interface ToolbarCondenseLayout {
  displayCondensed: boolean;
  importCondensed: boolean;
  historyCondensed: boolean;
  tight: boolean;
  centerInFlow: boolean;
  /** Both side clusters (file/history on the left, tools/display on the right). */
  clusterCls: string;
  /** The center slot: the mode carousel, or the fullscreen table title. */
  centerCls: string;
}

/**
 * The condense ladder as a pure function of the stage. Extracted so the policy is
 * testable: jsdom cannot measure the rects the ladder reacts to (that sweep is an
 * e2e), but it can pin that the stages are cumulative and that the final one
 * leaves the browser a layout it can actually satisfy.
 */
export function toolbarCondenseLayout(condense: number): ToolbarCondenseLayout {
  // Final stage: the carousel stops being absolutely positioned and becomes a
  // normal, shrinkable flex item between the two clusters - flexbox then makes
  // overlap structurally impossible - and degrades to icon-only so it asks for as
  // little of the remaining width as possible.
  const centerInFlow = condense >= 5;
  return {
    displayCondensed: condense >= 1,
    importCondensed: condense >= 2,
    historyCondensed: condense >= 3,
    // Squeeze the padding around the icon buttons when even the all-icons toolbar
    // still can't clear the carousel.
    tight: condense >= 4,
    centerInFlow,
    // The clusters are PINNED at every stage, in flow and out of it. They are not
    // merely "things we'd rather not squash": both own absolutely-positioned
    // popovers anchored to their own buttons (DisplayMenu's panel, the tool and
    // tool-group flyouts), so making either one shrinkable-and-scrollable - or
    // clipping the header - would clip those menus to a ~40px-tall strip. The
    // carousel's slot is therefore the only thing that gives, and the carousel
    // itself is bounded to that slot (see ModeCarousel's max-w-full).
    clusterCls: 'flex-shrink-0',
    // Positioning, plus how far the in-flow slot may give. It SHRINKS (so the
    // clusters reclaim width before they start running past the header's edge) but
    // never below one mode button: the carousel caps its own box at the slot, so a
    // slot allowed to reach 0 - which `min-w-0` permits, and which the carousel's
    // own scroll container makes its automatic minimum anyway - renders a
    // zero-width carousel. That is not "cramped", it is the mode switcher GONE,
    // and it is the app's only way into Plan/Export. With a floor the worst case is
    // one visible mode plus a scroll to the others.
    centerCls: centerInFlow ? 'min-w-[3rem] shrink px-1' : 'absolute -translate-x-1/2',
  };
}

export default function Toolbar({
  menuItems,
  saveFailed = false,
  onImport,
  onUndo,
  onRedo,
  canUndo,
  canRedo,
  placementModeActive,
  dataTableFullScreen,
}: ToolbarProps) {
  const { t } = useTranslation();
  const activeTool = useUIStore((s) => s.activeTool);
  const setActiveTool = useUIStore((s) => s.setActiveTool);
  const exitDataTableFullScreen = useUIStore((s) => s.exitDataTableFullScreen);
  const appMode = useUIStore((s) => s.appMode);
  // Toolbar icons appear at the 'toolbar_only' syntactic level and up; the
  // `--sh-*` color vars are the role colors there and a neutral gray at 'off'.
  const syntacticLevel = useUIStore((s) => s.syntacticLevel);
  const showToolbarIcons = syntacticLevelAtLeast(syntacticLevel, 'toolbar_only');
  // Read-only both in the closure-adjusted view AND in Plan/Export (the whole map is
  // non-editable there) - disable the geometry-mutating tools in both. Select/Pan/Zoom/
  // Measure stay enabled, so they remain available in every mode.
  // A remote SpeleoDB project opened without the write lock is read-only - disable
  // the mutating tools + Import exactly like closure-adjusted mode (T2a). The
  // backend edit gate is the real enforcement; this is the visual parity.
  const editingBlocked = useEditingBlocked();
  // The placement-mode dim's gray, drawn without a CSS filter (#576 NF-W12DMP-1): see
  // useGrayscaleTokens. The mode carousel dims itself the same way.
  const placementGray = useGrayscaleTokens(placementModeActive);
  // Survey shows the full toolbar; Plan/Export show only the non-mutating tools.
  // On top of that, the navigate group (Pan/Zoom) is hidden unless the user opted
  // in (#409) - the map pans on a plain drag and zooms on the wheel under every
  // tool, so the tools are redundant for anyone who can do both. Filtering the
  // ALREADY-filtered read-only layout is why filterToolbarItems re-tidies
  // separators on every pass: Plan/Export with navigation off is Select | Measure.
  const config = useProjectStore((s) => s.config);
  const showNavigationTools = navigationToolsEnabled(config);
  const toolbarItems = useMemo(() => {
    const base = appMode === 'survey' ? TOOLBAR_ITEMS : READ_ONLY_TOOLBAR_ITEMS;
    if (showNavigationTools) return base;
    return filterToolbarItems(base, (tool) => !isNavigationTool(tool));
  }, [appMode, showNavigationTools]);

  // Progressive condensation when the left (file/history) and right (tools/display)
  // clusters can't clear the absolutely-centered mode carousel. Because the carousel
  // is position:absolute it doesn't create CSS overflow, so we detect the collision
  // by measuring rects and drop labels in a fixed priority order:
  //   1. "Display" text  -> eye + caret only
  //   2. "Import" text   -> yellow import icon only
  //   3. Undo/Redo text  -> undo/redo icons only
  //   4. tighten the padding around the burger / import / undo / redo / eye buttons
  //   5. the carousel LEAVES absolute positioning and joins the flex flow, as an
  //      icon-only carousel, in a shrinkable slot (#260)
  // Stage 5 is the structural backstop: while the carousel is out of flow it can be
  // positioned on top of a cluster, and stages 1-4 can run out of labels to drop
  // before the gap is wide enough - which is how "Export" ended up rendering
  // underneath the select-tool button. As a normal flex item it is laid out BESIDE
  // the clusters, so overlap stops being expressible in the layout at all.
  //
  // That only holds if the carousel actually FITS the slot it is given, which is
  // why ModeCarousel caps its own box at `max-w-full` and scrolls inside it. A flex
  // item's box is not clipped by its parent's width: with the slot shrunk to a few
  // px the carousel's own box still measured its full intrinsic width and was laid
  // out straight across the tool cluster - a PERSISTENT overlap (surviving a 1.5s
  // settle) at 540 CSS px and below, i.e. reachable by the UI zoom alone. The slot
  // keeps a one-button floor so that cap can never reduce the carousel to nothing.
  //
  // Below roughly 460 CSS px the two clusters' intrinsic widths alone exceed the
  // header, and the surplus has to go somewhere: it stays inside the clusters (the
  // rightmost controls run past the header's right edge) rather than being clipped,
  // because both clusters own absolutely-positioned popovers that a clipping
  // context would cut off. Reclaiming that width needs a stage that REMOVES tool
  // controls (folding them into a menu), not one that resizes them.
  // The SpeleoStatusButton's presence widens the left cluster, so its visibility
  // gates re-measurement too.
  const speleoLinkPresent = useSpeleoStore((s) => s.link != null);
  // #270: the status button grows by a "!" badge when the project has changes only
  // on this computer and no lock protecting them, so that flip changes the left
  // cluster's width and must re-run the fit below, exactly like the button
  // appearing at all.
  const speleoUnpushedBadge = useSpeleoStore(
    (s) => s.link?.hasUnpushedEdits === true && !holdsWriteLock(s.link),
  );
  const CONDENSE_MAX = 5;
  const [condense, setCondense] = useState(0);
  const condenseRef = useRef(0);
  const [headerWidth, setHeaderWidth] = useState(0);
  // Header-relative x (px) to center the carousel on, or null to fall back to the
  // window center (50%) before the first measurement.
  const [carouselLeft, setCarouselLeft] = useState<number | null>(null);
  const headerRef = useRef<HTMLElement>(null);
  const leftRef = useRef<HTMLDivElement>(null);
  const centerRef = useRef<HTMLDivElement>(null);
  const rightRef = useRef<HTMLDivElement>(null);

  const setCondenseBoth = useCallback((n: number) => {
    condenseRef.current = n;
    setCondense(n);
  }, []);

  // Track the header's own width so a window/zoom/panel resize re-runs the fit.
  useEffect(() => {
    const header = headerRef.current;
    if (!header) return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (w != null) setHeaderWidth(w);
    });
    ro.observe(header);
    return () => ro.disconnect();
  }, []);

  // Reset to the full layout whenever the available width OR the natural content
  // width changes, so the step-up pass below re-collapses from scratch (this is how
  // the toolbar EXPANDS again when space is freed). Runs before the step-up effect.
  useLayoutEffect(() => {
    if (condenseRef.current !== 0) setCondenseBoth(0);
  }, [
    setCondenseBoth,
    headerWidth,
    appMode,
    dataTableFullScreen,
    showToolbarIcons,
    // canUndo/canRedo intentionally omitted: the undo/redo buttons now hold a
    // constant-width chevron slot, so their availability no longer changes the
    // left cluster width and must not kick off a reset -> re-fit on every edit.
    editingBlocked,
    speleoLinkPresent,
    speleoUnpushedBadge,
    // The navigate group appearing/disappearing changes the right cluster's
    // natural width (#409), so its toggle must re-run the fit from scratch.
    showNavigationTools,
    t,
  ]);

  // Measure + fit pass, after each render:
  //  1. If the carousel can't fit in the free gap between the two clusters, drop one
  //     more label (priority order above) to widen the gap, until it fits or we're
  //     at the max.
  //  2. Otherwise, center the carousel in that free gap so it sits in the empty
  //     space between the clusters rather than window-centered over a wide cluster
  //     (which is how "Export" ended up under the tool buttons).
  // Once the ladder reaches the in-flow stage there is nothing left to measure:
  // flexbox owns the position and cannot overlap, so the effect bails out.
  // Runs in a layout effect so every step settles before the browser paints (no
  // flicker). The guard skips the stale render right after a reset, where the
  // committed DOM is still at the old level but the ref has been forced to 0.
  useLayoutEffect(() => {
    if (condense !== condenseRef.current) return;
    // In-flow stage: flexbox positions the carousel, so there is no `left` to
    // compute and no overlap to detect. (The reset effect above still drops back
    // to stage 0 when space returns, so this is not a one-way door.)
    if (condense >= CONDENSE_MAX) return;
    const header = headerRef.current;
    const left = leftRef.current;
    const right = rightRef.current;
    const center = centerRef.current;
    if (!header || !left || !right || !center) return;
    const MARGIN = 12;
    const h = header.getBoundingClientRect();
    const c = center.getBoundingClientRect();
    const l = left.getBoundingClientRect();
    const r = right.getBoundingClientRect();
    // Not laid out yet (first paint) or a zero-size test env (jsdom): every rect is
    // 0, so skip until real geometry exists.
    if (c.width === 0 || h.width === 0) return;
    // Everything header-relative (style.left is relative to the positioned header).
    const half = c.width / 2;
    const leftEdge = l.right - h.left;
    const rightEdge = r.left - h.left;
    // The range of carousel centers that keep it clear of both clusters.
    const minCenter = leftEdge + MARGIN + half;
    const maxCenter = rightEdge - MARGIN - half;
    // No such range -> the carousel can't fit the gap; condense one more stage.
    // The last stage takes it out of absolute positioning entirely, so this loop
    // always terminates in a NON-overlapping layout - there is no "least, symmetric
    // overlap" fallback any more, which is what used to render "Export" underneath
    // the select-tool button (#260).
    if (minCenter > maxCenter) {
      setCondenseBoth(condenseRef.current + 1);
      return; // re-measure after the condensed render
    }
    // Prefer the window center, but nudge it just enough to stay in the free space
    // (so a wide right cluster can't overlap "Export").
    const windowCenter = h.width / 2;
    setCarouselLeft(Math.max(minCenter, Math.min(maxCenter, windowCenter)));
  });

  const { displayCondensed, importCondensed, historyCondensed, tight, centerInFlow, clusterCls, centerCls } =
    toolbarCondenseLayout(condense);
  const centerStyle: CSSProperties | undefined = centerInFlow
    ? undefined
    : { left: carouselLeft ?? '50%' };

  return (
    <header ref={headerRef} className="relative flex items-center justify-between px-4 py-2 border-b border-border bg-panel" data-testid="toolbar">
      {/* Left section: hamburger menu, Import, then Undo/Redo */}
      <div
        ref={leftRef}
        className={`flex items-center ${clusterCls} ${tight ? 'gap-0.5' : 'gap-2'} transition-opacity ${placementModeActive ? 'opacity-45' : ''}`}
        style={placementGray}
        data-testid="toolbar-file-controls"
      >
        {/* Disabled during placement mode so the file/project actions that moved
            into this menu (Save/Save As/Close/New/Open/Export) stay blocked, as
            they were when they lived in the now-disabled toolbar buttons. */}
        <AppMenu
          items={menuItems}
          triggerDanger={saveFailed}
          disabled={placementModeActive}
          compact={tight}
          aria-label={t('singleView.menu.aria')}
        />
        {/* SpeleoDB status button (#204/T2c) - right of the hamburger; hidden for
            local projects. One click drives the single-writer hand-off. */}
        <SpeleoStatusButton />
        {/* Import and Undo/Redo are survey-only (#243): Plan/Export are read-only
            workspaces, so the mutating file/history controls are HIDDEN there (not
            merely disabled) - undoing under the read-only map would mutate the survey
            out from under it. One `appMode === 'survey'` wrapper so the file and
            history halves can't drift apart. Fullscreen-table mode is a survey
            sub-state (entering Plan/Export force-exits it via setAppMode ->
            exitDataTableFullScreen, and only a survey-mounted control sets it), so the
            "Back to Map" button lives inside this same gate, swapped in for Import. */}
        {appMode === 'survey' && (
          <>
            {dataTableFullScreen ? (
              <Button
                icon={
                  <span style={{ color: 'var(--o)' } as CSSProperties}>
                    <ShrinkIcon size={16} />
                  </span>
                }
                onClick={() => { setSource('toolbar'); exitDataTableFullScreen(); }}
                size="sm"
                className="sh-tool ml-1"
                style={{ '--o': toolAccentVar('pan') } as CSSProperties}
                data-testid="toolbar-map-view-button"
              >
                {t('dataTable.fullScreen.exit')}
              </Button>
            ) : (
              <Button
                onClick={() => onImport()}
                disabled={placementModeActive || editingBlocked}
                size="sm"
                // Tighten the padding + drop the left margin at the final stage.
                className={tight ? '!px-1' : 'ml-1'}
                // The yellow import icon shows at the toolbar syntactic level, and is
                // forced on when condensed (icon-only) so the button is never empty.
                icon={showToolbarIcons || importCondensed ? <span style={{ color: 'var(--sh-comment)' }}><DownloadIcon size={18} /></span> : undefined}
                aria-label={importCondensed ? t('common.actions.import') : undefined}
                title={importCondensed ? t('common.actions.import') : undefined}
                data-testid="toolbar-import-button"
              >
                {importCondensed ? null : t('common.actions.import')}
              </Button>
            )}
            <div className={`w-px h-6 bg-border ${tight ? 'mx-1' : 'mx-2'}`} />
            <UndoHistoryDropdown
              mode="undo"
              disabled={placementModeActive || !canUndo}
              // [#303] An empty undo stack still has something to show on a linked
              // project: the Unwind entry that continues the walk past the commit
              // boundary. The button stays disabled (Undo / Ctrl+Z must keep doing
              // nothing); only HOVER opens the panel. Placement mode is excluded - that
              // reason for `disabled` must still suppress the panel entirely.
              hoverableWhenEmpty={!placementModeActive}
              onSingleAction={onUndo}
              testId="toolbar-undo-button"
              ariaLabel={t('common.actions.undo')}
              compact={tight}
            >
              <span className="inline-flex items-center gap-0.5">
                {historyCondensed ? <UndoIcon size={16} /> : t('common.actions.undo')}
                {/* Always occupy the chevron slot so toggling canUndo/placement never
                    changes the button width (a width change would re-trigger the
                    carousel-fit compaction). `invisible` hides paint but keeps layout. */}
                <span
                  className={`text-[10px] opacity-60 ${canUndo && !placementModeActive ? '' : 'invisible'}`}
                  aria-hidden="true"
                  data-testid="undo-chevron"
                >
                  <ChevronDownIcon size={10} />
                </span>
              </span>
            </UndoHistoryDropdown>
            <UndoHistoryDropdown
              mode="redo"
              disabled={placementModeActive || !canRedo}
              // [#303] The same reason as the undo side, in the other direction: an empty
              // redo stack still has the forward chain step to show ("Redo into next
              // SpeleoDB version"), and hover is the only way to reach it while the button
              // itself stays correctly disabled.
              hoverableWhenEmpty={!placementModeActive}
              onSingleAction={onRedo}
              testId="toolbar-redo-button"
              ariaLabel={t('common.actions.redo')}
              compact={tight}
            >
              <span className="inline-flex items-center gap-0.5">
                {historyCondensed ? <RedoIcon size={16} /> : t('common.actions.redo')}
                {/* See the undo chevron: constant-width slot so redo availability
                    never nudges the layout into a compaction step. */}
                <span
                  className={`text-[10px] opacity-60 ${canRedo && !placementModeActive ? '' : 'invisible'}`}
                  aria-hidden="true"
                  data-testid="redo-chevron"
                >
                  <ChevronDownIcon size={10} />
                </span>
              </span>
            </UndoHistoryDropdown>
          </>
        )}
      </div>

      {/* Center - the Survey/Plan/Export workspace carousel (#38, the workspace-mode reframe).
          The active map *tool* is already shown by the highlighted toolbar button on
          the right, so the center is free for the mode switch. Fullscreen table mode
          (a survey sub-state) shows the table's title instead. */}
      {dataTableFullScreen ? (
        /* Centered in the free gap between the side clusters (see the measure
           effect), so it never overlaps them - or, at the final stage, laid out
           between them by flexbox. */
        <div
          ref={centerRef}
          className={`${centerCls} font-medium truncate`}
          style={centerStyle}
          data-testid="toolbar-data-table-title"
        >
          {t('dataTable.title')}
        </div>
      ) : (
        /* Centered in the free gap between the side clusters (see the measure
           effect): it sits in the empty space instead of window-centered, so a wide
           right cluster can't overlap "Export". The gap center is independent of the
           carousel's own width, so cycling modes never drifts it. At the final stage
           it joins the flex flow instead, where overlap cannot happen at all. */
        <div
          ref={centerRef}
          className={centerCls}
          style={centerStyle}
        >
          <ModeCarousel disabled={placementModeActive} compact={centerInFlow} />
        </div>
      )}

      {/* Right section - tools and display menu; while the table is fullscreen,
          just the table picker (the "Back to Map" button lives on the left). */}
      <div ref={rightRef} className={`flex items-center gap-2 ${clusterCls}`}>
        {dataTableFullScreen ? (
          /* The "Back to Map" shrink button now lives on the left (in place of
             the project-close button); only the table picker stays on the right. */
          <TableSelectDropdown />
        ) : (
          <>
            <div
              className={`flex items-center gap-1 transition-opacity ${placementModeActive ? 'opacity-45' : ''}`}
              style={placementGray}
              data-testid="toolbar-tools-group"
            >
              {toolbarItems.map((item, index) => {
                if (item.kind === 'separator') {
                  return <div key={`sep-${index}`} className="w-px h-6 bg-border mx-1" />;
                }
                if (item.kind === 'standalone') {
                  return (
                    <ToolButton
                      key={item.id}
                      tool={item.id}
                      active={activeTool === item.id}
                      onClick={() => setActiveTool(item.id)}
                      disabled={placementModeActive || (editingBlocked && isMutatingTool(item.id))}
                    />
                  );
                }
                // group
                const group = TOOL_GROUPS.find((g) => g.id === item.groupId);
                if (!group) return null;
                return (
                  <ToolGroupButton
                    key={group.id}
                    group={group}
                    activeTool={activeTool}
                    onActivateTool={setActiveTool}
                    disabled={placementModeActive || (editingBlocked && group.tools.every(isMutatingTool))}
                  />
                );
              })}
            </div>
            {/* DisplayMenu is available in Survey + Plan (not Export - the Export panel's own
                controls decide what the exported image contains, so a survey Display menu there
                is a confusing no-op). The component keeps the survey-only concerns out of Plan
                internally: the Raw Data View / Closures Computed View switch (Plan forces the
                closure map) and the color overlays (the gas reachability gradient owns the
                coloring there). */}
            {appMode !== 'export' && <DisplayMenu condensed={displayCondensed} tight={tight} />}
          </>
        )}
      </div>
    </header>
  );
}

interface ToolButtonProps {
  tool: ToolType;
  active: boolean;
  onClick: () => void;
  disabled?: boolean;
}

function ToolButton({ tool, active, onClick, disabled = false }: ToolButtonProps) {
  const { t } = useTranslation();
  const [flyoutOpen, setFlyoutOpen] = useState(false);
  const hoverOpenTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const hoverCloseTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastPointerTypeRef = useRef('mouse');

  const config = useProjectStore((s) => s.config);
  const shortcuts = config?.shortcuts ?? defaultConfig.shortcuts;
  const shortcut = getShortcutForTool(tool, shortcuts as unknown as Record<string, string[]>);

  const handleMouseEnter = useCallback(() => {
    if (disabled) return;
    // Suppress informational flyout on touch - no hover on touch devices
    if (lastPointerTypeRef.current === 'touch') return;
    if (hoverCloseTimerRef.current) {
      clearTimeout(hoverCloseTimerRef.current);
      hoverCloseTimerRef.current = null;
    }
    if (!flyoutOpen) {
      hoverOpenTimerRef.current = setTimeout(() => {
        setFlyoutOpen(true);
      }, 300);
    }
  }, [disabled, flyoutOpen]);

  const handleMouseLeave = useCallback(() => {
    if (disabled) return;
    if (hoverOpenTimerRef.current) {
      clearTimeout(hoverOpenTimerRef.current);
      hoverOpenTimerRef.current = null;
    }
    hoverCloseTimerRef.current = setTimeout(() => {
      setFlyoutOpen(false);
    }, 150);
  }, []);

  useEffect(() => {
    return () => {
      if (hoverOpenTimerRef.current) clearTimeout(hoverOpenTimerRef.current);
      if (hoverCloseTimerRef.current) clearTimeout(hoverCloseTimerRef.current);
    };
  }, []);

  const handleClick = useCallback(() => {
    if (disabled) return;
    setSource('toolbar');
    onClick();
    setFlyoutOpen(false);
  }, [disabled, onClick]);

  useEffect(() => {
    if (disabled && flyoutOpen) {
      setFlyoutOpen(false);
    }
  }, [disabled, flyoutOpen]);

  return (
    <div
      className="relative"
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
      onPointerDown={(e) => { lastPointerTypeRef.current = e.pointerType; }}
    >
      <IconButton
        icon={<ToolIcon tool={tool} />}
        onClick={handleClick}
        disabled={disabled}
        aria-label={t(TOOL_LABEL_KEYS[tool])}
        size="sm"
        aria-pressed={active}
        className={`sh-tool ${active ? 'sh-tool--active' : ''}`}
        style={{ '--o': toolAccentVar(tool) } as CSSProperties}
        data-testid={`tool-${tool}`}
      />

      {flyoutOpen && !disabled && (
        <div
          className="absolute top-full right-0 mt-1 z-50 bg-panel border border-border rounded shadow-lg py-1 min-w-[10rem]"
          role="menu"
          aria-label={t(TOOL_LABEL_KEYS[tool])}
          data-testid={`subtool-flyout-${tool}`}
        >
          <button
            role="menuitem"
            className={`
              flex items-center gap-2 w-full px-3 py-1.5 text-sm text-left
              transition-colors
              ${active ? 'bg-accent-20 text-accent' : 'text-text-primary hover:bg-border'}
            `}
            onClick={handleClick}
            tabIndex={0}
            aria-current={active ? 'true' : undefined}
            data-testid={`subtool-${tool}`}
          >
            <span className="w-5 text-center"><ToolIcon tool={tool} /></span>
            <span className="flex-1">{t(TOOL_LABEL_KEYS[tool])}</span>
            {shortcut && <span className="text-text-secondary text-xs ml-2">{shortcut}</span>}
          </button>
        </div>
      )}
    </div>
  );
}

interface ToolGroupButtonProps {
  group: ToolGroupDef;
  activeTool: ToolType;
  onActivateTool: (tool: ToolType) => void;
  disabled?: boolean;
}

function ToolGroupButton({ group, activeTool, onActivateTool, disabled = false }: ToolGroupButtonProps) {
  const { t } = useTranslation();
  const [flyoutOpen, setFlyoutOpen] = useState(false);
  const [focusedIndex, setFocusedIndex] = useState(-1);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const flyoutRef = useRef<HTMLDivElement>(null);
  const hoverOpenTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const hoverCloseTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastPointerTypeRef = useRef('mouse');

  const lastSubtoolByGroup = useUIStore((s) => s.lastSubtoolByGroup);
  const config = useProjectStore((s) => s.config);
  const shortcuts = config?.shortcuts ?? defaultConfig.shortcuts;

  // Determine which subtool is currently displayed on the button
  const isGroupActive = group.tools.includes(activeTool);
  const displayTool: ToolType = isGroupActive
    ? activeTool
    : (lastSubtoolByGroup[group.id] ?? group.defaultTool);

  // Centralized close helper: closes flyout and restores focus to group button
  const closeFlyout = useCallback(() => {
    setFlyoutOpen(false);
    requestAnimationFrame(() => {
      buttonRef.current?.focus();
    });
  }, []);

  // Left-click: If group inactive, activate last-used subtool. If active, cycle.
  // Touch: toggle flyout instead of cycling (let user pick from list).
  const handleClick = useCallback(() => {
    if (disabled) return;
    // Touch tap opens/closes flyout instead of cycling
    if (lastPointerTypeRef.current === 'touch') {
      setFlyoutOpen((prev) => !prev);
      return;
    }
    setSource('toolbar');
    if (!isGroupActive) {
      onActivateTool(displayTool);
    } else {
      // Cycle to next subtool
      const currentIndex = group.tools.indexOf(activeTool);
      const nextIndex = (currentIndex + 1) % group.tools.length;
      const nextTool = group.tools[nextIndex];
      if (nextTool !== undefined) {
        onActivateTool(nextTool);
      }
    }
    setFlyoutOpen(false);
  }, [disabled, isGroupActive, displayTool, activeTool, group.tools, onActivateTool]);

  // Right-click: Toggle flyout
  const handleContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    if (disabled) return;
    setFlyoutOpen((prev) => !prev);
  }, [disabled]);

  // Hover: Open flyout after short delay, close after short delay (to survive gap)
  const handleMouseEnter = useCallback(() => {
    if (disabled) return;
    if (lastPointerTypeRef.current === 'touch') return;
    if (hoverCloseTimerRef.current) {
      clearTimeout(hoverCloseTimerRef.current);
      hoverCloseTimerRef.current = null;
    }
    if (!flyoutOpen) {
      hoverOpenTimerRef.current = setTimeout(() => {
        setFlyoutOpen(true);
      }, 300);
    }
  }, [disabled, flyoutOpen]);

  const handleMouseLeave = useCallback(() => {
    if (disabled) return;
    if (hoverOpenTimerRef.current) {
      clearTimeout(hoverOpenTimerRef.current);
      hoverOpenTimerRef.current = null;
    }
    hoverCloseTimerRef.current = setTimeout(() => {
      setFlyoutOpen(false);
    }, 150);
  }, [disabled]);

  // Close the flyout on outside mousedown / external scroll / resize via the
  // shared hook (the trigger button and the flyout surface are the "inside"
  // elements). The hook binds once when the flyout opens; since it listens on
  // mousedown, the interaction that opened the flyout has already completed, so
  // there's no need for the old microtask-deferred listener attach.
  useDismissableLayer({
    open: flyoutOpen,
    onDismiss: closeFlyout,
    contains: [buttonRef, flyoutRef],
  });

  useEffect(() => {
    if (disabled && flyoutOpen) {
      setFlyoutOpen(false);
    }
  }, [disabled, flyoutOpen]);

  // Focus management: when flyout opens, focus the active subtool item
  useEffect(() => {
    if (!flyoutOpen) {
      setFocusedIndex(-1);
      return;
    }

    // Find active subtool index, or default to first
    const activeIndex = group.tools.indexOf(activeTool);
    const initialIndex = activeIndex >= 0 ? activeIndex : 0;
    setFocusedIndex(initialIndex);
  }, [flyoutOpen, group.tools, activeTool]);

  // Focus the element when focusedIndex changes
  useEffect(() => {
    if (!flyoutOpen || focusedIndex < 0) return;
    const flyout = flyoutRef.current;
    if (!flyout) return;
    const items = flyout.querySelectorAll<HTMLButtonElement>('[role="menuitem"]');
    items[focusedIndex]?.focus();
  }, [flyoutOpen, focusedIndex]);

  // Cleanup hover timers on unmount
  useEffect(() => {
    return () => {
      if (hoverOpenTimerRef.current) clearTimeout(hoverOpenTimerRef.current);
      if (hoverCloseTimerRef.current) clearTimeout(hoverCloseTimerRef.current);
    };
  }, []);

  // Keyboard handler for the flyout
  const handleFlyoutKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (disabled) return;
    switch (e.key) {
      case 'ArrowDown': {
        e.preventDefault();
        setFocusedIndex((prev) => (prev + 1) % group.tools.length);
        break;
      }
      case 'ArrowUp': {
        e.preventDefault();
        setFocusedIndex((prev) => (prev - 1 + group.tools.length) % group.tools.length);
        break;
      }
      case 'Enter':
      case ' ': {
        e.preventDefault();
        const selectedTool = group.tools[focusedIndex];
        if (focusedIndex >= 0 && focusedIndex < group.tools.length && selectedTool !== undefined) {
          onActivateTool(selectedTool);
          closeFlyout();
        }
        break;
      }
      case 'Escape': {
        e.preventDefault();
        e.stopPropagation();
        closeFlyout();
        break;
      }
    }
  }, [disabled, focusedIndex, group.tools, onActivateTool, closeFlyout]);

  // Handle clicking a subtool in the flyout (mouse path also restores focus)
  const handleSubtoolClick = useCallback((tool: ToolType) => {
    if (disabled) return;
    setSource('toolbar');
    onActivateTool(tool);
    closeFlyout();
  }, [disabled, onActivateTool, closeFlyout]);

  const groupLabel = t(TOOL_GROUP_LABEL_KEYS[group.id] ?? group.id);

  return (
    <div
      className="relative"
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
      onPointerDown={(e) => { lastPointerTypeRef.current = e.pointerType; }}
    >
      <IconButton
        ref={buttonRef}
        icon={(
          <span className="relative flex items-center justify-center">
            <ToolIcon tool={displayTool} />
            <span
              className="absolute -bottom-1 -right-1 text-text-secondary pointer-events-none"
              aria-hidden="true"
            >
              <ChevronDownIcon size={8} />
            </span>
          </span>
        )}
        size="sm"
        onClick={handleClick}
        onContextMenu={handleContextMenu}
        disabled={disabled}
        aria-label={t(TOOL_LABEL_KEYS[displayTool])}
        aria-pressed={isGroupActive}
        aria-haspopup="true"
        aria-expanded={flyoutOpen}
        className={`sh-tool ${isGroupActive ? 'sh-tool--active' : ''}`}
        style={{ '--o': toolAccentVar(displayTool) } as CSSProperties}
        data-testid={`tool-group-${group.id}`}
      />

      {flyoutOpen && !disabled && (
        <div
          ref={flyoutRef}
          className="absolute top-full right-0 mt-1 z-50 bg-panel border border-border rounded shadow-lg py-1 min-w-[10rem]"
          role="menu"
          aria-label={t('toolbar.groupAria', { group: groupLabel })}
          onKeyDown={handleFlyoutKeyDown}
          data-testid={`subtool-flyout-${group.id}`}
        >
          {group.tools.map((tool, index) => {
            const shortcut = getShortcutForTool(tool, shortcuts as unknown as Record<string, string[]>);
            const isActive = tool === activeTool;
            const isFocused = index === focusedIndex;
            return (
              <button
                key={tool}
                role="menuitem"
                className={`
                  flex items-center gap-2 w-full px-3 py-1.5 text-sm text-left
                  transition-colors
                  ${isActive ? 'bg-accent-20 text-accent' : 'text-text-primary hover:bg-border'}
                  ${isFocused ? 'outline outline-2 outline-accent outline-offset-[-2px]' : ''}
                `}
                onClick={() => handleSubtoolClick(tool)}
                tabIndex={isFocused ? 0 : -1}
                aria-current={isActive ? 'true' : undefined}
                data-testid={`subtool-${tool}`}
              >
                <span className="w-5 text-center"><ToolIcon tool={tool} /></span>
                <span className="flex-1">{t(TOOL_LABEL_KEYS[tool])}</span>
                {shortcut && <span className="text-text-secondary text-xs ml-2">{shortcut}</span>}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

function ToolIcon({ tool }: { tool: ToolType }) {
  return (
    <span className="inline-flex items-center justify-center">
      <ToolGlyph tool={tool} size={20} />
    </span>
  );
}
