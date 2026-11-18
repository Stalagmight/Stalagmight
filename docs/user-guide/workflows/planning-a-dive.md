---
title: Planning a dive
description: "Plan a cave dive on your survey in Plan mode: choose a start, set cylinders and a gas strategy, and see how far your gas reaches and where to turn."
order: 12
feature: planning
---

# Planning a dive

Plan mode answers one question: on this gas, starting here, how far into the
cave can you go before you have to turn? It colours the survey by the gas it
takes to reach each station, marks the turnaround, and gives you the turn
pressure.

1. Click **Plan** in the mode switcher in the middle of the top bar.
2. Answer **Where does this dive start?**: click **★ Start from entrance**,
   or click a station on the map.
3. Check your cylinders in the **Cylinders** tab below the map, and your
   **Gas strategy** in the panel on the right.
4. Read the result: the coloured reach on the map, the turnaround marker, and
   the **Plan** readout in the panel.
5. To plan toward a particular lead, click its station: it becomes the
   endpoint.

> [!WARNING]
> Plan mode is an informational aid, not a dive plan to trust blindly, and it
> does not replace dive planning or gas management training. It has no
> decompression model, plans one open-circuit diver, and is only as good as
> the survey under it.

## Plan mode is read-only

The survey cannot change while you plan. The tools that edit the survey are
hidden, the Sections tab is hidden, and where the project has loop closures or
GPS fixes the map shows the Closures Computed View. Click **Survey** to go back to editing.
Your plan stays as it is when you switch modes; it is cleared when you open
another project.

## Choosing where the dive starts

**★ Start from entrance** starts from every zero datum marked **Entrance**,
each station counting from whichever entrance is closest in gas. See
[Zero datums and GPS fixes](../concepts/zero-datums-and-gps-fixes.md#entrance).
If the project has no zero datum, the panel warns that every station would be
unreachable: [set a zero datum](setting-a-zero-datum.md), or start from a
station instead.

To start somewhere else, click a station before choosing, click the star
drawn on any zero datum, or right-click a station and pick **Set as start
point**. Later, the **Origin** section offers **Pick on map** and **Reset**
(back to the entrance).

## Gas setup

**Cylinders tab** (below the map). The plan starts with two AL80s as back gas.
Add a cylinder from **Add cylinder...**, set **Fill** and **Role**, and untick
**On** to leave a cylinder out without deleting it. Only **Back gas** and
**Stage** count toward the gas you may use going in; **Dil-out**, **Bailout**
and **Deco** are not counted. **Conservative gas** (on by default) counts an
overfilled cylinder only at its rated pressure. There is no gas mix setting.

**Panel on the right** (also under **Adjust assumptions** before you choose a
start):

- **RMV (surface air consumption)** and **Swim speed**.
- **Gas strategy**:
  - **All thirds**: a third of each cylinder goes in. **Safety margin** holds
    back extra pressure from every cylinder.
  - **Half Minus: 1 stage** and **Half Minus: 2 stages**: each stage is
    breathed on the way in to halfway between full and **Stage reserve**
    (30 bar by default), then dropped, and less of the back gas is used.
    These need that many **Stage** cylinders that are switched on.
  - **Free**: you type the **Usable** pressure for each cylinder in the
    Cylinders tab.
  The first time you pick Half Minus or Free after opening a project, the
  panel asks you to confirm: "Use only if you're trained to balance gas across back gas and
  stages."
- **Units**: **Metric** or **Imperial**.

Only the way in is costed. The gas the strategy holds back is what brings you
home. **Settings > Gas planning** holds the default swim speed, the **Jump RMV
multiplier** (a jump costs more gas than a normal shot, 1.5 times by default),
and **Route through reference ties** (off: reference shots are not swum).

## Route, endpoint and way points

- **Endpoint**: click a station, or right-click it and pick **Set as
  endpoint**. Without one, the plan turns at the station farthest in budget.
  If the endpoint is out of reach, the plan turns early on the way there and
  says where.
- **Way points**: right-click a station and pick **Set as way point**. The
  dive visits them in order on the way to the endpoint, and their gas is paid
  for. Reorder or remove them in the **Route** section, or click a way point
  on the map to remove it. A way point that cannot be reached stops the plan
  until you remove it.
- **Marked points**: right-click a station and pick **Set marked point** to
  note a spot, with an optional note. They do not change the route or the gas.

## Turn pressure and stage drops

The **Plan** readout gives the **Backgas turnaround pressure** (or the
**Backgas switch pressure** when you drop stages), each **Stage N drop**
pressure, and the **Distance to turnaround**, **Max depth**, **Average depth**
and **One-way runtime** of the route. The turnaround is a U-turn marker on the
map.

With a staged strategy, each stage has a drop marker on the route. Drag it,
or select it and use the arrow keys, to move it along the route; right-click a
route station and pick **Set as stage drop** to put it there. Delete, a
double-click or **reset** in the readout puts it back. Moving a drop shallower
shortens the reach. Moving it deeper breathes into the stage reserve: the
marker turns red and the panel warns you.

## Scooters (DPV)

1. In the **DPV** tab, switch the rig on and set **Primary speed**, **Primary
   burn time**, **Backup speed**, **Backup burn time** and **Range reserve
   (%)**. **Usable range** shows the one-way range the rig gives you. A rig
   is always a primary plus a backup.
2. In the **Segments** tab, the way points split the route into legs. Tick
   **DPV** on the legs you ride; a leg without a speed of its own then takes
   the primary speed. Type a speed to give any leg its own pace.

The map marks where the scooters are parked, and the panel shows **Scooter
range used** and says when the scooter range, not gas, turns the dive.

## Depth profile

The **Profile** tab charts depth against **Distance** or **Time**, out to the
turnaround and back, with the turnaround, stage drops and way points marked.
**Export text** copies it as a table for a slate; **Export XML** writes a file
that Subsurface can import. Neither is a decompression schedule.

## Reading the map

The **Gas reach** legend explains the colours:

- Gold near the start to purple at the turn budget: within your gas.
- Faded: connected, but beyond your gas or scooter range.
- Grey: not connected to the start at all.

The planned route is drawn on top, coloured by leg, with jump legs drawn
differently.

## What is saved where

- Named plans are saved in a plan file next to the project (`<project
  name>.plnx` beside `<project name>.stalx`), never in the project itself. Use
  the bar at the top of the panel: **Save as...**, type a **Plan name**,
  **Save**; pick a plan from **Load a saved plan...**; **Delete**. Save the
  project once before saving plans.
- A saved plan keeps the start, endpoint, way points, marked points, stage
  drops, segments, cylinders and gas settings. It follows its stations through
  later survey edits; if one was deleted, the plan says so when it loads.
- An **Unsaved** badge shows changes to a loaded plan, and Stalagmight asks
  before you quit or load another plan over them. A plan you never saved is
  lost when you open another project or quit.
- Swim speed and the DPV rig are also kept in your settings for the next plan.
