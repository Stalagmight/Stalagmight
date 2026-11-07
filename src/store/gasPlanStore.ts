import { create } from 'zustand';

import { defaultConfig } from '../types/config';
import { TANK_PRESETS, cylinderFromPreset } from '../utils/tankPresets';
import type {
  Cylinder,
  Diver,
  DivePlanSnapshot,
  DpvRig,
  GasConfig,
  GasReachField,
  MarkedPoint,
  PlanSegment,
} from '../types/ipc.generated';
import type { StationRef } from '../types/project';
import { deepEqual, divePlanSnapshotOf, divePlanSnapshotsEqual } from '../utils/divePlan';
import { dpvRigInvalid } from '../utils/dpv';
import {
  planAnchorsForRefs,
  planRefsForAnchors,
  type LoadedPlanAnchors,
  type PlanAnchor,
} from '../utils/divePlanApi';

/**
 * Mirror the Rust caps (`core/state/dive_plan.rs`), the same way `GasPlanPanel` mirrors
 * `MAX_MARKED_LABEL_CHARS`. The backend enforces these on save (reject), on load (truncate) and on
 * the compute command (reject); mirroring them at the point of creation is what keeps a diver from
 * building a plan the app will later refuse to store.
 */
const MAX_WAYPOINTS = 64;
const MAX_MARKED_POINTS = 500;
/**
 * How long a pin edit waits before its anchors are captured (#310).
 *
 * Long enough that dropping a run of way points costs one round trip rather than one per click,
 * short enough that a normal "place a pin, go edit the survey" gesture finds the anchors already
 * taken. It is never the last word: the mutation seam flushes a pending capture before it dispatches
 * (see `flushAnchorCapture`), so the debounce is a latency optimisation, not a correctness window.
 */
const ANCHOR_CAPTURE_DEBOUNCE_MS = 300;

/**
 * Gas & route-planning sub-mode state (#38, Phase 1). Runtime-only - never
 * persisted. The store's intrinsic default is the compiled `defaultConfig.gas`;
 * on sub-mode entry `GasPlanPanel` overwrites `gasConfig` with the live PERSISTED
 * `projectStore.config.gas` (the user's saved gas prefs), and `uiStore.exitGasPlan`
 * resets it. Mirrors the distance-gradient pattern: every input setter nulls the
 * cached `reachField` so a stale reachable overlay never shows under changed inputs
 * while the new field is in flight.
 *
 * Phase 1 is single-diver open-circuit; the `Vec<Cylinder>` shape is kept so the
 * later CCR/team phases add no migration (PRD §5).
 */

/** Canonical starting rig: a matched twin AL80 back-gas set - the "call-it-an-80"
 *  baseline every cave diver recognizes (≈2295 L usable each at 207 bar). */
const DEFAULT_DIVER: Diver = { rmv_l_per_min: 20, label: null };

function defaultCylinders(): Cylinder[] {
  // Single source of truth: the canonical AL80 preset (tankPresets), as a twin set.
  const al80 = TANK_PRESETS.find((p) => p.id === 'al80');
  return al80 ? [cylinderFromPreset(al80), cylinderFromPreset(al80)] : [];
}

/**
 * A fresh span's default pace/propulsion (#317): no speed of its own, no scooter.
 *
 * ⚠ `speed_m_per_min: null` is load-bearing. The panel sends one segment per span from the moment
 * Plan mode opens, so seeding a NUMBER here (even the right one) permanently overrides the diver's
 * configured swim / survey speed - the engine would never see it again, and the Swim-speed field
 * would look live while changing nothing. `null` means "fly this span at the plan's speed"; only a
 * span the diver actually edits gets a number.
 */
function defaultSegment(): PlanSegment {
  return { speed_m_per_min: null, on_scooter: false };
}

/** The speed a span with no speed of its own is flown at - what the engine resolves `null` to. */
export function planSpeed(gasConfig: GasConfig): number {
  return gasConfig.survey_speed_m_per_min ?? gasConfig.swim_speed_m_per_min;
}

/**
 * The gas settings with NO control anywhere in the Plan panel (#317).
 *
 * Every other gas field is editable in-panel, so mirroring a Settings edit into it would fight the
 * diver's own working value. These two have Settings as their only source, which the seed-once
 * `hydrated` gate would otherwise swallow entirely: nudge the jump multiplier after having opened
 * Plan mode once and the new value never appears, with nothing on screen to explain why. (The DPV
 * rig is NOT one of these - it has its own in-panel editor and its own setter, `setDpvRig`.)
 */
export type SettingsOnlyGasFields = Pick<GasConfig, 'jump_rmv_multiplier' | 'include_reference_legs'>;

function settingsOnlyFieldsOf(gas: GasConfig): SettingsOnlyGasFields {
  return {
    jump_rmv_multiplier: gas.jump_rmv_multiplier,
    include_reference_legs: gas.include_reference_legs,
  };
}

/**
 * Force `segments` to the one-per-span invariant: exactly `waypointCount + 1` entries.
 *
 * The TS mirror of `normalize_segment_count` (core/state/dive_plan.rs), down to the fold rule.
 *
 * The setters below all preserve the length, but two inputs arrive from outside them: a `.plnx`
 * written before #317 existed carries NO segments at all, and a hand-built list can be any length.
 * Left alone, a short list means the Segments tab renders fewer rows than the route has spans and
 * the engine prices the missing ones at the plan speed while the panel shows nothing for them.
 *
 * A short list is PADDED with inherited spans - the file described those spans and simply did not
 * describe the later ones, which is what an unedited span has always meant.
 *
 * A long list is FOLDED into the final retained span, never truncated. A bare truncate throws away
 * the diver's statement about how those spans are flown, and it throws every field away in the
 * REACH-EXTENDING direction: a discarded slow speed reverts to the plan speed (cheaper gas per
 * metre), a discarded `on_scooter` stops spending range. The fold keeps the slowest effective speed
 * and any scooter use, so an over-long list can only ever cost more than the file claimed.
 */
function normalizeSegments(
  segments: PlanSegment[],
  waypointCount: number,
  speed: number
): PlanSegment[] {
  const want = waypointCount + 1;
  if (segments.length === want) return segments;
  // `want >= 1` always - a plan has at least the span from the origin into the open field - so
  // there is always a span to fold INTO.
  const out = segments.slice(0, want);
  while (out.length < want) out.push(defaultSegment());
  for (const tail of segments.slice(want)) {
    out[want - 1] = mergeSegments(out[want - 1]!, tail, speed);
  }
  return out;
}

/**
 * The speed domain the ENGINE will actually fly, mirrored here so a merge reads the same number the
 * engine would. Rust twins: `MIN_SEGMENT_SPEED_M_PER_MIN` / `MAX_SEGMENT_SPEED_M_PER_MIN` in
 * `models/project/gas/scooter.rs`, where the reasoning lives - the engine divides by the speed
 * twice, so a denormal or a wild value overflows the edge cost to `Infinity`/`NaN` and the field
 * collapses. Anything outside the range inherits the plan speed instead, so a merge that compared
 * the raw number would rank a span the engine is going to ignore.
 */
const MIN_SEGMENT_SPEED_M_PER_MIN = 0.1;
const MAX_SEGMENT_SPEED_M_PER_MIN = 1000;

/**
 * Collapse two spans into one when the way point between them goes away.
 *
 * NOT a mirror of the backend's `merge_segments_for_surviving_waypoints` (core/state/dive_plan.rs),
 * despite the identical arithmetic: that one reconciles a loaded plan whose anchors no longer
 * resolve, this one implements the panel's remove-way-point button. They share only the tie rule -
 * the SLOWER effective speed wins, a tie keeps `before` (so a span inheriting the plan speed keeps
 * inheriting it), and `on_scooter` is ORed. Both directions are the conservative one, and they are
 * conservative for OPPOSITE reasons: the slower speed spends more GAS per metre, while the scooter
 * flag spends RANGE and nothing else - it buys no reduced breathing rate - so dropping it would
 * hand the merged span free scooter range and REACH FURTHER than either half could. The
 * cross-language tests below pin that arithmetic in both places; the two callers are free to
 * diverge in everything else.
 */
