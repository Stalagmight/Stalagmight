import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { useUIStore } from '../../store/uiStore';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';
import { useShallow } from 'zustand/react/shallow';
import { useProjectStore } from '../../store/projectStore';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { useSpeleoStore, selectSpeleoReadOnly } from '../../store/speleoStore';
import type { StationRef, SectionId, ShotId, ShotKind } from '../../types/project';
import { getDeleteShotDialog } from '../../utils/contextMenuHelpers';
import { canSplitShot } from '../../utils/shotSplit';
import { combinePreview, queryCombineCandidate, queryCombineCandidateForSections } from '../../utils/combineSections';
import type { CombineCandidate } from '../../types/ipc/CombineCandidate';
import { formatShotId, formatStationIdWithName } from '../../utils/formatters';
import { setSource, trackContextMenuAction, trackDialogComplete } from '../../utils/telemetry';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import { MenuRow, MenuSeparator } from './MenuRow';
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
  /** Leading action icon, shown at the Icons+ highlighting level. */
  icon?: ReactNode;
  disabled?: boolean;
  /** Why the item is disabled, shown as its tooltip. */
  hint?: string | undefined;
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
  const { closeDialog, dialogData, openDialog, setActiveTool, setMergeStationA, triggerZoomToSection, multiSelection, geometryMode, appMode, syntacticLevel } = useUIStore(useShallow((s) => ({
    closeDialog: s.closeDialog,
    dialogData: s.dialogData,
    openDialog: s.openDialog,
    setActiveTool: s.setActiveTool,
    setMergeStationA: s.setMergeStationA,
    triggerZoomToSection: s.triggerZoomToSection,
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
      route.add(`${leg.from.section_id}:${leg.from.station_index}`);
      route.add(`${leg.to.section_id}:${leg.to.station_index}`);
    }
    planMenuRouteRef.current = { slots: field?.stage_drop_bars.length ?? 0, route };
  }
  const stageDropCount = planMenuRouteRef.current.slots;
  const planRouteHasStation = (r: StationRef): boolean =>
    planMenuRouteRef.current?.route.has(`${r.section_id}:${r.station_index}`) ?? false;
  // A remote SpeleoDB project without the lock is read-only (T2a) - treat it like
  // closure-adjusted for the right-click edit items.
  const speleoReadOnly = useSpeleoStore(selectSpeleoReadOnly);
  const { project, stations, setShotKind, unmergeStations, unmergeStationPair, removeLoopClosure } = useProjectStore();

  const menuRef = useRef<HTMLDivElement>(null);

  // Combine Sections (Survey only): offered on a section END station, and on a
  // section when exactly two sections are selected. Whether the two can be
  // combined, and why not, is the backend's answer, asked once when the menu
  // opens; `undefined` while it is in flight.
  const selectedSectionIds = [
    ...new Set(
      multiSelection
        .filter((s) => s.type === 'section' && s.sectionId !== undefined)
        .map((s) => s.sectionId as SectionId)
    ),
  ];
  const combineKey = ((): string | null => {
    const menuTarget = dialogData.target as { type?: string; ref?: StationRef; sectionId?: SectionId } | undefined;
    if (appMode !== 'survey' || !menuTarget) return null;
    if (menuTarget.type === 'station' && menuTarget.ref) {
      const { section_id, station_index } = menuTarget.ref;
      const shots = project?.sections.find((s) => s.id === section_id)?.shots.length;
      return shots !== undefined && (station_index === 0 || station_index === shots)
        ? `station:${section_id}:${station_index}`
        : null;
    }
    if (menuTarget.type === 'section' && menuTarget.sectionId !== undefined) {
      const [a, b] = selectedSectionIds;
      return selectedSectionIds.length === 2 && selectedSectionIds.includes(menuTarget.sectionId)
        ? `sections:${a}:${b}`
        : null;
    }
    return null;
  })();
  const [combineAnswer, setCombineAnswer] = useState<{ key: string; candidate: CombineCandidate | null } | null>(null);
  useEffect(() => {
    if (!combineKey) return;
    let live = true;
    const [kind, x, y] = combineKey.split(':').slice(0, 3);
    const query =
      kind === 'station'
        ? queryCombineCandidate({ section_id: Number(x), station_index: Number(y) })
        : queryCombineCandidateForSections(Number(x), Number(y));
    query.then(
      (candidate) => {
        if (live) setCombineAnswer({ key: combineKey, candidate });
      },
      () => {
        if (live) setCombineAnswer({ key: combineKey, candidate: null });
      }
    );
    return () => {
      live = false;
    };
  }, [combineKey]);
  const combineCandidate = combineAnswer?.key === combineKey ? combineAnswer.candidate : undefined;
  /** The Combine item: disabled while checking, and with the reason when blocked. */
  const combineItems = (label: string): ContextMenuItem[] => {
    if (!combineKey) return [];
    if (combineCandidate === undefined) {
      return [{ label, action: () => {}, disabled: true, hint: 'Checking whether these sections can be combined...' }];
    }
    if (combineCandidate === null) {
      return combineKey.startsWith('sections')
        ? [{ label, action: () => {}, disabled: true, hint: 'These sections cannot be combined' }]
        : [];
    }
    const blocked = combineCandidate.blocked_reason;
    return [
      {
        label,
        action: () => {
          openDialog('combine-sections', { candidate: combineCandidate, preview: combinePreview(combineCandidate) });
        },
        disabled: blocked !== null,
        hint: blocked ?? undefined,
      },
    ];
  };

  const target = dialogData.target as
    | { type: 'station'; ref: StationRef }
    // `alongT` (0..1 from the from-station) is present only for a map
    // right-click: it records where on the shot the pointer actually landed so
    // Insert Station can pre-place the station there. Absent for the data-table
    // menu, which has no click position on the line.
    | { type: 'shot'; sectionId: SectionId; shotId: ShotId; alongT?: number }
    | { type: 'section'; sectionId: SectionId }
    | { type: 'datatable-shot'; sectionId: SectionId; shotId: ShotId; isFirst: boolean; isLast: boolean; totalShots: number; sectionName: string }
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
    contains: [menuRef],
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
    ? project?.sections
        .find((s) => s.id === target.sectionId)
        ?.shots.find((sh) => sh.id === target.shotId)
    : null;
  /** The shot's current kind, defaulting to permanent when no shot resolves. */
  const shotKind: ShotKind = shot?.kind ?? 'Normal';

  const sameStationRef = (a: StationRef, b: StationRef): boolean => (
    a.section_id === b.section_id && a.station_index === b.station_index
  );

  /**
   * Reverse entries for a section-targeting menu (#24).
   *
   * Always offers the right-clicked section. When the user has several sections
   * multi-selected (and the target is one of them), a second entry reverses the
   * whole selection in one go - separate entries rather than one ambiguous item,
   * so it is always obvious how many sections are about to change.
   */
  const reverseItems = (sectionId: SectionId): ContextMenuItem[] => {
    // Deduped so the count the menu label promises matches the sections it acts
    // on. Defence in depth on the behaviour: a repeat is NOT a silent no-op - it
    // reverses the section twice, and the backend parity-reduces such a pair out
    // of `reversed_sections` (see `Command::reversed_section_ids`) precisely
    // because the second reverse is a real renumbering that cancels the first.
    const items: ContextMenuItem[] = [
      {
        label: 'Reverse Section',
        action: () => {
          openDialog('reverse-section', { sectionIds: [sectionId] });
        },
      },
    ];
    if (selectedSectionIds.length > 1 && selectedSectionIds.includes(sectionId)) {
      items.push({
        label: `Reverse ${selectedSectionIds.length} Sections`,
        action: () => {
          openDialog('reverse-section', { sectionIds: selectedSectionIds });
        },
      });
    }
    return items;
  };

  const buildMenuItems = (): ContextMenuItem[] => {
    if (!target) return [];

    if (target.type === 'station') {
      const sectionId = target.ref.section_id;
      const stationSection = project?.sections.find((s) => s.id === sectionId);
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
      const totalShots = stationSection?.shots.length ?? 0;
      const isFirstStation = target.ref.station_index === 0;
      const isLastStation = target.ref.station_index === totalShots;

      // Get geometry from neighbor shot for add-shot defaults
      const neighborShot = isFirstStation
        ? stationSection?.shots[0]
        : isLastStation
          ? stationSection?.shots[totalShots - 1]
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

      if (mergedStations.length > 0) {
        items.push({
          label: 'Unmerge Stations',
          action: () => {
            if (mergedStations.length === 1) {
              const mergeTarget = mergedStations[0]!;
              const isDirectMerge = project?.station_merges.some((merge) => {
                const direct = sameStationRef(merge.station_a, target.ref) && sameStationRef(merge.station_b, mergeTarget);
                const reverse = sameStationRef(merge.station_a, mergeTarget) && sameStationRef(merge.station_b, target.ref);
                return direct || reverse;
              }) ?? false;
              trackContextMenuAction('unmerge', 'context-menu');
              trackDialogComplete('context-menu');
              closeDialog();
              void (isDirectMerge
                ? unmergeStationPair(target.ref, mergeTarget)
                : unmergeStations(target.ref));
              return;
            }

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
                section_id: sectionId,
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
                section_id: sectionId,
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
      items.push(...combineItems('Combine Sections...'));
      if (!isFirstStation && !isLastStation) {
        items.push({
          label: 'Split Section Here',
          action: () => {
            openDialog('split-section-here', {
              sectionId,
              at: { type: 'station', station_index: target.ref.station_index },
              label: formatStationIdWithName(sectionId, target.ref.station_index, currentStation?.name),
            });
          },
        });
      }
      items.push({
        label: 'Edit Section',
        action: () => {
          openDialog('edit-section', { sectionId });
        },
      });
      items.push(...reverseItems(sectionId));
      items.push({
        label: 'Delete Section',
        action: () => {
          openDialog('confirm-delete', { type: 'section', sectionId });
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
      const section = project?.sections.find((s) => s.id === target.sectionId);
      const shotIndex = section?.shots.findIndex((sh) => sh.id === target.shotId) ?? -1;
      const totalShots = section?.shots.length ?? 0;
      const isFirst = shotIndex === 0 && totalShots > 0;
      const isLast = shotIndex >= 0 && shotIndex === totalShots - 1;
      const sourceShot = shotIndex >= 0 ? section?.shots[shotIndex] : undefined;
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
            openDialog('edit-shot', { sectionId: target.sectionId, shotId: target.shotId });
          },
        },
        {
          label: 'Insert Station...',
          action: () => {
            openDialog('insert-station', {
              sectionId: target.sectionId,
              shotId: target.shotId,
              shotLength: sourceShot?.length ?? 0,
              sectionName: section?.name ?? `Section ${target.sectionId}`,
              // Only set from a map right-click; the dialog prefills the exact
              // clicked spot when present and starts blank otherwise.
              alongT: target.type === 'shot' ? target.alongT : undefined,
            });
          },
          disabled: !sourceShot || !canSplitShot(sourceShot.length),
        },
        {
          label: 'Split Section Here',
          action: () => {
            if (!sourceShot || target.alongT === undefined) return;
            openDialog('split-section-here', {
              sectionId: target.sectionId,
              at: { type: 'shot', shot_id: target.shotId, distance: target.alongT * sourceShot.length },
              label: formatShotId(target.sectionId, shotIndex + 1),
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
            await setShotKind(target.sectionId, target.shotId, kind);
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
                section_id: target.sectionId,
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
                section_id: target.sectionId,
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
        label: 'Edit Section',
        action: () => {
          openDialog('edit-section', { sectionId: target.sectionId });
        },
      });
      items.push(...reverseItems(target.sectionId));
      items.push({
        label: 'Delete Section',
        action: () => {
          openDialog('confirm-delete', { type: 'section', sectionId: target.sectionId });
        },
        danger: true,
      });
      items.push({ label: '', action: () => {}, separator: true });
      // Position-aware Delete Shot using shared helper.
      // Disabled when section/shot lookup fails (stale target).
      const canDelete = shotIndex >= 0 && totalShots > 0;
      items.push({
        label: 'Delete Shot',
        action: () => {
          if (!canDelete) return;
          const { dialog, data } = getDeleteShotDialog(
            target.sectionId,
            target.shotId,
            isFirst,
            isLast,
            totalShots,
            section?.name ?? `Section ${target.sectionId}`,
          );
          openDialog(dialog, data);
        },
        danger: true,
        disabled: !canDelete,
      });

      return items;
    }

    if (target.type === 'section') {
      return [
        {
          label: 'Edit Section',
          action: () => {
            openDialog('edit-section', { sectionId: target.sectionId });
          },
        },
        ...reverseItems(target.sectionId),
        ...combineItems(`Combine ${selectedSectionIds.length} Sections...`),
        {
          label: 'Adjust Position...',
          action: () => {
            openDialog('section-adjust', { sectionId: target.sectionId });
          },
        },
        {
          label: 'Zoom to Section',
          action: () => {
            triggerZoomToSection(target.sectionId);
            trackDialogComplete('context-menu');
            closeDialog();
          },
        },
        { label: '', action: () => {}, separator: true },
        {
          label: 'Delete Section',
          action: () => {
            openDialog('confirm-delete', { type: 'section', sectionId: target.sectionId });
          },
          danger: true,
        },
      ];
    }

    if (target.type === 'datatable-shot') {
      const items: ContextMenuItem[] = [];

      // Look up the source shot for geometry copying
      const sourceSection = project?.sections.find((s) => s.id === target.sectionId);
      const sourceShot = sourceSection?.shots.find((sh) => sh.id === target.shotId);
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
                section_id: target.sectionId,
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
                section_id: target.sectionId,
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
            target.sectionId,
            target.shotId,
            target.isFirst,
            target.isLast,
            target.totalShots,
            target.sectionName,
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
      case 'Edit Section':
        return <EntityEditSymbol entity="section" size={18} />;
      case 'Reverse Section':
        return <EntityReverseSymbol entity="section" size={18} />;
      case 'Insert Station...':
        return <EntityAddSymbol entity="station" size={18} />;
      case 'Split Section Here':
        return <EntityUnlinkSymbol entity="section" size={18} />;
      case 'Merge With...':
        return <ToolGlyph tool="merge" size={16} />;
      case 'Unmerge Stations':
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
      case 'Delete Section':
        return <EntityDeleteSymbol entity="section" size={18} />;
      case 'Delete Station':
        return <EntityDeleteSymbol entity="station" size={18} />;
      case 'Delete Shot':
        return <EntityDeleteSymbol entity="shot" size={18} />;
      case 'Adjust Position...':
        return <EntityMoveSymbol entity="section" size={18} />;
      case 'Zoom to Section':
        return <ToolGlyph tool="zoom" size={16} />;
      default:
        // "Reverse N Sections" carries the count in its label, so it can't be a
        // literal case.
        if (label.startsWith('Reverse ')) {
          return <EntityReverseSymbol entity="section" size={18} />;
        }
        // "Combine Sections..." and "Combine 2 Sections...".
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
    // read-only navigation like "Zoom to Section"). Edits are made on the measured view.
    const geometryReadOnly = geometryMode === 'closureAdjusted' || speleoReadOnly;
    const READ_ONLY_SAFE_LABELS = new Set(['Zoom to Section']);
    items = buildMenuItems().map((item) => {
      const withIcon = item.separator ? item : { ...item, icon: iconForLabel(item.label) };
      return geometryReadOnly && !item.separator && !READ_ONLY_SAFE_LABELS.has(item.label)
        ? { ...withIcon, disabled: true }
        : withIcon;
    });
  }

  if (!target || !position || items.length === 0) {
    return null;
  }

  // Adjust position to stay within viewport
  const adjustedPosition = {
    x: Math.min(position.x, window.innerWidth - 200),
    y: Math.min(position.y, window.innerHeight - items.length * 36 - 20),
  };

  return (
    <div className="fixed inset-0 z-50">
      <div className="absolute inset-0" onClick={closeDialog} data-testid="context-menu-backdrop" />
      <div
        ref={menuRef}
        className="absolute bg-panel border border-border rounded-lg shadow-xl py-1 min-w-[180px]"
        data-testid="context-menu"
        style={{ left: adjustedPosition.x, top: adjustedPosition.y }}
      >
        {items.map((item, index) => {
          if (item.separator) {
            return <MenuSeparator key={index} />;
          }

          return (
            <MenuRow
              key={index}
              data-testid={`context-menu-item-${index}`}
              onClick={() => { setSource('context-menu'); item.action(); }}
              disabled={item.disabled}
              title={item.hint}
              label={item.label}
              icon={item.icon}
              showIcons={showIcons}
              className={`${item.disabled ? 'text-text-secondary cursor-not-allowed' : 'hover:bg-accent-10'} ${
                item.danger && !item.disabled ? 'text-error hover:bg-error-10' : ''
              }`}
            />
          );
        })}
      </div>
    </div>
  );
}
