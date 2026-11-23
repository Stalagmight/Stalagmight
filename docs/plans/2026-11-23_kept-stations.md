# Kept and archived stations (RS)

Status: design, revision 2 (2026-11-23), after two adversarial reviews (model
soundness, user-facing behaviour and data paths; their findings are cited as
M-n and U-n). Lands in #827 before `.stalx` version 2 ships. It replaces U1
(per-line junction names, see the station-graph plan's Deviations "package
U1"); every U1 rule this page does not restate is withdrawn.

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
  datums, placements, hand moves, closure ends, the solve and every frontend
  grouping refer to node ids.
- **Identity**: `(id, name, comment, external)`.
- **Archived record**: an identity a merge put away inside a node, with the
  lines whose own station it was.
- **Native line** of a node: a line through it that no archived record of the
  node names. **Own lines** of a record: its `lines`.
- **Shown identity** of a node: its own identity, unless a closure archives the
  node, in which case the shown identity of that closure's kept end (5.1).
  "Shown on" a line: some node on the line resolves to the identity.
- **Foreign slot** (M-P2-2, U-P2-1): a slot whose line is not a native line of
  the node that holds its shown identity; that is, a record names the slot's
  line, or the slot's node is closure-archived. A Split Line Here half, a line
  created at a station and an import join are native: no dot. A foreign slot
  shows the shown identity's name with a dot in its **home line's** colour.
- **Home line** of an identity: the first native line of the node holding it,
  in the model's line order (`Project.lines`, not the Sections panel grouping).
  It only picks the dot's colour.
- **Line length** (for rule K): the line's measured length exactly as line
  details compute it (`survey/stats.rs`: `Normal` and `Jump` shots, `Reference`
  shots excluded), compared as exact f64 values.
- **Rule K** (keep rule): of several candidates, keep the one with the longest
  line, ties to the lowest Station ID; a candidate with no line loses. A
  **node candidate** (merge and closure dialogs) is measured over every line
  its shown identity is shown on; a **per-line candidate** (0.6.0 members, U1
  aliases, records chosen by the epilogue or a detach) over its own lines only
  (M-P2-5).
- **Rule L** (letter rule, U-P1-1, M-P1-3): when an op shows an identity on a
  line where another identity already shows the same name (an N8 collision),
  the station that takes a letter suffix (`12` -> `12a`, `uniquify_station_name`)
  is the colliding identity that is shown on fewer lines; on a tie, the one
  that is new on the line (restored, imported, line-local). An identity shown on
  more lines than the colliding one is never lettered, and chain order never
  decides. The letter is chosen free on every line the lettered identity is
  shown on, so the pass terminates. Every automatic path uses rule L and reports
  each letter.

## 3. Model

```text
Station { id, name, comment, external, archived: Vec<ArchivedStation> }
ArchivedStation { id, name, comment, external, lines: Vec<LineId> }
LoopClosure { id, a, a_via, b, b_via, strategy, archived: Option<End> }  // End = A | B
```

`StationAlias` and `Station::aliases` are deleted; decode keeps a private
intermediate form for U1 alias rows that only the upgrade (8.2) consumes
(U-P2-13g).

Invariants (validate checks C1 and C2 before N8, and every resolution of
`shown` carries a visited set, so a crafted file reports instead of hanging):

- **A1** Archived ids are unique across live and archived ids, below 2^31 and
  below the station counter.
- **A2** Every line a record names passes the node.
- **A3** A line is named by at most one record of a node.
- **A4** A node with a slot has at least one native line.
- **A5** A record with no lines is history: it reserves its id, is never shown,
  and nothing restores it.
- **C1** A node is the archived end of at most one closure.
- **C2** Following "archived end -> kept end" never cycles.
- **N8** (replaces invariant 8) On each line, slots that show different shown
  identities have different names. Two slots of one shown identity on a line (a
  same-line closure) are allowed.

**Epilogue** (every op, including Combine, which today skips it, M-P1-4): clear
stale record lines (A2); enforce A4 on every node that lacks a native line (not
only nodes that lost a slot) by the restore of 4.3; run the closure
normalisation of 5.4; then resolve N8 collisions the op created by rule L,
except where the op carries the user's renames.

## 4. Merge, detach, delete

### 4.1 Merge

`merge_stations(a, b, keep, renames: Vec<(StationId, String)>)`, `keep` in
`{a, b}`.

1. Geometry exactly as today: `merge_direction` gives `(stationary, moving)`,
   `merge_ids(stationary, moving)` folds the moving node into the stationary one.
2. `merge_ids` archives the retired node **whole**: one record `{id, name,
   comment, external, lines: its native lines}`; the retired node's own records
   move into the survivor's list. Nothing is minted, and no comment is joined
   (`CommentRule::Join` is deleted).
3. If `keep == moving`, the merged node is re-identified in the same op: it takes
   the moving identity, and the stationary identity becomes a record with the
   stationary node's pre-merge native lines. The op passes
   `EditContext::with_reidentified(&[(stationary, moving)])`, and
   `settle_placements` must judge whether a placement is on the stationary side
   by the placement's **pre-edit** station (`before.placements[i].station`), not
   by its re-keyed id (M-P1-1: otherwise, in the common case where the
   stationary piece's placement is the merge station, the wrong piece moves).
   The raw layout and depths are then bit-identical for either keep; the merge
   preview reads depths at the merged node, whatever its id. The Computed View
   orders groups by station id, so it may differ in its last bits (M-P2-9); the
   oracles compare it within a tolerance.
4. **Merging the two ends of an archiving closure** (on any path: the API, a
   promotion, the Combine join): the closure's kept end is kept, and its
   archived end becomes the merge record; the dialog offers no choice and says
   why (M-P2-4).
5. **A closure-archived candidate** keeps showing its closure's kept end after
   the merge; its radio says "shown as K through a loop closure" (5.4).
6. `renames` apply in the same op. Every collision the merge creates (N8 on
   every line the kept shown identity will be shown on, closure subtrees
   included, M-P2-1) must be answered by `renames`, or the merge refuses with
   `OpError::NameTaken { line, name }`. The preview lists each collision with
   two answers: letter the other station by rule L (the default, e.g. "Rename
   Side / 12 to 12a") or rename the kept station (U-P2-6).

The preview (`merge_preview(a, b, keep)`) adds: both identities, the default
keep, the lines whose shown name changes, the collisions with their suggested
answers, and what becomes hidden (name, comment).

**Default keep** in the dialog: the station holding a datum (a fix or the
entrance), else rule K (U-P3-2).

**Merges without a dialog**:

- `create_line_and_merge`, import joins (`FromExisting`, `ManualSnap`) and the
  import dialog's create-line-and-merge create the new line ON the target
  station, so the target is kept. When the file gives the junction a name,
  comment or external that differs from the target's, a record `{id, file name,
  file comment, file external, lines: [new line]}` is minted for it, so Detach
  gives the file's station back; comments are never joined
  (`apply_station_comments` stops joining at a junction); when no Station ID is
  left the record is dropped and the receipt says so. `FromImporting` keeps the
  target line's station (U-P2-8).
- Orphan placement keeps the target (placed) station; rule K only decides when
  neither is placed (U-P2-9).
- Every collision is resolved by rule L and named in the import receipt (imports)
  or the action's notice.

### 4.2 Detach Line from Station (unmerge)

`detach_line(station, line, renames, rest_keep: Option<StationId>)`.

- **A record R names the line.** The line gets a new node with R's identity (id,
  name, comment, external); R leaves the node. When R names other lines too (a
  split junction that was merged away), all of R's lines come off together onto
  that node, since they were one station; the dialog says so (M-P2-3).
- **The line is native and every other line is named by a record**
  (`detach_keeps`): the line keeps the node's identity; the remaining lines get
  a new node whose identity is the record naming them, or, when several records
  remain, the one `rest_keep` picks (default rule K over their own lines); the
  others stay archived in the new node.
- **The line is native and another line is native too** (Split, Create Line or
  an import join): the line gets a fresh station with the node's own name and no
  comment (today's behaviour); on a closure-archived node the preview says the
  line showed K and will show the node's own name.
- Datums, closure ends and hand moves move by `via` as today, and a closure's
  `archived` marker follows its end; the identities this shows anew on other
  lines are part of the preview (M-P2-8).
- Every identity shown anew is checked by N8 on every line it will be shown on.
  The preview lists each collision with its rule-L suggestion; the dialog asks;
  an unanswered collision refuses with `NameTaken`. A letter given earlier
  because of the junction stays (the dialog says so, U-P3-9).
- History records (A5) stay with the node that keeps the identity they were
  archived under.

### 4.3 Deletes and other edits that take a line off a node

- **The node keeps a native line**: nothing changes identity.
- **The node's native lines are all gone** (A4): the record naming the remaining
  lines becomes the node's identity (rule K over own lines when several). The
  old identity is not dropped: if the record has no comment, the old comment
  moves with the node (and the old external when the record has none); the old
  identity itself becomes a history record (A5) holding whatever was not moved.
  The delete preview names every identity restored and every comment that
  becomes hidden ("Main / 12's comment is hidden: undo brings it back")
  (U-P1-3).
- **An edit moves a line's slot from a node to a new node** (a Combine tear, a
  range move, a split at the node): the new node takes the record naming that
  line, so the record never goes stale while its line still passes a node there
  (U-P1-4, M-P1-4c). A record becomes history only when its line no longer
  passes any node at that point (the shots were deleted) or the line is gone.
- A node with no slots left goes with its records.
- Collisions on these paths: rule L, reported in the op's notice.

### 4.4 Split, ranges, combine, append

- **Split Line Here** shares the node, native on both halves. A record naming
  the split line at the cut station gains the second half (no minted copy).
  Records of stations that move to the second half are repointed.
- **Range moves** repoint records like `via` hints (and 4.3's rule for slots
  moved to a new node).
- **Combine Lines** (M-P1-4, M-P2-7):
  - *Join*: the kept line's join station `jk` is kept; `jo` is archived with its
    native lines other than `o` (an absorbed line). `jo` becomes history only if
    `o` was its only native line. Combine's comment pick (Kept / Other / Both)
    stays: the user chooses it explicitly there, and Other / Both apply only
    when `jo` becomes history (otherwise `jo`'s comment stays in its record).
  - *Shared join* (`jo == jk`): the combined line is native at the node if `k` or
    `o` was; a record naming `k` or `o` drops that line.
  - *Tear* (Loop or PassesTwice): a restoring detach. When a record names the
    torn line, the fresh node takes that record's identity; when the torn line
    was the node's only native line, the `detach_keeps` distribution applies
    (the torn line keeps the node's identity, the other lines get the record
    naming them). The tear closure then archives the end that does not hold the
    identity the torn slot showed before, so nothing visible changes and Remove
    Closure brings the line's own station back.
  - *Dedupe*: when an archiving closure is deduplicated away, its archive moves
    to the closure that stays, subject to C1 and C2.
  - Combine runs the epilogue (3); its prefix plan and rename summary use shown
    names, and collisions on third lines use rule L.
- **Add .stalx** shifts archived ids and remaps record lines; closures keep
  their archived end; the C1/C2 normalisation runs on the appended closures.

## 5. Closures

### 5.1 Shown identity

`shown(node)` = `shown(kept end of c)` when closure `c` archives `node`, else
`node`. Everything that names a node for the user uses `shown` (labels, pills,
the data table, history, exports' names, search, Plan mode). Geometry, the
solve, metrics, the graph, selection, move tools, datums and every frontend
grouping keep node ids: the archived end stays a separate node with its own raw
position.

### 5.2 Create

`create_closure(a, a_via, b, b_via, strategy, keep: Option<StationId>,
renames)`. The archived end is the end not kept. A choice is allowed when its
archived end is not archived by another closure (C1) and `shown(keep)` is not
that end (C2). The dialog shows the two allowed choices; when rule K's choice
is not allowed, the default is **"Keep both names"** (`keep: None`, the closure
archives nothing), next to the other end if that one is allowed (U-P2-11).
Collisions: as 4.1 step 6, on every line the kept identity will be shown on.

### 5.3 Remove

Removing a closure drops its marker; nothing else changes, and `shown` re-resolves
(a chain X -> Y -> K that loses Y's closure shows Y again on Y's and X's lines).
Every removal path does this: Remove Closure (dialog with rename prompts on
collision), the #737 removal, deleting the kept end, Tie removal, Combine's
dedupe (4.4), the import funnel. The #738 removal (both ends one node) has
nothing to restore: 4.1 step 4 already made the archive a merge record.
Paths without a dialog: rule L, reported.

### 5.4 Normalisation and interactions

- **Closure normalisation** (M-P1-2), run by the epilogue after every node
  merge (Merge, promotion, Combine join, append) and by every upgrade: walk the
  closures in list order; a closure keeps its archive only if its archived end
  is not archived by an earlier closure (C1) and the chain from its kept end
  does not reach its archived end (C2); otherwise it archives nothing and the op
  reports it.
- **Promotion** (the sweep turns a sole-bridge closure into a merge): the merge
  keeps the closure's kept end and the other end becomes the merge record; a
  closure that archived nothing keeps today's survivor unless the survivor's
  name collides and the other end's does not; geometry first, then
  re-identify (4.1 step 3).
- **Merging a closure-archived node**: allowed; the merged node stays archived
  by that closure (4.1 step 5); normalisation settles two closures on one node.
- **Rename or comment through a foreign or closure-archived slot**: the cell or
  Edit Station first says whose station it is: "12 belongs to Main Line. Rename
  it on every line / Detach Side to get back its own station 0 / Cancel", or
  "Shown as Main / 20 through a loop closure. Rename Main / 20 / Remove the
  closure / Cancel" (U-P2-5).
- **Datums** set through a closure-archived slot go on that node, and the action
  says so (U-P2-15).
- **Same-line closure**: the line shows the kept identity at both slots (N8
  allows it); the archived slot carries the loop-end mark (6).

## 6. Display and wire

- **Wire** (U-P2-4): each slot keeps `id` = the node id (every grouping,
  command, move, datum and selection) and gains `shown_id`, `shown_name`,
  `shown_comment`, `home_line` (of the shown identity), `foreign`, and
  `loop_end` (the node is closure-archived). Display sites read the shown
  fields: the Station ID row, the pill superscript, the Station ID column, the
  Edit Station id row, IDs-mode labels, names everywhere.
- **Map and image labels** (M-P2-10, U-P2-3, U-P1-5): a closure-archived node is
  never labelled, in either view, by name, Station ID or comment;
  `point_representatives` never picks it. Its point is still drawn, selectable
  and movable as its own node.
- **Loop-end mark** (U-P2-2): an archived closure end carries a small loop mark
  next to the name (even on the kept end's home line, where it has no dot);
  closure texts name it "K (loop end on Side)"; the Remove Closure list shows
  the slot and what comes back ("17 comes back").
- **Station details**: show the shown identity. "Also on" lists the other lines
  with their pills; no per-line names. A closure-archived node's details read
  "Loop closure end, shown as Main / K", then its own position, depth and datum
  (U-P2-15).
- **Data table**: the shown name, with the home-line dot on foreign slots and
  the loop mark on loop ends; editing a name or comment there goes through the
  prompt of 5.4.
- **Pills**: station and shot pills pass `homeLineId` on foreign slots
  (`rs/P-pills`); plain-text contexts (warnings, toasts, aria, tooltips) write
  "Side / 12 (Main)" for a foreign slot (U-P3-8).
- **History rows** name the kept station: "Merge Side / 0 into Main / 12 (keeps
  12)", "Create loop closure: Side / 17 shown as Main / 3" (U-P3-6).
- **Plan mode**: routes, the ruler path and turn or drop lists collapse
  consecutive nodes that share a shown identity across a closure edge (keeping
  D18's "each turn station once"); a pin on a loop end reads "K (loop end)"
  (U-P2-14).
- **Stats**: the station count still counts closure-archived nodes; its hint
  says so (U-P3-11).

## 7. Dialogs and entry points

- **Merge Stations**: "Keep" with one radio per station (its pill: line, own
  name, Station ID; a closure-archived one says "shown as K through a loop
  closure"), default per 4.1. Below it: the lines that will show the kept name;
  "Side / 0 is archived" plus "with its comment" only when it has one; "Detach
  Line from Station brings it back". The geometry sentence reads "Side moves
  24 m onto Main / 12's position". One row per collision with its two answers.
- **Create Loop Closure**: "Both ends show:" with the allowed choices and
  "Keep both names" (5.2), plus collision rows.
- **Detach Line from Station**: what comes back (name, comment when there is
  one, Station ID); the lines that come off together; one row per collision; a
  picker for the remaining junction when several records could show it; a note
  that earlier letters stay.
- **Remove Closure**: what comes back; one row per collision.
- **Entry points that skip a dialog today** (U-P2-7): the "Also on" detach
  button, the context menu's single-closure Remove, the ClosuresSection remove
  button and the warning resolution's remove. Each opens its dialog whenever the
  preview reports something coming back or a collision.
- **Notices**: automatic letters (no-dialog merges, 4.3, 5.3, promotions) raise
  one notice per action naming each renamed station, under the action's dedup
  key (docs/notifications.md); imports report them only in the receipt
  (U-P3-7).

## 8. File format and upgrade

### 8.1 Version 2 records

- A station's records are objects `{id, name, comment?, external?, lines?}`
  (`lines` left out when empty, `name` required). A closure has `archived: "a" |
  "b"` when set.
- **Every file this build writes lists `"archived-stations"` in `requires`**,
  unconditionally (U-P2-12). It is the marker that a file follows RS: a version 2
  file without it was written by an earlier #827 build and takes 8.2. (An
  earlier build already opens the new keys read-only; the feature makes it say
  "made with a newer Stalagmight" instead of "damaged" when its own validation
  would fail, e.g. a closure-archived node's own name reused on its line.)
- `docs/file-format.md` (including section 10's reader, which may treat alias
  arrays as unknown), stalx-reader, the station-graph plan's rollout
  prerequisite for SpeleoDB's reader, and the test instance's fork parser name
  the feature and the shown-identity rule.

### 8.2 Version 2 files from earlier #827 builds

A load-time upgrade outside `decode` (decode never changes the model), at every
site that decodes a version 2 file into a model the app uses: Open, SpeleoDB
download and install, Restore an earlier copy, Add .stalx and Merge Projects
sources (U-P2-13d). It:

1. turns each U1 alias into a record; several aliases naming one line: the
   latest names it, the earlier become history (U-P2-13a); a null-name alias
   takes the name the line showed (U-P2-13b);
2. picks each node's shown identity by rule K over own lines among the node
   identity and its aliases, swapping ids when an alias wins; the old identity's
   external stays in its record (U-P2-13c); joined comments stay with the shown
   identity, records get none;
3. gives each closure an archived end by rule K over own lines, in list order,
   when allowed by C1 and C2, else none (never the other end) (U-P2-11);
4. resolves N8 by rule L;
5. reports, per line, every name shown differently, every comment hidden, every
   letter, and every closure left with no archive.

The file then behaves as the 0.6.0 route does (U-P2-13e): the status bar reads
"Older format: converted when you save", the original is kept in Recovery on
the first save, the notice stays until that save, local-only rewrites keep the
pending state, and its history is dropped with the existing banner
(`HISTORY_FORMAT_VERSION` 2). The notice's lead says "Upgraded from an earlier
version 2 file" (U-P2-13f). Station IDs swapped by step 2 change Ariane ids
once (declared).

### 8.3 0.6.0 migration

- Each merge group keeps its member on the longest line (rule K over own
  lines); the others become records with their own name, comment and external.
- Migrated `Tie` closures and 0.6.0 loop closures get an archived end as 8.2
  step 3.
- N8 by rule L (replacing `settle_junction_names`' chain-order lettering); the
  migration oracle checks that no identity shown on two or more lines is ever
  lettered.
- The conversion notice lists, grouped by line: stations now shown under
  another station's name, **comments now hidden** (`comments_hidden`, raised
  even when every name matches, U-P1-2), letters, and closures with no archive;
  it says Detach Line from Station or Remove Closure brings each back.
  "Comments were joined" no longer occurs for merges.

## 9. Exports and imports (U-P1-5)

- **Ariane (tml/tmlu)**: node ids for rows and CLOSURE rows (the format keys
  geometry by id); Ariane writes no names. An archived closure end's own Station
  ID appears as an Ariane node number (declared). `fold_station_comments` skips
  closure-archived nodes. A round trip loses records and closure archives, and
  Ariane ids change for swapped identities (declared, U-P3-10).
- **Compass, Walls, survex**: shown names. A closure-archived end exported under
  its kept end's name ties the two points by name, which those formats read as a
  loop tie.
- **CSV and JSON**: shown name and shown id (readings only: a shared id is how
  those consumers tie a loop); a same-line closure lists one id twice on that
  line. Stated in each exporter's module doc.
- **Image export**: no name, id or comment at a closure-archived node (6).
- **stalx-reader**: node ids per slot, plus the shown name per slot and a
  `shown_as` map; `station_comments` skips closure-archived nodes.
- **Imports**: closures an import creates (`import_funnel.rs` `push_closure`)
  take rule K with the allowed fallback, else none, collisions by rule L, all
  named in the receipt; joins per 4.1.

## 10. Undo

The station changeset column `archived` replaces `aliases`; the history row for
a record carries every field (`StationExtras` is `deny_unknown_fields`); the
digest's alias count becomes an archived count; the closure field rides the
closure list. `HISTORY_FORMAT_VERSION` goes to 2. The undo fuzz kit's operations
take `keep` and `renames`, and its `tamper` corrupts records and closure
archives.

## 11. Declared behaviour changes (replacing U1's bullets in the station-graph plan)

- A junction shows one station, the kept one, on every line; the others are
  archived with their own name and comment and come back with Detach Line from
  Station.
- Merge asks which station is kept and never joins comments. A collision is
  answered in the dialog (letter the other station, or rename the kept one).
- A loop closure shows its two ends as one station; the hidden end has no label
  but stays a point of its own; Remove Closure brings its name back.
- Upgraded files keep each junction's station on the longest line and archive
  one end of each closure the same way; the notice lists what is shown
  differently, hidden comments and letters.
- Imports keep the project's station at a join, with the file's own station
  archived.
- Deletes that leave a junction on lines that knew it by another station bring
  that station back, keeping a comment that would otherwise vanish.
- Exports: Ariane uses node ids; Compass, Walls, survex, CSV and JSON use shown
  names (CSV and JSON also shown ids); the image export never labels a hidden
  closure end.

## 12. Tests and fuzzing

- Model tests per rule: merge keeps either side with a bit-identical raw layout
  (including the M-P1-1 case: the stationary piece's placement on the merge
  station and the moving piece's placement first in the list); detach after
  merge restores the archived identity byte for byte when nothing collides, and
  needs an answer when something does; deletes restore by rule K and keep
  comments; tears restore records; Combine's join, shared join and tears keep
  A3 and A4; closures create, chain, normalise (the M-P1-2 sequences), promote
  and remove; N8 with same-line closures; rule L never letters an identity shown
  on more lines; A1 to A5, C1, C2 with crafted files reporting, not hanging.
- Properties on random surveys: merge(keep) then detach of the archived line
  restores every shown identity and record (exceptions: none beyond 4.2's
  together-detach); create closure then remove restores every shown identity;
  after every op, no record is history while its line still passes a node at
  its slot, and every comment shown before an op is shown after it or named in
  its preview.
- Existing undo fuzzers, the metamorphic (T5) and wire-equivalence (Z3) oracles
  and the migration oracles move to the new rules; T5, Z3 and the merge property
  compare the raw layout bitwise and the Computed View within a tolerance. All
  fuzzing runs on the farm; locally only single-seed replays.
- Frontend: dialog tests (pickers, collision rows, together-detach, keep both
  names), foreign and loop-end display, the four entry points, Plan mode
  collapse; Playwright for merge, detach, closure and remove-closure flows.

## 13. Packages

1. **RS-M (model)**: sections 3 to 5 and 10, the testkit, model tests.
2. **RS-F (format and upgrades)**, after RS-M: 8, the conversion notice, the
   migration oracles, docs (file-format, survey-model, the station-graph plan's
   declared changes and U1 deviations marked superseded).
3. **RS-W (wire and commands)**, after RS-M: 6's wire, previews, command
   signatures, generated types, the IPC mock, telemetry entries if new.
4. **RS-X (exports and imports)**, after RS-M: 9 and 4.1's import joins.
5. **RS-UI**, after RS-W: 6 and 7, i18n, vitest, Playwright.
6. **RS-T (oracles and farm)**, after RS-M and RS-W: 12's properties and fuzz
   oracles, farm jobs.

## Deviations (package RS-M1)

Decisions the model core took where this page is silent or the code
disagreed, each with its reason.

- **Merge Stations keeps the stationary station until RS-W.** The command
  carries no `keep` yet, so it calls `Project::merge_keeping_stationary`:
  the station whose piece stays put is kept and every collision is lettered
  by rule L. The current dialog tells the user the other piece "moves to
  join" that station, and it is the identity a merge kept before RS; the
  geometry is the same for either keep. RS-W passes the dialog's keep and
  renames to `merge_stations`. Tests written before RS merge the same way.
- **Rule L ties.** Of the identities sharing a name on a line, all but the
  strongest are lettered: most lines shown on, then already shown on that
  line before the op, then the lowest Station ID (the mirror of rule K's
  tie). "New on the line" is judged against the state before the whole op.
- **Default keep with two datum holders.** The datum preference applies when
  exactly one station holds a datum; when both do, rule K decides.
- **A closure-forced keep overrides `keep`.** Every path (the API, a
  promotion, the Combine join) must keep an archiving closure's kept end
  (4.1 step 4), so `merge_stations` keeps it whatever `keep` says and reports
  it (`MergeReport::keep_forced_by`, `MergeMove::keep_forced_by`) instead of
  refusing; `keep` that is neither station refuses (`OpError::InvalidKeep`).
- **Renames answer the op's own collisions.** They apply right after the
  mutation, before the epilogue, and an unanswered collision then refuses
  (`NameTaken`). A collision the epilogue itself makes (a restore, a sweep
  promotion, a #737 removal) is lettered by rule L even in an op with
  renames: those paths have no dialog. A rename names an identity by its
  Station ID; an id that is no station refuses.
- **Merge preview suggestions are rule L's.** `MergeMove::collisions` comes
  from the merge run on a copy with every collision lettered; when the kept
  identity is shown on fewer lines than the one it collides with, rule L
  letters the kept one, and that is the suggestion.
- **Detach keeps history records on the junction.** Records carry no
  ownership link, so in all three cases the history records (A5) stay with
  the node that keeps its own identity.
- **A tear with a several-line record.** A Combine tear takes one line off
  (`DetachMode::Tear`): when the record naming it names other lines too, the
  fresh node takes the record and those other lines become native at the
  junction (an id cannot be both a node and a record, A1). They show the
  same name as before, without the dot.
- **A tear of the junction's only native line.** Instead of the `detach_keeps`
  distribution (RS-M2, with the tear closure's archive), the record rule K
  picks among those naming the other lines becomes history, so the junction
  keeps a native line (A4) and Combine's slot map keeps its meaning. Nothing
  visible changes (those lines already showed the junction's identity, now
  without a dot); the archived identity comes back only by undo. M-P1-4 names
  this as the alternative; RS-M2 replaces it.
- **The RS 4.3 comment rule.** "No comment" is none or empty text; the old
  identity's external moves when the record has none. The history record
  keeps whatever did not move, and the report names a comment it hides
  (`RestoredIdentity::comment_hidden`), which the delete previews carry
  (`DeleteShotPreview::identities`, `DeleteRangePreview::identities`).
- **Epilogue order.** Drop stale attachments (record lines included), the A4
  restore on every node, settle the anchors, the sweep, the #737 removal,
  the closure normalisation (once, after every node merge of the op), rule
  L, validate. The sweep letters its own promotions' collisions too, so a
  sweep run outside a command leaves a valid model.
- **Combine (minimal, RS-M2 finishes it).** It runs the A4 restore as a
  safety net, rule L before its validation, the shared-join rule (records
  drop `k` and `o`), and its comment pick only when `jo` becomes history;
  its chain-order `uniquify_line_names` and prefix plan stay, on shown names.
  Promotions keep today's survivor (RS-M2: the closure's kept end).
- **Foreign slot.** Implemented as the precise rule of section 2: the slot's
  line is not a native line of the node holding its shown identity, so a
  same-line closure on a native line of the kept end is not foreign.
- **N8 reports.** In chain order, one `DuplicateStationName` per (first,
  later) pair of identities on a line; the identities are Station IDs of the
  nodes holding them.
- **Version 2 records before RS-F.** The encoder writes `archived` record
  objects and closures' `archived` end, the decoder reads them (an unknown
  record key is tier 1). U1 alias rows still decode, naively, into records
  (the latest naming a line wins, the earlier are history, a null name takes
  the station's own) until the 8.2 upgrade replaces this. The `requires`
  feature is not written yet. The file-format example row carries its record
  as an object; the prose is RS-F's.
- **Salvage and the migration (minimal, RS-F).** The renamed violations
  (`ArchivedIdCollision`, `ArchivedIdOutOfRange`, `StaleArchivedLine`) keep
  their repairs; `LineNamedTwice` drops the line from the later record,
  `NoNativeLine` makes the station's records history, C1 and C2 normalise.
  The migration builds records with joined comments and letters by chain
  order; the migration and wire-equivalence oracles accept the names the
  conversion notice declares and a junction's kept name on every line.
- **Import joins (minimal, RS-X).** The new line shows the kept junction;
  the file's different name becomes a record naming the new line (no comment
  or external yet), and new stations named like the junction are lettered
  and reported, as the funnel already did without an alias.
- **Wire (minimal, RS-W).** Each slot's name and comment are its shown
  identity's; `aliases` carries one entry per record line (a history record
  one with no line); `DetachLineResult::alias_id` is always `None`.
- **Undo kit.** Its merge keeps either station at random and answers the
  collisions with the preview's suggestions, one time in five with no
  answer (refused, nothing changed); its detach picks `rest_keep` at random.
- **Two T5 oracles ignored for RS-T.** The range-vs-singles metamorphic
  tests compare junction identities by the U1 rules; under RS a letter one
  route gives stays and a restore may pick another record when the routes'
  promotions differ. They are `#[ignore]`d with that reason until RS-T moves
  T5 to the new rules.

## Deviations (package RS-M2)

Decisions the closures and Combine package took where this page is silent
or the code disagreed, each with its reason.

- **A tear keeps the junction's position with its other lines.** When a
  Combine tear applies the `detach_keeps` distribution (the torn line was
  the junction's only native line), the junction's Station ID goes with
  the torn line, but its position, and a gauge or placement on it, stays
  with the other lines: the plan's gauge test and Combine's placement pick
  follow the position, not the id. The moving line, the layout and the
  Computed View's hold point are the same as before RS (geometry never
  changes); only which node carries which id differs, and the Combine
  oracles now find a torn junction's position on the node its other lines
  are on. This supersedes RS-M1's "tear of the junction's only native
  line" bullet: the distribution and the tear closure's archive are in.
- **When a tear closure archives nothing.** It archives the end that does
  not hold the identity the torn slot showed, except when neither end holds
  it, when C1 or C2 refuse the keep, or when that end is the other line's
  join station (the join folds it into the kept join station, so the
  combined line shows the kept join station there, as a Combine join always
  did).
- **A tear's new station and the prefix plan.** In case 3 the torn line's
  new station takes the junction's name, and the prefix plan runs before
  the tear closure archives it, so the plan may rename it as one of the
  other line's stations. It is hidden behind the tear closure; Remove
  Closure shows that name (lettered by rule L or answered in the dialog
  when it repeats).
- **Shared join where neither line was native.** The record naming the kept
  line keeps naming the combined line and only the absorbed line is
  dropped, instead of making the combined line native. Dropping both would
  remove the dot and make the kept line's own station history; keeping it
  changes nothing visible and Detach still gives that station back. When
  either line was native the page's rule applies.
- **The forced keep at a Combine join.** A closure between the two join
  stations that archives the kept line's (4.1 step 4) keeps the other join
  station's identity: the fold keeps the kept station's node for the
  geometry, then the node is re-identified (its pre-edit id told to the
  anchor algorithm), the kept join station becomes a record of its native
  lines but the combined one, and the comment pick applies when it becomes
  history. `CombineReport::join_keep_forced_by` names the closure.
- **Dedupe moves an archive only to a closure that has none.** The closure
  that stays keeps its own archive when it has one; C1 and C2 are then the
  normalisation's.
- **Promotions.** A promotion keeps the closure's kept end; for a closure
  that archives nothing, the kept end of another closure between the same
  two stations that archives one; else today's survivor, unless the
  survivor's name is shown by another station on a line through the
  retired end and the retired end's is not shown by another station on a
  line through the survivor. `Promotion` reports `kept` and `archived`.
  Its re-identification is not added to `OpOutcome::reidentified` or
  `EditReport::reidentified`: the retired id existed before the edit, so
  `pre_edit_id` would map it wrongly; the promotion itself carries it.
- **Create and Remove Loop Closure commands.** Until RS-W passes the
  dialog's answers, the Create Loop Closure command keeps both names
  (`keep: None`, so it shows no name twice) and Remove Loop Closure letters
  what comes back by rule L (`remove_closure_auto`). Set Station Comment
  addresses the identity the slot shows, as Set Station Name already did;
  RS-UI asks first whose station it is.
- **Closure previews.** `closure_naming` and `closure_removal` run the op
  on a copy with every collision lettered by rule L; their suggestions are
  those letters (the dialog's other answer renames the kept station).
  `renamed_lines` lists the lines where some station shows another
  identity afterwards, letters aside. On a tie rule L letters the station
  that is new on the line, so a restored end takes the letter when the
  user reused its name.
- **Keep choices.** `closure_keep_choices` measures rule K over node
  candidates (every line each end's shown identity is shown on); the
  allowed ends are listed `a` first. Two ends that show one identity tie
  on length and go to the lower Station ID.
- **Edits through a loop end at the model level.** `slot_owner` says whose
  station a slot shows (its node, shown identity, home line, foreign, the
  archiving closure, the record naming the slot's line); `set_shown_name`
  and `set_shown_comment` edit the shown identity; a datum set through a
  loop end stays on that node and `DatumSet::shown_as` names what it is
  shown as.
- **Rule L on third lines.** A junction the prefix plan renames is never
  lettered on a third line when it is shown on more lines than the station
  it collides with (before RS the junction took the letter there).
- **Undo kit.** Create Loop Closure keeps an allowed end or both names at
  random, and create and remove answer their collisions with the
  previews' suggestions four times in five; the fifth refuses and must
  change nothing.

## Deviations (package RS-F)

Decisions the format and upgrade package took where this page is silent or
the code disagreed, each with its reason.

- **Decode prepares the 8.2 upgrade; `Decoded::upgrade` applies it.** For a
  file without the feature, `decode` reads the file as written (`Decoded::
  project`; U1 alias rows already as records naming their lines, which is
  step 1 and the private form of U-P2-13g), runs steps 2 to 5 on a copy and
  validates that copy, so a file the upgrade cannot settle is refused at
  decode as the file's own damage; the copy and its report wait in
  `Decoded::earlier`. Decode still never changes the model it returns, and
  every site that hands a survey to the app calls `upgrade()`: the open
  (and so Add .stalx, Merge Projects and the open after a Restore), the
  download install, the geometry preview and stalx-reader. The violations
  the upgrade settles are N8, A4, C1 and C2; any other refuses.
- **Every earlier file is pending, even when nothing reads differently.** The
  first save adds the feature, after which an earlier build opens the file
  read-only, so that save always keeps the original in Recovery and the
  status bar always says the file is the older one; the notice appears only
  when a line shows something differently.
- **Status bar wording.** Both routes show the existing "Old format:
  converted when you save" (one string; 8.2 quotes it as "Older format").
- **The history of an earlier file** is dropped explicitly at the open (not
  left to the history format check), with the `EarlierVersion` banner when
  it held entries; until the first save, local-only rewrites copy the
  file's history verbatim, so an earlier build can still undo it.
- **Closed-file rewrites leave an earlier file earlier.** A Recents rename
  and a SpeleoDB link strip of a closed earlier file are byte-preserving
  rewrites that copy `requires`, as for any version 2 file; unlike a closed
  0.6.0 file, which they convert, it is upgraded at its next open.
- **Downloads.** An earlier SpeleoDB revision is converted on install, as a
  0.6.0 revision is (its notice uses the downloaded sentence; history kept
  by the Unwind waiver is dropped with the banner). A read-only one keeps
  its bytes and opens pending.
- **Rule K for a closure end in the upgrades** measures every line through
  the end's node (its identity is shown on all of them), not lines its
  identity reaches through other closures: the per-line candidate of M-P2-5.
  The list-order C1/C2 pass is the model's `normalise_closure_archives`
  (already on the base); a closure an earlier file already archived keeps
  that end unless the pass drops it.
- **Rule L in the upgrades** judges "already shown on the line" by what the
  file showed there (the record naming the line, else the node; for 0.6.0
  each member on its own line), never by chain order. Salvage letters with
  no such state: its ties go to the lowest Station ID.
- **The 8.3 oracle's rule L check.** "No identity shown on two or more
  lines is ever lettered" is checked as rule L states it: a lettered
  station shown on two or more lines must yield to a station of its former
  name shown on at least as many lines. Two junctions sharing a name on a
  line (possible in a crafted 0.6.0 file) cannot both keep it.
- **Migration ids.** The migration still claims Station IDs by the map's
  label representative, then rule K re-keys the junction node to the kept
  member's id, so every 0.6.0 Station ID still names the member it named;
  the verification maps by slot and is unaffected. Records keep their
  members' own comments; an empty 0.6.0 comment is none, as before.
- **The notice's lists.** Comments are compared with what each slot showed
  (U1: the junction's one comment on every line), so an earlier file's
  junction reports no hidden comment, while a closure-archived end does. A
  slot that now shows another station under the same name is not listed as
  a name change (only its hidden comment, if any). Each entry names its line
  first; a station shown as another names the kept station by its home line
  ("Side / 0 is now shown as Main / 12"); the details are in line order
  (names, then hidden comments, per line), closures last; over three
  entries the banner counts them by kind. The sentence that Detach Line from
  Station or Remove Closure brings each back shows only when a station is
  shown as another or a comment is hidden. The lead and that sentence are
  en.json strings (`ConversionNoticeView::source`, `brings_back`); the
  clauses stay worded in Rust, as before.
- **Feature files with alias rows, earlier files with records.** A file
  listing the feature but holding U1 `aliases` reads them as records (no
  writer of the feature writes them; nothing is lost), not as unknown
  content. An earlier file holding `archived` records or closure archives
  (only RS-M1 development builds wrote them) is upgraded by the U1 rules; a
  record with its own comment keeps it with its identity.
- **Salvage.** An earlier file's own damage is repaired first, then the
  upgrade runs (`SalvageReport::upgrade`), and Repair and Export
  Salvageable Data say the file was upgraded. A shot cycle salvage breaks
  now ties the new station to the old one by a closure that archives the new
  end, so the line shows what it showed.
- **API names for the integrator.** `MigratedExtras::names_changed` is now
  `changes` (`IdentityChanges`, shared with `UpgradeReport::changes`);
  `MigrationReport` drops `comments_joined` and renames `aliases_created` to
  `members_archived` and `alias_ids_minted` to `record_ids_minted`, adding
  `kept_by_length`, `closures_archived`, `closures_keep_both`,
  `shown_as_other` and `comments_hidden`; the decoder's and stalx-reader's
  `CountLimits::aliases` and `Limit::Aliases` are `archived` (nothing
  published reads version 2 yet). stalx-reader reads an earlier file
  upgraded; its names are still each node's own until RS-X.

## Deviations (package RS-X)

Decisions the exports and imports took where this page is silent or the
code disagreed, each with its reason.

- **Compass, Walls and survex have no exporter.** This build imports those
  formats but writes none, so section 9's shown-name rule for them has no
  writer to apply to; the export module doc says so. Every exporter that
  exists (Ariane, CSV, JSON, image, dive profile) follows section 9.
- **Ariane ids are node ids, documented as a round-trip loss.**
  `docs/ariane-export.md` declares that records and closure archives do not
  survive an Ariane round trip, that a closure-archived end's own Station ID
  is its Ariane node number, and that ids change once where a merge or an
  upgrade kept the moving station.
- **An import join compares the file's station with the target's own
  identity**, not its shown one, and the record's name falls back to the
  target's own name. The record stands for the station Detach Line from
  Station gives back; without one, Detach gives the node's own name and no
  comment (4.2 case 3), so only a difference from the node's own identity
  needs a record. A closure-archived target is the case where the two
  differ.
- **The file's comment and external id at the junction** are the last
  non-blank comment and the first non-blank external id the file gives that
  station (mirrored), the rules the funnel already applies to every other
  station.
- **The record's Station ID is minted after the line's stations.** U1 minted
  the alias id first. Minting it last means a record that does not fit
  costs the record, never the line; the receipt warns what was not kept.
- **Names settle around the file's own junction name in every case**, also
  when no record can be minted (U1 did only when it kept an alias), so the
  file's numbering does not depend on the Station ID budget. The new line's
  stations that repeat the junction's shown name are lettered by the funnel
  itself (rule L: the junction is shown on at least two lines, the new
  station on one), and only those whose file name was lettered are reported,
  as before; seeds are not.
- **Receipt tiers.** A file station archived at a join and each closure
  archive are notes (`ImportReceipt::info`): they are the designed outcome
  of every join, and the slot's dot shows them, so they do not turn a clean
  receipt into a dialog. A file station not kept, a station the project held
  before the import that a closure now shows as another, and every rule-L
  letter are warnings. A closure that archives nothing gets a note that it
  shows both names. Notes are capped like warnings (`cap_warnings` caps
  `info` too). Stations are named `Line / Station` by their identity's home
  line.
- **Imported closures** are decided in list order after the Jump/Reference
  classification (rule K's measured length counts Jumps, not References),
  with rule K over node candidates; "the allowed fallback" is the other end
  when C1 and C2 allow it. The allowed test is written in the funnel
  (`closure_keep_allowed`, the same rule RS-M2 adds as
  `Project::closure_keep_allowed`; the integrator calls that one). The
  normalisation then runs as a safety net, and rule L runs against a copy
  of the project taken before the import, made only when the file has
  closures (only a closure's archive can repeat a name across lines).
- **Create line and merge** (the New Line dialog snapped onto a station)
  gives no file identity, so it never archives; it keeps the target as
  before.
- **Orphan placement's keep rule is in the model, not yet wired.**
  `Project::placement_merge_keep` (the closure-forced end, else the placed
  station when only one is placed, the target when both are, rule K when
  neither is) and `merge_by_placement` implement U-P2-9, with tests. Orphan
  placement calls the same Merge Stations command as the dialog, which
  cannot tell them apart until RS-W adds `keep`; RS-W should treat a merge
  without the dialog's keep as `placement_merge_keep`. Until then the
  command keeps the stationary station, which is the target in every orphan
  placement onto an anchored survey (the orphan piece holds no datum), so
  the only difference is the case where neither piece is placed (target,
  not rule K).
- **Add .stalx and Merge Projects sources** need nothing beyond RS-M1's
  append (records shifted, closure archives kept, normalisation in the
  epilogue); core tests now carry records and closure archives through the
  files of both. A version 2 source written before RS is upgraded by RS-F's
  load-time upgrade, since both read sources with the open path's reader;
  saying so in the add receipt is RS-F's.

## Deviations (package RS-W)

Decisions the wire and commands package took where this page is silent
or the code disagreed, each with its reason.

- **The slot keeps `name` and `comment` as the shown identity's; the RS
  fields ride one optional `shown` object.** RS-M1 already made a slot's
  `name` and `comment` its shown identity's, and every display site reads
  them, so `shown_name` and `shown_comment` would repeat them on every slot
  of every refresh (the per-station cost `closures_with` and the old
  `aliases` avoid). Instead `Station.shown` (`ShownSlot`: `id` = the shown
  Station ID, `home_line`, `foreign`, `loop_end`, `loop_closure`) is sent
  only on foreign slots and loop ends; absent, the slot shows `id` natively.
  `Station::shown_id()` and the TS `shown?.id ?? id` read the Station ID to
  display. `id` stays the node id. The binary envelope carries the same as
  the sparse `station_shown` list, and `station_archived` replaces
  `station_aliases`, still in the unreleased version 9.
- **`archived` (records) replaces `aliases` on the slot**, one entry per
  record with all its lines (`ArchivedStationWire`: id, name, comment,
  lines), so a history record is an entry with no lines. `DetachLineResult`
  loses `alias_id`: a line shows its node's one identity.
- **A collision's two answers are rule L's and "letter the other one".**
  Section 4.1 step 6 offers "letter the other station" or "rename the kept
  station". Rule L may itself letter the kept station (it is shown on fewer
  lines), so the wire gives `letter` (rule L's) and `alternative` (the other
  identity of the pair lettered instead, with a letter free on every line it
  is shown on once the first has its name back), which is the kept station
  in the usual case. The dialog may also offer free text; the command takes
  any `(Station ID, name)` and refuses a collision left unanswered.
- **The Merge dialog gets both keeps at once.** `check_merge_connectivity`
  returns, per keep it may offer (both stations, or the forced one alone),
  the renamed lines, the collisions and the hidden station (`keeps`), so the
  keep radio needs no second round trip; the geometry is the default keep's
  run (it is the same for either keep). The Create Loop Closure preview does
  the same (`options`, one per allowed end plus "Keep both names"). The
  detach preview takes `rest_keep` instead, as its choice is rare.
- **No `renames` means no dialog.** Every station command takes `renames:
  Option<Vec<StationRename>>`: absent, rule L letters every collision and
  the result's `identities` names each letter; present (even empty), an
  unanswered collision refuses with `NameTaken`. So a caller that has no
  dialog yet (the agent harness, scripts, the frontend before RS-UI) keeps
  working. Merge `keep` absent keeps the stationary station (the identity a
  merge kept before RS, and what the current dialog's words say);
  Create Loop Closure `keep` absent keeps both names. All new command fields
  are `serde(default)`, so batches written before them still read.
- **Notices ride `CommandResult.identities`** (`IdentityNotice`: lettered,
  restored, unarchived), filled by the station commands, Delete Shot, Delete
  Line, the delete-and-split and range recipes and Combine; it is absent when
  empty. The dialogs' own renames are the user's and are not reported. Split
  Line Here reports none (the halves share the node, so nothing is restored
  or lettered). Imports report only in their receipt (RS 7), untouched here.
- **The delete previews carry the identities too** (`DeleteShotImpact` and
  `DeleteShotRangeImpact.identities`, read on the state before the delete, a
  restored record named by the record), so the confirm can say "Side / 0
  comes back; Main / 12's comment is hidden".
- **History rows.** A merge reads "Merge Side / 0 into Main / 12 (keeps 12)"
  whatever the keep (the archived station first, both on the slots the user
  picked); a row worded on the state after the merge (an import's preview
  rows) takes the slot on a native line as the kept one, which is the
  target an import join keeps. A closure with a kept end reads "Create loop
  closure: Side / 17 shown as Main / 3"; one keeping both names keeps the
  old "Create loop closure between A and B". Remove Loop Closure keeps its
  label.
- **Three read-only queries are new**: `preview_loop_closure`,
  `preview_remove_loop_closure` and `preview_station_identity` (whose
  station a slot shows, every line a rename or comment edit through it
  changes, the archiving closure, the line's own archived station). Rename
  and comment commands already address the shown identity; RS-UI asks
  first. Their telemetry events are registered.
- **Frontend, minimal (RS-UI finishes it).** SurveyData keeps
  `stationArchived` and `stationShown`; the rename check addresses the shown
  identity on every line it is shown on (a loop end's too); the station
  details list records where they listed aliases; the store's merge, detach
  and closure actions and the batch commands accept the answers. The e2e
  mock models kept stations (a merge's `keep`, renames, records, foreign
  slots) and closure archives. Of the Playwright specs that touch merges,
  closures and station names (nine files), the three tests that asserted
  U1's per-line junction names now read the kept name on both lines; the
  rest of the Playwright suite was not run here.

## Deviations (package RS-UI)

Decisions the frontend package took where this page is silent or the code
disagreed, each with its reason.

- **Every dialog with a preview sends `renames`, an empty list included.**
  Merge Stations, Create Loop Closure, Detach Line from Station and Remove
  Closure send the answers whenever their naming preview answered, so a
  collision the preview did not show refuses (`NameTaken`) instead of being
  lettered behind the user's back. Without a preview (it failed, or the pair
  is refused) they send none, and rule L letters with a notice, as before.
- **Two answers per collision, no free text.** Each collision row offers
  rule L's letter (checked) and the backend's alternative, as 4.1 step 6
  says; a free-text name would need its own N8 check on every line, and
  Rename Station is one step away.
- **The Merge dialog's keep radios replace its Station A / B rows.** Each
  radio is that station's pill (line, own name, Station ID), so the rows
  would say the same twice; without a keep preview the old rows stay and
  Merge sends no `keep`. The geometry sentence reads "Side moves 24 m onto
  Main / 12's position" (7), so it no longer says the moving piece joins a
  station it may be the kept one of.
- **Create before the closure preview answers keeps both names.** The keep
  radios appear when `preview_loop_closure` answers; a Create sent before
  keeps both names (nothing hidden), the conservative choice. When rule K's
  end may not be kept, a note says why "Keep both names" is the default.
- **Plain text for a foreign slot needs the line.** `station(ref, true)`
  reads `Side / 12 (Main)`; without the line (a pill's main part, a table
  cell) it is the bare name, and the pill's dot says whose it is.
- **One plain label for a loop end: `Main / 3 (loop end on Side)`.** 6 asks
  for "K (loop end on Side)" in closure texts and "K (loop end)" on Plan
  pins; the pins use the same label (it is the one plain-text label of the
  slot), which also says where the loop end is.
- **Plan mode's route, ruler, turn and drop lists are the engine's.** The
  frontend lists no route stations by name: the route, the ruler's path,
  the turn frontier and the drops come from the gas engine and the ruler
  query by node and are drawn on the map; the endpoint and way-point rows
  name the slots the user picked, which read the plain label above. The
  collapse across closure edges therefore belongs to the engine's outputs
  (`survey/gas.rs`, the ruler's path), outside RS-UI; it is left to RS-T or
  a follow-up.
- **Station details.** The Name and Station ID rows show the shown
  identity; a foreign slot adds "12 belongs to Main"; a loop end opens with
  "Loop closure end, shown as Main / 3" and "Its own station 17 (Station ID
  17) is hidden"; position, depth and datum stay its own. "Also on" rows
  name the other line only. The records list ("Archived here", which 6
  does not mention) replaces "Also known as": each record with the lines
  whose own station it was, or "kept for its Station ID" for history (A5),
  and its comment.
- **The one-click entry points ask on more than "comes back".** The detach
  button opens Detach Line from Station when the preview restores a
  station, keeps the junction on the line (others get a station back),
  takes lines off together, offers a remaining-junction choice, changes
  what a loop closure made the line show, or names a collision; the remove
  buttons when the closure hides a station or a name would repeat. A
  failed preview acts at once (the command reports its own refusal). The
  warning resolution counts an opened dialog as leaving the flow
  (`navigated`), not as resolved.
- **The whose-station prompt (5.4).** Edit Station shows it in place of its
  buttons (one dialog is open at a time), when the name or the comment
  changes through a foreign slot or a loop end; the data table opens a
  small dialog (`shown-identity-edit`) before sending the cell's edit, and
  Cancel drops it. "Detach Side to get back its own station 0" opens Detach
  Line from Station with that line chosen; "Remove the closure" opens Remove
  Closure. A datum set through a loop end is said before saving (Edit
  Station's datum section names the loop end), since the command result
  does not carry `DatumSet::shown_as` on the wire.
- **The data table's prompt is a dialog of its own, with telemetry.** The
  `shown-identity-edit` dialog gets the per-dialog telemetry set every
  dialog carries (`action.dialog.*`, `dialog-cancel.*`, `dwell.dialog.*`,
  `active-dwell.dialog.*`; the registry goes from 756 to 760 events), the
  one Rust change of this package.
- **The keep radios name each station by its own identity.** A
  closure-archived candidate's slot shows its kept end's name, so the radio
  draws a pill of the candidate's own name and Station ID and says "shown
  as K through a loop closure" beside it (7: "its pill: line, own name,
  Station ID").
- **The data table's dot and loop mark ride the row's gutter pill.** The
  Name column stays plain editable text; the gutter pill carries the dot
  and the loop mark, the Station ID column reads the shown Station ID, and
  a shot row's pill carries each foreign end's dot.
- **Map labels.** Besides the name and the Station ID, a loop end's station
  comment label is not drawn either (6 says "by name, Station ID or
  comment").
- **Notices.** `executeCommand` raises the notice for every command that
  reports `identities`, not only the station commands, so Delete Shot,
  Combine and the range recipes say what they lettered or restored too. A
  restore whose old comment is hidden names the old Station ID (the notice
  carries no old name).
- **The stations stat's hint is a tooltip** on the Stations figure of
  Project stats.
- **History rows are the backend's** (RS-W); the frontend shows them as
  given.
- **The e2e mock's merge preview defaults to the first slot in line
  order**, the station the mock keeps without `keep`, not the datum or
  rule K default; its collisions letter by the same rule-L shape (a merge's
  kept station is on more lines; a restored station is new on its line).

## Deviations (package RS-T)

Decisions the oracles and properties package took where this page is
silent or the code disagreed, each with its reason.

- **The section 12 properties live in one seeded model fuzzer.**
  `survey/rs_property_tests.rs` runs random synthetic surveys through every
  operation that moves identities and checks, after every step, validity,
  that a refusal changes nothing, no stale record, every shown comment kept
  or named, rule L, and the dialogs' answers; per merge, either keep's raw
  layout bit for bit and Computed View within 1e-6 m, and merge then
  detach; per closure, create then remove. A CI gate runs 24 seeds of 50
  steps (about a second); `rs_property_sweep` is the farm's window
  (`STALAGMIGHT_RUST_RS_SEEDS`, `_FIRST_SEED`, `_STEPS`, `_TRACE`). The
  model level, not `AppState`, because the properties read previews and
  reports the wire flattens; the command level is covered by the undo,
  hostile, concurrency and metamorphic fuzzers below.
- **One property more than the page lists: the dialogs' answers.** A merge,
  detach or closure given its preview's suggested renames must be accepted,
  and one given no answers may be refused for a collision only when its
  preview listed one (`rs-answers`). Without it a preview that misses a
  collision passes every other property while the dialog fails.
- **Merge then detach restores everything only when the archived station
  held no records.** Records carry no ownership link (RS-M1), so a retired
  junction's own records stay on the kept node after the detach of its
  line: the lines they name keep showing the kept station. For such a merge
  the property checks the archived station's own identity (name, comment,
  external) and that it is back on exactly the lines its record named.
  Merges that also ran a promotion, a restore, a letter or the user's
  renames are not compared (the detach cannot undo those); neither are
  closures created with renames or letters.
- **"No record is history while its line still passes a node at its
  slot"** is checked by following the slot's shots, wherever a split, a
  range move or Combine took them: the node at either end of the shot next
  to the slot (and past shots the step made, Insert Station or a reversal)
  must be the record or hold it for that line. A tear of a line through one
  junction twice gives the record a node of its own at the other slot of
  the same line (deviation RS-M1, "a tear with a several-line record"),
  which passes; Combine's join stations are left out, since RS 4.4 drops
  the absorbed line there, and the kept line at a shared join, on purpose.
- **"Named in its preview" is strict where a preview exists.** A merge may
  hide only the identity `merge_preview` names (`hidden`), a detach
  nothing (`detach_naming` names nothing hidden), a delete what
  `preview_delete_shot` or `preview_delete_shot_range` name (restores with
  a hidden or moved comment, the archived ends of their promotions). A
  promotion a merge's or detach's sweep makes is not in those previews, so
  a comment it hides is reported (none on the gate seeds; the farm will
  say whether the previews need a field). Steps with no preview (Delete
  Line, Split Line Here, Insert Station, Reverse, Combine, Add .stalx,
  datums) are held to their report. Combine's comment pick is the user's
  choice (RS 4.4): both join stations' comments may change, and Both may
  join them.
- **Rule L is checked on what the step reports**, every letter in its
  identity report (and its sweep's), over the lines each identity is shown
  on after the step.
- **Two bugs the properties found on the gate seeds are fixed here**,
  being small and in the model's own contract. `detach_naming` refused
  ("Station N does not exist") a detach that goes through when the
  detach's sweep promotes the closure that alone joined the restored
  station's piece, archiving that station: the preview now names the
  station as the detach left it (a record of the promoted node). And the
  sweep letters every collision standing when its promotions end, the
  command's own included, but its letters stayed in `SweepReport::lettered`
  only, so previews listed no collision, the dialog's empty answers were
  refused with `NameTaken`, and no notice named the letter (against "every
  automatic path reports each letter"): `IdentityReport::lettered` of every
  command and edit now carries the sweep's letters first. Both are pinned
  by hand-built tests in the same file.
- **Z3 and the migration oracles are RS-F's.** Their move to rule K and
  rule L is on `rs/F` (the conversion notice's names, hidden comments and
  letters); this package does not edit them a second time, which would
  conflict at integration. The keep-independence of the geometry the page
  asks of Z3 is asserted by the model fuzzer and by T5's new pair, on every
  merge path rather than on migrated junctions only.
- **T5 under RS.** The two range-vs-singles oracles RS-M1 ignored run again,
  with every pinned seed and shape count. The singles' intermediate
  promotions leave RS traces, each asserted: the junction's comment is the
  identity it ends with, never a join (the partner end's, or the edited
  end's when the promotion kept it and the partner had none); a junction
  promoted both ways shows each route's own kept station (each its
  promotion's survivor, with the same two ids); the singles' junction may
  show another of the junction's identities than the atomic route only
  when each holds the other and a single delete's promotion kept it or its
  restore gave it back; a station rule L lettered in one route only, from
  the singles' first promotion on (a later restore of an identity the
  promotion brought in letters too) or by a range that promoted, is
  compared under the atomic route's name. A new pair,
  `merge-keeping-either-station`, merges a fresh free line keeping either
  station on the two twins and compares the raw layout bit for bit, the
  Computed View within 1e-6 m, the verdict and the allocators.
- **The undo fuzzer answers the dialogs symbolically.** Merge, Detach,
  Create Loop Closure and Remove Closure moves draw a keep (none, the
  dialog's default, either station), an answer (none, rule L's letters,
  the other answers, an empty list) and a `rest_keep`, resolved on the live
  state through the app's own previews, so a replay resolves them again;
  the new trace fields are `serde(default)`. The concurrency soak answers
  half its merges from a preview read before the command, which another
  thread may have made stale; the hostile fuzzer sends keeps and renames
  no dialog would. The datum, format, crash and transition fuzzers keep the
  commands' defaults (no keep, no renames): their oracles do not read
  identities.
- **The harness invariants count stations, not slots.** Besides moving
  `checkProjectInvariants` from U1 to RS, its stats check compared
  `station_count` (a junction once, RS U-P3-11) with the slot count, which
  fired on every survey with a junction; it now counts nodes. An ignored
  core test (`rs_wire_dump`) writes real project-state payloads of the RS
  shapes so the harness oracle can be checked against them.

## Deviations (package RS-INT)

Decisions the integration took where the packages left a seam, each with its
reason.

- **Orphan placement is not rewired to `placement_merge_keep`.** RS-X put the
  rule in the model (`Project::placement_merge_keep`, `merge_by_placement`)
  and left wiring it to the Merge Stations command for after RS-W. Orphan
  placement suggests only targets on an anchored piece for an unanchored one
  (`import_suggest.rs`), so the command's default without a `keep` (the
  station whose piece stays put) is already the target, and a closure-forced
  keep is applied by the merge itself. The only case where the two rules
  differ, neither piece placed, cannot reach orphan placement. The rule stays
  in the model with its tests as the statement of 4.1's orphan rule; a new
  path that can merge two unplaced pieces without a dialog should call it.
- **Add .stalx and Merge Projects report a converted source by counts.** RS-F
  upgrades an earlier source as it is read (8.2) and RS-X left the receipt
  wording to RS-F; neither added it, and a 0.6.0 source was never reported
  either. The add receipt now carries a note (`info`) and the merge result a
  `SOURCE_CONVERTED` warning: which older file it was, that the file itself
  is unchanged, and what its lines show differently, counted by kind (the
  banner's own clauses). No station is named: the copy renumbers lines and
  Station IDs, so the banner's "Side / 0" would not match what the user
  sees. The Merge Projects dialog's toast joins every warning instead of
  showing only the first.
- **`uniquify_line_names` stays, unused by any operation.** Both RS-M2
  (Combine) and RS-F (migration, upgrade) replaced its chain-order lettering
  by rule L; its doc says so. Only its own unit test calls it.
- **The funnel's local `closure_keep_allowed` is the model's.** RS-X wrote a
  copy because RS-M2 was not on its base; the two were identical, so the
  import funnel now calls `Project::closure_keep_allowed`.
- **stalx-reader shows the archived end's kept name.** RS-F's reader tests
  expected each node's own name per slot; RS-X's reader writes the shown name
  per slot and the `shown_as` map (section 9). The tests follow RS-X and also
  check `shown_as`.
- **User guide and undo doc follow RS.** No package owned the user guide; it
  still described U1 (merged stations keeping their own names on each line, a
  together-detach keeping an alias). The merge, closure and concept pages and
  `docs/undo-redo.md` now describe the kept station, archiving, the Keep and
  "Both ends show" choices and hidden closure ends. The screenshots are not
  retaken.
- **Plan mode's route collapse stays open.** RS-UI found that no frontend view
  lists route, ruler or turn stations by name (the engine and the ruler
  produce them by node), and RS-T did not take it. Collapsing consecutive
  nodes that share a shown identity across a closure edge (section 6) needs
  the gas engine's and the ruler's outputs to change, which is a follow-up.

## Deviations (package RS-FX1)

Decisions the model fixes after the farm's first round and the model
review took where this page is silent or the code disagreed, each with its
reason.

- **The kept station is shown, also over a loop end (model review 1).**
  4.1 step 5 holds only when the closure-archived candidate is the one
  kept. When the merge keeps the other station, the closure that archived
  the one not kept drops its archive before the fold
  (`Project::unarchive_for_keep`), so the merged node shows the kept
  station on every line. Without it the node stayed archived and showed
  that closure's kept end, so the station the user kept, and its comment,
  were shown nowhere while `MergeReport::kept` named it: against section
  1, item 3 ("the kept station is the one shown everywhere"). The review's
  other answer (keep step 5 and name the kept station as hidden behind the
  closure) would leave that contradiction. Every merge path does the same:
  a sweep promotion (`Promotion::unarchived`) and Combine's join. Each drop
  is reported (`MergeReport::unarchived`, `MergeMove::unarchived`, first in
  `identities.unarchived`), and the Merge dialog says the closure stays.
  Two closure-archived candidates: the kept one's archive stays and the
  other's goes, so the normalisation's list order no longer overrides the
  keep. Geometry is untouched (an archive is identity only).
- **Every comment an op hides is reported (farm rs-comment on merge,
  detach, range delete, delete shot, delete line).** `IdentityReport::
  hidden` (`HiddenComment`) compares the comments shown before the op with
  those shown after it, in `run_op`, the edit epilogue and Combine, so
  promotions, restores and moved loop ends inside an op are named, also a
  comment a restore moved to a record that a promotion then archived
  (`held_by`). Left out: the dialog's own hidden station (a merge's
  archived station, a closure's archived end, which have their own field)
  and a restore's `comment_hidden`. A comment another identity shows
  afterwards (moved by a restore, taken or joined by Combine's pick) or one
  that is gone (its station deleted, its text replaced) is not hidden. A
  diff rather than per-path bookkeeping, so a path added later cannot hide
  a comment silently. The previews carry it (`MergeMove::comments_hidden`,
  `DetachNaming::comments_hidden`, the delete previews' `identities`); on
  the wire `IdentityNotice.hidden`, `MergeKeepPreview.comments_hidden`,
  `DetachLinePreview.comments_hidden`; the notice, the Merge and Detach
  dialogs and the delete panel say each.
- **The range preview runs Split too (farm 7a5ed348b262).** For an
  interior range whose gap-close is offered, `DeleteRangePreview::
  split_weld` is the split run on a copy (`SplitWeld` gains `identities`),
  so the Split choice names what its promotions hide; the wire's
  `split_identities` on both delete previews. The delete panel did not show
  the previews' identities at all; it now says them in the notice's words.
- **A dialog's renames are judged after the sweep (farm e546f002311b).**
  They still apply right after the mutation, but a collision of the op's
  own that they leave unanswered refuses only when it still stands after
  the sweep and the normalisation, or the sweep lettered it
  (`Project::run_op_answered`). A collision a promotion merges away
  (the restored station folded into another) is no collision, which is
  what the preview, the op run with rule L, says; before, the dialog's
  empty answer was refused for it. A collision the epilogue itself makes is
  lettered by rule L as before.
- **The sweep normalises before rule L (farm c36d3c33f104).** A promotion
  keeping a loop end whose closure's kept end is shown through the other
  end makes the archives cycle; in that state every node of the cycle shows
  itself, so rule L measured the wrong lines and lettered the identity
  shown on more. The sweep now normalises after its promotions
  (`SweepReport::unarchived`, then in the op's `identities.unarchived`) and
  letters after that. The farm seed's own promotion no longer cycles (the
  kept-station rule above settles it); the hand-built test keeps the loop
  end.
- **A tear of a several-line record stays as RS-M1 made it; the property
  was wrong (farm f953c8f1b475, decided for the product).** The torn line's
  fresh node takes the record (4.4: "the fresh node takes that record's
  identity"), and the record's other lines show the junction natively. An
  id is one node or one record (A1), and the tear exists to separate the
  two lines (both still reach the other join station, so taking them off
  together would close the loop again), so one of the record's lines must
  do without it; the torn side is the one Remove Closure can give it back
  to. `rs-stale-record` now excuses a record that a tear closure ties back
  to its junction, as it excused a tear of a line through one junction
  twice.
- **Detach names what changes elsewhere (model review 2).**
  `DetachNaming::shown_elsewhere` (wire `shown_elsewhere`, type
  `ShownElsewhere`) compares every slot away from the junction before and
  after the detach (a detach adds and removes no slot, so slots match by
  line and index): a loop end whose closure's kept end came off with the
  lines, a station a promotion merged. `shown_before` is set in every case
  when the detached line showed a loop closure's station and shows another
  afterwards (it was only in case 3), and the dialog names what the line
  shows from the line's own result (in case 2 the junction's name, not the
  one the other lines get back).
- **Combine names a join station that stays a record (model review 3).**
  When the other join station is also native on a third line, it stays a
  record naming that line (4.4), its comment with it, and the comment pick
  does not apply. `CombineReport::join_archived` (wire `CombinePlan.
  join_archived`) names it; its hidden comment is in `identities.hidden`;
  the dialog explains instead of offering the pick. The RS property's
  Combine exemption is narrowed to the case where the pick applies.
- **The merge preview's renamed lines include every slot shown
  differently.** Besides the slots of the two merged identities, a line on
  which any slot shows another identity after the merge is listed (a merge
  adds and removes no slot, so slots match by line and index): a safety net
  for a promotion the merge's sweep makes, which merges rarely do (a merge
  joins pieces, so its closures become loops), and the same check the
  Detach preview's `shown_elsewhere` makes.
- **Records still carry no owner link (model review 4).** An owner link
  would change the file format (records gain an owner), the undo changeset
  and the wire; it is left for a follow-up. Until then the merge preview
  names the lines Detach will not give back (`MergeMove::not_given_back`,
  the archived station's own records' lines), and the dialog says "brings
  it back on its own lines; Side keeps showing Main / 12" instead of
  promising every line.
- **The migration oracle explains a loop end's comment (farm
  dbf19ed85330).** A range delete over a migrated loop end whose node has
  no comment of its own lists no removed comment: the slot showed its kept
  end's comment (5.1), which the delete keeps. 0.6.0 listed the slot's
  copy. The wire-equivalence oracle classes it; not a product change.
- **The farm seeds are pinned.** `rs_properties_hold_on_the_farm_seeds`
  replays the nine RS property seeds at the farm's step counts (about a
  second), `farm_seed_range_delete_over_a_loop_end_lists_no_comment` the
  migration seed; `kept_report_tests` pins each mechanism by hand.

## Deviations (package RS-FX2)

Decisions the data-paths fixes took where this page is silent or the code
disagreed, each with its reason.

- **Farm finding dbf19ed85330 is the oracle's.** In the four farm seeds a
  0.6.0 loop end with an empty comment (`""`) is archived behind its
  closure's kept end, so the wire shows the kept end's comment there (5.1).
  A range delete over the loop end takes only the node's own comment, which
  is none (an empty 0.6.0 comment is no comment, 8.3); the kept end's stays
  with the kept end. The port's preview lists none, which is right; 0.6.0
  listed the empty comment, and the wire oracle filtered empty legacy
  comments only where the slot now shows no comment. The oracle now records
  every empty legacy comment, whatever the slot shows; the seeds are pinned.
- **Rule L's letter search is bounded.** The letter is the first free on
  every line the station is shown on (unchanged), found from per-line
  cursors that a pass keeps current. When the station's lines take more
  than 64 letters in a row between them, it is the first letter no slot of
  the survey shows instead (free on its lines too), so a crafted survey
  whose lines take letters in turn cannot make every search walk them all.
  A letter a pass frees (the only `5a` on a line lettered to `5aa`) may not
  be offered again in that pass, as a cursor already past it stays. Either
  way the letter is free on every line, and it differs from the first free
  one only in crafted or pathological surveys.
- **Data-2 covers every quadratic step of the upgrade, not rule L alone.**
  Decode runs the earlier file's upgrade, and three crafted shapes each hung
  it: a long line of one name (rule L), 50,000 closures chaining one line
  (rule K measuring the line per closure, C2 walking each chain in the
  normalisation and in validate, every slot's shown identity walking it
  again) and 40,000 junctions whose records win rule K (a topology rebuild
  and a walk of every datum, closure and placement per re-key). Each is
  fixed in place (lines measured once, `ShownIndex` resolving each chain
  once, C2 by roots, `rekey_nodes` in one walk) and pinned with a time
  bound. The review's alternative, a cap on the collisions an earlier file
  may bring, was not taken: the same rules run in the 0.6.0 migration,
  where a cap would refuse real files.
- **A decode deadline, for readers only.** `DecodeOptions::deadline`
  (`DecodeError::DeadlinePassed`) is checked between decode's phases (the
  parse, the file's invariants, the upgrade's steps, the final check),
  never inside one, as stalx-reader's own deadline is. Only stalx-reader
  sets it; the app's opens have no deadline, as before.
- **A hidden comment in the import receipt.** An imported closure's archive
  entry adds "and its comment is hidden" (and Remove Closure "brings its
  name and comment back") when the archived end's own comment has text and
  the kept end shows another one. It stays in the entry's tier (RS-X): a
  warning for a station the project held before the import, a note for one
  the file brought, whose comment the user has not seen yet.
- **Read-only files.** A read-only earlier revision reloaded in place does
  not load its history into the session (the earlier-version banner), and
  the cache copy keeps it, as the local open of such a file does. Every
  read-only file's conversion banner, local or downloaded, takes a new
  sentence (`ConversionFile::ReadOnly`): it is only shown converted, and
  the file stays as it is.
- **Counting records.** An alias row counts once plus once for its line,
  as a record counts once plus once per line, and a record's lines count
  as they arrive, so the documented cap holds for both forms and is met
  before a crafted array is held.
- **A record a repair leaves with no line is reported.** Salvage names each
  record that a stale-line clear or A3's repair left with no line
  (`Repair::ArchivedBecameHistory`), worded like a dropped record: it is
  history (A5), so nothing brings its name and comment back.

## Deviations (package RS-FX3)

Decisions the Plan-mode, wire and frontend fixes took where this page is
silent or the code disagreed, each with its reason. They supersede RS-UI's
"Plan mode's route, ruler, turn and drop lists are the engine's" and "One
plain label for a loop end", and RS-INT's "Plan mode's route collapse stays
open".

- **Plan mode's collapse is presentation, after the engine.** The gas engine
  walks, prices, measures and places drops on the node route as before; then
  `GasReach::collapse_route` lists each run of consecutive route nodes joined
  by closure ties whose nodes show one identity as one entry (section 6).
  Every number stays the node route's, since a tie costs no gas, time or
  distance; only the lists and the indices into them (drops, way points, the
  profile) move. A project whose closures archive nothing takes a fast path
  and is unchanged bit for bit (the determinism and golden tests hold).
- **An entry names its run's first node; the last entry names the route
  target**, so `final_turn` is still the last route station. The new
  `GasReach::route_ends` keeps each entry's arriving and leaving node, and the
  wire draws each leg from one entry's leaving node to the next one's
  arriving node: a leg still runs between its own shot's slots (a loop end's
  own point in the raw layout), and no tie leg joins a loop end and its kept
  end.
- **One profile point per entry, with the run's deepest depth.** Both nodes
  are surveyed points the route passes, and `route_max_depth_m` already takes
  the deeper; the merged points carry equal gas, time and distance. The
  turnaround stays one point (`2n - 1` points), so the export's slate and
  Subsurface log have no zero-length point at a loop end (X8).
- **The turn frontier lists each shown station once**: the node holding the
  identity when it is on the frontier, else the lowest Station ID. All nodes
  that show one identity are joined by archive closures, so this is the
  "across a closure edge" of section 6.
- **A drop pinned on another node of its default station's run is the
  default** (a no-op, like a release in place), as D18 makes a pin on another
  slot of a junction. A pin anywhere else is priced as before.
- **The ruler collapses its station list, not its wire slots.**
  `AlongLine::path_stations` lists a loop end and its kept end once. The
  wire's `path_stations` stay D19's two ends of each traversed shot, because
  the map tints the path by slot: a loop end's own point is tinted with the
  shot it ends, as a junction's two slots are. A path across a tie alone (no
  shot) falls back to the collapsed list.
- **"K (loop end)" is a Plan pin label of its own** (`planStation`,
  `identity.loopEndPinLabel`): the origin, endpoint, way point, marked point
  and turn readouts, the map legend's origin and the segment boundaries'
  tooltips. Closure texts and every other plain label keep "K (loop end on
  Side)". A foreign slot's pin keeps "Side / 12 (Main)".
- **Detach's remaining-junction pick (ui-1).** The pick's preview is held
  apart from the line's default preview; Detach waits while it is out; picking
  rule K's record clears the pick (the command's default is the same record);
  a pick whose preview fails goes back to the default with a note.
- **One prompt per grid commit (ui-2).** The stations grid takes
  `onBatchEdit`: a paste, fill or range clear is one commit, whose edits on
  foreign slots and loop ends are listed in one prompt (each station, the
  lines it changes on); the native edits of the commit wait for the answer.
  Going on sends every edit in the commit's order, one command each (the
  station commands have no batch form, so it is still not one undo step, and
  a refusal part-way leaves the earlier edits applied, as for a native paste
  before). A lone edit keeps the single-station prompt with its Detach and
  Remove the closure. The grid gains `allowCellMove` (off for the stations
  grid): a batch host otherwise offers moving cells by their edge, which the
  stations grid never offered.
- **Map comment edits (ui-3).** The Comment tool asks at the overlay's
  commit (the comment is typed first, as in a data-table cell); Move Comment
  asks at each click on such a slot, picking up (the comment leaves the
  kept station on every line) and dropping (it lands on it). The prompt
  dialog takes an `onProceed` for these map actions and closes before running
  it, since the drop may open the swap dialog.
- **The read-only gate after a preview (ui-5).** The one-click helpers check
  `isEditingBlockedNow()` once the preview has answered (or failed), toast
  the editing-disabled notice under `EDIT_DISABLED` and report `failed`. The
  whose-station prompt's three edit buttons (go on, Remove the closure,
  Detach) follow `useEditingBlocked`; Cancel stays enabled.
- **Why rule K's end is refused (ui-6).** `LoopClosureNaming::rule_k_refused`
  (`KeepRefusalReason`: C1 before C2 when both hold) words the note, which
  names both ends and, for C1, the station the other end is shown as; it is
  shown also when no end may be kept (no keep choice is offered then).
  Without a reason (an older backend) the note is neutral. "Keep both names"
  and a Remove that brings nothing back say "No name changes." with one
  sentence per end another closure shows as another station.
- **Remove Closure waits (ui-7)** until the chosen closure's preview has
  answered or failed; a failed preview still removes without renames.
- **The e2e mock (ui-8)** keeps rule K as its first end (a simplification it
  already had) and checks C1 before C2. Closure collisions follow rule L's
  shape as the other previews do: creating shows the kept end's shown
  identity on the other end's lines (the other station takes the letter);
  removing shows the returning station on its lines (it takes the letter,
  being new there). Its refusals use the backend's `InvalidKeep` and
  `NameTaken` wording.
- **Notices close together join (ui-9)** instead of keying each action:
  a key per command would still collide for two of the same command, and a
  key per call would defeat the stack's burst dedup. A notice that lands
  within the burst window of a fresh one is appended to it; the same words
  twice are said once; later notices get their own toast as before.

## Deviations (package RS-INT2)

Decisions the second integration (RS-FX1, RS-FX2 and RS-FX3 merged into
sg/rs, in that order) took where the packages overlapped, each with its
reason.

- **One oracle excuse for farm finding dbf19ed85330.** RS-FX1 and RS-FX2
  each fixed the wire-equivalence oracle for the same migration seed. RS-FX2
  records an empty 0.6.0 comment as no comment on every slot (an archived
  loop end's own comment is then none, so a range delete over it lists
  none). RS-FX1 let a range delete drop any listed slot that shows another
  station while its node has no comment now. With RS-FX2's excuse in place,
  RS-FX1's fires only where a non-empty 0.6.0 comment is missing from the
  port's node: a lost comment, which the oracle must report, not excuse. So
  RS-FX1's `shows_anothers_comment` retain and its one-seed test
  `farm_seed_range_delete_over_a_loop_end_lists_no_comment` are gone (that
  name in RS-FX1's last bullet is superseded); RS-FX2's four-seed test pins
  35006357 too. The migration CI window passes without the dropped excuse,
  and the four seeds still fail with RS-FX2's fix reverted on the merged
  tree.
- **`reaches` goes with its last caller.** RS-FX2's C2 normalisation walks
  trees by root (`chain_root`) instead of following `kept_of` per closure;
  RS-FX1 kept the old helper next to its new `holds_text`. Nothing else
  calls it.
- **The e2e mock's Detach preview carries RS-FX1's fields.** RS-FX3 moved
  the preview into `mockDetachPreview` (shared by the preview and the detach
  commands' rename checks); it now also returns `shown_elsewhere` and
  `comments_hidden` (empty: the mock runs no promotion and moves no loop
  end). RS-FX3's new vitest fixtures (the rest-picker preview, the notice
  burst) gain the same required fields (`DetachLinePreview`,
  `IdentityNotice.hidden`).
- **Two RS-INT items are closed by the fix packages.** RS-INT's "Plan
  mode's route collapse stays open" is superseded by RS-FX3's collapse
  (`GasReach::collapse_route`, `AlongLine::path_stations`), and RS-T's
  follow-up (the Merge and Detach previews did not name what a promotion
  inside the op hides) by RS-FX1's `comments_hidden` and `shown_elsewhere`.