function mergeSegments(before: PlanSegment, after: PlanSegment, speed: number): PlanSegment {
  const effective = (s: PlanSegment) => {
    const v = s.speed_m_per_min;
    return v !== null &&
      Number.isFinite(v) &&
      v >= MIN_SEGMENT_SPEED_M_PER_MIN &&
      v <= MAX_SEGMENT_SPEED_M_PER_MIN
      ? v
      : speed;
  };
  return {
    speed_m_per_min:
      effective(after) < effective(before) ? after.speed_m_per_min : before.speed_m_per_min,
    on_scooter: before.on_scooter || after.on_scooter,
  };
}

/**
 * A loaded plan's span speeds, with every one the engine would not fly - outside
 * `[MIN_SEGMENT_SPEED_M_PER_MIN, MAX_SEGMENT_SPEED_M_PER_MIN]`, or not a finite number -
 * dropped back to inheriting the plan speed (#576 NF-UI-2). Only a hand-edited or foreign
 * `.plnx` carries one: the Segments grid never stores such a speed. Left in place it showed
 * in the grid as the span's explicit speed while the engine silently flew the plan speed,
 * so dropping it changes no planned number - it makes the row say what is flown.
 */
function dropOutOfRangeSpeeds(segments: PlanSegment[]): { segments: PlanSegment[]; dropped: number } {
  let dropped = 0;
  const out = segments.map((seg) => {
    const v = seg.speed_m_per_min;
    if (v === null || (Number.isFinite(v) && v >= MIN_SEGMENT_SPEED_M_PER_MIN && v <= MAX_SEGMENT_SPEED_M_PER_MIN)) {
      return seg;
    }
    dropped += 1;
    return { ...seg, speed_m_per_min: null };
  });
  return { segments: dropped > 0 ? out : segments, dropped };
}

/**
 * The five plan pins, reduced to bare station refs, in one fixed slot order (#310).
 *
 * Everything anchor-related walks this shape, so an anchor, an `unresolved` flag and the pin it
 * belongs to are aligned by construction rather than by three separate loops kept in step by hand.
 */
export interface PlanPinRefs {
  origin: StationRef | null;
  endpoint: StationRef | null;
  waypoints: StationRef[];
  markedPoints: StationRef[];
  stageDropOverrides: (StationRef | null)[];
}

/**
 * One value per pin slot, in `PlanPinRefs` shape.
 *
 * Named for the SLOTS, not for anchors: `PlanAnchorSet` is already the GENERATED type for the
 * backend's own anchor bundle (`src/types/ipc/PlanAnchorSet.ts`, snake_case fields), and two types
 * with one name and different field casing is how a wrong one gets imported.
 */
export interface PinSlots<T> {
  origin: T | null;
  endpoint: T | null;
  waypoints: (T | null)[];
  markedPoints: (T | null)[];
  stageDropOverrides: (T | null)[];
}

/** Pins the backend could no longer resolve, in `PlanPinRefs` shape. Always exactly as long as the
 *  pin arrays it indexes - every setter that adds, drops or reorders a pin moves its flag with it,
 *  and every round trip re-derives the whole set (see `deriveFlags`). */
export interface PlanPinFlags {
  origin: boolean;
  endpoint: boolean;
  waypoints: boolean[];
  markedPoints: boolean[];
  stageDropOverrides: boolean[];
}

const NO_FLAGS: PlanPinFlags = {
  origin: false,
  endpoint: false,
  waypoints: [],
  markedPoints: [],
  stageDropOverrides: [],
};

/** All-clear flags sized to `pins`, for a pin set that was just installed wholesale. */
function noFlagsFor(pins: PlanPinRefs): PlanPinFlags {
  return {
    origin: false,
    endpoint: false,
    waypoints: pins.waypoints.map(() => false),
    markedPoints: pins.markedPoints.map(() => false),
    stageDropOverrides: pins.stageDropOverrides.map(() => false),
  };
}

/**
 * The flags after a ROUTE-input change: `patch`'s slots go back to placeable, and the stage-drop
 * flags are dropped alongside the pinned drops that every route input clears.
 *
 * A flag only means anything next to the pin it indexes, so it has to travel with it. Left to
 * itself, `unresolved.waypoints` keeps its old length past a remove and every later flag lands on
 * the wrong row; `unresolved.stageDropOverrides` outlives the drops entirely and blocks Save for a
 * pin that no longer exists.
 */
function routeInputFlags(f: PlanPinFlags, patch: Partial<PlanPinFlags> = {}): PlanPinFlags {
  return { ...f, stageDropOverrides: [], ...patch };
}

/** Nothing came back for this slot - no anchor, or no ref. */
function unplaced(value: unknown): boolean {
  return value === null || value === undefined;
}

/**
 * Which pins the backend could not place, from a reply aligned to `pins`.
 *
 * One rule for BOTH round trips: a slot is unresolved when it holds a ref and the backend handed
 * back nothing for it - no anchor (capture) or no ref (re-resolve). Deriving the whole set rather
 * than patching it is what keeps the flags from drifting out of step with the pins they index.
 * Clearing is asymmetric on purpose: a REMAP that re-resolves the original anchor (e.g. after a
 * survey undo) clears a flag; a CAPTURE never does (see `preserveFlaggedSlots`).
 */
function deriveFlags<T>(pins: PlanPinRefs, placed: PinSlots<T>): PlanPinFlags {
  return {
    origin: pins.origin !== null && unplaced(placed.origin),
    endpoint: pins.endpoint !== null && unplaced(placed.endpoint),
    waypoints: pins.waypoints.map((_w, i) => unplaced(placed.waypoints[i])),
    markedPoints: pins.markedPoints.map((_m, i) => unplaced(placed.markedPoints[i])),
    stageDropOverrides: pins.stageDropOverrides.map(
      (d, i) => d !== null && unplaced(placed.stageDropOverrides[i])
    ),
  };
}

/**
 * Merge a fresh capture over the previous anchors, keeping the OLD anchor for every slot that is
 * currently flagged unresolved. A flagged slot's ref is known-stale, so the anchor the backend just
 * built from it names whichever station slid into that index - adopting it would be the #310
 * rebinding with extra steps. The old anchor still names the original station and lets a survey
 * undo bring the pin back. `prev === null` (nothing captured yet) keeps the slot un-anchored.
 */
function preserveFlaggedSlots(
  fresh: PinSlots<PlanAnchor>,
  prev: PinSlots<PlanAnchor> | null,
  flagged: PlanPinFlags
): PinSlots<PlanAnchor> {
  const keep = <T,>(f: boolean, freshV: T | null, prevV: T | null): T | null =>
    f ? prevV : freshV;
  return {
    origin: keep(flagged.origin, fresh.origin, prev?.origin ?? null),
    endpoint: keep(flagged.endpoint, fresh.endpoint, prev?.endpoint ?? null),
    waypoints: fresh.waypoints.map((v, i) =>
      keep(flagged.waypoints[i] === true, v, prev?.waypoints[i] ?? null)
    ),
    markedPoints: fresh.markedPoints.map((v, i) =>
      keep(flagged.markedPoints[i] === true, v, prev?.markedPoints[i] ?? null)
    ),
    stageDropOverrides: fresh.stageDropOverrides.map((v, i) =>
      keep(flagged.stageDropOverrides[i] === true, v, prev?.stageDropOverrides[i] ?? null)
    ),
  };
}

/** Slot-wise OR: a flag that was up stays up (a capture never clears - see `captureAnchors`). */
function orFlags(a: PlanPinFlags, b: PlanPinFlags): PlanPinFlags {
  return {
    origin: a.origin || b.origin,
    endpoint: a.endpoint || b.endpoint,
    waypoints: a.waypoints.map((v, i) => v || b.waypoints[i] === true),
    markedPoints: a.markedPoints.map((v, i) => v || b.markedPoints[i] === true),
    stageDropOverrides: a.stageDropOverrides.map((v, i) => v || b.stageDropOverrides[i] === true),
  };
}

/** Every filled slot, in slot order - the order the two IPC calls' arrays are aligned to. */
function collectPresent<T>(s: PinSlots<T>): T[] {
  const out: T[] = [];
  const push = (v: T | null) => {
    if (v !== null && v !== undefined) out.push(v);
  };
  push(s.origin);
  push(s.endpoint);
  s.waypoints.forEach(push);
  s.markedPoints.forEach(push);
  s.stageDropOverrides.forEach(push);
  return out;
}

