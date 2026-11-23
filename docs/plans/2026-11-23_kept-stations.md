# Kept and archived stations (RS)

Status: design, 2026-11-23. Lands in #827 before `.stalx` version 2 ships. It
replaces U1 (per-line junction names, see the station-graph plan's Deviations
"package U1"); every U1 rule this page does not restate is withdrawn.

## 1. What the user asked for

1. Every link pill takes the colour of the line it is part of, with a colour dot
   instead of the entity icon, legible for any line colour (done: `rs/P-pills`).
2. Pill typography: the context ("Main Line:") smaller and muted, the main part
   at the regular size; station pills end with the Station ID as a muted
   superscript (done: `rs/P-pills`).
3. A merge asks which station is **kept**. The kept station is the one shown
   everywhere; the other stations are shown nowhere. A shot on a line whose own
   station was not kept shows the kept station's name with a colour dot that
   says it belongs to another line. The same applies to loop closures.
4. The stations that were not kept are **archived**, not deleted: id, name and
   comment are kept. An archived name does not count toward name uniqueness, so
   the user can reuse it on the line it came from. Unmerging brings the archived
   station back with its name and comment; if its name is now taken on its line,
   the unmerge dialog asks for a new one.

Decisions the user took when asked (2026-11-23):

- It lands in #827, so version 2 ships once, with these semantics.
- Closures: "same as merge". Creating a closure asks which end is kept; the
  other end is archived (its name and Station ID hidden, its name free for
  reuse); in the Raw Data View its point stays on the map without a label;
  Remove Closure restores it, with the rename prompt on a collision.
