import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { useUIStore } from '../../store/uiStore';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';
import { useShallow } from 'zustand/react/shallow';
import { useProjectStore } from '../../store/projectStore';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { useSpeleoStore, selectSpeleoReadOnly } from '../../store/speleoStore';
import type { StationRef, LineId, ShotId, ShotKind } from '../../types/project';
import { getDeleteShotDialog } from '../../utils/contextMenuHelpers';
import { canSplitShot } from '../../utils/shotSplit';
import {
  combinePreview,
  queryCombineCandidates,
  queryCombineCandidateForLines,
} from '../../utils/combineLines';
import type { CombineCandidate } from '../../types/ipc/CombineCandidate';
import { lineDisplayName } from '../../utils/stationIdentity';
import { setSource, trackContextMenuAction, trackDialogComplete } from '../../utils/telemetry';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import { MENU_ROW_HEIGHT_PX, MENU_ROW_HINT_HEIGHT_PX, MenuRow, MenuSeparator } from './MenuRow';
import { ChevronRightIcon } from './icons';
import { showBlockedNotice } from '../../utils/blockedNotice';
import { anchorOf, SubmenuFlyout, type SubmenuAnchor } from './ContextSubmenu';
import { CAVE_SECTION_VIEW_LABEL_KEYS, caveSectionHeaderMenuItems, caveSectionLineMenuItems } from './caveSectionMenuItems';

/** The widest the menu grows; a hint line wraps within it. */
const MENU_MAX_WIDTH_PX = 288;
import {
  ArrowUpRightIcon,
  BookmarkIcon,
  FlagIcon,
  StageDropIcon,
  TargetIcon,
  WaypointIcon,
} from './icons';
import { SHOT_KIND_CYCLE, SHOT_KIND_MENU_LABEL } from '../../utils/shotKind';
import {
  EntityDeleteSymbol,
  EntityEditSymbol,
  EntityDatumSymbol,
  EntityJumpSymbol,
  EntityJumpUnmarkSymbol,
  EntityReferenceKindSymbol,
  EntityReferenceUnmarkSymbol,
  EntityAddSymbol,
  EntityUnlinkSymbol,
  EntityMoveSymbol,
  EntityReverseSymbol,
  ToolGlyph,
} from './toolIcons';

interface ContextMenuItem {
  label: string;
  action: () => void;
  /** Rows of a flyout opened from this row (on hover, or a click), in place of
   *  `action`. Rows of a flyout may open their own (`ContextSubmenu.tsx`). */
  submenu?: ContextMenuItem[];
  /** Leading action icon, shown at the Icons+ highlighting level. */
  icon?: ReactNode;
  disabled?: boolean;
  /** Why the item is disabled, shown as its tooltip. */
  hint?: string | undefined;
  /** Why a read-only view or file disabled the item: its tooltip only (no
   *  second line, which every row of the menu would then carry), and a click
   *  on the row shows it as the blocked-edit notice. */
  reason?: string | undefined;
  danger?: boolean;
  separator?: boolean;
}

/**
 * The Plan workspace's route-picking menu (#298) for a station.
 *
 * `stageDropCount` is how many stage drops the current rig actually has (0 for All-thirds and any
 * rig that fell back to it). With exactly one there is no ambiguity, so a single "Set as stage
 * drop" is offered; with more, one FLAT entry per stage names the slot explicitly - the shared
 * menu has no submenu support, and an unlabelled "set as stage drop" would leave you guessing
 * which of two markers you just moved.
 *
 * ⚠ The stage-drop entries appear ONLY when the right-clicked station is on the planned route
 * (`onRoute`). A stage drop is a position ALONG the route, and every other writer enforces that:
 * the drag snaps to the nearest route station and the arrow-key nudge is clamped to the route's
 * index range. Writing an off-route station would put the diver's ▼ somewhere they did not click
 * (the engine re-places it at the first route station past the new threshold) AND move the
 * penetration budget using that station's shortest-path gas, which is not gas along the route the
 * plan actually flies - i.e. the unsafe direction. Hiding the entry is the honest option: the
 * action has no meaning off the route.
 */
function buildPlanMenuItems(
  ref: StationRef,
  stageDropCount: number,
  onRoute: boolean,
  closeDialog: () => void,
  t: TFunction
): ContextMenuItem[] {
  const gas = () => useGasPlanStore.getState();
  const pick = (label: string, icon: ReactNode, run: () => void): ContextMenuItem => ({
    label,
    icon,
    action: () => {
      run();
      closeDialog();
    },
  });

  const slots = onRoute ? stageDropCount : 0;
  const stageDrops: ContextMenuItem[] =
    slots === 1
      ? [
          pick(t('gasPanel.menu.setStageDrop'), <StageDropIcon size={16} />, () =>
            gas().setStageDropOverride(0, ref)
          ),
        ]
      : Array.from({ length: slots }, (_unused, i) =>
          pick(t('gasPanel.menu.setStageNDrop', { n: i + 1 }), <StageDropIcon size={16} />, () =>
            gas().setStageDropOverride(i, ref)
          )
        );

  return [
    pick(t('gasPanel.menu.setStart'), <FlagIcon size={16} />, () => gas().setOrigin(ref)),
    pick(t('gasPanel.menu.setWaypoint'), <WaypointIcon size={16} />, () => gas().addWaypoint(ref)),
    pick(t('gasPanel.menu.setEndpoint'), <TargetIcon size={16} />, () => gas().setEndpoint(ref)),
    ...stageDrops,
    pick(t('gasPanel.menu.setMarked'), <BookmarkIcon size={16} />, () => gas().addMarkedPoint(ref)),
  ];
}

