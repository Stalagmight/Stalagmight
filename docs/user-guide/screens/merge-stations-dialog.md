---
title: Merge Stations dialog
description: "The Merge Stations dialog confirms the two stations to merge, says which piece of the survey moves and by how much, and when a merge is refused."
order: 13
---

# Merge Stations dialog

Confirms which two stations get merged. Opens after the second click with the
**Merge** tool, see [Merging stations](../workflows/merging-stations.md).

![Merge Stations dialog](../images/merge-dialog.png)

1. **Close**: nothing merged.
2. **Station A**: the first station clicked, and its section.
3. **Station B**: the second station clicked, and its section.
4. **Cancel**: same as 1.
5. **Merge**

When both pieces are already placed by their own
[zero datum](../concepts/zero-datums-and-gps-fixes.md), a note under the
stations says which piece moves, how far it moves to meet the other one, and
that its zero datum becomes a GPS fix.

## Details

- One side moves onto the other, together with everything already merged to
  it:
  - Only one side has a zero datum: that side stays put and the other moves.
  - Neither has one: station A's side moves onto station B.
  - Both have one: one piece keeps its place and the other moves, as the note
    says. The zero datum of the piece that moves stays, as a
    [GPS fix](../concepts/zero-datums-and-gps-fixes.md): it no longer moves the
    Raw Data View, and the Closures Computed View bends the survey toward it.
- When the merge comes from dragging a station onto another with a move tool,
  the note says that the piece moves, but not how far.
- If a loop closure joined the two stations you merge, the merge removes it and
  says so.
- **Undo** reverses a merge, and brings back a loop closure it removed.

The dialog does not open when:

- The two stations are already merged, or a loop closure already joins them. A
  notification says so.
- Their sections are already connected: they are on one section, or joined
  through other merges and loop closures. Merging would make a loop, so the
  [Create Loop Closure dialog](closure-dialog.md) opens instead.
