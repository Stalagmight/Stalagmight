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