export default function ContextMenu() {
  const { closeDialog, dialogData, openDialog, setActiveTool, setMergeStationA, triggerZoomToLine, multiSelection, geometryMode, appMode, syntacticLevel } = useUIStore(useShallow((s) => ({
    closeDialog: s.closeDialog,
    dialogData: s.dialogData,
    openDialog: s.openDialog,
    setActiveTool: s.setActiveTool,
    setMergeStationA: s.setMergeStationA,
    triggerZoomToLine: s.triggerZoomToLine,
    multiSelection: s.multiSelection,
    geometryMode: s.geometryMode,
    appMode: s.appMode,
    syntacticLevel: s.syntacticLevel,
  })));
  const { t } = useTranslation();
  // How many stage drops the CURRENT rig produces (#298) - the backend's own count, so the menu
  // can never offer a slot the engine would ignore (All-thirds and a stage-short rig report none).
  //
  // ⚠ Read ONCE, when the menu mounts, and never re-read. Two reasons, both real:
  // (a) every gas-input setter nulls `reachField` for the length of the refetch - and a plain
  //     left-click in Plan now sets the endpoint - so the very common click-then-right-click
  //     gesture would otherwise open a menu with the stage rows silently missing;
  // (b) as a live subscription the count can change WHILE the menu is open, inserting rows under
  //     the pointer and shifting the whole menu (its `top` is derived from `items.length`).
  // The last known count is the honest answer for the menu's lifetime: it is what the map is
  // currently drawing. `useRef` + lazy init keeps it stable across the re-renders the layer's
  // other subscriptions cause.
  // The count is `stage_drop_bars`, the engine's override INDEX space (one entry per counted stage,
  // always), not `stage_drop_refs` (emptied when nothing is reachable) - so the menu's slot numbers
  // always match the slots `setStageDropOverride` writes and the rows the panel renders.
  const planMenuRouteRef = useRef<{ slots: number; route: Set<string> } | null>(null);
  if (planMenuRouteRef.current === null) {
    // Read the LAST KNOWN field, not the live one. `reachField` is null for the length of every
    // refetch, and in Plan a plain left-click is itself an input (it sets the endpoint) - so the
    // click-then-right-click gesture opens this menu inside that window, where the live field would
    // report zero stage slots and an empty route, hiding the stage-drop entries and marking every
    // station off-route. `lastReachField` is the plan the map is still drawing.
    const gas = useGasPlanStore.getState();
    const field = gas.reachField ?? gas.lastReachField;
    const route = new Set<string>();
    for (const leg of field?.route ?? []) {
      route.add(`${leg.from.line_id}:${leg.from.station_index}`);
      route.add(`${leg.to.line_id}:${leg.to.station_index}`);
    }
    planMenuRouteRef.current = { slots: field?.stage_drop_bars.length ?? 0, route };
  }
  const stageDropCount = planMenuRouteRef.current.slots;
  const planRouteHasStation = (r: StationRef): boolean =>
    planMenuRouteRef.current?.route.has(`${r.line_id}:${r.station_index}`) ?? false;
  // A remote SpeleoDB project without the lock is read-only (T2a) - treat it like
  // closure-adjusted for the right-click edit items.
  const speleoReadOnly = useSpeleoStore(selectSpeleoReadOnly);
  const { project, stations, setShotKind, removeLoopClosure, fileReadOnlyReason } = useProjectStore();
  // A file made with a newer Stalagmight refuses every edit (plan 6.4).
  const fileReadOnly = (fileReadOnlyReason ?? null) !== null;

  const menuRef = useRef<HTMLDivElement>(null);

  // Combine Lines (Survey only): offered on a line END station, and on a
  // line when exactly two lines are selected. Whether the two can be
  // combined, and why not, is the backend's answer, asked once when the menu
  // opens; `undefined` while it is in flight.
  const selectedLineIds = [
    ...new Set(
      multiSelection
        .filter((s) => s.type === 'line' && s.lineId !== undefined)
        .map((s) => s.lineId as LineId)
    ),
  ];
  const combineKey = ((): string | null => {
    const menuTarget = dialogData.target as { type?: string; ref?: StationRef; lineId?: LineId } | undefined;
    if (appMode !== 'survey' || !menuTarget) return null;
    if (menuTarget.type === 'station' && menuTarget.ref) {
      const { line_id, station_index } = menuTarget.ref;
      const shots = project?.lines.find((s) => s.id === line_id)?.shots.length;
      return shots !== undefined && (station_index === 0 || station_index === shots)
        ? `station:${line_id}:${station_index}`
        : null;
    }
    if (menuTarget.type === 'line' && menuTarget.lineId !== undefined) {
      const [a, b] = selectedLineIds;
      return selectedLineIds.length === 2 && selectedLineIds.includes(menuTarget.lineId)
        ? `lines:${a}:${b}`
        : null;
    }
    return null;
  })();
  const [combineAnswer, setCombineAnswer] = useState<{ key: string; candidates: CombineCandidate[] } | null>(null);
  // The open flyout chain: one anchor per level (the row it hangs off, by
  // index in its list, and that row's rect). Empty when no flyout is open.
  const [submenuPath, setSubmenuPath] = useState<SubmenuAnchor[]>([]);
  const flyoutsRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!combineKey) return;
    let live = true;
    const [kind, x, y] = combineKey.split(':').slice(0, 3);
    const query: Promise<CombineCandidate[]> =
      kind === 'station'
        ? queryCombineCandidates({ line_id: Number(x), station_index: Number(y) })
        : queryCombineCandidateForLines(Number(x), Number(y)).then((c) => (c ? [c] : []));
    query.then(
      (candidates) => {
        if (live) setCombineAnswer({ key: combineKey, candidates });
      },
      () => {
        if (live) setCombineAnswer({ key: combineKey, candidates: [] });
      }
    );
    return () => {
      live = false;
    };
  }, [combineKey]);
  const combineCandidates = combineAnswer?.key === combineKey ? combineAnswer.candidates : undefined;
  /**
   * The combine row, always with a hint line (visible under the label, and as
   * its tooltip): "Checking...", the reason it is disabled, or what it joins.
   * Whether it is offered is known synchronously (an END station, or exactly
   * two selected lines), so the row is always there from the first render:
   * disabled with "Checking..." while the backend's answer is in flight, then
   * enabled, or disabled with the reason. Inserting it only once the answer
   * arrived shifted every row below it under the pointer (and moved the whole
   * menu, whose `top` follows `items.length`), the same hazard the stage-drop
   * count above is frozen against. Its label may change in place.
   *
   * On a station, by how many lines end at the point:
   * - two: one row, "Combine Line With <the other>...", opening the dialog;
   * - three: "Combine Lines..." with a submenu of every pair ("A with B");
   * - four or more: "Combine Lines..." > "A with..." > the others, nested.
   * Lines are "combined", never "merged": merging is what stations do.
   * The first-named line is the one the dialog keeps by default. Each
   * pair is enabled, or disabled with its own reason.
   */
  const combineItems = (label: string): ContextMenuItem[] => {
    if (!combineKey) return [];
    const disabled = (hint: string): ContextMenuItem[] => [{ label, action: () => {}, disabled: true, hint }];
    if (combineCandidates === undefined) {
      return disabled('Checking which lines meet here...');
    }
    const [combineCandidate] = combineCandidates;
    if (combineCandidate === undefined) {
      return disabled(
        combineKey.startsWith('lines')
          ? 'These lines cannot be combined'
          : 'No other line ends at this station'
      );
    }
    // Refused outright, or neither line can be the kept one.
    const blockedOf = (c: CombineCandidate) =>
      c.blocked_reason ?? (c.blocked_keeping_a && c.blocked_keeping_b ? c.blocked_keeping_a : null);
    const name = (id: LineId) => lineDisplayName(project?.lines.find((s) => s.id === id)?.name);
    const open = (c: CombineCandidate, keep: LineId) => () => {
      openDialog('combine-lines', { candidate: c, keep, preview: combinePreview(c) });
    };
    const howTheyMeet = (c: CombineCandidate) =>
      c.join_kind === 'closure' ? 'Across a loop closure' : c.join_kind === 'merged' ? 'Merged here' : 'Side by side here';
    if (combineKey.startsWith('lines')) {
      const { line_a: a, line_b: b } = combineCandidate;
      const across = combineCandidate.join_kind === 'closure' ? ', across a loop closure' : '';
      const blocked = blockedOf(combineCandidate);
      return [
        {
          label,
          action: open(combineCandidate, a),
          disabled: blocked !== null,
          hint: blocked ?? `${name(a)} and ${name(b)} meet end to end${across}`,
        },
      ];
    }
    const clicked = Number(combineKey.split(':')[1]);
    const shotsOf = (c: CombineCandidate, id: LineId) => (c.line_a === id ? c.shot_count_a : c.shot_count_b);
    const otherOf = (c: CombineCandidate, id: LineId) => (c.line_a === id ? c.line_b : c.line_a);
    // Every line at the point, the clicked one first.
    const lines: LineId[] = [clicked];
    for (const c of combineCandidates) {
      for (const id of [c.line_a, c.line_b]) if (!lines.includes(id)) lines.push(id);
    }
    if (lines.length === 2) {
      const partner = otherOf(combineCandidate, clicked);
      const blocked = blockedOf(combineCandidate);
      return [
        {
          label: `Combine Line With ${name(partner)}...`,
          action: open(combineCandidate, clicked),
          disabled: blocked !== null,
          hint: blocked ?? howTheyMeet(combineCandidate),
        },
      ];
    }
    const pairOf = (x: LineId, y: LineId) =>
      combineCandidates.find((c) => (c.line_a === x && c.line_b === y) || (c.line_a === y && c.line_b === x));
    const pairRow = (rowLabel: string, c: CombineCandidate, keep: LineId): ContextMenuItem => ({
      label: rowLabel,
      action: open(c, keep),
      disabled: blockedOf(c) !== null,
      hint: blockedOf(c) ?? howTheyMeet(c),
    });
    let submenu: ContextMenuItem[];
    if (lines.length === 3) {
      // Every pair, the clicked line's first.
      submenu = combineCandidates.map((c) => {
        const first = c.line_a === clicked || c.line_b === clicked ? clicked : c.line_a;
        return pairRow(`${name(first)} with ${name(otherOf(c, first))}`, c, first);
      });
    } else {
      // Each line, then whom it can be combined with.
      submenu = lines.flatMap((first) => {
        const rows = lines.flatMap((second) => {
          const c = second === first ? undefined : pairOf(first, second);
          if (!c) return [];
          const shots = shotsOf(c, second);
          return [pairRow(`${name(second)} (${shots} ${shots === 1 ? 'shot' : 'shots'})`, c, first)];
        });
        return rows.length === 0 ? [] : [{ label: `${name(first)} with...`, action: () => {}, submenu: rows }];
      });
    }
    const openCount = combineCandidates.filter((c) => blockedOf(c) === null).length;
    return [
      {
        label,
        action: () => {},
        hint:
          openCount === 0
            ? `None of the ${lines.length} lines ending here can be combined`
            : `${lines.length} lines end here`,
        submenu,
      },
    ];
  };

  const target = dialogData.target as
    | { type: 'station'; ref: StationRef }
    // `alongT` (0..1 from the from-station) is present only for a map
    // right-click: it records where on the shot the pointer actually landed so
    // Insert Station can pre-place the station there. Absent for the data-table
    // menu, which has no click position on the line.
    | { type: 'shot'; lineId: LineId; shotId: ShotId; alongT?: number }
    | { type: 'line'; lineId: LineId }
    // A Section header in the Sections panel (#327).
    | { type: 'cave-section'; sectionId: number }
    | { type: 'datatable-shot'; lineId: LineId; shotId: ShotId; isFirst: boolean; isLast: boolean; totalShots: number; lineName: string }
    | null;
  const position = dialogData.position as { x: number; y: number } | undefined;

  // Close on outside mousedown / external scroll / resize (shared hook). This
  // component only mounts while the context-menu dialog is active, so `open` is
  // always true here. Escape is kept separate - the hook doesn't own it.
  /** The scroller this menu was opened over, named by the opener. Only a menu
   *  raised from a scrolling LIST has one: it is pinned to cursor coordinates
   *  and rendered from the dialog layer, so its own ancestors say nothing about
   *  what can strand it, and probing the coordinates finds its own backdrop.
   *  A map right-click passes nothing and is therefore never dismissed by a
   *  panel scrolling elsewhere, which is the behaviour that matters there. */
  const scrollScope = dialogData.scrollScope as string | undefined;
  // Every caller must answer this, because BOTH defaults have now shipped a bug:
  // inheriting "survive" let the GlobalView menu act on the wrong project after
  // its list scrolled, and inheriting "close" brought back the map menu's
  // flash-and-vanish. A menu either NAMES the scroller that moves it
  // (`scrollScope`) or declares it has none (`survivesScroll`). Missing both is
  // a caller that has not decided - warn loudly in dev rather than pick for it.
  const survivesScroll = dialogData.survivesScroll === true;
  if (import.meta.env.DEV && scrollScope === undefined && !survivesScroll) {
    console.warn(
      '[ContextMenu] neither scrollScope nor survivesScroll given. A cursor-pinned ' +
        'menu must say which scroll invalidates it; see useDismissableLayer.',
    );
  }
  const scrollScopeRef = useRef<HTMLElement | null>(null);
  scrollScopeRef.current = scrollScope ? document.querySelector<HTMLElement>(scrollScope) : null;

  useDismissableLayer({
    open: true,
    onDismiss: closeDialog,
    contains: [menuRef, flyoutsRef],
    anchorScope: scrollScopeRef,
    // Only a caller that has NAMED the scroller which moves this menu can be
    // trusted to survive an unrelated one. Without a scrollScope the menu is
    // pinned to captured cursor coordinates with nothing to re-measure against,
    // so any scroll can leave it over a different row while its actions still
    // target what was under the cursor - the GlobalView recent-projects menu
    // acting on the wrong project.
    // A caller that NAMED its scroller, or one that declared it has none.
    survivesUnrelatedScroll: scrollScope !== undefined || survivesScroll,
  });
  useEffect(() => {
    const handleEscape = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        closeDialog();
      }
    };
    document.addEventListener('keydown', handleEscape);
    return () => {
      document.removeEventListener('keydown', handleEscape);
    };
  }, [closeDialog]);

  // Get shot data for toggle reference
  const shot = target?.type === 'shot'
    ? project?.lines
        .find((s) => s.id === target.lineId)
        ?.shots.find((sh) => sh.id === target.shotId)
    : null;
  /** The shot's current kind, defaulting to permanent when no shot resolves. */
  const shotKind: ShotKind = shot?.kind ?? 'Normal';

  const sameStationRef = (a: StationRef, b: StationRef): boolean => (
    a.line_id === b.line_id && a.station_index === b.station_index
  );

  /**
   * Reverse entries for a line-targeting menu (#24).
   *
   * Always offers the right-clicked line. When the user has several lines
   * multi-selected (and the target is one of them), a second entry reverses the
   * whole selection in one go - separate entries rather than one ambiguous item,
   * so it is always obvious how many lines are about to change.
   */
  const reverseItems = (lineId: LineId): ContextMenuItem[] => {
    // Deduped so the count the menu label promises matches the lines it acts
    // on. Defence in depth on the behaviour: a repeat is NOT a silent no-op - it
    // reverses the line twice, and the backend parity-reduces such a pair out
    // of `reversed_lines` (see `Command::reversed_line_ids`) precisely
    // because the second reverse is a real renumbering that cancels the first.
    const items: ContextMenuItem[] = [
      {
        label: 'Reverse Line',
        action: () => {
          openDialog('reverse-line', { lineIds: [lineId] });
        },
      },
    ];
    if (selectedLineIds.length > 1 && selectedLineIds.includes(lineId)) {
      items.push({
        label: `Reverse ${selectedLineIds.length} Lines`,
        action: () => {
          openDialog('reverse-line', { lineIds: selectedLineIds });
        },
      });
    }
    return items;
  };

  const buildMenuItems = (): ContextMenuItem[] => {
    if (!target) return [];

    if (target.type === 'station') {
      const lineId = target.ref.line_id;
      const stationLine = project?.lines.find((s) => s.id === lineId);
      const currentStation = stations.find((station) => sameStationRef(station.station_ref, target.ref));
      const mergedStations = currentStation?.merged_with ?? [];
      const directClosures = (() => {
        const deduped = new Map<string, { closureId: bigint; stationRef: StationRef }>();
        for (const closure of currentStation?.closures_with ?? []) {
          if (closure.is_transitive) continue;
          const key = closure.closure_id.toString();
          if (!deduped.has(key)) {
            deduped.set(key, {
              closureId: closure.closure_id,
              stationRef: closure.station_ref,
            });
          }
        }
        return Array.from(deduped.values());
      })();
      const totalShots = stationLine?.shots.length ?? 0;
      const isFirstStation = target.ref.station_index === 0;
      const isLastStation = target.ref.station_index === totalShots;

      // Get geometry from neighbor shot for add-shot defaults
      const neighborShot = isFirstStation
        ? stationLine?.shots[0]
        : isLastStation
          ? stationLine?.shots[totalShots - 1]
          : undefined;
      const stationSourceGeometry = neighborShot ? {
        azimuth_in: neighborShot.azimuth_in,
        azimuth_out: neighborShot.azimuth_out,
        depth_in: neighborShot.depth_in,
        depth_out: neighborShot.depth_out,
        pitch_in: neighborShot.pitch_in,
        pitch_out: neighborShot.pitch_out,
      } : undefined;

      const items: ContextMenuItem[] = [
        {
          label: 'Edit Station',
          action: () => {
            openDialog('edit-station', { stationRef: target.ref });
          },
        },
        {
          label: 'Merge With...',
          action: () => {
            setActiveTool('merge');
            setMergeStationA(target.ref);
            trackDialogComplete('context-menu');
            closeDialog();
          },
        },
      ];

      // A junction (plan 5.2): the station is shared by two or more lines, and
      // any one of them can leave it. The dialog names the lines, says what a
      // detach does, and preselects the clicked slot's line.
      if (mergedStations.length > 0) {
        items.push({
          label: 'Detach Line from Station...',
          action: () => {
            trackContextMenuAction('unmerge', 'context-menu');
            openDialog('unmerge-stations', {
              stationRef: target.ref,
              mergedStations,
            });
          },
        });
      }

      if (directClosures.length > 0) {
        items.push({
          label: 'Remove Closure',
          action: () => {
            if (directClosures.length === 1) {
              trackContextMenuAction('remove-closure', 'context-menu');
              trackDialogComplete('context-menu');
              closeDialog();
              void removeLoopClosure(directClosures[0]!.closureId);
              return;
            }

            openDialog('remove-closure', {
              anchorStation: target.ref,
              closures: directClosures,
            });
          },
        });
      }

      // "Add Shot Before" - only for station 0 (first station). prependMode only
      // controls where the shot is inserted; the entered azimuth is stored as-is.
      if (isFirstStation) {
        items.push({
          label: 'Add Shot Before',
          action: () => {
            openDialog('draw-shot', {
              fromStation: {
                line_id: lineId,
                station_index: 0,
              },
              prependMode: true,
              allowZeroLength: true,
              sourceGeometry: stationSourceGeometry,
            });
          },
        });
      }

      // "Add Shot After" - only for last station
      if (isLastStation) {
        items.push({
          label: 'Add Shot After',
          action: () => {
            openDialog('draw-shot', {
              fromStation: {
                line_id: lineId,
                station_index: totalShots,
              },
              appendMode: true,
              allowZeroLength: true,
              sourceGeometry: stationSourceGeometry,
            });
          },
        });
      }

      items.push({ label: '', action: () => {}, separator: true });
      items.push({
        // This exact English string is quoted by ExportControls' TML-unanchored-warning
        // dialog copy (dialogs.tmlUnanchoredWarning.* in en.json). Translating this label
        // will desync that quote - update both together.
        label: 'Set Zero Datum',
        action: () => {
          openDialog('edit-station', {
            stationRef: target.ref,
            forceEnableZeroDatum: true,
            focusGpsInput: true,
          });
        },
      });
      items.push({ label: '', action: () => {}, separator: true });
      items.push(...combineItems('Combine Lines...'));
      if (!isFirstStation && !isLastStation) {
        items.push({
          label: 'Split Line Here',
          action: () => {
            openDialog('split-line-here', {
              lineId,
              at: { type: 'station', station_index: target.ref.station_index },
            });
          },
        });
      }
      items.push({
        label: 'Edit Line',
        action: () => {
          openDialog('edit-line', { lineId });
        },
      });
      items.push(...reverseItems(lineId));
      items.push({
        label: 'Delete Line',
        action: () => {
          openDialog('confirm-delete', { type: 'line', lineId });
        },
        danger: true,
      });
      items.push({ label: '', action: () => {}, separator: true });
      items.push({
        label: 'Delete Station',
        action: () => {
          openDialog('confirm-delete', { type: 'station', stationRef: target.ref });
        },
        danger: true,
      });

      return items;
    }

    if (target.type === 'shot') {
      const line = project?.lines.find((s) => s.id === target.lineId);
      const shotIndex = line?.shots.findIndex((sh) => sh.id === target.shotId) ?? -1;
      const totalShots = line?.shots.length ?? 0;
      const isFirst = shotIndex === 0 && totalShots > 0;
      const isLast = shotIndex >= 0 && shotIndex === totalShots - 1;
      const sourceShot = shotIndex >= 0 ? line?.shots[shotIndex] : undefined;
      const sourceGeometry = sourceShot ? {
        azimuth_in: sourceShot.azimuth_in,
        azimuth_out: sourceShot.azimuth_out,
        depth_in: sourceShot.depth_in,
        depth_out: sourceShot.depth_out,
        pitch_in: sourceShot.pitch_in,
        pitch_out: sourceShot.pitch_out,
      } : undefined;

      const items: ContextMenuItem[] = [
        {
          label: 'Edit Shot',
          action: () => {
            openDialog('edit-shot', { lineId: target.lineId, shotId: target.shotId });
          },
        },
        {
          label: 'Insert Station...',
          action: () => {
            openDialog('insert-station', {
              lineId: target.lineId,
              shotId: target.shotId,
              shotLength: sourceShot?.length ?? 0,
              lineName: lineDisplayName(line?.name),
              // Only set from a map right-click; the dialog prefills the exact
              // clicked spot when present and starts blank otherwise.
              alongT: target.type === 'shot' ? target.alongT : undefined,
            });
          },
          disabled: !sourceShot || !canSplitShot(sourceShot.length),
        },
        {
          label: 'Split Line Here',
          action: () => {
            if (!sourceShot || target.alongT === undefined) return;
            openDialog('split-line-here', {
              lineId: target.lineId,
              at: { type: 'shot', shot_id: target.shotId, distance: target.alongT * sourceShot.length },
            });
          },
          disabled: !sourceShot || target.alongT === undefined,
        },
        // A context menu has room to offer both other kinds outright, which is
        // clearer than the table cell's cycle (#27). Only the kinds the shot is
        // not already are listed. Going back to permanent reads as "Unmark
        // <what it currently is>" rather than a generic "Mark as Permanent",
        // since that is how the user thinks of undoing the mark.
        ...SHOT_KIND_CYCLE.filter((kind) => kind !== shotKind).map((kind) => ({
          // Hardcoded English to match every other label in this file; the
          // menu has no i18n wiring (tracked separately).
          label:
            kind === 'Normal'
              ? `Unmark ${SHOT_KIND_MENU_LABEL[shotKind]}`
              : `Mark as ${SHOT_KIND_MENU_LABEL[kind]}`,
          action: async () => {
            trackContextMenuAction('set-shot-kind', 'context-menu');
            await setShotKind(target.lineId, target.shotId, kind);
            trackDialogComplete('context-menu');
            closeDialog();
          },
        })),
      ];

      // "Add Shot Before" - only for first shot (prepend mode)
      if (isFirst) {
        items.push({
          label: 'Add Shot Before',
          action: () => {
            openDialog('draw-shot', {
              fromStation: {
                line_id: target.lineId,
                station_index: 0,
              },
              prependMode: true,
              allowZeroLength: true,
              sourceGeometry,
            });
          },
        });
      }

      // "Add Shot After" - only for last shot
      if (isLast) {
        items.push({
          label: 'Add Shot After',
          action: () => {
            openDialog('draw-shot', {
              fromStation: {
                line_id: target.lineId,
                station_index: totalShots,
              },
              appendMode: true,
              allowZeroLength: true,
              sourceGeometry,
            });
          },
        });
      }

      items.push({ label: '', action: () => {}, separator: true });
      items.push({
        label: 'Edit Line',
        action: () => {
          openDialog('edit-line', { lineId: target.lineId });
        },
      });
      items.push(...reverseItems(target.lineId));
      items.push({
        label: 'Delete Line',
        action: () => {
          openDialog('confirm-delete', { type: 'line', lineId: target.lineId });
        },
        danger: true,
      });
      items.push({ label: '', action: () => {}, separator: true });
      // Position-aware Delete Shot using shared helper.
      // Disabled when line/shot lookup fails (stale target).
      const canDelete = shotIndex >= 0 && totalShots > 0;
      items.push({
        label: 'Delete Shot',
        action: () => {
          if (!canDelete) return;
          const { dialog, data } = getDeleteShotDialog(
            target.lineId,
            target.shotId,
            isFirst,
            isLast,
            totalShots,
            lineDisplayName(line?.name),
          );
          openDialog(dialog, data);
        },
        danger: true,
        disabled: !canDelete,
      });

      return items;
    }

    if (target.type === 'line') {
      return [
        {
          label: 'Edit Line',
          action: () => {
            openDialog('edit-line', { lineId: target.lineId });
          },
        },
        ...reverseItems(target.lineId),
        ...combineItems(`Combine ${selectedLineIds.length} Lines...`),
        {
          label: 'Adjust Position...',
          action: () => {
            openDialog('line-adjust', { lineId: target.lineId });
          },
        },
        {
          label: 'Zoom to Line',
          action: () => {
            triggerZoomToLine(target.lineId);
            trackDialogComplete('context-menu');
            closeDialog();
          },
        },
        // "Move to Section" (#327): the line, or the line selection it is in.
        ...caveSectionLineMenuItems(target.lineId, selectedLineIds, project, t, closeDialog),
        { label: '', action: () => {}, separator: true },
        {
          label: 'Delete Line',
          action: () => {
            openDialog('confirm-delete', { type: 'line', lineId: target.lineId });
          },
          danger: true,
        },
      ];
    }

    if (target.type === 'cave-section') {
      return caveSectionHeaderMenuItems(target.sectionId, project, t, closeDialog);
    }

    if (target.type === 'datatable-shot') {
      const items: ContextMenuItem[] = [];

      // Look up the source shot for geometry copying
      const sourceLine = project?.lines.find((s) => s.id === target.lineId);
      const sourceShot = sourceLine?.shots.find((sh) => sh.id === target.shotId);
      // Copy geometry only (azimuth, depth, pitch) - not temperature/marker_index
      const sourceGeometry = sourceShot ? {
        azimuth_in: sourceShot.azimuth_in,
        azimuth_out: sourceShot.azimuth_out,
        depth_in: sourceShot.depth_in,
        depth_out: sourceShot.depth_out,
        pitch_in: sourceShot.pitch_in,
        pitch_out: sourceShot.pitch_out,
      } : undefined;

      // "Add Shot Before" - only for first shot (prepend mode)
      if (target.isFirst) {
        items.push({
          label: 'Add Shot Before',
          action: () => {
            openDialog('draw-shot', {
              fromStation: {
                line_id: target.lineId,
                station_index: 0,
              },
              prependMode: true,
              allowZeroLength: true,
              sourceGeometry,
            });
          },
        });
      }

      // "Add Shot After" - only for last shot
      if (target.isLast) {
        items.push({
          label: 'Add Shot After',
          action: () => {
            openDialog('draw-shot', {
              fromStation: {
                line_id: target.lineId,
                station_index: target.totalShots,
              },
              appendMode: true,
              allowZeroLength: true,
              sourceGeometry,
            });
          },
        });
      }

      // Separator before delete (only if there are add items above)
      if (items.length > 0) {
        items.push({ label: '', action: () => {}, separator: true });
      }

      // "Delete Shot" - position-aware using shared helper
      items.push({
        label: 'Delete Shot',
        action: () => {
          const { dialog, data } = getDeleteShotDialog(
            target.lineId,
            target.shotId,
            target.isFirst,
            target.isLast,
            target.totalShots,
            target.lineName,
          );
          openDialog(dialog, data);
        },
        danger: true,
      });

      return items;
    }

    return [];
  };

  // Leading action icon per menu item (shown at the Icons+ highlighting level).
  const showIcons = syntacticLevelAtLeast(syntacticLevel, 'icons');
  const iconForLabel = (label: string): ReactNode => {
    switch (label) {
      case 'Edit Station':
        return <EntityEditSymbol entity="station" size={18} />;
      case 'Edit Shot':
        return <EntityEditSymbol entity="shot" size={18} />;
      case 'Edit Line':
        return <EntityEditSymbol entity="line" size={18} />;
      case 'Reverse Line':
        return <EntityReverseSymbol entity="line" size={18} />;
      case 'Insert Station...':
        return <EntityAddSymbol entity="station" size={18} />;
      case 'Split Line Here':
        return <EntityUnlinkSymbol entity="line" size={18} />;
      case 'Merge With...':
        return <ToolGlyph tool="merge" size={16} />;
      // The junction detach (plan 5.2) was "Unmerge Stations" before the station graph.
      case 'Detach Line from Station...':
      case 'Remove Closure':
        return <EntityUnlinkSymbol entity="station" size={18} />;
      case 'Add Shot Before':
      case 'Add Shot After':
        return <EntityAddSymbol entity="shot" size={18} />;
      // #27: the shot glyph with the same letter badge the data table shows,
      // so "R" here and "R" in the J/R cell are visibly the same thing. The
      // Unmark variants strike the letter through.
      case `Mark as ${SHOT_KIND_MENU_LABEL.Jump}`:
        return <EntityJumpSymbol entity="shot" size={18} />;
      case `Mark as ${SHOT_KIND_MENU_LABEL.Reference}`:
        return <EntityReferenceKindSymbol entity="shot" size={18} />;
      case `Unmark ${SHOT_KIND_MENU_LABEL.Jump}`:
        return <EntityJumpUnmarkSymbol entity="shot" size={18} />;
      case `Unmark ${SHOT_KIND_MENU_LABEL.Reference}`:
        return <EntityReferenceUnmarkSymbol entity="shot" size={18} />;
      case 'Set Zero Datum':
        return <EntityDatumSymbol entity="station" size={18} />;
      case 'Delete Line':
        return <EntityDeleteSymbol entity="line" size={18} />;
      case 'Delete Station':
        return <EntityDeleteSymbol entity="station" size={18} />;
      case 'Delete Shot':
        return <EntityDeleteSymbol entity="shot" size={18} />;
      case 'Adjust Position...':
        return <EntityMoveSymbol entity="line" size={18} />;
      case 'Zoom to Line':
        return <ToolGlyph tool="zoom" size={16} />;
      default:
        // "Reverse N Lines" carries the count in its label, so it can't be a
        // literal case.
        if (label.startsWith('Reverse ')) {
          return <EntityReverseSymbol entity="line" size={18} />;
        }
        // "Combine Lines...", "Combine Line With <name>..." and "Combine 2 Lines...".
        if (label.startsWith('Combine ')) {
          return <ToolGlyph tool="merge" size={16} />;
        }
        // Fallback for any unmapped label (every other current menu label is
        // handled by an explicit case above).
        return <ArrowUpRightIcon size={16} />;
    }
  };

  // Plan / Export are READ-ONLY: the survey mutation menu must not appear at all (not merely
  // disabled). In Plan a right-click is the route-picking menu (#298) - start, way point, stage
  // drop, marked point, endpoint. These act on PLAN data (the .plnx sidecar), never on the survey,
  // so they are deliberately not gated by the survey read-only predicates: a closure-adjusted view
  // or a SpeleoDB lock must not stop you planning a dive. Export, or a non-station target in Plan,
  // gets no menu. Survey keeps the full menu, with the closure-adjusted (read-only) view disabling
  // mutations as before.
  //
  // Labels are i18n keys and each item carries its own `icon`, NOT `iconForLabel` - that helper
  // matches on English label strings and would silently fall back to the generic arrow under any
  // other locale.
  let items: ContextMenuItem[];
  if (appMode !== 'survey') {
    items =
      appMode === 'plan' && target?.type === 'station'
        ? buildPlanMenuItems(target.ref, stageDropCount, planRouteHasStation(target.ref), closeDialog, t)
        : [];
  } else {
    // Survey view: closure-adjusted geometry is read-only - disable mutations (all except
    // read-only navigation like "Zoom to Line"). Edits are made on the measured view.
    const geometryReadOnly = geometryMode === 'closureAdjusted' || speleoReadOnly || fileReadOnly;
    // What the disabled rows say on hover (demo finding I-10: they read as
    // enabled, and nothing said why). The file wins: leaving the view or
    // taking a lock would not make it editable.
    const readOnlyReason = fileReadOnly
      ? t('editGate.readOnlyFileNotice')
      : speleoReadOnly
        ? t('editGate.readOnlyNotice')
        : t('map.readOnlyPill.resumeRawTitle');
    const READ_ONLY_SAFE_LABELS = new Set(['Zoom to Line', ...CAVE_SECTION_VIEW_LABEL_KEYS.map((k) => t(k))]);
    const gate = (item: ContextMenuItem): ContextMenuItem => {
      const nested = item.submenu ? { ...item, submenu: item.submenu.map(gate) } : item;
      return geometryReadOnly && !item.separator && !READ_ONLY_SAFE_LABELS.has(item.label)
        ? { ...nested, disabled: true, reason: readOnlyReason }
        : nested;
    };
    items = buildMenuItems().map((item) => {
      const withIcon = item.separator ? item : { ...item, icon: item.icon ?? iconForLabel(item.label) };
      return gate(withIcon);
    });
  }

  if (!target || !position || items.length === 0) {
    return null;
  }

  // Adjust position to stay within viewport. A row with a hint is taller (its
  // hint is a fixed single line, so the height is known before it renders), and
  // a menu with hints may be as wide as its max width.
  const menuHeight = items.reduce(
    (height, item) => height + MENU_ROW_HEIGHT_PX + (item.hint !== undefined ? MENU_ROW_HINT_HEIGHT_PX : 0),
    0
  );
  const menuWidth = items.some((item) => item.hint !== undefined) ? MENU_MAX_WIDTH_PX : 200;
  const adjustedPosition = {
    x: Math.min(position.x, window.innerWidth - menuWidth),
    y: Math.min(position.y, window.innerHeight - menuHeight - 20),
  };

  return (
    <div className="fixed inset-0 z-50">
      <div className="absolute inset-0" onClick={closeDialog} data-testid="context-menu-backdrop" />
      <div
        ref={menuRef}
        className="absolute bg-panel border border-border rounded-lg shadow-xl py-1 min-w-[180px]"
        data-testid="context-menu"
        style={{ left: adjustedPosition.x, top: adjustedPosition.y, maxWidth: MENU_MAX_WIDTH_PX }}
      >
        {items.map((item, index) => {
          if (item.separator) {
            return <MenuSeparator key={index} />;
          }

          const hasSubmenu = item.submenu !== undefined;
          const expanded = submenuPath[0]?.index === index;
          const openSubmenuFrom = (row: HTMLElement) => setSubmenuPath([anchorOf(row, index)]);
          return (
            <MenuRow
              key={index}
              data-testid={`context-menu-item-${index}`}
              onClick={(e) => {
                // A row the read-only gate turned off stays clickable so it can
                // say why (`reason`, demo finding N-12): the notice, never the action.
                if (item.disabled) {
                  if (item.reason !== undefined) showBlockedNotice(item.reason);
                  return;
                }
                if (hasSubmenu) {
                  if (expanded) setSubmenuPath([]);
                  else openSubmenuFrom(e.currentTarget);
                  return;
                }
                setSource('context-menu');
                item.action();
              }}
              onMouseEnter={(e) => {
                if (hasSubmenu && !item.disabled) {
                  if (!expanded) openSubmenuFrom(e.currentTarget);
                } else if (submenuPath.length > 0) {
                  setSubmenuPath([]);
                }
              }}
              aria-haspopup={hasSubmenu ? 'menu' : undefined}
              aria-expanded={hasSubmenu ? expanded : undefined}
              disabled={item.disabled}
              title={item.hint}
              hint={item.hint}
              reason={item.disabled ? item.reason : undefined}
              label={item.label}
              icon={item.icon}
              showIcons={showIcons}
              className={`${item.disabled ? 'text-text-secondary cursor-not-allowed' : 'hover:bg-accent-10'} ${
                expanded ? 'bg-accent-10' : ''
              } ${item.danger && !item.disabled ? 'text-error hover:bg-error-10' : ''}`}
            >
              {hasSubmenu && <ChevronRightIcon size={14} className="text-text-secondary flex-shrink-0" />}
            </MenuRow>
          );
        })}
      </div>
      <div ref={flyoutsRef}>
        {submenuPath.map((anchor, level) => {
          // The rows of this level: walk the open chain down from the top.
          let list: ContextMenuItem[] | undefined = items;
          for (const step of submenuPath.slice(0, level + 1)) list = list?.[step.index]?.submenu;
          return list ? (
            <SubmenuFlyout
              key={level}
              level={level + 1}
              anchor={anchor}
              items={list}
              path={submenuPath}
              setPath={setSubmenuPath}
            />
          ) : null;
        })}
      </div>
    </div>
  );
}
