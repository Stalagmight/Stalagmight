# Dive and gas planning (Plan mode)

Plan mode answers one question: on this gas, starting here, how far into the cave
can the diver get before they have to turn? Everything else - the route line, the
turn dots, the stage drops, the pressure gauges - is a view onto that answer.

Plan is one of the three workspace modes, read-only over the survey
([SPEC.md 6.1](SPEC.md#61-workspace-modes)). The engine is
`src-tauri/crates/models/src/project/gas.rs` (+ `gas/model.rs`, `gas/physics.rs`,
`gas/pinned.rs`),
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

`penetration_budget` returns the litres a diver may burn on the way in. Every
strategy is a rule on the **gauge** (#576): it names the pressure each cylinder
is turned on or dropped at, and the budget credits the gas between the cylinder's
real fill and that pressure. Gas is always real-gas corrected: `deliverable_gas`
divides by `air_z_factor`, a quadratic fitted to the conservative end of
tabulated air Z (1.045 at 207 bar) and clamped to `>= 1.0` so the derate can
never become a credit. One helper (`in_usable_bar`) gives each cylinder's usable
bar, and the budget (`gauge_window_litres`: the Z-corrected gas from `fill` down
to `fill - usable`), the back-gas turn gauge, the stage drop gauges and the
per-cylinder usable column all read it, so the turn the diver obeys and the reach
the map draws describe the same gas by construction.

With `conservative_tank_gas` on (the default) every share is taken of the
cylinder's basis `min(fill, rated)` - the "call-it-an-80" convention - so an
overfill never earns a bigger share; the gas is still counted from the real
fill. With it off the basis is the fill.

Only `back_gas` and `stage` roles are penetration gas; `dil_out`, `bailout` and
`deco` are excluded everywhere. A cylinder with `enabled = false` is filtered out
before anything is computed - budget, gauges, drops - so a diver can toggle rig
variants without deleting rows.

| Strategy (`ReservePreset`) | Back gas: bar spent | Stage: bar spent | Stage drops |
|---|---|---|---|
| `all_thirds` (default) | basis/3, less the safety margin | same rule for stages | none |
| `one_stage` (Half Minus) | basis/4 | stage 1: (basis - reserve)/2 | 1 |
| `two_stage` (Half Minus) | basis/6 | first two stages: (basis - reserve)/2 | 2 |
| `free` | the diver's own `usable_gas_bar` | same | one per stage with usable gas |

The turn (back gas) or drop (stage) pressure is `fill - bar spent`, clamped to
`[0, fill]`. A 12 L tank at 207 bar on thirds turns at 207 - 69 = 138 bar and is
credited the 739.8 L between 207 and 138 bar. Because the top of a tank holds less
gas per bar than the bottom (Z rises with pressure), a third of the gauge holds
less than a third of the gas: the volume rule this replaced credited 792.4 L and
showed a 141.0-bar gauge that agreed with neither. Below about 96 bar, where Z is
1, gas is linear in pressure and the two rules coincide; above it the gauge rule
is shorter (about 6.6% at 207 bar, 8.6% at 232 and 15% at 300 for a tank at its
rating). An overfill under the cap is turned at `fill - rated/3`: 12 L rated 207
filled to 250 turns at 181 bar and is credited the 681 L between 250 and 181.

**What changed for the diver** (#576, decided "thirds of the gauge"): the turn
gauge now reads the hand third (a third of the gauge off the fill), and above
about 96 bar that is lower than the gauge before #576 showed - a 12 L tank at
207 bar turns at 138 bar (141 before), one at 300 bar at 200 (211 before). A diver
who turns on the new gauge breathes a little more before turning than on the old
one (706 -> 740 L at 207 bar, +4.7%; 801 -> 909 L at 300 bar, +13.5%). Every tank
still keeps at least two thirds of its real gas at the turn - a third of the gauge
holds less than a third of the gas, so the thirds hold with gas to spare - and the
reach the map draws only shrinks: it is credited the gas between the fill and the
new turn, not the old volume third.

Half Minus, in diver terms: the stage carries the penetration, so you dip less of
your back gas. A stage spends half of what its basis holds above the reserve,
`(basis - reserve)/2`, on the way in - it is dropped at `fill - (basis -
reserve)/2`, which is `(fill + reserve)/2` for a stage at or below its rating -
and the other half is breathed back down to `reserve` on the exit after you pick
it up. An overfilled stage under the cap spends the same bar as a rated one,
read off its real gauge: 11 L rated 207 filled to 230 with a 30-bar reserve is
dropped at 230 - 88.5 = 141.5 bar. `reserve` is
`reserve_profile.stage_reserve_bar`: default 30 bar, editable in the panel,
capped at 100 by `normalize`. Stages are taken in rig order and only the first
`required_stages` of them count - there is no per-stage drop-order field, and
ignoring a spare stage only shrinks the budget.

The All-thirds safety margin (`all_thirds_safety_margin_bar`, default 0) raises
every budget cylinder's turn pressure by that many bar on top of thirds (12 L at
207 bar with a 10-bar margin turns at 148); it only shrinks usable gas, so
`normalize` caps a large value and resets a negative one. Free hands the diver
the whole calculation: they type a usable-bar allowance per cylinder and the
engine budgets exactly the Z-derated span from the real fill down to
`fill - usable` - the pressure the turn gauge and a Free stage's drop show. The
usable bar is clamped to `[0, capped fill]` so an oversized value cannot credit
gas the tank does not hold; under the conservative cap that ceiling is the rated
pressure. A Free allowance larger than the fill spends the whole tank and leaves
no reserve (with the cap on, an overfill still keeps the bar above its rating) -
it is the diver's own setting, accepted as is (#576 RV9a-5-D2;
`gas_free_clamps_usable_over_fill`). The window is never slid down to `[rated - usable, rated]`: the same
usable bar holds less gas at an overfill than at rated (12 L rated 207 bar,
filled to 250, 100 bar usable: 1018 L between 250 and 150 bar, against 1096 L
between 207 and 107). An overfill never buys extra penetration, and switching the
cap off never credits a tank less.

**A lower fill can hold more gas** (#576 RV9a-5, decided "keep physics,
document"). Where a rule spends a set number of bar whatever the fill - a Free
allowance below the fill, with the cap on or off, or the share of the rating an
overfill is held to under the cap - a lower fill takes that bar lower in the
cylinder, where Z is smaller and a bar holds more gas. So a lower fill can show a
slightly larger budget, and a little more reach. The 12 L tank rated 207 on
thirds turns at 181 bar filled to 250 and is credited 681.3 L; filled to its
rating it turns at 138 and is credited 739.8 L (+8.6%). With a 100-bar Free
allowance the same tank holds 1018.0 L at 250 bar and 1096.2 L at 207 (+7.7%).
The review's sweep found rises of up to +9.8%, a Free plan whose back gas went
from 313.8 to 179.0 bar: 3979 -> 4371 L (that tank's 184-bar allowance is more
than the lower fill, so there it spends the whole tank).

This is the real gas, not a rule bending. Each budget is the decided share of its
own gauge: the bar the rule names, taken off that fill. The turn or drop gauge
still reads the fill less that bar, and the reserve the diver keeps - the gas
below that gauge, in bar as the rule sets it - is never credited, so the extra
litres are never reserve handed to the budget. Elsewhere (thirds, quarters and
sixths at or below the rating or without the cap, Half Minus stages at or below
it, a Free allowance that takes the whole fill) the bar spent falls with the fill
and the budget with it. The pin sweeps in `gas/pin_monotone_tests.rs` are scoped
accordingly: they do not claim a lower fill never shows more reach; they assert
that a pin never adds what a lowered input takes away - the pinned budget rises
by no more than the plan without pins does - and the tree sweep, whose fills stay
out of these windows, that it never rises at all
(`a_lower_fill_can_be_credited_more_under_a_free_allowance_or_an_overfill_under_the_cap`
pins the numbers above). One exception, also only in these windows (#576
RV10a-1): the lower fill raises a stage's safe gas, which can carry its default
drop onto a deeper pin. The pin then reproduces the default - a pin on its
default station is the default - and the next stage starts at the default drop
instead of at the pin, so it is credited the route between the two. There the
pinned budget can rise by more than the plan without pins' does - on the review's
reseeded loop sweep (seed 0x5EED0004) 2222.0 -> 2630.6 L, against 2564.7 -> 2630.6
L without the pins - but never above that plan's budget, which the sweeps assert
for every plan, lowered or not; such a lowering is counted, not set aside. The
engine is unchanged: the lower fill's plan is the one its own rules give.

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
pin says how much of that stage is breathed in - the route's cumulative gas at
the pinned station, minus the cumulative already assigned to earlier drops,
clamped to `[0, tank empty]`. **A moved drop never buys reach**
([#576](https://github.com/Stalagmight/Stalagmight/issues/576)): the plan credits
the stage that amount capped at the strategy's own safe IN gas, so it recomputes
on `base_budget + sum(min(breathed, safe) - safe)` and can never reach further
than the same plan without the moved drops. The pins are re-checked on every
recompute, so this holds whatever moved under a saved pin since it was set.

A pinned drop is reported at the station the diver pinned (#576 NF-W12GAS2-1,
decided D1): on a merged station the route can pass the pin's zero-cost twin just
before the pin, at the same gas, and the drop is still named, drawn and exported
(panel, map marker, profile, slate) at the pin's own pass, not at the twin.

**A pinned drop decides the route** (#576 RV8c-2, decided D1-D6; `gas/pinned.rs`).
A drop the diver pinned is a place the dive goes, so the route passes it the way it
passes a way point:

- **D1, no endpoint**: out through every pin, in drop order, then on to the
  deepest station *beyond* the last pin - a station a shortest path from the
  previous point reaches through the pin, never one only a backtrack reaches.
  The deepest station anywhere is no longer a candidate.
- **D2, with an endpoint**: through the pins first, then on to the endpoint. A pin
  off the endpoint's passage is visited and the dive doubles back from it; the
  backtrack is charged like any other leg (two pins on different branches: drop 1's
  branch, back, drop 2's branch). Out of budget, the dive turns at the last
  station it can afford on that route, as toward any endpoint.
- **D3, the gas runs out first**: when the budget (or the scooter range) ends
  before a pin, the dive turns at the last station it can afford on the way there.
  The stage is carried the whole dive, the drop is reported at its pin past the
  turnaround - its `stage_drop_route_indices` entry is beyond the route's last
  station - and is never flagged over-reserve (the marker and the readout apply
  the same rule); the map draws it grey, the legend keys it, and the readout says
  "not reached" (`utils/gasReach.ts` `stageDropReached`). Its keyboard slider sits
  one step past the turnaround: Left pulls the pin onto the turnaround, Right has
  nowhere deeper to go. The pin is priced as if reached: what it strands stays
  stranded.
- A pin the route already passes before its own turn - on the way-point chain, or
  on the way to an earlier pin (drop 2 moved shallower than drop 1) - stays at that
  first pass: the stage is dropped there full, and the dive is not sent back to it.
- With way points, every pin the way-point chain does not pass is visited after
  the last way point, in drop order (D6: kept, also for a pin that sits between
  two way points on another branch).
- A pin the dive cannot reach from the previous point is ignored, and that stage
  keeps its default drop (#576 FZ-ORACLE-1-D1). The previous point is the last way
  point (or the origin) for the first pin, the previous usable pin for later ones.
  With an origin set, a pin in a part of the cave the origin does not connect to
  was already ignored this way; with no origin the roots are every zero datum, so
  two pins can each sit in a part some datum reaches, and the second one, which
  the dive cannot fly to from the first, is now ignored too instead of leaving the
  map dark.

The route through the pins does not depend on the budget; only where the dive
turns on it does. So the gas each pin has cost is read off the pinned route before
the budget exists - the ordering stays acyclic - and LESS gas can only turn a
pinned plan earlier, never send it somewhere else (the rule this replaced,
NF-PGAS3-1, priced a pin only when the pin-free route happened to pass it: a change
that moved the pin-free route handed the stranded gas back, and the same pinned plan
reached further on less gas). On a scooter the way to each pin is the cheapest gas
its range covers (NF-W12GAS-1-D2, "Which way a scooter dive takes" under Segments
and the DPV), so the route depends on the range, never on the gas: a smaller range
can send the dive to a pin another way, where the pin strands another share of its
stage.

**The overlay shows only where the pinned dive goes** (D4). The reach masks, the
gas-to-reach the map colors by and the turn dots are priced on the route through
the pins, the way a way-point plan prices them: a station the route passes on the
way to the last pin costs what the route has spent at that pass, and every other
station costs the route to the last pin plus the way on from there. So the map
paints only stations a diver who drops at the pins can be at. In the review's
scenario 12178 (stage pinned at B:1, 555.97 L, turn at B:1) the old overlay also
lit A:1, A:2 and C:1 - each within 555.97 L of the entrance, none within reach of
a diver who has swum out to B:1 - and offered turn dots there; now it paints the
entrance and B:1 and offers no dot, since the dive can end nowhere else. A dive
that turns before a pin (D3) paints the way to its turn and nothing more. Every
turn dot is a station the plan really turns at when that dot is clicked as the
endpoint (D2 routes it through the pins first), and the way to a pin is transit,
so it carries no dot, as the way to a way point does not. The pinned route does
not depend on the budget, so less gas only takes stations off the map, and its
gas price for a station is never below the plan without pins, so no pin paints a
station that plan cannot afford on gas.

The scooter range is priced on the same walk (RV9a-1): every station the pinned
dive passes on the way to its turn, and the turn, is painted, and nothing the
range does not reach along that walk is. In the review's seed 0x9a0058be a stage
pinned on a deep loop takes the dive 312 m round it to 0:5, 796 m of a 1484 m
range, where the shallow line - the cheaper gas - is 684 m long: the pinned plan
turns at 0:5 and paints it. Before, the pinned plan kept the other plan's masks,
so its dive turned on a station its own map left dark
(`rv9a_1_a_scootered_dive_pinned_round_a_loop_is_painted_to_its_turn`; the loop
sweep `round_a_loop_less_gas_or_range_never_lets_a_pinned_plan_gain_more_than_without_its_pins`
checks every route to the turn is painted). The plan without pins then still
priced the range along its gas-cheapest way, the line, so it turned at 0:4 on
range and left 0:5 dark - the pin lit a station the plan without it did not. It
now takes the way its range covers (NF-W11GAS-1, see "Which way a scooter dive
takes" under Segments and the DPV), and the pinned walk is one of the ways it
considers, so no pin paints a station the plan without pins leaves dark.

The cost of D4, accepted with the decision: a pin is a way point for the overlay
even when it sits on its own default station, so pinning a drop where the default
already is can change the overlay (stations up another branch now price the way
back from the pin) while the budget and the drop stay those of the default.

What each kind of pin does to the plan:

- **Shallower** than the strategy point: the stage is dropped part-full, the
  surplus stranded, the budget shrinks and the reach shortens; the dive turns
  where the lower budget runs out on the route through the pin.
- **Deeper**: the marker stays where the diver put it and turns red,
  `stage_drop_over_reserve[k]` is set, the panel warns, and the drop gauge reads
  what the stage gauge shows there - breathing the stage that far eats into its
  reserve. The plan does not count that gas: the budget stays that of the
  default drop, and so does the turn for a pin on the pin-free route; the overlay
  can only lose the stations the dive through the pin no longer goes to (D4). Set
  only for a genuine drag deeper
  (`breathed > safe + 1e-6`); past tank-empty the breathed gas clamps, so the
  drop snaps back to the empty point. **The next stage starts at the pin, not
  at that empty point** (D5): the dive carries the dry stage to its pin, and the
  stage behind it is credited only the route from there. Starting it at the
  empty point made a smaller first stage hand more of the route to the next one,
  so less gas raised the budget - in the W9-GAS sweep 18 of 563 such lowerings,
  one TwoStage plan from 1302 to 1805 L, its turn moving on a station
  (`case_791_a_smaller_first_stage_no_longer_hands_the_route_to_the_second`).
  A later default drop is placed from the pin on, so it can land at the
  turnaround.
- **Onto the station the default already sits on**: the default's pressure,
  credit and flag. The default marker sits at the first station whose gas
  *reaches* the threshold, so its gas usually overshoots; scoring that as a fresh
  pin would fabricate an over-reserve flag out of a jiggle. It counts only where
  the threshold really lands on the pinned route: a default clamped onto the
  turnaround (the threshold is past the end of the route) is not the default, and
  a pin there strands the stage's unbreathed gas like any shallow pin - forgiving
  it handed that gas back (RV8c-2, scenario 10172).
- **Off the route** - on another branch, past the end of the route, or past where
  a shrunken plan now turns: D1-D3 above. Reachable in normal use - `.plnx`
  persists overrides, so a plan reloaded after the survey, the endpoint, the way
  points or the rig changed can carry a pin the new route does not pass. Pricing
  such a pin from the passage beyond the turn once credited it up to 1248 L against
  the 698 L plan without it; ignoring it (NF-PGAS3-1) gave back its stranded gas.
  An unresolvable pin, or one in another graph component, is the same as no pin.

Before the cap, a deeper pin extended the reach by the gas breathed into the
reserve. That made a saved pin dangerous to any later input change: a pin left on
the station that WAS the default turned into a deeper drag as soon as the default
moved shallower, and credited the overshoot.

Arrival gas comes from the pinned route's own cumulative gas at the pin's pass -
not from the station's gas-to-reach, which with way points is measured from the
last way point and for a station passed on the way out reports the cost of coming
*back* to it. A moved drop's
displayed pressure is inverted off the Z curve by bisection
(`drop_pressure_for_contribution`) from the stage's real fill - the same gauge
every default drop (`fill - usable`) is read on - so a drop moved to exactly the
default's litres reads the default pressure, and an overfilled stage pinned past
the conservative-capped tank-empty clamp reads the overfill still in it, not 0.
The reach stays exact-litres from the graph.

## Routing

The route is the ordered leg list from the origin through every way point to the
turnaround, populated for any strategy once something is reachable.

**Origin** - a hint. Unset means the zero datums (the entrance); a set origin that
no longer resolves falls back to them rather than blanking the field.

**Endpoint** - a hint: the lead being planned toward. Set it by left-clicking a
station in Plan, by clicking a turn-frontier dot, or from the station context
menu. In budget, it is the turnaround. Out of budget, the plan stays on the
passage the diver chose and turns earlier - the target becomes the last in-budget
station on the path toward it (on a scooter, the way toward it the range covers,
if there is one: "Which way a scooter dive takes" below). Only with no endpoint
at all, or one that is
unusable (deleted under a loaded plan, or in another graph component), does it
fall back to the deepest in-budget station *anywhere*, and `turn_toward_endpoint`
reports which happened. The UI needs that flag because the two read identically
otherwise, and saying "turning at X on the way there" about an unrelated lead is
the failure the walk-toward-the-endpoint rule exists to prevent.

**Way points** - a constraint, not a hint. Each costs one extra Dijkstra seeded at
the previous way point, with leg costs accumulated, so a station's gas is what it
costs *after* flying the whole chain and adding a way point costs real gas rather
than only redrawing a line. On a scooter, a leg whose cheapest way is out of range
when it gets there takes the way its range covers ("Which way a scooter dive
takes" under Segments and the DPV). A way point that cannot be reached, or that no longer
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
swum distance on measured coordinates, while `applyGasRoute` (surveyLayer/auxiliaryData.ts, entered through `SurveyLayer.setGasRoute`) draws
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

Riding a span spends that allowance in **range, not line**: each unit's battery
drains by its own clock, not the odometer. A metre ridden at `speed` costs unit
`u` the fraction `1 / (burn_u x min(speed, cruise_u))` of its battery, and the
span charges the allowance whichever unit that metre drains harder, scaled to
metres: `max_u weaker_range / (burn_u x min(speed, cruise_u))`
(`range_per_metre`, `gas/scooter.rs`; the reserve scales both sides and
cancels). On a matched rig at or above cruise that is a metre per metre. With a
45 m/min x 45 min primary and a 30 m/min x 90 min backup (10% reserve, cap
1822.5 m), a span ridden at 30 m/min costs 1.5 m per metre, so the scootered
line stops at 1215 m - the primary's 45 x 0.9 minutes at 30 m/min. Exact for a
plan whose scootered spans are all limited by the same unit (every one-span
plan); where different spans are limited by different units, the single number
is on the conservative side. So `scooter_distance_m` is range *spent* and can
exceed the line covered. A station
whose route spends more than the usable range drops out of the reach masks, and
`turn_on_scooter_range` reports when the range cap - not gas - is what moved the
turnaround (set from the route target itself, not a cave-global scan). Riding
buys speed, and through it cheaper gas per metre; it does not change the
breathing rate.

**Which way a scooter dive takes** (#576 NF-W11GAS-1, decided "price along the
route"). Gas and range are read off one route, and with a scooter the plan
without pins chooses that route for both: to each station, the cheapest gas
among the ways its range covers (`gas/range_route.rs`). Where the cheapest gas is
in range that is the gas-cheapest way itself, so a plan the range does not bind
is unchanged. Round a loop the cheaper gas can run out of range while the way
round - shorter and deeper, more gas - is in range on gas the diver has, and the
plan takes it. In the review's seed 0x9a0058be the endpoint 0:6 is 684 m along a
shallow line to 0:5 (1744 m of a 1484 m range there) or 312 m round a deep loop:
the plan used to turn at 0:4 on range, 1158 m spent; it now goes round the loop
to 0:6 on 2410 of its 2524 L and 1089 m of range, and paints the loop and 0:5
(`nf_w11gas_1_a_scootered_dive_without_pins_takes_the_way_round_the_loop_its_range_covers`).
Each station is priced on that way - its gas-to-reach is the way round's, more
than the line's - so the colour and the max-penetration readout describe the
dive the plan flies, and `scooter_distance_m` is the range the route to the turn
spends. An endpoint out of budget on its in-range way is approached along that
way, turning where the gas runs out; one no way reaches in range is approached
as before, along the cheapest gas, turning where the range or the gas runs out.

Every leg takes its way by the same rule (#576 NF-W12GAS-1, decided D2 "extend
to all legs"). The open part of the route - after the last way point, or after a
pinned plan's last pin - is priced per station as above. A leg to a way point,
or to a pin (the pinned walk visits its pins like way points), commits one way to
its end: the gas-cheapest one where that is in range when the leg gets there,
else the cheapest gas among the ways in range, priced along it - the chain's gas,
range and time, `waypoint_gas_litres` and the pin's arrival gas are that way's.
With the seed's way point at 0:5 the leg used to take the line, run out of range
on it and leave the plan without a turnaround; it now goes round the loop, and
the dive is the one without the way point
(`nf_w12gas_1_a_way_point_leg_round_a_loop_takes_the_way_its_range_covers`). With
the stage pinned at 0:5 the pinned dive used to turn at 0:4 on range, the drop
not reached; it now reaches the pin round the loop and turns at 0:6
(`nf_w12gas_1_a_pin_leg_round_a_loop_takes_the_way_its_range_covers`). A leg no
way reaches in range keeps the gas-cheapest way, and the plan turns where the
range runs out on it, as before. A leg is chosen on its own, given what the legs
before it have spent, not together with the legs after it (Known limits).

The search keeps at most 16 ways per station - a cave survey is mostly a tree, so
this binds only in a maze - and a way it does not keep can only turn the dive
earlier, never later: every price is that of a real route. A pinned plan flies
its own walk (D4); its legs to the last way point are the plan without pins' own,
and the rest of it is one of the ways that plan's open part considers, so no pin
paints a station the plan without pins leaves dark. With less range the way the
range covers can cost more gas, so a range lowering can turn the dive on more
route gas, to the same station or one painted before, within the budget and the
lowered range. It can also send a leg to a way point or a pin another way: the
plan is then another dive - it passes other stations, a pin reached on more gas
strands less of its stage, so the pinned budget can rise (never above the plan
without pins'), and with less range spent on the way there it can reach further
after it. Each plan is still checked on its own: within the budget and the range,
nothing lit that the plan without pins leaves dark. The pin sweeps in
`gas/pin_monotone_tests.rs` assert this for every plan, lowered or not, and check
every scootered turn's range against the route itself (its shots' line times the
range a metre costs); each runs on its own seed and on three more
(`RESEEDS`), with 0 violations on all. On the loop sweep's seed: 0 stations lit by
a pin that the plan without pins leaves dark (375 before NF-W11GAS-1), 1133
stations painted on the way round, 12859 turns checked against their route (859
past a way point, 2513 on a route the pins decide), 36 range lowerings sending a
leg another way. Two sweeps draw scooter plans with one or two way points, half of
them on the line past the loop: round a loop 3479 turns past a way point and 457
on a route the pins decide checked against their route, 57 range lowerings
sending a leg another way; in tree caves 1306 and 179, and no leg ever takes
another way (one way to each station). The way a leg takes is checked against
every simple route too: on 1236 small loop caves without way points, every
station's paint and price
(`without_pins_a_scooter_plan_paints_each_station_at_the_cheapest_gas_its_range_covers`),
and on 1984 with one way point, the gas the way point is reached on - 71 of them
round the loop, 662 with no way in range
(`a_way_point_leg_takes_the_cheapest_gas_its_range_covers`).

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

Engine semantics are pinned by `gas/tests.rs` and the topic test modules beside
it (`gas/*_tests.rs`); run them with `cargo test -p stalagmight-models --lib
project::gas`. Before changing the engine, read `gas_reach_field_no_waypoints_is_identical_to_the_plain_field`,
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
- A scooter leg to a way point or a pin takes the cheapest gas its range covers
  given the legs before it, not together with the legs after it (NF-W12GAS-1-D2):
  a cheap-gas way that spends most of the range can leave a later leg - or the
  open part after the last point - short, where a dearer, shorter way would not.
  The plan then turns where the range runs out, never later; but with more range
  the same plan can show less than with a little less, which takes the shorter
  way.
- `Team` and `PlanMode` (open-circuit vs CCR bailout) exist in `gas/model.rs` so
  the multi-diver and CCR phases need no migration, but the entry point takes one
  diver and its cylinders. Everything shipped is single-diver open-circuit.