/** The inverse of `collectPresent`: scatter a reply back over the slots that were filled. */
function redistribute<T, U>(shape: PinSlots<T>, values: (U | null)[]): PinSlots<U> {
  let next = 0;
  const take = (v: T | null): U | null =>
    v === null || v === undefined ? null : (values[next++] ?? null);
  return {
    origin: take(shape.origin),
    endpoint: take(shape.endpoint),
    waypoints: shape.waypoints.map(take),
    markedPoints: shape.markedPoints.map(take),
    stageDropOverrides: shape.stageDropOverrides.map(take),
  };
}

interface GasPlanState {
  /**
   * R9/R10 (#395): whether the diver has answered "Where does this dive start?" - by picking
   * the entrance (`chooseEntrance`), placing a real origin (`setOrigin` with a non-null ref), or
   * loading a saved plan (`applyLoadedPlan`). Gates the sidebar between the single-question entry
   * card and the full rig, and gates the reach-field fetch / bottom tables panel the same way.
   *
   * Survives mode switches (#244) and a `setOrigin(null)` reset (the panel's own "Reset" button,
   * or the project-switch default fallback in `MapCanvas`) - both leave `origin` at the entrance
   * default without re-asking the question. Only `reset()` (a fresh project load) returns it to
   * `false`.
   */
  originChosen: boolean;
  /**
   * #395: the diver clicked "Pick on map" in the Origin section and the next station click sets
   * the ORIGIN rather than the endpoint - a one-shot arm/disarm, mirroring `placementMode`
   * (uiStore). Consumed (and cleared) by `setOrigin`, so any origin pick - map click or Escape -
   * always leaves it disarmed.
   */
  originPickArmed: boolean;
  /** Penetration origin; `null` = the zero-datum entrance default. */
  origin: StationRef | null;
  /** Chosen route endpoint (the lead being planned); `null` = the deepest in-budget point. */
  endpoint: StationRef | null;
  /** #298: ordered intermediate points the dive must visit, in order, between origin and
   *  endpoint. The engine re-bases the whole reach field through them, so adding one costs
   *  real gas rather than only re-drawing the route line. */
  waypoints: StationRef[];
  /** #317: per-span pace/propulsion, one entry per span between way points - always exactly
   *  `waypoints.length + 1` (span 0 runs from the origin, the last covers the open field past
   *  the final way point). A GAS INPUT like way points themselves: the engine derates in-leg gas
   *  consumption while `on_scooter`, so every setter nulls `reachField` the same way. */
  segments: PlanSegment[];
  /** #298: arbitrary map annotations saved with the plan (a photo stop, a lead, a restriction).
   *  Never sent to the gas engine - marking a station cannot change the route or the budget. */
  markedPoints: MarkedPoint[];
  diver: Diver;
  cylinders: Cylinder[];
  /** Working gas config; seeded from `Config.gas`, edited in-mode (units/speeds/reserve). */
  gasConfig: GasConfig;
  /** Latest reachability field from the backend (null until fetched / after any input change). */
  reachField: GasReachField | null;
  /**
   * The last NON-null field, kept across the refetch window that `reachField` nulls.
   *
   * Every input setter blanks `reachField` so stale colours are not painted under new inputs, and
   * in Plan a plain left-click is an input (it sets the endpoint). So the ordinary
   * click-then-right-click gesture opens the context menu while `reachField` is null, and a menu
   * built from the live field would silently drop its stage-drop entries and treat every station
   * as off-route. This is what the menu reads instead: the plan the map is still drawing.
   *
   * Only ever consumed for MENU SHAPE, never for a number the diver acts on. A route point picked
   * off a route that has since moved is safe by construction - the engine keeps a stage drop only
   * when the pinned station is on (or extends) the route it recomputes, and otherwise falls back to
   * the strategy default.
   */
  lastReachField: GasReachField | null;
  /** Per-stage drop overrides (#246), aligned by drop index with the field's `stage_drop_refs`:
   *  the route station the diver dragged each ▼ marker to (`null` = strategy default). Cleared
   *  whenever the rig / config / origin / endpoint changes (which reshuffles the drops), exactly
   *  like `reachField`. Passed to `compute_gas_reach_field` so the backend recomputes the plan. */
  stageDropOverrides: (StationRef | null)[];
  /** #244: id of the currently-loaded saved plan, or null when the working inputs are an
   *  unsaved exploration. Set on load; cleared on reset and when the loaded plan is deleted. */
  loadedPlanId: number | null;
  /** #244: the snapshot as loaded / as of the last successful Save. The baseline the dirty
   *  check compares the live inputs against (see `selectIsPlanDirty`). */
  loadedPlanBaseline: DivePlanSnapshot | null;
  /** [#576 W71-NF6] What the last plan load dropped, for the bar's two notices: stations the plan
   *  referenced that no longer exist (the backend's `stale_ref_count`) and span speeds out of range
   *  (#576 NF-UI-2). Held here rather than in the bar, so a load answered after the bar closed (a
   *  mode switch mid-load) still shows them back in Plan. Cleared by `reset()` and by the bar. */
  loadDropped: { staleRefs: number; speeds: number };
  /** Session-only: the user accepted the advanced (staged) reserve presets. Cleared by
   *  `reset()` on every project load (`applyDisplaySettings`), so a fresh project re-prompts.
   *  Never persisted. */
  pushyAck: boolean;
  /** #244: the working gas config has been seeded from the persisted app config for this
   *  session. Gates the panel's one-time seed so re-ENTERING Plan mode does not re-seed and
   *  clobber a loaded plan / working inputs. Cleared by `reset()` (project load / new session). */
  hydrated: boolean;
  /**
   * #317: the persisted values of the gas settings that have NO in-panel control, as last seen.
   *
   * Lives in the store rather than in a `useRef` beside the effect that reads it: `GasPlanPanel`
   * unmounts on every mode switch, so a ref-held marker is thrown away and re-seeded each time -
   * which makes "leave Plan, change the rig in Settings, come back" look exactly like a first
   * observation and silently drop the edit.
   */
  lastSeenPersistedGas: SettingsOnlyGasFields | null;
  /**
   * #310: one backend anchor per live pin, and the pin refs they were taken against.
   *
   * A pin is a positional `StationRef`, so any survey edit that renumbers stations leaves it IN
   * RANGE naming a different physical station - the plan silently re-prices a different route, and
   * a Save bakes the wrong stations into the `.plnx`. The anchors are the fix: the backend names a
   * station by the shots either side of it, so it survives the renumbering and can hand back a
   * corrected ref. `anchoredPins` is what makes that safe - anchors are only ever applied to the
   * exact pin set they were captured against, so a reply that raced a pin edit is dropped instead
   * of being scattered over slots that have since moved.
   */
  anchors: PinSlots<PlanAnchor> | null;
  anchoredPins: PlanPinRefs | null;
  /** The same, for `loadedPlanBaseline`'s pins - so a renumbering edit re-resolves the dirty-check
   *  baseline too and an untouched plan does not read as unsaved afterwards. */
  baselineAnchors: PinSlots<PlanAnchor> | null;
  baselineAnchoredPins: PlanPinRefs | null;
  /** Pins whose anchor no longer resolves to a station (the shots on BOTH sides are gone). The pin
   *  and its anchor are kept - a survey undo can bring the station back - but a flagged pin is
   *  never sent to the gas engine and blocks a Save. */
  unresolved: PlanPinFlags;

