import { Fragment, useCallback, useEffect, useId, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { useProjectStore } from '../../store/projectStore';
import { defaultConfig } from '../../types/config';
import { flushGasConfigWrite, queueGasConfigWrite } from '../../utils/gasConfigDisk';
import { formatStationId } from '../../utils/formatters';
import { formatFixed } from '../../i18n/format';
import { gasAssumptionsSummary } from '../../utils/gasAssumptionsSummary';
import { formatGasLength, formatPressure, formatVolume } from '../../utils/formatGas';
import { ChevronRightIcon } from '../common/icons';
import { GasAssumptionsSections, REQUIRED_STAGES } from './GasAssumptionsSections';
import { DivePlanBar } from './DivePlanBar';
import type { ReservePreset } from '../../types/ipc.generated';

/** Mirrors the Rust `MAX_MARKED_LABEL_CHARS` (core/state/dive_plan.rs). Capping the input here
 *  means a label typed in the panel can never be the reason a whole plan fails to save; the
 *  backend guard remains, for a hand-edited `.plnx`. */
const MAX_MARKED_LABEL_CHARS = 120;

/** One-way runtime readout (#317), always in minutes - the engine's own value, never re-derived. */
function formatRouteRuntime(min: number): string {
  return `${formatFixed(min, undefined, 1)} min`;
}

/**
 * Gas & route-planning setup sidebar (#38). The right Dock slot while in Plan mode:
 * origin, diver RMV, gas strategy + per-stage reserve, units, and the plan readout. The cylinder
 * table and DPV rig editor live in the bottom panel's "Cylinders" and "DPV" tabs (see
 * PlanTablesPanel) - this sidebar reads the cylinders only to compute the turn / drop pressures. Every input nulls the cached
 * field via gasPlanStore, so MapCanvas refetches + repaints the reachable overlay. Volumes follow
 * the master units toggle; RMV flips L·min⁻¹ ↔ cuft·min⁻¹.
 */
export function GasPlanPanel() {
  const { t } = useTranslation();
  const {
    originChosen,
    originPickArmed,
    origin,
    endpoint,
    waypoints,
    markedPoints,
    diver,
    cylinders,
    gasConfig,
    reachField,
    stageDropOverrides,
    unresolved,
    pushyAck,
    chooseEntrance,
    setOriginPickArmed,
    setOrigin,
    setEndpoint,
    removeWaypoint,
    moveWaypoint,
    removeMarkedPoint,
    setMarkedPointLabel,
    setDiver,
    setGasConfig,
    setPushyAck,
    setStageDropOverride,
  } = useGasPlanStore(
    useShallow((s) => ({
      originChosen: s.originChosen,
      originPickArmed: s.originPickArmed,
      origin: s.origin,
      endpoint: s.endpoint,
      waypoints: s.waypoints,
      markedPoints: s.markedPoints,
      diver: s.diver,
      cylinders: s.cylinders,
      gasConfig: s.gasConfig,
      reachField: s.reachField,
      stageDropOverrides: s.stageDropOverrides,
      unresolved: s.unresolved,
      pushyAck: s.pushyAck,
      chooseEntrance: s.chooseEntrance,
      setOriginPickArmed: s.setOriginPickArmed,
      setOrigin: s.setOrigin,
      setEndpoint: s.setEndpoint,
      removeWaypoint: s.removeWaypoint,
      moveWaypoint: s.moveWaypoint,
      removeMarkedPoint: s.removeMarkedPoint,
      setMarkedPointLabel: s.setMarkedPointLabel,
      setDiver: s.setDiver,
      setGasConfig: s.setGasConfig,
      setPushyAck: s.setPushyAck,
      setStageDropOverride: s.setStageDropOverride,
    }))
  );
  // A staged strategy the user selected but hasn't yet acknowledged (inline advanced gate).
  const [pendingPushy, setPendingPushy] = useState<ReservePreset | null>(null);

  // Seed the working gas config from the live PERSISTED app config the FIRST time Plan
  // mode opens this session, falling back to the compiled defaults when no config is
  // loaded. Gated on the store's `hydrated` flag: this panel remounts on every mode
  // switch, and re-seeding would clobber a loaded plan / working inputs now that the gas
  // store persists across mode switches (#244). Cleared on project load, so a new session
  // re-seeds. Display units follow the app-wide MASTER units (Settings ▸ Units): feet ⇒
  // imperial, so an imperial app opens an imperial gas panel (the in-panel toggle still
  // overrides for the session).
  useEffect(() => {
    if (useGasPlanStore.getState().hydrated) return;
    const cfg = useProjectStore.getState().config;
    const masterImperial = cfg?.units?.length === 'feet';
    // Deep-clone the seed so editing the working config never mutates the live persisted
    // config (or the compiled default) through a shared nested reference (reserve_profile / dpv).
    setGasConfig({
      ...structuredClone(cfg?.gas ?? defaultConfig.gas),
      units: masterImperial ? 'imperial' : 'metric',
    });
    // Pass the config the seed came from so `syncSettingsOnlyGasFields` (below) has its
    // "already observed" baseline from the moment of hydration, regardless of which of
    // this component's two effects React happens to run first.
    useGasPlanStore.getState().markHydrated(cfg?.gas ?? defaultConfig.gas);
  }, [setGasConfig]);

  // Mirror any Settings-only gas field EDITED IN SETTINGS during this session into the working
  // config (#317): the jump RMV multiplier and the include-reference-legs toggle have no
  // in-panel control at all, so Settings is their only source - which made the seed-once
  // `hydrated` gate above (deliberately, #244: re-seeding on every mode-switch remount would
  // clobber working inputs and a loaded plan) swallow them entirely: edit one after Plan mode has
  // already opened once this session and it would never appear, with nothing on screen to explain
  // why.
  //
  // The diffing this needs to stay safe - apply an actual edit, but leave a loaded plan's own
  // values alone when Settings did not change - must survive a PANEL REMOUNT (this component
  // remounts on every mode switch), so it lives in the store (`syncSettingsOnlyGasFields`), not a
  // component ref: a ref resets on remount and loses exactly the "last observed" memory this needs.
  const persistedGasConfig = useProjectStore((s) => s.config?.gas ?? defaultConfig.gas);
  useEffect(() => {
    useGasPlanStore.getState().syncSettingsOnlyGasFields(persistedGasConfig);
  }, [persistedGasConfig]);

  // #480: with no RESOLVABLE zero datum, `Project::zero_datum_roots` (backend,
  // pathfinding.rs) returns an empty root set, and the entrance origin (`origin === null`)
  // walks from nothing - every station comes back unreachable with no explanation on screen.
  // Checked against `stations` (the same positional list `zero_datum_roots`' own
  // `station_index` is built from), not `project.zero_datums.length`: a datum whose station no
  // longer resolves can still leave a stale entry in `zero_datums` while producing exactly the
  // same degenerate field (the codebase already learned this the hard way for TML export
  // anchoring - see `ExportControls.tsx`'s `tmlAllSectionsUnanchored`, #411). Surfaced here
  // rather than inferred from the computed field so the warning is deterministic and shows up
  // BEFORE the first fetch, not just after a round trip confirms it.
  const entranceHasNoZeroDatum = useProjectStore((s) => {
    const zeroDatums = s.project?.zero_datums ?? [];
    return !zeroDatums.some((zd) =>
      s.stations.some(
        (st) =>
          st.station_ref.section_id === zd.station_ref.section_id &&
          st.station_ref.station_index === zd.station_ref.station_index
      )
    );
  });

  // Swim-speed disk persist (#241, shared queue since the #317 review round): the in-memory
  // working config updates immediately on every keystroke; the disk write goes through
  // `queueGasConfigWrite` (trailing-debounced, merged, serialized against the rig editor's
  // writes - see utils/gasConfigDisk.ts). Flush on unmount so a value typed within the
  // debounce window right before leaving Plan mode still reaches disk.
  useEffect(
    () => () => {
      void flushGasConfigWrite();
    },
    []
  );

  // Disarm an origin pick on the way out of Plan mode (#395 review): this panel unmounts
  // whenever `appMode` leaves `plan`, but the armed flag itself deliberately survives mode
  // switches in the store, and Escape is the only other disarm path. Left armed and
  // abandoned, it would consume the very next plain map click - in Survey mode, or in Plan
  // mode again after a fresh remount - as an origin overwrite instead of a normal click.
  useEffect(
    () => () => {
      useGasPlanStore.getState().setOriginPickArmed(false);
    },
    []
  );
  const persistSwimSpeed = useCallback((v: number) => {
    queueGasConfigWrite({ swim_speed_m_per_min: v });
  }, []);

  const units = gasConfig.units;
  const originLabel = origin
    ? formatStationId(origin.section_id, origin.station_index)
    : t('gasPanel.origin.entrance');

  // Way-point readouts, all from the engine (#298) - the panel never re-derives a
  // budget-dependent number, or it drifts from the overlay the way #233's gauges did.
  // `waypoint_gas_litres[i]` is the cumulative gas at way point i, or null from the first one the
  // walk could not reach.
  //
  // ⚠ These are only meaningful once a field has ARRIVED. Every gas-input setter nulls
  // `reachField` for the duration of the refetch, and an absent entry reads `undefined == null`,
  // so keying "unreachable" off the value alone would flash every row red on each RMV keystroke,
  // reorder or endpoint pick. Gate on the field's presence first (same shape as the route readout
  // further down), and treat a not-yet-computed way point as "pending", not as bad news.
  const waypointGas = reachField?.waypoint_gas_litres ?? [];
  const gasKnown = reachField != null;
  const stalledIdx = gasKnown ? waypointGas.findIndex((g) => g == null) : -1;
  // Only report a stall for a way point that actually exists in the CURRENT list: the field can
  // lag a keystroke behind the store, and warning about a way point the user just removed reads
  // as a bug.
  const stalledWaypoint = stalledIdx >= 0 && stalledIdx < waypoints.length ? stalledIdx : null;
  // A way point whose anchor no longer resolves to a station (a survey edit removed both of its
  // neighbouring shots). Distinct from a stall: the engine is never asked while one exists, so it
  // outranks the stall warning below.
  const unresolvedIdx = unresolved.waypoints.findIndex(Boolean);
  const unresolvedWaypoint =
    unresolvedIdx >= 0 && unresolvedIdx < waypoints.length ? unresolvedIdx : null;
  // Over budget = the detour alone costs more gas than the diver has. Distinct from a stall
  // (which is "cannot be reached at all") and gets its own, actionable message.
  const waypointsOverBudget =
    reachField != null && waypointGas.some((g) => g != null && g > reachField.budget_litres);

  // The engine turned short of the chosen endpoint (it is out of budget, so the plan runs as far
  // toward it as the gas allows). The station it actually turns at, or null when the endpoint IS
  // the turnaround. `final_turn_ref` is the engine's own answer - never re-derived here.
  const turn = reachField?.final_turn_ref ?? null;
  // Whether the engine's turn station is genuinely on the way to the pick, or the deepest-anywhere
  // fallback (endpoint deleted under a loaded plan, or in another component). The two look the same
  // from here - both are "a turn station that is not the endpoint" - so the engine reports which.
  // Saying "on the way there" about an unrelated passage is the exact failure the walk-toward-the-
  // endpoint rule exists to prevent.
  const turnTowardEndpoint = reachField?.turn_toward_endpoint === true;
  const endpointOutOfReach =
    endpoint != null &&
    turn != null &&
    (turn.section_id !== endpoint.section_id || turn.station_index !== endpoint.station_index)
      ? turn
      : null;

  // Count only ENABLED stages: the engine filters disabled tanks before counting stages
  // (gas.rs `active`), so a disabled stage must not keep a staged strategy selectable here or the
  // panel would show a staged plan + its reserve control while the engine has fallen back to
  // All-thirds (#245 UI/backend parity - this drives the button-enable, isStaged, and re-check).
  const stageCount = cylinders.filter((c) => c.role === 'stage' && c.enabled).length;
  const strategy = gasConfig.default_reserve;
  // Effective staged state: a staged strategy AND the rig actually carries enough stages. Mirrors
  // the backend's stage-count fallback, so removing stages after selecting a staged strategy hides
  // the staged-only reserve control instead of leaving it stale (UI/backend parity).
  const isStaged =
    (strategy === 'one_stage' || strategy === 'two_stage') && stageCount >= REQUIRED_STAGES[strategy];
  const reserveBar = gasConfig.reserve_profile.stage_reserve_bar;

  // Three non-additive gauges, each relative to its own cylinder's start (back gas = one unit).
  // They come from the BACKEND (single source of truth): it already applies the conservative cap,
  // the normalized reserve, and the stage-count fallback, so the panel can't drift from the
  // reachable overlay. `stage_drop_bars` has one entry per counted stage of the effective strategy.
  const backgasTurn = reachField ? reachField.backgas_turn_bar : null;
  // Per-stage drops (pressure + swum distance from origin), aligned 1:1; one readout row each - any
  // number of stages (Free is uncapped). When drops exist the diver switches to back gas after
  // dropping the stages, so the back-gas gauge reads "switch pressure", not "turn pressure".
  //
  // ⚠ Gate on a real turnaround, the same way `hasRoute` below does. `stage_drop_bars` comes out of
  // the strategy and the cylinders, so it is populated whenever the rig has stages - even when the
  // engine found no target at all and emptied `stage_drop_refs`/`stage_drop_distances_m`. Since
  // #298 that state is reachable in ordinary use (a way point that is disconnected, deleted
  // mid-edit, or out of budget stalls the whole plan). Rendering "Stage 1 drop: 119 bar" for a dive
  // that cannot be flown is worse than the turnaround case these rows sit next to: it names a
  // pressure at which to leave a tank behind, and the map is drawing no ▼ to go with it.
  const stageDropsKnown = reachField != null && reachField.final_turn_ref != null;
  const stageDrops = stageDropsKnown && reachField
    ? reachField.stage_drop_bars.map((bar, i) => ({
        bar,
        distance: reachField.stage_drop_distances_m[i] ?? null,
        // Dragged deeper than the safe drop (#246): breathing into reserve, flagged red. The
        // engine is the single source of truth for this, same as the pressure.
        overReserve: reachField.stage_drop_over_reserve[i] === true,
        // Whether the diver moved this drop (so we offer a reset back to the strategy spot).
        overridden: stageDropOverrides[i] != null,
      }))
    : [];
  const hasStageDrops = stageDrops.length > 0;
  const anyOverReserve = stageDrops.some((d) => d.overReserve);

  // Route metrics (backend single source of truth), shown once a real route is known. Gate on the
  // route having at least one LEG - not just final_turn_ref - so a degenerate single-station route
  // (origin == turnaround when only the origin is in budget) doesn't render a meaningless
  // "Distance to turnaround: 0 m". The distance is the one-way swum line length to the turnaround;
  // depths are the route's deepest point + the distance-weighted average depth.
  // ⚠ Also require a turnaround. Since #298 a route can be drawn with NO target: when the
  // way-point chain itself is unflyable the engine still returns the committed chain so the diver
  // can see where the plan broke, but there is no turnaround, so labelling that partial line
  // "Distance to turnaround" would report a confident number for a dive that cannot be made.
  // `route` is a list of LEGS (`route_legs` builds it with `windows(2)`), not of stations, so `> 0`
  // already means "at least one leg" - a degenerate single-station route has zero of them.
  const hasRoute = (reachField?.route.length ?? 0) > 0 && reachField?.final_turn_ref != null;
  const routeMaxDepth = reachField?.route_max_depth_m ?? null;
  const routeAvgDepth = reachField?.route_avg_depth_m ?? null;

  // Scooter (DPV) readout (#317) - every value here is read straight off the field, never
  // re-derived. Runtime is shown whenever a route exists, rig or not (the segment speed still
  // applies either way - see gas_reach_field_without_a_rig_ignores_the_scooter_flag); the range
  // comparison and the turn reason need a configured rig, since with none nothing is scootered.
  const routeRuntimeMin = reachField?.route_runtime_min ?? null;
  const hasDpvRig = gasConfig.dpv != null;
  const scooterDistance = reachField?.scooter_distance_m ?? null;
  const scooterRangeLimit = reachField?.scooter_range_limit_m ?? null;
  // ⚠ The engine's own answer - comparing scooterDistance against scooterRangeLimit looks
  // equivalent and is not: the route already stops at whichever cap bound first, so the two read
  // as merely equal in exactly the case being asked about (GasReachField::turn_on_scooter_range).
  const turnOnScooterRange = reachField?.turn_on_scooter_range === true;

  // Advanced strategies route through an inline acknowledgment the first time per session: the
  // staged (Half-Minus) presets AND Free, which lets the diver set reserves by hand (incl. a
  // zero-reserve plan), so it's at least as advanced as the staged ones. All-thirds and an
  // already-acked strategy apply immediately.
  const isAdvancedPreset = (preset: ReservePreset) => REQUIRED_STAGES[preset] > 0 || preset === 'free';
  const selectPreset = (preset: ReservePreset) => {
    if (isAdvancedPreset(preset) && !pushyAck) {
      setPendingPushy(preset);
      return;
    }
    setGasConfig({ ...gasConfig, default_reserve: preset });
  };
  const confirmPushy = () => {
    if (!pendingPushy) return;
    // Re-check: the rig may have lost stages while the ack panel was open (the Cylinders tab is
    // visible alongside it). Only apply if the rig still satisfies the strategy - otherwise we'd
    // show a staged strategy selected on a stage-short rig while the backend falls back to
    // All-thirds (broken UI/backend parity). Insufficient → dismiss without applying.
    if (stageCount >= REQUIRED_STAGES[pendingPushy]) {
      setPushyAck(true);
      setGasConfig({ ...gasConfig, default_reserve: pendingPushy });
    }
    setPendingPushy(null);
  };
  const setReserveBar = (n: number) =>
    setGasConfig({
      ...gasConfig,
      reserve_profile: { ...gasConfig.reserve_profile, stage_reserve_bar: n },
    });
  const safetyMarginBar = gasConfig.reserve_profile.all_thirds_safety_margin_bar;
  const setSafetyMargin = (n: number) =>
    setGasConfig({
      ...gasConfig,
      reserve_profile: { ...gasConfig.reserve_profile, all_thirds_safety_margin_bar: n },
    });

  // Props shared by both render branches below (#395): the entry card's "Adjust assumptions"
  // disclosure and the full rig's always-visible sidebar section render the exact same inputs.
  const assumptionsProps = {
    units,
    diver,
    gasConfig,
    setDiver,
    setGasConfig,
    persistSwimSpeed,
    stageCount,
    strategy,
    isStaged,
    reserveBar,
    safetyMarginBar,
    pendingPushy,
    selectPreset,
    confirmPushy,
    setPendingPushy,
    setReserveBar,
    setSafetyMargin,
  };

  // R9/R10 (#395): live, never hardcoded - the same values the disclosure reveals.
  const assumptionsSummary = gasAssumptionsSummary(diver, cylinders, gasConfig, t);
  const [assumptionsExpanded, setAssumptionsExpanded] = useState(false);
  const assumptionsRegionId = useId();

  if (!originChosen) {
    return (
      <div
        className="h-full bg-panel flex flex-col overflow-auto text-sm text-text-primary"
        data-testid="gas-panel"
      >
        <div className="px-3 py-2 border-b border-border shrink-0">
          <h2 className="font-semibold">{t('gasPanel.title')}</h2>
          <p className="text-xs opacity-70">{t('gasPanel.subtitle')}</p>
        </div>

        <div className="p-3 space-y-3" data-testid="gas-entry-card">
          <h3 className="text-sm font-semibold">{t('gasPanel.entry.heading')}</h3>
          <p className="text-xs opacity-70">{t('gasPanel.entry.helper')}</p>
          <button
            type="button"
            className="w-full text-sm px-3 py-2 rounded border border-accent bg-accent/10 hover:bg-accent/20"
            onClick={chooseEntrance}
            data-testid="gas-start-entrance"
          >
            {t('gasPanel.entry.startFromEntrance')}
          </button>
          {entranceHasNoZeroDatum && (
            <p className="text-xs text-warning" data-testid="gas-entrance-no-zero-datum-hint">
              {t('gasPanel.entry.noZeroDatumHint')}
            </p>
          )}

          <div className="pt-2 space-y-1">
            <p className="text-xs opacity-60" data-testid="gas-assumptions-summary">
              {assumptionsSummary}
            </p>
            <button
              type="button"
              onClick={() => setAssumptionsExpanded(!assumptionsExpanded)}
              className="flex items-center gap-1 text-xs text-text-secondary hover:text-text-primary"
              data-testid="gas-assumptions-toggle"
              aria-expanded={assumptionsExpanded}
              aria-controls={assumptionsRegionId}
            >
              <ChevronRightIcon
                size={12}
                className={assumptionsExpanded ? 'rotate-90 transition-transform' : 'transition-transform'}
              />
              <span>{t('gasPanel.entry.adjustAssumptions')}</span>
            </button>
            {assumptionsExpanded && (
              <div id={assumptionsRegionId} className="space-y-4 pt-1">
                <GasAssumptionsSections {...assumptionsProps} />
              </div>
            )}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div
      className="h-full bg-panel flex flex-col overflow-auto text-sm text-text-primary"
      data-testid="gas-panel"
    >
      <div className="px-3 py-2 border-b border-border shrink-0">
        <h2 className="font-semibold">{t('gasPanel.title')}</h2>
        <p className="text-xs opacity-70">{t('gasPanel.subtitle')}</p>
      </div>

      <div className="p-3 space-y-4">
        {/* Previous dive plans (#244): save / load / delete named plans */}
        <DivePlanBar />
        {/* Origin */}
        <section className="space-y-1">
          <SectionLabel>{t('gasPanel.origin.label')}</SectionLabel>
          <div className="flex items-center justify-between gap-2">
            <span data-testid="gas-origin-readout" className="font-mono">
              {originLabel}
            </span>
            <button
              type="button"
              className="text-xs px-2 py-0.5 rounded border border-border hover:bg-border disabled:opacity-40"
              onClick={() => setOrigin(null)}
              disabled={origin === null}
              data-testid="gas-reset-origin"
            >
              {t('gasPanel.origin.reset')}
            </button>
          </div>
          {origin === null && entranceHasNoZeroDatum && (
            // #480: the entrance origin (`origin === null`), whether chosen via the entry
            // CTA (`chooseEntrance` only sets `originChosen`, leaving `origin` at its null
            // default) or restored by a loaded plan (`applyLoadedPlan`) - same degenerate-field
            // cause as the entry card's hint above, and `origin` reads null either way, so this
            // can't be told apart from a healthy entrance plan without checking the project
            // directly.
            <p className="text-xs text-warning" data-testid="gas-origin-no-zero-datum-warning">
              {t('gasPanel.origin.noZeroDatumWarning')}
            </p>
          )}
          {/* #395: one-shot "Pick on map" - the next station click sets the origin instead of
              the endpoint. Escape or a second press disarms (see useKeyboardShortcuts). */}
          <button
            type="button"
            aria-pressed={originPickArmed}
            className={`text-xs px-2 py-0.5 rounded border w-full ${
              originPickArmed
                ? 'border-accent ring-1 ring-accent bg-accent/10'
                : 'border-border hover:bg-border'
            }`}
            onClick={() => setOriginPickArmed(!originPickArmed)}
            data-testid="gas-pick-origin"
          >
            {t(originPickArmed ? 'gasPanel.origin.pickArmed' : 'gasPanel.origin.pick')}
          </button>
          <p className="text-xs opacity-60">{t('gasPanel.origin.hint')}</p>
        </section>

        {/* Route: the endpoint plus the ordered way points (#298). The way-point rows are the only
            place the visit order can be CHANGED - the map pins can add and remove, but ordering is
            a list operation, and order is what the engine actually routes on. */}
        <section className="space-y-1" data-testid="gas-route-section">
          <SectionLabel>{t('gasPanel.route.label')}</SectionLabel>
          <div className="flex items-center justify-between gap-2">
            <span className="text-xs opacity-70">{t('gasPanel.route.endpointLabel')}</span>
            <div className="flex items-center gap-2">
              <span data-testid="gas-endpoint-readout" className="font-mono text-xs">
                {endpoint
                  ? formatStationId(endpoint.section_id, endpoint.station_index)
                  : t('gasPanel.route.endpointAuto')}
              </span>
              <button
                type="button"
                className="text-xs px-2 py-0.5 rounded border border-border hover:bg-border disabled:opacity-40"
                onClick={() => setEndpoint(null)}
                disabled={endpoint === null}
                data-testid="gas-clear-endpoint"
              >
                {t('gasPanel.route.clearEndpoint')}
              </button>
            </div>
          </div>
          {/* The pick is never silently rewritten - it stays yours, and clearing whatever put it out
              of budget restores the full reach. But the turnaround marker has moved, so say why
              rather than leaving the diver to wonder where their endpoint went. */}
          {endpointOutOfReach && (
            <p className="text-xs opacity-70" data-testid="gas-endpoint-out-of-reach">
              {t(
                turnTowardEndpoint
                  ? 'gasPanel.route.endpointOutOfReach'
                  : 'gasPanel.route.endpointUnusable',
                {
                  id: formatStationId(
                    endpointOutOfReach.section_id,
                    endpointOutOfReach.station_index
                  ),
                }
              )}
            </p>
          )}

          <div className="pt-1">
            <span className="text-xs opacity-70">{t('gasPanel.route.waypointsLabel')}</span>
            {waypoints.length === 0 ? (
              <p className="text-xs opacity-60" data-testid="gas-waypoints-empty">
                {t('gasPanel.route.waypointsEmpty')}
              </p>
            ) : (
              <ol className="space-y-0.5 mt-0.5" data-testid="gas-waypoint-list">
                {waypoints.map((w, i) => (
                  <li
                    key={`${w.section_id}:${w.station_index}:${i}`}
                    className="flex items-center gap-2 text-xs"
                    data-testid={`gas-waypoint-row-${i}`}
                  >
                    <span className="opacity-60 w-4 text-right">{i + 1}.</span>
                    <span className="font-mono flex-1">
                      {formatStationId(w.section_id, w.station_index)}
                    </span>
                    {unresolved.waypoints[i] && (
                      <span className="text-error" data-testid={`gas-waypoint-unresolved-${i}`}>
                        {t('gasPanel.route.unresolvedBadge')}
                      </span>
                    )}
                    {/* Cumulative gas at this way point, straight from the engine - the panel must
                        never re-derive a budget-dependent number (#233 lesson). `null` means the
                        chain stalled here or before it. */}
                    <span
                      className={gasKnown && waypointGas[i] == null ? 'text-error' : 'opacity-70'}
                      data-testid={`gas-waypoint-gas-${i}`}
                    >
                      {!gasKnown || waypointGas.length <= i
                        ? t('gasPanel.route.pending')
                        : waypointGas[i] == null
                          ? t('gasPanel.route.unreachable')
                          : formatVolume(waypointGas[i]!, units)}
                    </span>
                    <button
                      type="button"
                      className="px-1 rounded border border-border hover:bg-border disabled:opacity-30"
                      onClick={() => moveWaypoint(i, -1)}
                      disabled={i === 0}
                      aria-label={t('gasPanel.route.moveUp')}
                      title={t('gasPanel.route.moveUp')}
                      data-testid={`gas-waypoint-up-${i}`}
                    >
                      &uarr;
                    </button>
                    <button
                      type="button"
                      className="px-1 rounded border border-border hover:bg-border disabled:opacity-30"
                      onClick={() => moveWaypoint(i, 1)}
                      disabled={i === waypoints.length - 1}
                      aria-label={t('gasPanel.route.moveDown')}
                      title={t('gasPanel.route.moveDown')}
                      data-testid={`gas-waypoint-down-${i}`}
                    >
                      &darr;
                    </button>
                    <button
                      type="button"
                      className="px-1 rounded border border-border hover:bg-border"
                      onClick={() => removeWaypoint(i)}
                      aria-label={t('gasPanel.route.remove')}
                      title={t('gasPanel.route.remove')}
                      data-testid={`gas-waypoint-remove-${i}`}
                    >
                      &times;
                    </button>
                  </li>
                ))}
              </ol>
            )}
            {waypoints.length > 0 && (
              <p className="text-xs opacity-60 mt-0.5">{t('gasPanel.route.waypointsHint')}</p>
            )}
            {unresolvedWaypoint !== null && (
              <p className="text-xs text-error mt-1" data-testid="gas-waypoint-warning-unresolved">
                {t('gasPanel.route.warnUnresolved', { n: unresolvedWaypoint + 1 })}
              </p>
            )}
            {unresolvedWaypoint === null && stalledWaypoint !== null && (
              <p className="text-xs text-error mt-1" data-testid="gas-waypoint-warning-unreachable">
                {t('gasPanel.route.warnUnreachable', { n: stalledWaypoint + 1 })}
              </p>
            )}
            {stalledWaypoint === null && waypointsOverBudget && (
              <p className="text-xs text-error mt-1" data-testid="gas-waypoint-warning-over-budget">
                {t('gasPanel.route.warnOverBudget')}
              </p>
            )}
          </div>
        </section>

        {/* Marked points (#298): annotation only, never a gas input. */}
        <section className="space-y-1" data-testid="gas-marked-section">
          <SectionLabel>{t('gasPanel.marked.label')}</SectionLabel>
          {markedPoints.length === 0 ? (
            <p className="text-xs opacity-60" data-testid="gas-marked-empty">
              {t('gasPanel.marked.empty')}
            </p>
          ) : (
            <ul className="space-y-0.5" data-testid="gas-marked-list">
              {markedPoints.map((m, i) => (
                <li
                  key={`${m.station.section_id}:${m.station.station_index}:${i}`}
                  className="flex items-center gap-2 text-xs"
                  data-testid={`gas-marked-row-${i}`}
                >
                  <span className="font-mono">
                    {formatStationId(m.station.section_id, m.station.station_index)}
                  </span>
                  <input
                    type="text"
                    value={m.label ?? ''}
                    onChange={(e) => setMarkedPointLabel(i, e.target.value)}
                    placeholder={t('gasPanel.marked.labelPlaceholder')}
                    // Mirrors the backend's MAX_MARKED_LABEL_CHARS, so a label typed here can
                    // never be long enough for the save to reject the whole plan.
                    maxLength={MAX_MARKED_LABEL_CHARS}
                    className="flex-1 min-w-0 bg-transparent border border-border rounded px-1 py-0.5"
                    data-testid={`gas-marked-label-input-${i}`}
                  />
                  <button
                    type="button"
                    className="px-1 rounded border border-border hover:bg-border"
                    onClick={() => removeMarkedPoint(i)}
                    aria-label={t('gasPanel.marked.remove')}
                    title={t('gasPanel.marked.remove')}
                    data-testid={`gas-marked-remove-${i}`}
                  >
                    &times;
                  </button>
                </li>
              ))}
            </ul>
          )}
          {markedPoints.length > 0 && (
            <p className="text-xs opacity-60">{t('gasPanel.marked.hint')}</p>
          )}
        </section>

        <GasAssumptionsSections {...assumptionsProps} />

        {/* Readout */}
        <section className="space-y-1 border-t border-border pt-3" data-testid="gas-readout">
          <SectionLabel>{t('gasPanel.readout.label')}</SectionLabel>
          {reachField ? (
            <dl className="grid grid-cols-2 gap-x-2 gap-y-1 text-xs">
              <dt className="opacity-70">
                {t(
                  hasStageDrops
                    ? 'gasPanel.readout.backgasSwitchPressure'
                    : 'gasPanel.readout.backgasTurnPressure'
                )}
              </dt>
              <dd data-testid="gas-readout-backgas-turn" className="text-right font-mono">
                {backgasTurn != null ? formatPressure(backgasTurn, units) : '--'}
              </dd>
              {stageDrops.map((drop, i) => (
                <Fragment key={i}>
                  <dt className="opacity-70">
                    {t('gasPanel.readout.stageDropPressure', { n: i + 1 })}
                    {drop.overridden && (
                      <button
                        type="button"
                        data-testid={`gas-readout-stage${i + 1}-reset`}
                        className="ml-1 text-[10px] underline opacity-70 hover:opacity-100"
                        onClick={() => setStageDropOverride(i, null)}
                        title={t('gasPanel.readout.resetDropHint')}
                      >
                        {t('gasPanel.readout.resetDrop')}
                      </button>
                    )}
                  </dt>
                  <dd
                    data-testid={`gas-readout-stage${i + 1}-drop`}
                    data-over-reserve={String(drop.overReserve)}
                    className={`text-right font-mono ${drop.overReserve ? 'text-error' : ''}`}
                  >
                    {formatPressure(drop.bar, units)}
                    {drop.distance != null && (
                      <span className="opacity-60">
                        {' @ '}
                        {formatGasLength(drop.distance, units)}
                      </span>
                    )}
                  </dd>
                </Fragment>
              ))}
              {anyOverReserve && (
                <div
                  data-testid="gas-readout-over-reserve-warning"
                  className="col-span-2 text-error"
                >
                  {t('gasPanel.readout.overReserveWarning')}
                </div>
              )}
              <dt className="opacity-70">{t('gasPanel.readout.budget')}</dt>
              <dd data-testid="gas-readout-budget" className="text-right font-mono">
                {formatVolume(reachField.budget_litres, units)}
              </dd>
              <dt className="opacity-70" title={t('gasPanel.readout.maxPenetrationHint')}>
                {t('gasPanel.readout.maxPenetration')}
              </dt>
              <dd
                data-testid="gas-readout-max"
                className="text-right font-mono"
                title={t('gasPanel.readout.maxPenetrationHint')}
              >
                {formatVolume(reachField.max_reachable_gas, units)}
              </dd>
              {hasRoute && (
                <>
                  <dt className="opacity-70" title={t('gasPanel.readout.routeDistanceHint')}>
                    {t('gasPanel.readout.routeDistance')}
                  </dt>
                  <dd
                    data-testid="gas-readout-route-distance"
                    className="text-right font-mono"
                    title={t('gasPanel.readout.routeDistanceHint')}
                  >
                    {formatGasLength(reachField.route_line_length_m, units)}
                  </dd>
                  {routeMaxDepth != null && (
                    <>
                      <dt className="opacity-70">{t('gasPanel.readout.maxDepth')}</dt>
                      <dd data-testid="gas-readout-max-depth" className="text-right font-mono">
                        {formatGasLength(routeMaxDepth, units, 1)}
                      </dd>
                    </>
                  )}
                  {routeAvgDepth != null && (
                    <>
                      <dt className="opacity-70">{t('gasPanel.readout.avgDepth')}</dt>
                      <dd data-testid="gas-readout-avg-depth" className="text-right font-mono">
                        {formatGasLength(routeAvgDepth, units, 1)}
                      </dd>
                    </>
                  )}
                  {routeRuntimeMin != null && (
                    <>
                      <dt className="opacity-70" title={t('gasPanel.readout.routeRuntimeHint')}>
                        {t('gasPanel.readout.routeRuntime')}
                      </dt>
                      <dd
                        data-testid="gas-readout-route-runtime"
                        className="text-right font-mono"
                        title={t('gasPanel.readout.routeRuntimeHint')}
                      >
                        {formatRouteRuntime(routeRuntimeMin)}
                      </dd>
                    </>
                  )}
                  {hasDpvRig && scooterRangeLimit != null && (
                    <>
                      <dt className="opacity-70" title={t('gasPanel.readout.scooterRangeHint')}>
                        {t('gasPanel.readout.scooterRange')}
                      </dt>
                      <dd data-testid="gas-readout-scooter-range" className="text-right font-mono">
                        {formatGasLength(scooterDistance ?? 0, units)}
                        {' / '}
                        {formatGasLength(scooterRangeLimit, units)}
                      </dd>
                    </>
                  )}
                  {hasDpvRig && turnOnScooterRange && (
                    <div
                      data-testid="gas-readout-scooter-turn-warning"
                      className="col-span-2 text-warning"
                    >
                      {t('gasPanel.readout.turnOnScooterRange')}
                    </div>
                  )}
                </>
              )}
            </dl>
          ) : (
            <p className="text-xs opacity-60">{t('gasPanel.readout.computing')}</p>
          )}
        </section>
      </div>
    </div>
  );
}

function SectionLabel({ children }: { children: ReactNode }) {
  return <h3 className="text-xs font-semibold uppercase tracking-wide opacity-60">{children}</h3>;
}