- Upgrade rule (0.6.0 files, and junctions in version 2 files written by earlier
  #827 builds): keep **the station that is part of the longest line**.

## 2. Terms

- **Node**: a `Station` record, a vertex of the survey graph. Geometry, shots,
  datums, placements, hand moves and closure ends refer to node ids. Nothing in
  this design changes a node's geometry.
- **Identity**: `(id, name, comment, external)`. A node shows one identity.
- **Archived record**: an identity a merge put away inside the node, with the
  lines it was the own station of.
- **Native line** of a node: a line through it that no archived record of the
  node names.
- **Home line** of a node: its first native line in line-list order (the
  Sections panel order).
- **Shown identity** of a node: its own identity, unless a closure archives the
  node, in which case the shown identity of the closure's kept end (section 5).
- **Foreign slot**: a slot whose line is not the home line of the slot's shown
  identity. A foreign slot shows the shown identity's name with a dot in the
  home line's colour.
- **Rule K** (the keep rule): of two or more candidates, keep the one whose
  longest native line is longest (line length as line details show it: the sum
  of the line's measured shot lengths, splays excluded as there); ties go to the
  lowest Station ID.

## 3. Model

```text
Station { id, name, comment, external, archived: Vec<ArchivedStation> }
ArchivedStation { id, name, comment, external, lines: Vec<LineId> }
LoopClosure { id, a, a_via, b, b_via, strategy, archived: Option<End> }  // End = A | B
```

`StationAlias` and `Station::aliases` are deleted.

Invariants (new or replaced; numbered for tests and validate):

- **A1** Archived ids are unique across live and archived ids, below 2^31 and
  below the station counter (replaces the alias-id rules).
- **A2** Every line an archived record names passes the node (stale lines are
  cleared by the build epilogue, as alias lines are today).
- **A3** A line is named by at most one archived record of a node.
- **A4** A node with a slot has at least one native line.
- **A5** An archived record with no lines is history only: it reserves its id
  and is never shown or restored.
- **C1** A node is the archived end of at most one closure.
- **C2** Following "archived end -> kept end" never cycles (a forest).
- **N8** (replaces invariant 8) On each line, slots that show **different**
  shown identities have different names. Two slots of one shown identity on a
  line (a same-line closure) are not a collision. Archived names and the own
  name of a closure-archived node count nowhere.

## 4. Merge, detach, delete

### 4.1 Merge

`merge_stations(a, b, keep, kept_name: Option<String>)`, `keep` in `{a, b}`.

1. Geometry exactly as today: `merge_direction` gives `(stationary, moving)`,
   `merge_ids(stationary, moving)` folds the moving node into the stationary one
   (placements, datums and anchors keyed as today).
2. `merge_ids` archives the retired node **whole**: one record `{id, name,
   comment, external, lines: its native lines}`; the retired node's own archived
   records move into the survivor's list as they are. Nothing is minted, and no
   comment is joined (`CommentRule::Join` is deleted).
3. If `keep == moving`, the merged node is re-identified in the same op: it takes
   the moving identity, and the stationary identity becomes a record with the
   stationary node's pre-merge native lines. The op passes
   `EditContext::with_reidentified(&[(stationary, moving)])` so that pre-edit
   positions, placements and datum origins resolve as today (inventory answer
   (a); the same mechanism `restore_alias_identities` uses).
4. `kept_name`, when given, renames the kept identity in the same op.
5. Refusals, added to today's: the kept name (or `kept_name`) collides by N8 on
   any line it will be shown on -> `OpError::NameTaken { line, name }`. The
   preview reports every collision and a free suggestion
   (`uniquify_station_name`), so the dialog can ask before the user confirms.

The preview (`merge_preview(a, b, keep)`) adds: the kept and archived identity,
the lines whose shown name changes, the collisions, the suggestion, and the
default keep (rule K).

**Merges without a dialog** (`create_line_and_merge`, import joins
`FromExisting` / `ManualSnap`, the import dialog's create-line-and-merge, orphan
placement): keep the station that was already in the project (the one the user
targeted). A station of the new line whose name collides with the kept name on
that line takes a letter suffix (`12` -> `12a`), named in the import receipt,
as U1 did for a line without an alias. If both stations predate the action
(orphan placement), apply rule K; if the kept name collides, keep the other; if
both collide, the kept name takes a letter suffix and the op reports it.

### 4.2 Detach Line from Station (unmerge)

`detach_line(station, line, renames: Vec<(StationId, String)>, rest_keep:
Option<StationId>)`.

- **The line has an archived record R at the node.** The line gets a new node
  with R's identity (id, name, comment, external); R leaves the node. When R
  names other lines too (a split half), R stays for them and the detached line
  gets a freshly minted id with R's name and comment.
- **The line is native and every other line is covered by records**
  (`detach_keeps`): the line keeps the node's identity. The remaining lines get a
  new node whose identity is the record covering them; when several records
  remain, `rest_keep` picks one (default rule K) and the others stay archived in
  the new node.
- **The line is native and another line is native too** (a Split or Create Line
  junction): the line gets a fresh station with the same name and no comment
  (today's behaviour).
- Datums, closure ends and hand moves move by `via` as today; a closure's
  `archived` marker follows its end.
- Every identity shown anew is checked by N8 on the lines it will be shown on.
  A collision without a rename in `renames` refuses with `NameTaken`; the preview
  lists the identities shown anew, their lines, the collisions and suggestions,
  and the dialog asks for each.

### 4.3 Deletes and the epilogue

When an edit leaves a node whose native lines are all gone, the epilogue
(`restore_alias_identities`, renamed `restore_archived_identities`) makes the
record covering the remaining lines the node's identity (rule K among several);
the old identity becomes a history record (A5). A node with no slots left goes
with its records. A restored name that collides by N8 takes a letter suffix and
the op reports it (no dialog on these paths). The delete previews list such
restores.

### 4.4 Split, ranges, combine, append

- **Split Line Here** shares the node (native on both halves, no record). A
  record naming the split line at the cut station gains the second half (no
  minted copy). Records of stations that move to the second half are repointed.
- **Range moves** repoint records like `via` hints.
- **Combine Lines**: the join keeps the kept line's join station; the other join
  station becomes a history record (its line is absorbed). Combine keeps its
  comment pick (Kept / Other / Both): the user chooses it explicitly in that
  dialog and the other identity can never come back. A torn junction becomes a
  closure whose archived end is the fresh station (kept end: the junction that
  stays), so the torn slot shows the junction's name with a dot.
- **Add .stalx** shifts archived ids and remaps their lines; closures keep their
  archived end.

## 5. Closures

### 5.1 Shown identity

`shown(node)` = `shown(kept end of c)` when closure `c` archives `node`, else
`node`. C1 and C2 make this terminate. Everything that names a node for the user
(labels, pills, the data table, history labels, exports' names) uses `shown`.
Geometry, the solve, metrics and the graph keep using node ids: the archived end
stays a separate node with its own raw position.

### 5.2 Create

`create_closure(a, a_via, b, b_via, strategy, keep: Option<StationId>,
kept_name: Option<String>)`. The archived end is the end not kept. Allowed only
when the archived end is not already archived by another closure (C1) and
`shown(keep)` is not the archived end (C2). When neither choice is allowed (both
ends already archived, or both already show one identity), the closure archives
nothing (`archived: None`) and the dialog says why. N8 applies to the kept
identity on the archived end's lines; a collision needs `kept_name` or refuses.
The dialog default is rule K.

### 5.3 Remove

Every path that removes a closure restores the archived end's own identity:
Remove Closure (dialog: rename prompt on collision, `renames` like detach), the
#738 and #737 removals, deleting the kept end, Tie removal, Combine's dedupe and
the import funnel. Paths without a dialog letter-suffix a collision and report
it. A chain (X archived to Y, Y archived to K) re-resolves: removing Y's closure
shows Y again, and X shows Y.

### 5.4 Interactions

- **Promotion** (the sweep turns a sole-bridge closure into a merge): the merge
  keeps the closure's kept end and archives the other end as a merge record with
  its own comment; geometry first, then re-identify (4.1 step 3). A closure that
  archived nothing promotes with today's survivor.
- **Merging a closure-archived node**: allowed. Closure ends follow the merged
  node, so the merged node stays archived by that closure and shows its kept
  end; the preview says so ("shown as K, loop closure"). If two closures would
  now archive one node, the lower closure id keeps its archive and the others
  archive nothing; the op reports it.
- **Rename or comment through a closure-archived slot** edits the shown identity
  (the kept end). Datums stay on the node.
- **Same-line closure**: the line shows the kept identity at both slots (N8
  allows it).

## 6. Display and wire

- Each slot carries the node id (for commands), the shown id, shown name and
  shown comment, the home line of the shown identity, `foreign`, and
  `label_hidden` (the node is closure-archived).
- Map labels (Station names and Station IDs modes) label each shown identity
  once per position: a closure-archived node is unlabeled in the Raw Data View
  (its point stays), and coincides with its kept end in the Computed View.
- Station details show the shown identity. "Also on" lists the other lines with
  their pills; no per-line names. A closure-archived node's details show the
  kept identity and the closure.
- The data table shows the shown name, with the home-line dot on foreign slots;
  editing a name or comment there edits the shown identity.
- Pills: station and shot pills pass `homeLineId` on foreign slots (prepared in
  `rs/P-pills`).
- History rows name shown identities.

## 7. Dialogs

- **Merge Stations**: "Keep" with one radio per station (its pill: line, name,
  Station ID), default rule K. Below: the lines that will show the kept name, and
  "<archived> is archived with its comment; Detach Line from Station brings it
  back". A collision shows a required rename field prefilled with the
  suggestion.
- **Create Loop Closure**: the same picker ("Show this point as"), or a sentence
  when there is no choice (5.2).
- **Detach Line from Station**: what comes back (name, comment, Station ID); a
  rename field per collision; a picker for the junction that remains when
  several records could show it.
- **Remove Closure**: the station that comes back; a rename field on collision.
- Automatic renames (4.1 no-dialog, 4.3, 5.3) raise one notice per action
  naming each renamed station (docs/notifications.md: toast, dedup key per
  action).

## 8. File format and upgrade

- **Version 2 records.** A station's archived records are objects `{id, name,
  comment?, external?, lines: [..]}` (the alias arrays go). A closure has
  `archived: "a" | "b"` when set. A file with any archived record or closure
  archive lists `"archived-stations"` in `requires`, so an earlier #827 build
  opens it read-only instead of refusing it as malformed.
- **Reading earlier #827 files** (alias arrays, no `requires` entry): a
  load-time upgrade outside `decode` (decode never changes the model) turns each
  alias into an archived record, picks each junction's shown identity by rule K
  among the node identity and its aliases (swapping ids when an alias wins), and
  gives each closure an archived end by rule K. Joined comments cannot be split:
  they stay with the shown identity, and the records get none. The conversion
  notice lists the lines whose shown names changed. History entries of such a
  file are dropped with the existing banner (`HISTORY_FORMAT_VERSION` 2).
- **0.6.0 migration**: each merge group keeps its member on the longest line
  (rule K); the other members become archived records with their own name,
  comment and external (no joining). Migrated `Tie` closures and 0.6.0 loop
  closures get an archived end by rule K. The conversion notice lists, per line,
  each station now shown under another line's name, and says its own name and
  comment come back with Detach Line from Station or Remove Closure. "Comments
  were joined" no longer occurs for merges.
- **stalx-reader** and `docs/file-format.md` describe both forms; the reader
  example resolves shown identities. The SpeleoDB fork's minimal parser on the
  test instance is updated to accept the new records (outside this repository).

## 9. Undo

The station changeset column `archived` replaces `aliases`; the closure's new
field rides the closure list. `HISTORY_FORMAT_VERSION` goes to 2, so history in
earlier #827 files is dropped with the existing banner. The undo fuzz kit's
operations take `keep` and renames.

## 10. Tests and fuzzing

- Model tests per rule: merge keeps either side (geometry identical either way,
  identity as picked); a detach after a merge restores the archived identity
  byte for byte (name, comment, external, id) when nothing collides, and asks
  when something does; deletes restore by rule K; closures create, chain,
  promote, remove and restore; N8 including same-line closures; A1 to A5, C1, C2.
- Property: for random surveys, merge(keep) then detach of the archived line is
  the identity on every node's shown identity and records, modulo the documented
  split-half case; create closure then remove restores every shown identity.
- The existing undo fuzzers, the metamorphic and wire-equivalence oracles and
  the migration oracles are updated to the new rules and run on the farm
  (never locally beyond single seeds). The migration oracle checks rule K, no
  joined comments, and that the notice lists every shown-name change.
- Frontend: dialog tests for the pickers and rename prompts, data table and
  Station details for foreign slots, Playwright specs for merge, detach,
  closure and remove-closure flows.

## 11. Packages

1. **RS-M (model)**: types, invariants, naming and N8, merge, detach, epilogue,
   split, ranges, combine, append, closures (create, remove, chains, promotion,
   every removal path), changeset column and history version, testkit.
2. **RS-F (format)**, after RS-M: version 2 records and `requires`, the
   earlier-#827 load-time upgrade, the 0.6.0 migration, the conversion notice,
   stalx-reader, `docs/file-format.md`, migration oracles.
3. **RS-W (wire and commands)**, after RS-M: slot projection, previews, command
   signatures (`keep`, `kept_name`, `renames`, `rest_keep`), generated types, the
   e2e IPC mock, telemetry registry entries if any are new.
4. **RS-X (exports and imports)**, after RS-M: shown names in every exporter,
   the import funnel's keep rule and letter suffixes, Add .stalx.
5. **RS-UI**, after RS-W: the four dialogs, Station details, data table, map
   labels, history labels, notices, pill `homeLineId` wiring, i18n, vitest and
   Playwright.
6. **RS-T (oracles and farm)**, after RS-M and RS-W: undo fuzz kit, T5
   metamorphic, Z3 wire equivalence, new property tests, farm jobs.