  /** #395: answer "Where does this dive start?" with the entrance (leaves `origin: null`). */
  chooseEntrance: () => void;
  /** #395: arm/disarm the one-shot "Pick on map" origin picker. */
  setOriginPickArmed: (armed: boolean) => void;
  setOrigin: (origin: StationRef | null) => void;
  setEndpoint: (endpoint: StationRef | null) => void;
  /** #298: append a way point to the end of the visit order. */
  addWaypoint: (ref: StationRef) => void;
  /** #298: drop way point `index`; a stray index is a no-op. */
  removeWaypoint: (index: number) => void;
  /** #298: swap way point `index` with its neighbour (`-1` earlier, `+1` later). Out-of-range
   *  moves are a no-op, so the panel's arrows need no bounds logic of their own. */
  moveWaypoint: (index: number, delta: number) => void;
  /** #317: replace the whole segment list (the Segments-tab idiom - mirrors `setCylinders`: the
   *  caller computes the new array, the store just nulls the stale gas-dependent state). */
  setSegments: (segments: PlanSegment[]) => void;
  /** #298: drop a marked point on `ref`. */
  addMarkedPoint: (ref: StationRef) => void;
  /** #298: remove marked point `index`. */
  removeMarkedPoint: (index: number) => void;
  /** #298: set (or clear, with an empty string) marked point `index`'s label. */
  setMarkedPointLabel: (index: number, label: string) => void;
  /**
   * #310: take a backend anchor for every pin the plan currently holds, right now.
   *
   * Capture is debounced off each pin edit, so most of the time this finds the anchors already
   * taken and returns without an IPC call. The mutation seam awaits it BEFORE dispatching an edit,
   * which is the point: a pin placed 50ms ago must be anchored against the survey as it stands
   * before the edit, or there is nothing to correct it back to afterwards.
   */
  flushAnchorCapture: () => Promise<void>;
  /**
   * #310: ask the backend where the anchored pins live in TODAY's numbering and write the answer
   * back. Called by the mutation seam after an edit lands and before the project refetch, so the
   * reach field is recomputed against corrected pins rather than stale ones.
   *
   * A pin whose anchor no longer resolves is KEPT and flagged (see `unresolved`) rather than
   * dropped: an undo of the deletion brings the station back, and a later remap clears the flag.
   */
  remapPinsFromAnchors: () => Promise<void>;
  /**
   * #317: adopt a Settings-only gas field the user changed during this session.
   *
   * Applies ALWAYS, including with a plan loaded. That is deliberate and is the one place the
   * plan's snapshot is allowed to move underneath it: these fields have no in-panel control, so
   * "the setting is ignored while a plan is open" would be indistinguishable from a broken setting.
   * The loaded plan simply goes dirty through the ordinary `selectIsPlanDirty` path and the diver
   * saves or discards it, exactly as if they had edited a field by hand.
   */
  syncSettingsOnlyGasFields: (persistedGas: GasConfig) => void;
  setDiver: (diver: Diver) => void;
  setCylinders: (cylinders: Cylinder[]) => void;
  setGasConfig: (gasConfig: GasConfig) => void;
  /**
   * #317: install (or, with `null`, remove) the diver's DPV rig - a gas input, but edited from its
   * own in-panel editor rather than a `setGasConfig` patch, so it gets its own setter.
   *
   * Removing the rig also resets every segment with `on_scooter: true` back to `{on_scooter: false,
   * speed_m_per_min: null}`. Left alone, a segment the diver had marked scootered keeps flying at
   * whatever speed the rig auto-filled (e.g. 45 m/min) with no rig behind it any more - the engine
   * would go on pricing gas at a pace the diver can no longer sustain, so the field and the route
   * both stay too long, the unsafe direction. A segment with no `on_scooter` flag is untouched: an
   * explicit swim-pace speed the diver typed by hand has nothing to do with the rig.
   */
  setDpvRig: (rig: DpvRig | null) => void;
  setReachField: (reachField: GasReachField | null) => void;
  setPushyAck: (ack: boolean) => void;
  /** #244: mark the gas config seeded for this session (see `hydrated`). `persistedGas` is the
   *  config the seed came from; passing it makes the panel's mount order irrelevant by giving
   *  `syncSettingsOnlyGasFields` its baseline up front (#317). */
  markHydrated: (persistedGas?: GasConfig) => void;
  /** Pin (or, with `null`, reset) stage `index`'s drop to a route station. Does NOT null
   *  `reachField`, so the existing drop markers stay put during the ms-fast refetch instead of
   *  flickering; the fetch effect depends on `stageDropOverrides` and swaps the field in on arrival. */
  setStageDropOverride: (index: number, ref: StationRef | null) => void;
  /** #244: apply a loaded plan's snapshot to the inputs and mark it the clean baseline.
   *  Sets all six gas inputs at once (the reach-field effect recomputes) and nulls the
   *  cached field so a stale overlay never shows under the new plan. `anchors` are the ones the
   *  backend resolved the snapshot's refs FROM (#310), so a load needs no capture round trip.
   *  Returns how many span speeds the load dropped as out of range (#576 NF-UI-2), for the
   *  caller's one notice. */
  applyLoadedPlan: (
    planId: number,
    snapshot: DivePlanSnapshot,
    anchors?: LoadedPlanAnchors | null
  ) => { droppedSpeeds: number };
  /** #244: after a successful Save / Save as, rebind the baseline to the EXACT snapshot that
   *  was persisted (passed in, not re-derived - so an edit made during the async save can't
   *  read clean-but-stale) under the given plan id. */
  markPlanSaved: (planId: number, baseline: DivePlanSnapshot) => void;
  /** #244: forget the loaded plan (e.g. it was deleted) without touching the inputs. */
  clearLoadedPlan: () => void;
  /** [#576 W71-NF6] Set (with zeros, clear) the last load's notices; see `loadDropped`. */
  setLoadDropped: (staleRefs: number, speeds: number) => void;
  /** Restore defaults. Called from `applyDisplaySettings` on project load / new session
   *  (NOT on a mode switch - the plan persists across mode switches now, #244). */
  reset: () => void;
}

const initialState = (): Pick<
  GasPlanState,
  | 'originChosen'
  | 'originPickArmed'
  | 'origin'
  | 'endpoint'
  | 'waypoints'
  | 'segments'
  | 'markedPoints'
  | 'diver'
  | 'cylinders'
  | 'gasConfig'
  | 'reachField'
  | 'lastReachField'
  | 'stageDropOverrides'
  | 'loadedPlanId'
  | 'loadedPlanBaseline'
  | 'loadDropped'
  | 'pushyAck'
  | 'hydrated'
  | 'lastSeenPersistedGas'
  | 'anchors'
  | 'anchoredPins'
  | 'baselineAnchors'
  | 'baselineAnchoredPins'
  | 'unresolved'
> => ({
  originChosen: false,
  originPickArmed: false,
  origin: null,
  endpoint: null,
  waypoints: [],
  segments: [defaultSegment()],
  markedPoints: [],
  diver: DEFAULT_DIVER,
  cylinders: defaultCylinders(),
  // Deep-clone so editing the working config (incl. nested reserve_profile / dpv) never
  // mutates the shared compiled default through an aliased object reference.
  gasConfig: structuredClone(defaultConfig.gas),
  reachField: null,
  lastReachField: null,
  stageDropOverrides: [],
  loadedPlanId: null,
  loadedPlanBaseline: null,
  loadDropped: { staleRefs: 0, speeds: 0 },
  pushyAck: false,
  hydrated: false,
  lastSeenPersistedGas: null,
  anchors: null,
  anchoredPins: null,
  baselineAnchors: null,
  baselineAnchoredPins: null,
  unresolved: NO_FLAGS,
});

/** The live pins, in the shape everything anchor-related walks. */
function pinRefsOf(
  s: Pick<
    GasPlanState,
    'origin' | 'endpoint' | 'waypoints' | 'markedPoints' | 'stageDropOverrides'
  >
): PlanPinRefs {
  return {
    origin: s.origin,
    endpoint: s.endpoint,
    waypoints: s.waypoints,
    markedPoints: s.markedPoints.map((m) => m.station),
    stageDropOverrides: s.stageDropOverrides,
  };
}

/** The same, for a persisted snapshot (the dirty-check baseline). */
function pinRefsOfSnapshot(snapshot: DivePlanSnapshot | null): PlanPinRefs | null {
  if (snapshot === null) return null;
  return {
    origin: snapshot.origin,
    endpoint: snapshot.endpoint,
    waypoints: snapshot.waypoints,
    markedPoints: snapshot.marked_points.map((m) => m.station),
    stageDropOverrides: snapshot.stage_drop_overrides,
  };
}

/** True when the held anchors describe exactly the pins the store holds right now. */
function anchorsAreCurrent(s: GasPlanState): boolean {
  if (s.anchoredPins === null || !deepEqual(s.anchoredPins, pinRefsOf(s))) return false;
  return deepEqual(s.baselineAnchoredPins, pinRefsOfSnapshot(s.loadedPlanBaseline));
}

/** Pending debounced capture, and the capture currently on the wire (module state: there is
 *  exactly one store). */
let captureTimer: ReturnType<typeof setTimeout> | null = null;
let captureInFlight: Promise<void> | null = null;

/**
 * Take an anchor for every live pin AND every baseline pin, in one call.
 *
 * Both halves matter: the live pins are what the map and the engine read, and the baseline is
 * what `selectIsPlanDirty` compares them against - re-resolving one without the other would
 * report a plan the diver never touched as unsaved after any renumbering edit.
 */
