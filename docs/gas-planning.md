# Dive and gas planning (Plan mode)

Plan mode answers one question: on this gas, starting here, how far into the cave
can the diver get before they have to turn? Everything else - the route line, the
turn dots, the stage drops, the pressure gauges - is a view onto that answer.

Plan is one of the four workspace modes (Survey, Data, Plan, Export), read-only over the survey
([SPEC.md 6.1](SPEC.md#61-workspace-modes)). The engine is
`src-tauri/crates/models/src/project/gas.rs` (+ `gas/model.rs`, `gas/physics.rs`),
reached through the single `compute_gas_reach_field` command. Umbrella issue
[#38](https://github.com/Stalagmight/Stalagmight/issues/38); its design PRD was
removed from the repo before open-sourcing, so this page is the reference.

## The reach field

To a diver: you can spend a fixed slice of your gas going in - a third of it,
say - and the rest is what brings you home. The reach field colours every station
by the gas it costs to get there, greys out everything past that slice, and marks
the stations on the boundary. Those are your turn points.

In code it is a multi-source Dijkstra over the survey graph, weighted by gas:

```
edge gas (litres) = rmv_eff x ata(mean leg depth) x (length / speed_in)
ata(z)            = 1 + max(z, 0) / 10            (10 msw ~ 1 bar)
mean leg depth    = (z_from + z_to) / 2
rmv_eff           = rmv x survey_rmv_multiplier   ( x jump_rmv_multiplier on a virtual leg)
speed_in          = the span's segment speed (below), else survey_speed_m_per_min,
                    else swim_speed_m_per_min
```

(`gas/physics.rs` - pure, golden-tested.) A station is reachable when its
gas-to-reach is finite and `<= budget_litres`; the turn frontier is every
reachable station bordering one that is not. Given the reach mask, frontier
membership is pure adjacency - the bordering test ignores edge weights, so even
a non-swimmable reference tie counts as a border.

Two things the model deliberately does not do. **The exit is never simulated** -
only the in-leg is accumulated, and the gas to get home is held back by the
strategy (below) rather than costed leg by leg, so `swim_speed_m_per_min` reaches
the engine only as the fallback when no survey speed is set. And **it never
triggers the closure solve**: it consumes `compute_station_coordinates` (measured,
instant), because reachability is topological and adjustment moves coordinates by
sub-metre amounts - same reasoning as the distance field, see
[closures.md](closures.md).

### What the graph contains

`build_survey_graph_weighted` (`models/project/pathfinding.rs`) emits every shot,
plus a zero-weight edge per station merge and per loop closure. Zero is correct
because both join points that are physically the same - which is also why the walk
re-settles gas across zero-cost edges (`relax_zero_cost`). Without that, one node
of a merged pair reads reachable and its twin, in the same spot, reads unreachable
and lands on the frontier.

Jumps and reference shots part company here ([SPEC.md 3](SPEC.md#3-glossary)):

| Leg | In the gas graph | Cost |
|---|---|---|
| Normal shot | yes | base RMV |
| Jump | yes - a diver swims it | RMV x `jump_rmv_multiplier` (default 1.5) |
| Reference | no, unless `gas.include_reference_legs` | infinite weight (opted in: also x `jump_rmv_multiplier`) |

A reference tie gets an infinite weight rather than being dropped, because
`along_line_between` keys a parallel shot-edge map on the graph builder's
edge-emission order - the edge set has to stay identical, and an infinite weight
never relaxes. Opted back in, a reference carries the jump multiplier too:
charging base RMV would make the route cost *less* than it did before the
jump/reference split, overstating reach.

That direction is the rule the engine is written to - every clamp and fallback
errs toward less reach. `GasConfig::normalize` resets a below-1.0 RMV multiplier
and a negative reserve and treats a survey speed faster than the swim speed as
symmetric; a non-positive in-speed or in-RMV bails the field to all-unreachable
rather than dividing by zero into a wholly reachable cave.

## What the budget is made of

`penetration_budget` returns the litres a diver may burn on the way in. Cylinder
gas is always real-gas corrected: `deliverable_gas` divides by `air_z_factor`, a
quadratic fitted to the conservative end of tabulated air Z (1.045 at 207 bar) and
clamped to `>= 1.0` so the derate can never become a credit. With
`conservative_tank_gas` on (the default) the pressure is first capped to rated -
the "call-it-an-80" convention - so an overfill counts only its rated capacity.

Only `back_gas` and `stage` roles are penetration gas; `dil_out`, `bailout` and
`deco` are excluded everywhere. A cylinder with `enabled = false` is filtered out
before anything is computed - budget, gauges, drops - so a diver can toggle rig
variants without deleting rows.

| Strategy (`ReservePreset`) | Back gas counted | Stage term | Stage drops |
|---|---|---|---|
| `all_thirds` (default) | 1/3 of deliverable, less the safety margin | same rule for stages | none |
| `one_stage` (Half Minus) | 1/4 | stage 1's IN half | 1 |
| `two_stage` (Half Minus) | 1/6 | first two stages' IN halves | 2 |
| `free` | the diver's own `usable_gas_bar` | same | one per stage with usable gas |

Half Minus, in diver terms: the stage carries the penetration, so you dip less of
your back gas. A stage is breathed from full down to `(fill + reserve)/2` on the
way in - that is its drop pressure - contributing `(fill - reserve)/2` of
pressure; the other half is breathed back down to `reserve` on the exit after you
pick it up. `reserve` is `reserve_profile.stage_reserve_bar`: default 30 bar,
editable in the panel, capped at 100 by `normalize`. Stages are taken in rig
order and only the first `required_stages` of them count - there is no per-stage
drop-order field, and ignoring a spare stage only shrinks the budget.

The All-thirds safety margin (`all_thirds_safety_margin_bar`, default 0) holds
back the deliverable gas in the top N bar of every budget cylinder, on top of
thirds; it only shrinks usable gas, so `normalize` caps a large value and resets a
negative one. Free hands the diver the whole calculation: they type a usable-bar
allowance per cylinder and the engine budgets exactly the Z-derated span from full
down to `fill - usable`, clamped to `[0, capped fill]` so an oversized value
cannot credit gas the tank does not hold.

A staged strategy the rig cannot support falls back to All-thirds.
`effective_preset` counts enabled `stage` cylinders; short of `required_stages`
the budget, the gauges, the drops and the per-cylinder usable column all switch
together, and `GasPlanPanel` mirrors the same count so the UI cannot show a staged
strategy selected while the engine computes thirds.

## Stage drops, and moving them

A drop point is where a stage runs dry on the way in - the station you clip it off
and leave it. The engine places each at the first route station whose cumulative
route gas reaches that stage's threshold, thresholds being cumulative across
stages so drop 2 always lands at or past drop 1. A threshold past the turnaround
(the cave is shorter than the staged gas) clamps to the last route station, so
every stage yields exactly one drop and the per-stage refs, gauges and distances
stay aligned 1:1.

Drops are movable ([#246](https://github.com/Stalagmight/Stalagmight/issues/246)):
drag the marker, arrow-key it along the route, Delete or double-click to reset. A
pin redefines how much of that stage is breathed in - the route's cumulative gas
at the pinned station, minus the cumulative already assigned to earlier drops,
clamped to `[0, tank empty]`. The plan recomputes on
`base_budget + sum(actual - safe)`:

- **Shallower** than the strategy point: the stage is dropped part-full, the
  surplus stranded, the budget shrinks and the reach shortens.
- **Deeper**: the stage is breathed into its reserve, the budget grows, the reach
  extends, and `stage_drop_over_reserve[k]` is set - the marker turns red and the
  panel warns. A warning, not a block; the diver asked for it. Set only for a
  genuine drag deeper (`contribution > safe + 1e-6`), and past tank-empty the
  contribution clamps, so the drop snaps back to the empty point.
- **Onto the station the default already sits on**: an exact no-op. The default
  marker sits at the first station whose gas *reaches* the threshold, so its gas
  usually overshoots; scoring that as a fresh contribution would fabricate an
  over-reserve plan out of a jiggle.
- **Off the route**, unresolvable, or non-finite: falls through to the strategy
  default. Reachable in normal use - `.plnx` persists overrides, so a plan
  reloaded after the survey or endpoint moved can carry a pin the new route no
  longer passes.

Arrival gas comes from the route's own cumulative gas at the pin's first pass -
not from the station's gas-to-reach, which with way points is measured from the
last way point and for a station passed on the way out reports the cost of coming
*back* to it; an early drop scored against that inflated figure reads as a deep
one and grows the budget. The one exception is a station past the end of the route
on the same line, which a legitimately deeper drag reaches: there the route has
nothing to say and `gas_in` is exact. A station on a different branch yields no
arrival gas at all and keeps the strategy default. The walk runs against the base
budget's route, which keeps the ordering acyclic - the final route depends on the
budget, which depends on these contributions - and both coincide when nothing is
overridden. A moved drop's displayed pressure is inverted off the Z curve by
bisection (`drop_pressure_for_contribution`); the reach stays exact-litres from
the graph.

## Routing

The route is the ordered leg list from the origin through every way point to the
turnaround, populated for any strategy once something is reachable.

**Origin** - a hint. Unset means the zero datums (the entrance); a set origin that
no longer resolves falls back to them rather than blanking the field.

**Endpoint** - a hint: the lead being planned toward. Set it by left-clicking a
station in Plan, by clicking a turn-frontier dot, or from the station context
menu. In budget, it is the turnaround. Out of budget, the plan stays on the
passage the diver chose and turns earlier - the target becomes the last in-budget
station on the path toward it. Only with no endpoint at all, or one that is
unusable (deleted under a loaded plan, or in another graph component), does it
fall back to the deepest in-budget station *anywhere*, and `turn_toward_endpoint`
reports which happened. The UI needs that flag because the two read identically
otherwise, and saying "turning at X on the way there" about an unrelated lead is
the failure the walk-toward-the-endpoint rule exists to prevent.

**Way points** - a constraint, not a hint. Each costs one extra Dijkstra seeded at
the previous way point, with leg costs accumulated, so a station's gas is what it
costs *after* flying the whole chain and adding a way point costs real gas rather
than only redrawing a line. A way point that cannot be reached, or that no longer
resolves, stalls the plan: every later one reports `None`, the whole field reads
unreachable, and the route shows only the chain committed before the break.
Skipping it silently would hand back a route ignoring a point the dive was
required to hit. Capped at 64, enforced on save, on load and on the live command.

Two subtleties fall out of the chain. A station the chain swims *through* is
reported at its pass-through cost, not the after-chain cost of returning to it -
otherwise the entrance itself would price as an out-and-back and put a spurious
turn dot in the middle of the diver's own route. And the frontier is keyed on the
after-chain "could the dive END here" mask, with chain stations and their
zero-cost twins excluded: a station on the way to a way point is transit.

**Marked points** are pure annotation - a photo stop, a lead, a restriction to
rig. Saved with the plan, never sent to the engine, so marking a station cannot
move the route or the budget. Capped at 500; labels at 120 characters.

### Route metrics

`route_line_length_m` is the one-way *swum* distance from origin to turnaround:
3-D slope lengths over real shot legs only. A leg is swum when it joins adjacent
stations within one section (`is_swim_leg`); a merge or closure tie is a
zero-distance edge between the same physical point, and its measured-coordinate
gap (a closure's misclosure) was never swum, so it is excluded from the length and
the depth weighting. `route_max_depth_m` is max `z` over *all* route stations, tie
endpoints included - both ends of a tie are surveyed stations the diver passes
through. `route_avg_depth_m` is distance-weighted over the swum legs.

None of this is the length of the highlight on screen: the engine measures 3-D
swum distance on measured coordinates, while `SurveyLayer.applyGasRoute` draws
through the displayed geometry - in Plan, the closure-adjusted 2-D plan view.

The Profile tab ([#299](https://github.com/Stalagmight/Stalagmight/issues/299))
charts `route_profile` - depth against distance or runtime, out to the turn and
back - and exports it as Subsurface-shaped XML or a plain-text slate table. The
way in is priced at each span's own speed on the survey RMV; the way out at
`swim_speed_m_per_min` on the base RMV, which `normalize` guarantees is no
slower - so the return is never under-stated, including for a span that was
scootered in.

## Segments and the DPV

The way points divide the route into **segments** - exactly `waypoints + 1`
spans, segment k running from way point k-1 to k and the last out into the open
field. Each span carries an optional speed and an `on_scooter` flag
(`PlanSegment`; the Segments tab is the editor). A `null` speed means "fly this
span at the plan speed" - only a span the diver edits gets a number, so the
Swim-speed field stays live for everything else - and a speed outside
`[0.1, 1000]` m/min is ignored the same way (`resolve_segments` falls back to
the plan speed rather than flying a denormal). Each segment has an identity
color (`utils/segmentColors.ts`): the route
highlight paints each leg with its segment's color, and the table row carries a
matching chip, keyed off the engine's own `waypoint_route_indices` rather than
re-matching stations client-side (a route can double back, and a merge makes one
physical station several refs).

A DPV rig (`GasConfig.dpv`, edited in the Plan dock's **DPV** tab, next to the
cylinders) is a primary scooter plus a mandatory backup - there is no
dead-scooter swim-out to model, so the exit mirrors the entry span for span and
the gas half of the reach predicate is untouched. The rig's usable one-way range
is the weaker unit by LINE COVERED (`speed x burn` - not halved: primary flies
in, backup flies out), less a reserve fraction for battery age and flow
(`DpvRig::usable_range_m`; reserve normalized into `[0, 0.9]`).

Riding a span spends that allowance in **range, not line**: at or above the
slower unit's cruise speed a metre of line costs a metre of range, and a
below-cruise span costs `slower_cruise / speed` per metre - the battery drains
by the clock, not the odometer (`range_per_metre`, `gas/scooter.rs`). So
`scooter_distance_m` is range *spent* and can exceed the line covered. A station
whose route spends more than the usable range drops out of the reach masks, and
`turn_on_scooter_range` reports when the range cap - not gas - is what moved the
turnaround (set from the route target itself, not a cave-global scan). Riding
buys speed, and through it cheaper gas per metre; it does not change the
breathing rate.

A span with the flag off is swum - towing the units or having parked them,
numerically identical - so the park point is *derived*, never placed: the
boundary after the last scootered span (`scooter_drop_ref`), which can only be a
way point. The engine keeps a rig-less segment's explicit speed on purpose
(variable pace is useful without a scooter), so the UI owns the intuitive
contract: unticking a span's DPV box returns it to the plan speed, and removing
the rig (`gasPlanStore.setDpvRig(null)`) resets every scootered span the same
way - otherwise the auto-filled cruise speed would keep pricing gas at a pace
the diver no longer has, and the plan would stay too long until someone noticed.

## Display values are engine-computed

Every budget-dependent number the panel shows comes from `GasReachField`; the UI
formats it and nothing else. This is a rule, not an observation. The engine
applies a conservative cap, a normalized reserve, a stage-count fallback and
several clamps, so a readout that re-derives the same figure client-side drifts
from what the map overlay enforces, usually in the unsafe direction. That was the
[#233](https://github.com/Stalagmight/Stalagmight/issues/233) gauge bug, and the
fix was to move the gauges into the engine. `backgas_turn_bar`, `stage_drop_bars`,
`stage_drop_over_reserve`, `stage_drop_distances_m`, `cylinder_usable_bars`, the
three `route_*` metrics, `waypoint_gas_litres` and `turn_toward_endpoint` all
exist for this reason. Where the UI must mirror engine logic (greying a control,
say), mirror the *effective* state, not the raw user selection.

Two gating habits go with it. The field is `null` for the duration of every
refetch - each input setter blanks it so stale colours are not painted under new
inputs - so a readout must check the field is present before reading a missing
value as bad news. And a row naming a pressure to leave a tank at must gate on a
real turnaround, not on the rig having stages: `stage_drop_bars` comes from the
strategy alone, so a stalled way point makes it non-empty for an unflyable dive.

## Saved plans (`.plnx`)

A plan is exactly the input tuple of `compute_gas_reach_field` plus the marked
points (`models/dive_plan.rs`). The reach field is never stored - loading a plan
means "set the inputs and recompute", so a stale overlay cannot be shown.

Storage is a sibling SQLite file beside the `.stalx` (`persistence/plnx.rs`),
independent of the project database: in-place ACID writes with `synchronous=FULL`,
which suits an incrementally maintained row store (the `.stalx` rewrites
wholesale, so it uses tmp-then-rename - see [saving.md](saving.md)). The file is
stamped once with the project's `created_at`, and ownership is probed before the
store is opened for use, so a `.plnx` belonging to another project is never
written or schema-initialized - the probe reads only the identity row. A project never saved has nowhere to anchor a
sidecar, so plan persistence is refused until its first real save.

On disk, station references are stable `StationKey` anchors (`AfterShot(uuid)`),
not the positional `StationRef` the wire form carries - the invariant merges,
closures and datums already hold. `Project::key_for_ref` canonicalizes on save and
`resolve_key` re-resolves on load, so a saved plan tracks the same physical
station across survey edits.

The LIVE store holds the same guarantee
([#310](https://github.com/Stalagmight/Stalagmight/issues/310)): every pin -
origin, endpoint, way points, marked points, stage-drop overrides - carries an
opaque anchor beside its positional ref, captured through `plan_anchors_for_refs`
and re-resolved through `plan_refs_for_anchors` after every mutating command and
undo/redo, *before* the state refetch, so the reach field never recomputes on a
stale ref. A pin whose anchor no longer resolves is kept and flagged, never
retargeted: a flagged way point stalls the compute client-side, the others are
omitted from the request, and Save refuses while any flag is up. A capture never
re-anchors a flagged slot - only a remap that resolves the original anchor (a
survey undo) or the user re-placing the pin unlocks it. Reversing a section
needs no special-cased pin mirroring; the anchor round trip covers it.

The two directions differ deliberately. Save **refuses** a way point or marked
point that no longer anchors, telling the user to remove it: dropping it would be
silent data loss, since the frontend rebaselines its dirty check on the snapshot
it sent and the plan would read "saved" while the file held a shorter route. Load
**drops** dangling entries and counts each into `stale_ref_count`, which the bar
surfaces ("N stations this plan referenced no longer exist"); a dangling origin or
endpoint nulls to its default instead. Dropping is also what keeps a loaded plan
computable - a dead way point would stall the field on load. Load re-applies the
caps too (truncating, not refusing), because the threat they exist for is a
hand-edited file, which never crosses the write path.

Dirty tracking is frontend-side: `divePlanSnapshotOf` builds the snapshot from the
live store and `selectIsPlanDirty` deep-compares it against the baseline set at
load or last successful save, so anything added to that helper is dirty-tracked
and close-guarded for free. An unsaved exploration is never dirty; the plan
survives Survey/Plan/Export switches and resets only on project load.

## IPC surface

| Command | Purpose |
|---|---|
| `compute_gas_reach_field` | The whole plan in one call: reach mask, gas-to-reach, turn frontier, route, stage drops, gauges, route metrics. Re-runs on every input change, no debounce. Re-validates diver and cylinders through the model constructors, normalizes the gas config, rejects more than 64 way points and an over-long segment list (`validate_plan_inputs`), runs on `spawn_blocking`. `Ok(None)` when no project is open. |
| `list_dive_plans` | Saved plans beside the open project, newest activity first. Empty (not an error) with no project, no `.plnx`, or a foreign one. |
| `load_dive_plan` | One plan, anchors re-resolved, `stale_ref_count` reported. |
| `save_dive_plan` | Insert (`id: None`) or overwrite; canonicalizes refs to anchors first. |
| `delete_dive_plan` | Remove one plan. |
| `plan_anchors_for_refs` / `plan_refs_for_anchors` | The live-pin anchor round trip (#310): read-only, batched, capped. |
| `export_dive_profile` / `render_dive_profile_text` | The Profile tab's XML / plain-text exports (#299). A second door into the engine, so they share `validate_plan_inputs` + `validate_plan_segments` with `compute_gas_reach_field`. |

Frontend seams, all under `src/`: `store/gasPlanStore.ts` (plan inputs,
runtime-only), `components/SingleSystemView/GasPlanPanel.tsx` (setup sidebar +
readout), its `DivePlanBar.tsx` (save/load/delete); the bottom Plan dock's
`PlanTablesPanel.tsx` with `CylindersTable.tsx` (the Cylinders tab, whose
add-cylinder + conservative-gas controls ride in the tab strip) +
`DpvRigEditor.tsx` (the DPV tab), `SegmentsTable.tsx` and `ProfileChart.tsx` -
both tables render
on the shared `SpreadsheetGrid` (#403), with column defs in
`segmentsGridColumns.tsx` / `cylindersGridColumns.tsx` and the pure edit
reducers (unit conversion, clamps, the scooter coupling) in
`segmentsGridEdits.ts` / `cylindersGridEdits.ts`;
`components/Map/MapCanvas.tsx` (fetch, turn dots, draggable drops, final-turn
marker), `components/Map/useGasPlanPointMarkers.ts` (datum stars, way points,
marked points), `components/Map/useGasPlanDropMarkers.ts` (stage drops + the
scooter park marker), `components/Map/GasLegend.tsx`, `utils/gasReach.ts` (the
overlay ramp), and `utils/segmentColors.ts` + `utils/dpv.ts` (segment identity
colors; the rig editor's Rust-mirroring helpers).

Engine semantics are pinned by 112 tests in `gas/tests.rs`. Before changing it,
read `gas_reach_field_no_waypoints_is_identical_to_the_plain_field`,
`gas_reach_field_out_of_budget_endpoint_turns_early_on_its_own_branch`,
`gas_reach_field_override_to_default_station_is_a_no_op` and
`gas_reach_field_drop_pinned_on_the_outbound_chain_cannot_inflate_the_budget` -
each pins a rule that reads like an implementation detail and is not.

## Known limits

- [#311](https://github.com/Stalagmight/Stalagmight/issues/311) - the stage-drop
  marker's keyboard nudge and its `aria-valuenow` re-derive the route position by
  first match, so on a way-point route that doubles back they act on the wrong
  pass.
- The exit leg is never costed into the budget - the strategy's reserve pays for
  the way home. The #299 profile draws the return at `swim_speed_m_per_min` for
  swum spans and at the span's own pace for scootered ones (the mandatory backup
  rides the same line home - the same assumption the one-way range cap already
  makes). A scootered exit mirrors the entry span for span, so `gas_out ==
  gas_in` and the reserves stay correct; the flip side is that a plan whose
  out-speed genuinely differs from its in-speed on a span has no way to say so
  (noted on `DpvRig`).
  `stressed_rmv_multiplier` and the two `reserve_profile.recalc_hold_*` pressures
  are still carried in the config for later phases; nothing computes with them.
- `Team` and `PlanMode` (open-circuit vs CCR bailout) exist in `gas/model.rs` so
  the multi-diver and CCR phases need no migration, but the entry point takes one
  diver and its cylinders. Everything shipped is single-diver open-circuit.