async function captureAnchors(): Promise<void> {
  const before = useGasPlanStore.getState();
  if (anchorsAreCurrent(before)) return;
  const livePins = pinRefsOf(before);
  const basePins = pinRefsOfSnapshot(before.loadedPlanBaseline);
  const liveRefs = collectPresent(livePins);
  const baseRefs = basePins === null ? [] : collectPresent(basePins);
  if (liveRefs.length + baseRefs.length === 0) {
    useGasPlanStore.setState({
      anchors: null,
      anchoredPins: livePins,
      baselineAnchors: null,
      baselineAnchoredPins: basePins,
    });
    return;
  }
  let taken: (PlanAnchor | null)[];
  try {
    taken = await planAnchorsForRefs([...liveRefs, ...baseRefs]);
  } catch (e) {
    // No anchors is a degradation, not a failure: the pins keep working positionally (which is
    // what they did before #310) until the next edit tries again. Throwing here would turn a
    // backend hiccup into a failed survey edit.
    console.debug('[gas-plan] anchor capture failed', e);
    return;
  }
  if (!Array.isArray(taken)) return;
  // Drop a reply that raced a pin edit. The anchors are positional within THIS request, so
  // scattering them over a pin set that has since changed would hand slot N's anchor to a
  // different station - the exact silent mis-binding the anchors exist to prevent.
  //
  // ⚠ The guard compares pin VALUES, not project identity, so it leans on `reset()` running
  // synchronously on every project load (`uiStore.applyDisplaySettings`): that clears the pins
  // before any await can resume, so a reply from the outgoing project finds a pin set that no
  // longer matches and is dropped. A load path that swapped the survey WITHOUT resetting the store
  // would defeat it - two projects can hold identical refs.
  const after = useGasPlanStore.getState();
  if (!deepEqual(pinRefsOf(after), livePins)) return;
  if (!deepEqual(pinRefsOfSnapshot(after.loadedPlanBaseline), basePins)) return;
  const freshAnchors = redistribute(livePins, taken.slice(0, liveRefs.length));
  // The flags fall straight out of the reply: a ref the backend cannot anchor names no real
  // station. BUT a slot that is ALREADY flagged is exempt: its kept ref is known-stale, and the
  // station now sitting at that index is whatever slid into it - anchoring it here would bind the
  // wrong station and clear the flag against it, the exact rebinding #310 exists to prevent. A
  // flagged slot keeps its previous anchor (a survey undo can still resolve the ORIGINAL station
  // through it) and its flag; only a remap that re-resolves that original anchor, or the user
  // explicitly re-placing the pin (whose setter clears the flag first), may unlock it.
  const liveAnchors = preserveFlaggedSlots(freshAnchors, after.anchors, after.unresolved);
  const flags = orFlags(deriveFlags(livePins, liveAnchors), after.unresolved);
  const flagsChanged = !deepEqual(flags, after.unresolved);
  useGasPlanStore.setState({
    anchors: liveAnchors,
    anchoredPins: livePins,
    baselineAnchors:
      basePins === null ? null : redistribute(basePins, taken.slice(liveRefs.length)),
    baselineAnchoredPins: basePins,
    unresolved: flags,
    // A flag change moves what the engine is allowed to see (`selectComputeGate`), so the painted
    // field describes inputs that are no longer the ones in force - blanked exactly like a pin move.
    ...(flagsChanged ? { reachField: null, lastReachField: null } : {}),
  });
}

function runAnchorCapture(): Promise<void> {
  const started = captureAnchors().finally(() => {
    if (captureInFlight === started) captureInFlight = null;
  });
  captureInFlight = started;
  return started;
}

/** Re-arm the debounce. Called from the store subscription below, so it covers every pin
 *  mutation - including a direct `setState` - rather than each setter remembering to. */
function scheduleAnchorCapture(): void {
  if (captureTimer !== null) clearTimeout(captureTimer);
  captureTimer = setTimeout(() => {
    captureTimer = null;
    void runAnchorCapture();
  }, ANCHOR_CAPTURE_DEBOUNCE_MS);
}

function cancelAnchorCapture(): void {
  if (captureTimer !== null) clearTimeout(captureTimer);
  captureTimer = null;
}

/**
 * [#576 W71-NF5] How many times `reset()` has run. An in-place SpeleoDB tree swap resets the
 * store without moving any project identity (same session, `created_at` and path), so a dive-plan
 * save answered after it would mark the reset store as holding that plan, unsaved. The bar
 * compares this across the save and drops such an answer.
 */
let resetCount = 0;

/** The number of `reset()` calls so far; see {@link resetCount}. */
export function gasPlanResetCount(): number {
  return resetCount;
}

export const useGasPlanStore = create<GasPlanState>((set, get) => ({
  ...initialState(),

  flushAnchorCapture: async () => {
    cancelAnchorCapture();
    if (captureInFlight !== null) await captureInFlight;
    if (anchorsAreCurrent(get())) return;
    await runAnchorCapture();
  },

  remapPinsFromAnchors: async () => {
    const s = get();
    const anchors = s.anchors;
    const pinsAtCapture = s.anchoredPins;
    if (anchors === null || pinsAtCapture === null) return;
    // Anchors that describe a pin set the store has since moved past are not safe to scatter.
    if (!deepEqual(pinsAtCapture, pinRefsOf(s))) return;
    const basePins = s.baselineAnchoredPins;
    const baseAnchorSet = basePins === null ? null : s.baselineAnchors;
    const liveAnchors = collectPresent(anchors);
    const baseAnchors = baseAnchorSet === null ? [] : collectPresent(baseAnchorSet);
    if (liveAnchors.length + baseAnchors.length === 0) return;
    let resolved: (StationRef | null)[];
    try {
      resolved = await planRefsForAnchors([...liveAnchors, ...baseAnchors]);
    } catch (e) {
      console.debug('[gas-plan] pin re-resolve failed', e);
      return;
    }
    if (!Array.isArray(resolved)) return;
    const now = get();
    // Same race guard as the capture: a pin edit during the round trip invalidates the alignment.
    if (!deepEqual(pinsAtCapture, pinRefsOf(now))) return;
    // The baseline is guarded SEPARATELY, not with an early return: a Save that landed in between
    // replaced the baseline (making these anchors stale for it) while leaving the live pins - and
    // the live pins are the ones the map and the engine read.
    const baselineIntact = deepEqual(basePins, pinRefsOfSnapshot(now.loadedPlanBaseline));

    const liveOut = redistribute(anchors, resolved.slice(0, liveAnchors.length));
    // Derived from the PINS, not from which slots happened to hold an anchor - the same rule the
    // capture uses. A pinned ref with no anchor at all is just as unplaceable as one whose anchor
    // stopped resolving, and reading it as "fine" would hand it straight to the engine.
    const flags = deriveFlags(pinsAtCapture, liveOut);
    // An unresolved pin KEEPS its old ref: the station may come back on an undo, and dropping the
    // diver's chosen lead because a shot was deleted is a worse answer than showing it as stalled.
    const kept = <T>(next: T | null, current: T): T => next ?? current;
    const nextPins: PlanPinRefs = {
      origin: liveOut.origin ?? now.origin,
      endpoint: liveOut.endpoint ?? now.endpoint,
      waypoints: now.waypoints.map((w, i) => kept(liveOut.waypoints[i] ?? null, w)),
      markedPoints: now.markedPoints.map((m, i) => kept(liveOut.markedPoints[i] ?? null, m.station)),
      stageDropOverrides: now.stageDropOverrides.map((d, i) =>
        d === null ? null : kept(liveOut.stageDropOverrides[i] ?? null, d)
      ),
    };
    const moved = !deepEqual(nextPins, pinsAtCapture);
    const baseOut =
      basePins === null || baseAnchorSet === null || !baselineIntact
        ? null
        : redistribute(baseAnchorSet, resolved.slice(liveAnchors.length));
    const nextBasePins: PlanPinRefs | null =
      basePins === null || baseOut === null
        ? null
        : {
            origin: baseOut.origin ?? basePins.origin,
            endpoint: baseOut.endpoint ?? basePins.endpoint,
            waypoints: basePins.waypoints.map((w, i) => kept(baseOut.waypoints[i] ?? null, w)),
            markedPoints: basePins.markedPoints.map((m, i) =>
              kept(baseOut.markedPoints[i] ?? null, m)
            ),
            stageDropOverrides: basePins.stageDropOverrides.map((d, i) =>
              d === null ? null : kept(baseOut.stageDropOverrides[i] ?? null, d)
            ),
          };
    const baselineMoved = nextBasePins !== null && !deepEqual(nextBasePins, basePins);
    const flagsChanged = !deepEqual(flags, now.unresolved);
    if (!moved && !baselineMoved && !flagsChanged) return;
    const baseline = now.loadedPlanBaseline;
    set({
      origin: nextPins.origin,
      endpoint: nextPins.endpoint,
      waypoints: nextPins.waypoints,
      markedPoints: now.markedPoints.map((m, i) => ({ ...m, station: nextPins.markedPoints[i]! })),
      stageDropOverrides: nextPins.stageDropOverrides,
      unresolved: flags,
      anchoredPins: nextPins,
      ...(baselineMoved && baseline !== null && nextBasePins !== null
        ? {
            loadedPlanBaseline: {
              ...baseline,
              origin: nextBasePins.origin,
              endpoint: nextBasePins.endpoint,
              waypoints: nextBasePins.waypoints,
              marked_points: baseline.marked_points.map((m, i) => ({
                ...m,
                station: nextBasePins.markedPoints[i]!,
              })),
              stage_drop_overrides: nextBasePins.stageDropOverrides,
            },
            baselineAnchoredPins: nextBasePins,
          }
        : {}),
      // The pins moved, so the painted field and the menu-shape fallback both describe a route
      // through stations that are no longer the ones pinned. Blanked exactly like every other
      // route-input change; the refetch this triggers re-derives them.
      //
      // A flag change counts as a move even when no ref did. `selectComputeGate` withholds a
      // flagged pin - and STALLS the whole compute on a flagged way point - so the field on screen
      // was computed from inputs that are no longer the ones in force, and the gate then blocks
      // every refetch that would have replaced it. Left alone it stays painted indefinitely, with
      // the panel's numbers reading as current.
      ...(moved || flagsChanged ? { reachField: null, lastReachField: null } : {}),
    });
  },

  syncSettingsOnlyGasFields: (persistedGas) => {
    const next = settingsOnlyFieldsOf(persistedGas);
    const s = get();
    // First observation is the seed, not an edit: the panel's hydrate already applied this config.
    if (s.lastSeenPersistedGas === null) {
      set({ lastSeenPersistedGas: next });
      return;
    }
    // FIELD BY FIELD, never as a pair. These two share nothing but the accident of having no
    // in-panel control, and copying both because ONE moved overwrites the other with whatever
    // Settings happens to hold - so nudging the jump multiplier would silently flip a loaded
    // plan's reference-legs toggle too, in the one place a plan's own snapshot is allowed to move
    // underneath it.
    const prev = s.lastSeenPersistedGas;
    const patch: Partial<SettingsOnlyGasFields> = {};
    if (prev.jump_rmv_multiplier !== next.jump_rmv_multiplier) {
      patch.jump_rmv_multiplier = next.jump_rmv_multiplier;
    }
    if (prev.include_reference_legs !== next.include_reference_legs) {
      patch.include_reference_legs = next.include_reference_legs;
    }
    if (Object.keys(patch).length === 0) return;
    // Deep-cloned for the same reason the hydrate seed is: the working config must never share a
    // nested reference with the live persisted config.
    set({
      gasConfig: { ...s.gasConfig, ...structuredClone(patch) },
      lastSeenPersistedGas: next,
      reachField: null,
      // Either field can move the route and its drop positions (the jump multiplier re-prices
      // legs, the reference toggle changes the graph), so pinned drops clear exactly as every
      // other gas/route-input setter clears them - a ▼ dragged on the old route must not be
      // sent into the recompute against the new one.
      stageDropOverrides: [],
      unresolved: routeInputFlags(s.unresolved),
    });
  },

  // #395: the entrance IS the default origin (`origin: null`), so choosing it answers the
  // question without moving any pin.
  chooseEntrance: () => set({ originChosen: true }),
  setOriginPickArmed: (armed) => set({ originPickArmed: armed }),

  // Every pin setter maintains `unresolved` alongside the pins it moves (#310). Picking a pin
  // CLEARS its flag: the flag says "the backend could not place the ref this slot held", and the
  // slot no longer holds it. Without that, one dangling origin latched the flag forever and
  // `selectComputeGate` went on nulling every replacement the diver clicked - the field silently
  // rooting at the entrance while the origin star sat where they put it.
  //
  // #395: only a NON-null ref answers the entry question - `setOrigin(null)` (the panel's Reset
  // button, or the project-switch default fallback in MapCanvas) intentionally leaves
  // `originChosen` untouched, so the full rig stays up rather than resurrecting the entry card.
  // `originPickArmed` always clears here: this IS the pick landing, whether it came from the
  // one-shot "Pick on map" button or the ordinary pre-origin click-to-set-origin flow.
  setOrigin: (origin) =>
    set((s) => ({
      origin,
      originChosen: origin !== null ? true : s.originChosen,
      originPickArmed: false,
      reachField: null,
      stageDropOverrides: [],
      unresolved: routeInputFlags(s.unresolved, { origin: false }),
    })),
  setEndpoint: (endpoint) =>
    set((s) => ({
      endpoint,
      reachField: null,
      stageDropOverrides: [],
      unresolved: routeInputFlags(s.unresolved, { endpoint: false }),
    })),
  // Way points are a ROUTE input: every edit re-bases the whole field, so they clear the cached
  // field and the pinned drops exactly like setEndpoint. Marked points are pure annotation and
  // deliberately do NOT - clearing the field there would flash the overlay off for nothing.
  // Capped, and a repeat of a station already on the list is a no-op. Both mirror the backend:
  // `save_dive_plan` REFUSES an over-long list, so without the cap here the diver could build a plan
  // in the UI - paying an extra Dijkstra per way point on every keystroke - and only discover the
  // wall at save time, on a message that names neither the limit nor which entry to drop. The
  // label field is already capped for exactly this reason. A duplicate is separately meaningless:
  // it burns a slot, adds a second identical panel row, and makes the panel's per-way-point gas
  // read the LATER cumulative while the overlay paints the earlier pass.
  addWaypoint: (ref) =>
    set((s) => {
      const dup = s.waypoints.some(
        (w) => w.section_id === ref.section_id && w.station_index === ref.station_index,
      );
      if (dup || s.waypoints.length >= MAX_WAYPOINTS) return {};
      // A new way point is always appended at the end of the visit order, so it always splits
      // the LAST span (the open field past the previous final way point) into two: the leg
      // leading up to it and the leg leading on from it. Both inherit the split segment's
      // pace/propulsion - the diver hasn't said anything different about either half yet.
      const splitSeg = s.segments[s.segments.length - 1] ?? defaultSegment();
      return {
        waypoints: [...s.waypoints, ref],
        segments: [...s.segments.slice(0, -1), splitSeg, { ...splitSeg }],
        reachField: null,
        stageDropOverrides: [],
        unresolved: routeInputFlags(s.unresolved, {
          waypoints: [...s.unresolved.waypoints, false],
        }),
      };
    }),
  removeWaypoint: (index) =>
    set((s) => {
      if (index < 0 || index >= s.waypoints.length) return {};
      // Dropping way point `index` collapses the two spans it used to separate - segment
      // `index` (leading into it) and segment `index + 1` (leading on from it) - into one.
      // The merge takes the SLOWER of the two speeds and keeps the scooter flag if EITHER half
      // had it: the safe direction on both counts, mirroring the backend's own reconciliation of
      // a stale way point on load (`merge_segments_for_surviving_waypoints`,
      // core/state/dive_plan.rs). A slower span costs more gas per metre; a scootered one spends
      // range without buying any gas back, so forgetting the flag would extend the plan's reach.
      // Both spans always exist: `segments` is normalised to `waypoints.length + 1` at every
      // entry point, and `index` is in range, so `index + 1` is at most the last span.
      const merged: PlanSegment = mergeSegments(
        s.segments[index]!,
        s.segments[index + 1]!,
        planSpeed(s.gasConfig)
      );
      return {
        waypoints: s.waypoints.filter((_w, i) => i !== index),
        segments: [...s.segments.slice(0, index), merged, ...s.segments.slice(index + 2)],
        reachField: null,
        stageDropOverrides: [],
        // The flags are indexed by way point, so the dropped one's flag has to go with it. Leaving
        // it behind leaves `selectComputeGate.skip` true against a way point that no longer exists
        // - the panel's warning is bounded by `waypoints.length` and disappears, so the plan stops
        // computing with nothing on screen saying why.
        unresolved: routeInputFlags(s.unresolved, {
          waypoints: s.unresolved.waypoints.filter((_f, i) => i !== index),
        }),
      };
    }),
  moveWaypoint: (index, delta) =>
    set((s) => {
      const to = index + delta;
      if (index < 0 || index >= s.waypoints.length || to < 0 || to >= s.waypoints.length) {
        return {};
      }
      const next = s.waypoints.slice();
      const moved = next[index]!;
      next[index] = next[to]!;
      next[to] = moved;
      const flags = s.unresolved.waypoints.slice();
      const movedFlag = flags[index] ?? false;
      flags[index] = flags[to] ?? false;
      flags[to] = movedFlag;

      // Segments do NOT travel with the way point - a span is bounded by two of them, so none
      // belongs to the one being moved - but they cannot simply stay put either (#317). Span `k`
      // joins way points `k-1` and `k` (the origin and the open field standing in at the ends), so
      // swapping `lo` with `hi` re-attaches spans `lo`, `lo+1`, `hi` and `hi+1` to different
      // physical passages. Those are RESET to the inherited default - the state a freshly created
      // leg gets - rather than kept, so a pace or a scooter flag the diver set for one passage is
      // never silently applied to another.
      //
      // The single exception is the span BETWEEN an adjacent pair (`lo+1 === hi`): it still joins
      // the same two points, only walked the other way round, so it keeps what it was given.
      // Length-sync holds trivially - a swap changes no counts.
      const lo = Math.min(index, to);
      const hi = Math.max(index, to);
      const rebound = new Set([lo, lo + 1, hi, hi + 1]);
      if (hi === lo + 1) rebound.delete(lo + 1);
      return {
        waypoints: next,
        segments: s.segments.map((seg, k) => (rebound.has(k) ? defaultSegment() : seg)),
        reachField: null,
        stageDropOverrides: [],
        unresolved: routeInputFlags(s.unresolved, { waypoints: flags }),
      };
    }),
  // A segment is a GAS INPUT: its speed divides straight into every edge cost on that span
  // (`gas = rmv x ata(depth) x length / speed`), so a slower span costs more gas and reaches less
  // far. `on_scooter` spends the rig's range but does NOT derate the breathing rate - scootering
  // gets no reduced-RMV credit (PRD section 10), which is the conservative direction. Either way
  // the field is stale, so this nulls `reachField` and clears `stageDropOverrides` exactly like
  // `setCylinders`/`setGasConfig`.
  setSegments: (segments) =>
    set((s) => ({
      segments: normalizeSegments(segments, s.waypoints.length, planSpeed(s.gasConfig)),
      reachField: null,
      stageDropOverrides: [],
      unresolved: routeInputFlags(s.unresolved),
    })),
  addMarkedPoint: (ref) =>
    set((s) =>
      s.markedPoints.length >= MAX_MARKED_POINTS
        ? {}
        : {
            markedPoints: [...s.markedPoints, { station: ref, label: null }],
            unresolved: { ...s.unresolved, markedPoints: [...s.unresolved.markedPoints, false] },
          },
    ),
  removeMarkedPoint: (index) =>
    set((s) =>
      index < 0 || index >= s.markedPoints.length
        ? {}
        : {
            markedPoints: s.markedPoints.filter((_m, i) => i !== index),
            unresolved: {
              ...s.unresolved,
              markedPoints: s.unresolved.markedPoints.filter((_f, i) => i !== index),
            },
          },
    ),
  setMarkedPointLabel: (index, label) =>
    set((s) => {
      if (index < 0 || index >= s.markedPoints.length) return {};
      const next = s.markedPoints.slice();
      // Store the RAW value. Trimming here would fight the controlled input this feeds: the panel
      // renders `value={m.label ?? ''}`, so typing a space produces "photo ", the setter would store
      // "photo", and React would restore the shorter value into the DOM - eating the space on every
      // keystroke and making a multi-word label untypable by hand. (`fill()` in an e2e dispatches
      // one event with the whole string, so it never sees this.) Normalising a keystroke against a
      // controlled value is the same trap as the signed-decimal fields.
      //
      // The trim happens once, at the boundary that needs it: `divePlanSnapshotOf` before the save.
      next[index] = { ...next[index]!, label: label === '' ? null : label };
      return { markedPoints: next };
    }),
  setDiver: (diver) =>
    set((s) => ({
      diver,
      reachField: null,
      stageDropOverrides: [],
      unresolved: routeInputFlags(s.unresolved),
    })),
  setCylinders: (cylinders) =>
    set((s) => ({
      cylinders,
      reachField: null,
      stageDropOverrides: [],
      unresolved: routeInputFlags(s.unresolved),
    })),
  setGasConfig: (gasConfig) =>
    set((s) => ({
      gasConfig,
      reachField: null,
      stageDropOverrides: [],
      unresolved: routeInputFlags(s.unresolved),
    })),
  setDpvRig: (rig) =>
    set((s) => {
      // Only a REMOVAL (a rig was in force, and now none is) resets segments. Installing a rig, or
      // swapping one rig for another, leaves every segment's speed/scooter flag exactly as the
      // diver left it - there is nothing unsafe about a scootered span with a rig still behind it.
      const removed = s.gasConfig.dpv !== null && rig === null;
      return {
        gasConfig: { ...s.gasConfig, dpv: rig },
        segments: removed
          ? s.segments.map((seg) =>
              seg.on_scooter ? { on_scooter: false, speed_m_per_min: null } : seg
            )
          : s.segments,
        reachField: null,
        stageDropOverrides: [],
        unresolved: routeInputFlags(s.unresolved),
      };
    }),
  // `lastReachField` only ever moves forward to a real field - it is never nulled here, so it
  // survives the refetch window. `reset()` clears both, so it cannot leak across a project switch.
  setReachField: (reachField) =>
    set(reachField ? { reachField, lastReachField: reachField } : { reachField }),
  setPushyAck: (pushyAck) => set({ pushyAck }),
  markHydrated: (persistedGas) =>
    set((s) => ({
      hydrated: true,
      // Seed the Settings-only baseline here too, so the panel's mount order cannot matter: a
      // hydrate that ran before the first `syncSettingsOnlyGasFields` already establishes what
      // "unchanged" means. Never overwrites a value a sync has since moved on from.
      ...(persistedGas !== undefined && s.lastSeenPersistedGas === null
        ? { lastSeenPersistedGas: settingsOnlyFieldsOf(persistedGas) }
        : {}),
    })),
  setStageDropOverride: (index, ref) => {
    // Guard BEFORE set so a stray negative index is a true no-op (returning {} from the updater
    // would still merge a fresh state object and wake subscribers for nothing).
    if (index < 0) return;
    set((s) => {
      const next = s.stageDropOverrides.slice();
      while (next.length <= index) next.push(null);
      next[index] = ref;
      // Padded and cleared in lockstep with the drops: a flag left over from a previous drop at
      // this index would mask the one the diver just pinned out of `selectComputeGate` and block
      // Save, for a pin that is now perfectly placeable.
      const flags = s.unresolved.stageDropOverrides.slice();
      while (flags.length < next.length) flags.push(false);
      flags[index] = false;
      return { stageDropOverrides: next, unresolved: { ...s.unresolved, stageDropOverrides: flags } };
    });
  },

  applyLoadedPlan: (planId, snapshot, anchors) => {
    // An engine-invalid rig (speed or burn <= 0) is dropped WHOLE, mirroring what
    // `GasConfig::normalize` does at the compute door (`validate_plan_inputs`) - the engine
    // will plan rig-less either way, so keeping it here would show a live toggle and a
    // usable-range figure over a plan the engine is not actually range-capping (the #233
    // effective-state rule). Only reachable from a hand-edited `.plnx`: the editor cannot
    // construct one, and the config file normalizes on load. Dropped from the BASELINE too
    // (this whole normalized snapshot is the baseline), so a freshly loaded plan stays
    // clean rather than instantly dirty.
    const rigDropped =
      snapshot.gas_config.dpv != null && dpvRigInvalid(snapshot.gas_config.dpv);
    // Grounding first (a grounded span's speed is nulled for its own reason), then the range.
    const speedsChecked = dropOutOfRangeSpeeds(
      rigDropped
        ? snapshot.segments.map((seg) =>
            seg.on_scooter ? { on_scooter: false, speed_m_per_min: null } : seg
          )
        : snapshot.segments
    );
    // A plan written before segments existed carries none, and a hand-edited one can carry any
    // number; either way the panel and the engine need exactly one per span from the first render.
    const normalized: DivePlanSnapshot = {
      ...snapshot,
      gas_config: rigDropped
        ? { ...snapshot.gas_config, dpv: null }
        : snapshot.gas_config,
      // The PLAN's own speed, not the working config's: the snapshot brings its gas config with it
      // and is installed wholesale below, so folding against anything else would rank the spans on
      // a pace this plan is never flown at.
      //
      // Dropping the rig also grounds every scootered span, exactly as `setDpvRig(null)` does
      // live: the engine gates `on_scooter` off without a rig but still flies the span's
      // auto-filled scooter pace, and a flag left latent here would spring back to life on the
      // next rig install - a scootered plan reappearing from a rig the load just refused.
      segments: normalizeSegments(
        speedsChecked.segments,
        snapshot.waypoints.length,
        planSpeed(snapshot.gas_config)
      ),
    };
    const pins = pinRefsOfSnapshot(normalized)!;
    // The backend resolved these refs FROM these anchors moments ago, so they are already correct
    // for the pins being installed - seeding them here saves the load path a capture round trip
    // and, more importantly, closes the window where a survey edit could land against no anchors.
    const seeded: PinSlots<PlanAnchor> | null =
      anchors === null || anchors === undefined
        ? null
        : {
            origin: anchors.origin,
            endpoint: anchors.endpoint,
            waypoints: anchors.waypoints,
            markedPoints: anchors.marked_points,
            stageDropOverrides: anchors.stage_drop_overrides,
          };
    set({
      // #395: a loaded plan answers the entry question outright, whether or not its own origin
      // ended up at the entrance default.
      originChosen: true,
      // A stale arm from before the load (e.g. Escape never fired, or the panel unmounted mid-pick)
      // must not consume the next plain map click as an origin overwrite on the just-loaded plan.
      originPickArmed: false,
      origin: normalized.origin,
      endpoint: normalized.endpoint,
      waypoints: normalized.waypoints,
      markedPoints: normalized.marked_points,
      diver: normalized.diver,
      cylinders: normalized.cylinders,
      gasConfig: normalized.gas_config,
      stageDropOverrides: normalized.stage_drop_overrides,
      segments: normalized.segments,
      reachField: null,
      loadedPlanId: planId,
      loadedPlanBaseline: normalized,
      // Live and baseline pins are identical the instant a plan loads, so one anchor set serves
      // both; they diverge on the first edit, and the next capture splits them.
      anchors: seeded,
      anchoredPins: seeded === null ? null : pins,
      baselineAnchors: seeded,
      baselineAnchoredPins: seeded === null ? null : pins,
      // All-clear, but SIZED to the pins being installed rather than the empty `NO_FLAGS`: the
      // flags are length-synced from the first render, so the next capture derives an identical
      // set instead of reading as a change and blanking the field for nothing. (The backend drops
      // a pin whose anchor no longer places, so a loaded plan's pins are placeable by definition.)
      unresolved: noFlagsFor(pins),
    });
    return { droppedSpeeds: speedsChecked.dropped };
  },
  markPlanSaved: (planId, baseline) =>
    set({ loadedPlanId: planId, loadedPlanBaseline: baseline }),
  clearLoadedPlan: () =>
    set({ loadedPlanId: null, loadedPlanBaseline: null, baselineAnchors: null, baselineAnchoredPins: null }),
  setLoadDropped: (staleRefs, speeds) => set({ loadDropped: { staleRefs, speeds } }),

  reset: () => {
    resetCount += 1;
    set(initialState());
    // After the set, not before: the subscription below re-arms the debounce on the pin change
    // this very reset just made, and a capture left armed would fire against the next project.
    cancelAnchorCapture();
  },
}));

/**
 * Re-arm the anchor capture whenever the pins move (#310).
 *
 * One subscription rather than a call in each of the eight pin-mutating setters: a setter added
 * later cannot forget to opt in, and a direct `setState` (the panel's own reverse/undo paths, and
 * tests) is covered for free. Only the pin REFS are compared, so relabelling a marked point or
 * editing the rig does not schedule anything.
 */
useGasPlanStore.subscribe((state, previous) => {
  if (deepEqual(pinRefsOf(state), pinRefsOf(previous))) return;
  scheduleAnchorCapture();
});

/** #244: true when a plan is loaded and the live inputs diverge from its saved
 *  baseline. A never-saved exploration (`loadedPlanId === null`) is never dirty. */
export function selectIsPlanDirty(s: GasPlanState): boolean {
  if (s.loadedPlanId === null || s.loadedPlanBaseline === null) return false;
  return !divePlanSnapshotsEqual(divePlanSnapshotOf(s), s.loadedPlanBaseline);
}

/** #310: whether the plan pins anything at all - what lets the mutation seam skip the whole
 *  anchor round trip (two IPC calls per edit) for the overwhelmingly common empty plan. The
 *  BASELINE's pins count too: a loaded plan whose live pins were all cleared still compares
 *  against baseline refs (`selectIsPlanDirty`), and skipping the round trip would leave those
 *  in the old numbering after a renumbering edit - the exact silent rebind the baseline anchors
 *  exist to prevent. */
export function selectHasPins(s: GasPlanState): boolean {
  if (collectPresent(pinRefsOf(s)).length > 0) return true;
  const base = pinRefsOfSnapshot(s.loadedPlanBaseline);
  return base !== null && collectPresent(base).length > 0;
}

/** #310: any pin the backend could no longer place. Blocks a Save - persisting a pin the backend
 *  cannot anchor is exactly how a plan acquires a silently wrong station. */
export function selectHasUnresolvedPins(s: GasPlanState): boolean {
  const f = s.unresolved;
  return (
    f.origin ||
    f.endpoint ||
    f.waypoints.some(Boolean) ||
    f.markedPoints.some(Boolean) ||
    f.stageDropOverrides.some(Boolean)
  );
}

/** Cache so `selectComputeGate` returns a STABLE drops array. Rebuilding it per call would make
 *  the map's `useShallow` subscription see a new value on every unrelated store write and refetch
 *  the reach field in a loop. */
let maskedDropsCache: {
  drops: (StationRef | null)[];
  flags: boolean[];
  out: (StationRef | null)[];
} | null = null;

function maskedDrops(drops: (StationRef | null)[], flags: boolean[]): (StationRef | null)[] {
  if (!flags.some(Boolean)) return drops;
  if (maskedDropsCache !== null && maskedDropsCache.drops === drops && maskedDropsCache.flags === flags) {
    return maskedDropsCache.out;
  }
  // Masked to null, NOT spliced out: the array is aligned by index with the field's
  // `stage_drop_refs`, so removing an entry would silently move every later stage's drop.
  const out = drops.map((d, i) => (flags[i] ? null : d));
  maskedDropsCache = { drops, flags, out };
  return out;
}

/**
 * #310: what the reach-field fetch is allowed to send.
 *
 * A stale positional pin is not a harmless one: after a delete it still names a station that
 * EXISTS, just the wrong one, so the engine happily prices a route the diver never asked for. So
 * anything the backend could not re-place is withheld here rather than sent:
 *  - a way point STALLS the whole compute (`skip`), because the plan is defined as visiting it in
 *    order and there is no honest field to draw without it - the #298 contract;
 *  - the origin and endpoint DEGRADE to their defaults (entrance / deepest in-budget), which is
 *    the same graceful fallback a plan with a dangling anchor gets on load;
 *  - a stage drop falls back to the strategy default.
 */
export function selectComputeGate(s: GasPlanState): {
  skip: boolean;
  origin: StationRef | null;
  endpoint: StationRef | null;
  stageDropOverrides: (StationRef | null)[];
} {
  return {
    skip: s.unresolved.waypoints.some(Boolean),
    origin: s.unresolved.origin ? null : s.origin,
    endpoint: s.unresolved.endpoint ? null : s.endpoint,
    stageDropOverrides: maskedDrops(s.stageDropOverrides, s.unresolved.stageDropOverrides),
  };
}
