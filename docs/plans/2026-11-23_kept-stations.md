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
- **Native slot** (RS-FX5): a slot that shows its own node's identity on a
  native line of the node: not a foreign slot and not a loop end (a node a
  closure archives). Only native slots count for N8.
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
- **Rule L** (letter rule, U-P1-1, M-P1-3): when an op shows an identity
  through a native slot of a line where another native slot already shows the
  same name for another identity (an N8 collision; a foreign slot or a loop
  end never makes one, RS-FX5), the station that takes a letter suffix (`12`
  -> `12a`, `name_with_free_letter`) is the colliding identity that is shown
  on fewer lines (every line, its foreign slots and loop ends included); on a
  tie, the one that is new on the line by its own name (restored, imported,
  line-local). An identity shown on more lines than the colliding one is never
  lettered, and chain order never decides. The letter is chosen free among the
  native slots of every line the lettered identity is native on, so the pass
  terminates. Every automatic path uses rule L and reports each letter.

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
- **N8** (replaces invariant 8; RS-FX5) On each line, the native slots that
  show different identities have different names. A foreign slot and a loop
  end show another station's identity (with the home-line dot or the loop
  mark) and count for no uniqueness, against native slots or each other:
  Main / 12 kept at Branch's start, Branch shows the dotted 12 and keeps its
  own 12. A rename is checked on the lines its identity is native on.

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
6. `renames` apply in the same op. Every collision the merge creates must be
   answered by `renames`, or the merge refuses with `OpError::NameTaken {
   line, name }`. The preview lists each collision with two answers: letter
   the other station by rule L (the default, e.g. "Rename Side / 12 to 12a")
   or rename the kept station (U-P2-6). Since RS-FX5 the merge itself makes
   none: the lines that show the kept identity anew (the archived station's,
   closure subtrees included, M-P2-1) show it through foreign slots or loop
   ends, and its own lines keep their native names. A collision comes only
   from what the epilogue does (a closure archive the normalisation drops, a
   restore, a promotion), which rule L letters and the preview lists.

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
  or the action's notice. A join that archives the file's station there (or
  lands on a loop end) shows the target through a dotted slot, so the new
  line's own station of the target's name keeps it (RS-FX5); only a native
  join slot (no record, the target no loop end) letters it.

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
- Every identity shown anew through a native slot is checked by N8 on that
  line (RS-FX5: the restored station on its own lines; a line that shows it
  through a foreign slot or a loop end is no collision).
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
Collisions: none (RS-FX5). The archived end and its closure subtree become
loop ends, which count for no uniqueness, and an allowed keep drops no other
archive, so creating a closure never repeats a name; the preview lists no
collision.

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
  allows it: the archived slot is a loop end); the archived slot carries the
  loop-end mark (6).

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
  (`rs/P-pills`); plain-text contexts (search results, aria, tooltips,
  Plan lists) and the words of a prose pill (running text: warnings, toasts,
  history rows, closure texts, dialog sentences, RS-PROSE) write "12 (Main)"
  and "Side / 12 (Main)" for a foreign slot and "12 (loop end on Side)" and
  "Main / 12 (loop end on Side)" for a loop end (U-P3-8, RS-FX5), so two
  "12"s on one line read apart; one helper on each side words it.
- **History rows** (prose pills since RS-PROSE) name the kept station: "Merge Side / 0 into Main / 12 (keeps
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
  24 m onto Main / 12's position". One row per collision with its two answers
  (since RS-FX5 only for a collision the epilogue makes; the archived line's
  own station named like the kept one is none).
- **Create Loop Closure**: "Both ends show:" with the allowed choices and
  "Keep both names" (5.2). No collision rows (RS-FX5, 5.2).
- **Detach Line from Station**: what comes back (name, comment when there is
  one, Station ID); the lines that come off together; one row per collision; a
  picker for the remaining junction when several records could show it; a note
  that earlier letters stay.
- **Remove Closure**: what comes back; one row per collision (the returning
  end's name against the native stations of its own lines).
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
4. resolves N8 by rule L (native slots only, RS-FX5: a line that now shows a
   junction's kept station through a foreign slot beside a station of its own
   of that name letters nothing);
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
- N8 by rule L (replacing `settle_junction_names`' chain-order lettering),
  native slots only (RS-FX5): a merge group's lines other than the kept
  member's show it through foreign slots, so their own stations keep their
  names; rule L fires only where native slots repeat a name, which a 0.6.0
  line does not (its load letters a crafted repeat as 0.6.0 showed it). The
  migration oracle checks that every letter answers two native slots and that
  no identity shown on two or more lines is lettered for one on fewer.
- The conversion notice lists, grouped by line: stations now shown under
  another station's name, **comments now hidden** (`comments_hidden`, raised
  even when every name matches, U-P1-2), letters (since RS-FX5 none for a
  dotted repeat), and closures with no archive;
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
  line. Since RS-FX5 a name may repeat on a line (the dotted junction and the
  line's own station of its name), and the shown id keeps the rows
  unambiguous: the dotted slot carries the kept station's id, the one its
  home line's rows carry. Stated in each exporter's module doc.
- **Image export**: no name, id or comment at a closure-archived node (6).
- **stalx-reader**: node ids per slot, plus the shown name per slot and a
  `shown_as` map; `station_comments` skips closure-archived nodes. A name may
  repeat on a section at a junction or a loop end (RS-FX5); its doc says to
  tie stations by `station_ids`.
- **Imports**: closures an import creates (`import_funnel.rs` `push_closure`)
  take rule K with the allowed fallback, else none, all named in the receipt
  (an archive letters nothing, RS-FX5); joins per 4.1.

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
- A line may show one name twice (RS-FX5): a junction slot showing the kept
  station (with its home line's dot) or a loop end (with the loop mark) counts
  for no uniqueness, so the line's own station of that name keeps it. Plain
  texts name the dotted one "12 (Main)" and a loop end "12 (loop end on
  Side)".
- Merge asks which station is kept and never joins comments. A collision is
  answered in the dialog (letter the other station, or rename the kept one);
  the archived line's own station named like the kept one is none.
- A loop closure shows its two ends as one station; the hidden end has no label
  but stays a point of its own; Remove Closure brings its name back.
- Upgraded files keep each junction's station on the longest line and archive
  one end of each closure the same way; the notice lists what is shown
  differently, hidden comments and letters (none for a dotted repeat).
- Imports keep the project's station at a join, with the file's own station
  archived; the file's other stations keep their names beside the dotted
  junction.
- Deletes that leave a junction on lines that knew it by another station bring
  that station back, keeping a comment that would otherwise vanish.
- Exports: Ariane uses node ids; Compass, Walls, survex, CSV and JSON use shown
  names (CSV and JSON also shown ids, which tell a dotted name from the line's
  own one); the image export never labels a hidden closure end.

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

## Deviations (package RS-FX4)

Decisions the record-ownership fix (model review finding 4) took where this
page is silent or the code disagreed, each with its reason. They supersede
RS-M1's "Detach keeps history records on the junction", RS-T's "Merge then
detach restores everything only when the archived station held no records"
and RS-FX1's "Records still carry no owner link".

- **An owner link, not nesting.** A record gains `into: Option<StationId>`:
  unset when the node's own identity holds it, else the Station ID of another
  record of the same node, the station that held it when a later merge
  archived that station whole. The list order stays the order (oldest first;
  a record comes before the one it is under). Section 3 reads
  `ArchivedStation { id, name, comment, external, into, lines }`. Nesting
  would have changed every flat-list site (A2's stale-line clear, A3,
  `record_for`, Split's and the range moves' repointing, Add's remap, id
  reservation, the decoder's caps and counts) into a tree walk, and made the
  file's record, the changeset row and the wire recursive (a decoder depth
  cap of their own). The link costs one optional key in the file's record
  object, one optional field in the changeset row (serde, so `StationExtras`
  stays `deny_unknown_fields`) and on the wire, and the flat sites stay as
  they were. Its price, an owner that dangles or cycles in a crafted file,
  is invariant A6 below, checked in one linear walk.
- **A6, and A5 extended.** A record's `into` names another record of the
  same node that is not history, and following `into` never comes back to a
  record (`Violation::DanglingArchivedOwner`, `ArchivedUnderHistory`,
  `ArchivedOwnerCycle`; file rule 13). A history record owns nothing: a record
  that loses its last line hands the records under it to its own owner, which
  every epilogue does after the stale-line clear (`drop_stale_attachments`,
  `Station::settle_record_owners`, run on every station since an edit may
  have emptied a record itself, as Combine's shared join does). Ids stay
  unique across the whole node (A1) and A3 stays per node, as the list is
  still the node's one list. Salvage puts a record with a bad owner under its
  station's identity (`Repair::ArchivedOwnerCleared`), one walk per station,
  so a crafted 200,000-record owner cycle is refused and salvaged in about a
  second.
- **Merge.** The fold (`Station::archive_whole`) moves the retired node's
  records under the retired identity (those directly under it get
  `into = retired`), then pushes the retired identity with no `into`. Keeping
  the moving station (4.1 step 3, `Station::swap_identity`) puts the
  stationary node's own records under the stationary record and brings the
  moving identity's records back under the node. A retired identity that
  would be history (no native line) owns nothing, so its records stay under
  the node's identity.
- **Detach case 1 gives back the whole former junction.** The record naming
  the line leads to the record directly under the junction's identity that
  holds it (`Station::record_top`; itself when it has no owner); that record
  comes back as the line's node, every line its subtree names comes off with
  it (the together-detach of 4.2, listed in `DetachNaming::together` and the
  dialog's "come off with" sentence), and the records under it stay inside it,
  those directly under it with `into` unset (`Station::take_subtree`). So in
  model review finding 4's sequence, Detach of LX or of LX2 gives X back on
  both, with R inside it. Taking one line alone has a path that exists: a
  second Detach on the restored node (LX2 from X gives R back); no new UI.
  The merge preview's `not_given_back` (and the dialog sentence it fed, with
  its two strings) is gone, as Detach now gives everything back.
- **Detach case 2 and the A4 restore pick among the records directly under
  the identity.** `rest_keep` and rule K choose among those, each measured
  over the lines its subtree names (the lines its identity is shown on once
  restored, as a node candidate is; equal to its own lines for a record with
  nothing under it, so a flat survey picks as before); a `rest_keep` naming a
  record inside another refuses (`InvalidKeep`). The chosen record's own
  records come under the new identity; in case 2 every other record goes
  with the remaining lines, keeping its owner, but the history records
  directly under the junction's identity, which stay with it; in the restore
  the old identity becomes a history record that owns nothing.
- **The history row names every line that comes off.** A Detach whose
  archived station comes back on other lines too reads `Detach lines "Side"
  and "Far" from Main / 4`, as case 2's row already named the lines that
  come off (N-2); before, it named the addressed line alone, also for a
  several-line record.
- **History records stay with the identity they were archived under**: one
  under a record comes back inside it, one directly under the junction's
  identity stays with the junction.
- **A tear takes the one record off.** Combine's tear separates one line, so
  the torn line's fresh node takes the record naming it alone
  (`Station::take_record`); the records under it move up to its owner at the
  junction. Combine's forced join keep re-identifies with `swap_identity`;
  the plain join goes through the merge's fold.
- **Split, ranges and append.** Split and the range moves only rewrite
  record lines, so owners are untouched; Add .stalx shifts `into` with the
  record ids.
- **The upgrades flatten.** Neither U1 alias rows nor 0.6.0 merge members
  knew owners: `keep_longest` (8.2 step 2 and 8.3) first puts every record
  under its node's identity, so an `into` an earlier file holds (no build
  wrote one) is dropped, and A6 is among the violations the 8.2 upgrade
  settles. The migration oracle checks that no migrated record has an owner.
- **No history or file version bump.** The record shape changed (an optional
  `into`) inside version 2 of both, which no release has (the latest tag,
  v0.5.5, writes neither): a record without `into` reads as before, and an
  earlier #827 development build opens a file with `into` read-only (an
  unknown record key is tier 1) and refuses a history entry with it (its
  record rows are `deny_unknown_fields`), which only those unreleased builds
  meet.
- **Display.** `ArchivedStationWire::into` (TS `ArchivedStation.into`);
  "Archived here" lists a record inside another under it (a record whose
  owner is missing or cycles at the top, so each is listed once). The
  whose-station prompt's own station (`SlotOwner::own_station`,
  `StationOwner.own_station`) is the station a Detach of the line gives back,
  the top record, so "Detach Side to get back its own station 0" names what
  Detach does. The e2e mock keeps an owner per archived slot: a merge
  archives the junction not kept whole, a Detach of any of its lines gives
  it back together, case 2 picks among the members directly under the kept
  station. The agent harness's wire oracle checks A6.
- **The RS-T property is strict again.** `rs-merge-detach` compares every
  station (records and owners included) and every slot after merge then
  Detach, with no flat-record exception; it passes on the 24 gate seeds and
  the nine pinned farm seeds. Its remaining exclusions (a merge or detach
  that also ran a promotion, a restore, a letter or the user's renames) are
  unchanged.

## Deviations (package RS-FX5)

The user's decision (2026-11-23), recorded as given: **"Dotted slots don't
collide."** A slot that shows another station's identity does not count for
name uniqueness on its line. That means a foreign slot (its line is named by
an archived record, so it shows the kept station with the kept station's
home-line dot) and a loop end (a closure-archived node, shown with the loop
mark). Example: Main/12 is kept. Branch's start shows "12" with Main's dot,
and Branch's own 13th station keeps its surveyed name "12". No letter is
given. (It came from upgrading real surveys, where rule L lettered far more
branch stations than any file had before RS.) Sections 2, 3, 4.1, 4.2, 5.2,
5.4, 6, 7, 8.2, 8.3, 9 and 11 above are rewritten for it. Decisions this
package took where the decision is silent, each with its reason; they
supersede RS-UI's "Plain text for a foreign slot needs the line" and its
collision rows for Create Loop Closure, RS-M2's closure-preview suggestions
for a creation, RS-X's "Names settle around the file's own junction name in
every case" and RS-F's "The 8.3 oracle's rule L check".

- **One predicate, `NativeSlots`.** A slot is native when no closure archives
  its node and no record of the node names its line; it then shows its own
  node's identity. Built once per pass (the shown index and every
  `(node, line)` a record names), so a hub of thousands of records costs its
  record lines once, not a scan of its records per slot. N8 (validate), rule
  L, the rename check, the dialogs' alternatives, Combine's prefix plan and
  the migration oracle all read it; the wire's `shown` is present exactly on
  the slots it calls not native, so the frontend, the e2e mock and the agent
  harness read "no `shown`" for it.
- **Rule L keeps its measure, judges "new" and frees the letter natively.**
  The lettered identity is still the one shown on fewer lines, counting
  every line it is shown on (foreign slots and loop ends included: that is
  how visible a rename is). "Already on the line" is now "shown there by a
  native slot before the op" (`native_pairs`): a station a line showed only
  through a dotted slot was not there by its own name. The letter is the
  first free among the native slots of the lines the identity is native on,
  so a dotted `5a` does not block `5a` (one rule for letters and renames; a
  rename to `5a` would be allowed there too).
- **A rename is checked on the identity's native lines only** (point 3):
  `conflicts_with_name` and `names_around` look at the lines where the
  renamed identity is native (`native_lines_of_identity`: none for a loop
  end's own identity) and at native slots of other identities there. A
  rename through a foreign slot or a loop end addresses the kept station
  (5.4), so it is checked on the kept station's own lines; the frontend's
  `lineWithStationNamed` and the e2e mock's rename check do the same.
- **Which paths still letter.** A merge's own mutation never makes a
  collision now: every line that shows the kept identity anew shows it
  through a foreign slot or a loop end, and the kept identity's own lines
  keep their native names. A merge can still letter what its epilogue does
  (a closure archive the normalisation drops, a promotion, a restore), so
  the Merge dialog keeps its collision rows. A closure's creation never
  makes one (5.2). Detach, Remove Closure, the A4 restore, Combine and
  imports still can: each shows an identity natively where it was not.
- **Combine's prefix plan reads native names only.** A dotted slot or a
  loop end keeps showing another line's station on the combined line, so
  it neither causes nor takes a prefix, and the retired join name counts
  only when the other line showed it natively. Before, a dotted 12 on the
  kept line against the other line's own 12 prefixed both lines and renamed
  the kept junction (Main / 12 -> A12) through the dotted slot.
- **Imports letter only at a native join slot.** The funnel letters a new
  station named like the junction's shown name once the record decision is
  made, and only when the junction's slot on the new line is native (no
  record names the line there: the file gives the junction no name, comment
  or external id of its own, or no Station ID was left for the record; and
  the target is not a loop end). A junction the file gives no name of its
  own takes the shown name for the creation rules' fill, as before, but the
  settle reads it as the name the line's own station there will have: the
  record's (the target's own name) when the file's comment or external id
  makes one, else none, so a dotted junction letters none of the file's
  stations. The closures' pass no longer runs rule L, and the copy of the
  project taken before the import for it is gone: an imported closure's
  archive makes loop ends only, and its keeps hold C1 and C2 in list order.
- **The upgrades.** The 8.2 upgrade letters a name only where native slots
  repeat it, which a U1 file holds only if crafted (U1's own N8 forbade it).
  The 0.6.0 migration keeps its rule L pass as the shared `settle` step and
  a safety net, but on a migrated survey it fires nowhere: a merge group's
  other lines show the kept member through foreign slots, and the 0.6.0 load
  itself letters a crafted repeat on one line, as 0.6.0 showed it. The
  conversion notice lists no letter for a dotted repeat. Salvage letters
  native repeats only, by the same pass.
- **Dead code removed.** The Create Loop Closure preview's collisions
  (`ClosureKeepPreview.collisions` on the wire, always empty; a debug
  assertion stands for it), the dialog's collision rows, answers and their
  renames for it, the e2e mock's create collisions, and the undo fuzzer's
  reading of them (a drawn answer but "none" is the empty list the dialog
  sends); the import funnel's rule L over its closures, the project copy it
  needed and its receipt sentence ("Station X was renamed Y: after a loop
  closure ..."); `uniquify_line_names` (chain order, already unused, kept by
  RS-INT) and `uniquify_station_name` (unused by any operation). The
  command's `renames` on Create Loop Closure stays: it is the API that
  renames in the same op, and the dialog still sends an empty list so a
  refusal is never lettered behind the user's back.
- **Plain text names a dotted slot by its home line** (point 4). One helper
  per side words every text that names a slot without the dot or the loop
  mark: `slot_name_text`, `slot_text` and `shot_text` in the model
  (`slot_text.rs`), `stationLabelOf` and `shotLabelAt` in the frontend
  (`labels.station`, `labels.shotAt`, now qualified with or without the
  line). Native `12`, `Side / 12`; foreign `12 (Main)`, `Side / 12 (Main)`;
  loop end `12 (loop end on Side)`, `Main / 12 (loop end on Side)`; a shot
  `Side: 12 (Main) -> 13` (arrow U+2192). The backend's `station_text`, the
  five copies of the shot label (validation warnings, Reverse Line's
  warnings, created shots' warnings, shot refusals, history rows), the
  history `NameBook`, the split refusal and label, Combine's removed-tie
  warnings and the receipt's joined station read the model's helper. The
  Warnings tab strips a warning's `Shot <label> ` prefix by the frontend's
  `shotAt(.., true)`, so the two must word it the same (pinned by vitest).
  Pills draw bare names (`labels.stationPill`, `labels.shotPillAt`), and a
  shot pill now draws the loop mark on a loop-end end too, as a station pill
  does. Identity labels (records, previews, notices) name an identity by
  its home line and stay as they are; Plan pins keep `K (loop end)`
  (RS-FX3).
- **Exports.** No exporter ties stations by name: Compass, Walls and survex
  are import-only, Ariane ties by node id and writes no names, the image
  labels each point once. CSV and JSON write the shown name and the shown
  Station ID, and the id keeps every row unambiguous: the dotted slot carries
  the kept station's id, the one its home line's rows carry, and the line's
  own station its own; their module docs now say the id, not the name, is the
  key. Tests with the user's example pin each format (CSV, JSON, Ariane,
  image, stalx-reader). stalx-reader keeps its API (no foreign flag): its
  `station_names` doc says a section may repeat a name at a junction or a
  loop end and to tie stations by `station_ids`.
- **Oracles.** `rs-rule-l` now also fails a letter for a collision that is
  not between two native slots; the migration oracle requires every letter
  to answer two native slots; the agent harness's N8 counts slots without
  `shown`; the metamorphic oracle's encoder-refusal test crafts a native
  repeat. All gate seeds pass; the farm windows are for the farm.
- **No file or history version bump.** A file this build writes may now show
  a name twice on a line through a dotted slot. An earlier #827 build that
  lists the same feature holds the old N8 and refuses such a file as
  damaged; only those unreleased development builds meet it (the latest
  tag, v0.5.5, writes neither version 2).

## Deviations (package RS-FX6)

Decisions the fixes after the real-app capture of the redesign (the
capture's issues P1-1 to P3-19) took where this page is silent or the code
disagreed, each with its reason. They supersede RS-UI's "One plain label for
a loop end" and RS-FX3's "`K (loop end)` is a Plan pin label of its own"
(the pin label below), and RS-UI's "The data table's dot and loop mark ride
the row's gutter pill" where it says the Name column stays plain text.

- **A collision answer names its station by role and Station ID (P1-1).**
  The two stations of a collision share a name, so "Rename Side / 0 to 0a"
  read the same for both answers. Each answer now says which station it
  renames: the dialog's own station by its role ("the station that comes
  back" in Detach and Remove Closure, "the kept station" in Merge), a
  station native on the collision's line as that line's own ("Side's own 0
  (Station ID 29)"), any other by its home line ("Main / 12 (Station ID
  12)"). The Station ID alone already tells them apart; the role says why
  the user might pick it. No wire change: the dialogs know their own
  station.
- **The Detach lead follows the chosen answer (P2-2).** The preview's names
  are rule L's run, so its `name` is the default letter. The dialog shows
  each station's name as the chosen answers leave it (a rename the answers
  make, else the station's own name when it is in a collision, else the
  preview's), in the one-sentence lead, "It gets back", the After Detach
  list and "it will show". No second preview round trip.
- **The IPC enum contract (P2-3).** `ConversionSource::EarlierVersion2`
  takes `serde(rename = "earlier_version_2")`. `tests/ipc_enum_contract.rs`
  deserialises every literal of every IPC enum's ts-rs declaration (unit
  literals, internally tagged tags, externally tagged variant names) and
  writes it back; an unknown variant or another string fails, and a tagged
  variant with fields counts as recognised when serde asks for its fields.
  A second test fails when a generated `src/types/ipc/*.ts` declares a
  literal union the contract does not list, so a new enum cannot skip it.
  The thirteen output-only enums derive `Deserialize` for the round trip
  (the frontend only reads them, and the reading direction is the one that
  broke). Struct field names are not covered: the only IPC fields with a
  digit (`recalc_hold_stage1_bar`, `recalc_hold_stage2_bar`) are generated
  as serde writes them.
- **The full-screen table starts at the toolbar's bottom edge (P2-4).**
  The view publishes `--toolbar-bottom` (re-measured when a banner resizes
  the view) beside `--toolbar-h`; the floating menus keep `--toolbar-h`.
- **A dotted station in words (P2-5).** The pill's home-line mark carries a
  tooltip ("Belongs to Main") and screen-reader text ("(Main)"), so a pill
  reads as plain text writes the slot. The Name column renders a muted
  qualifier after the bare name: "(Main)" on a foreign slot, "(loop end)" on
  a loop end (its line is the row's own, so "loop end on Side" would repeat
  it); the edit, the copy and the sort read the bare name.
- **Name repeats ride the merge preview (P2-6).**
  `Project::name_repeats` lists every (line, name, the line's own station,
  the dotted station, loop end or not) where a dotted slot or a loop end
  shows the name of a native slot of the same line. `MergeMove::repeats` is
  the merge's copy minus the state before it (so only repeats the merge
  makes), on the wire `MergeKeepPreview.repeats` (`NameRepeat`). The dialog
  says one sentence per repeat; the e2e mock lists them the same way.
- **A restore's hidden comment by line, name and text (P2-7).**
  `RestoredIdentity` gains `old_name` (set by the A4 restore) and
  `old_home_line`, filled by `IdentityReport::with_hidden` from the state
  before the op (the line whose departure caused the restore no longer
  passes the node after it); the wire carries both. Notices word every
  hidden comment as the dialogs do, "Annex / 3's comment ("...") is hidden:
  Undo brings it back". A history record reads "3 (Station ID 54), no
  longer on any line", without its former line: a history record keeps no
  line by definition (A5), and remembering one would add a key to the file's
  record and the changeset row, which this package does not take.
- **A closure's loop end by its own identity in the conversion notice
  (P2-8).** A closure left keeping both names names an end that another
  closure archives as "Main / M4 (now shown as Side / S4)"; any other end
  by its slot text as before.
- **Hidden comments per identity in the notice (P3-9).** `CommentHidden`
  carries the identity whose comment the slot showed; the banner counts
  identities and lists each once, under the first line it is on ("the
  comment of M4 on Main and Annex is hidden"). The upgrade and migration
  reports' `comments_hidden` stay per slot, as the oracles count them.
- **A loop end's Plan pin (P3-10).** "Loop / L8 (loop end, shown as Main /
  8)": the closure's archive names the end's own station; without it (an
  older backend) "Loop end on Loop, shown as Main / 8". The closures section
  reads "Loop / L8, the loop end, is shown as Main / 8."
- **One prompt wording (P3-11).** Both whose-station variants say "Rename
  Main / 8 to 8E on Main and Loop" ("Change the comment of Main / 8 on Main
  and Loop" for a comment); the lines once the preview answers, the new name
  from the caller.
- **What travels inside an archived junction (P3-12).** Read from the
  slots' records (`Station.archived`, every slot of a node carries them):
  the Merge dialog names the records directly under the station it
  archives ("archived inside Side / 0 by an earlier merge, stays inside
  it"), the Detach dialog the records under the one that comes back ("stays
  archived inside it: detach Upper again to get it back"). History records
  are not named. No wire change.
- **The note on earlier letters (P3-13)** shows when a line that gets the
  station back already shows its own name followed by letters (`0a`, `0b`):
  the only trace a letter leaves. Without one the note said nothing true.
- **The remaining-junction picker (P3-15)** draws each record as a pill by
  its home line; the lead reads "They get back" when the station comes back
  on several lines (the line keeps the junction and two or more others come
  off).
- **The history list (P3-16)** shows each description whole, wrapped to at
  most three lines; the 40-character cut is gone.
- **Sections panel dots (P3-17)** take `pillDot` against the panel
  (`panelDotOf`), drawn as a 1 px border so the hover ring stays free.
- **A pinned action row (P3-18).** Merge Stations, Detach Line from Station,
  Create Loop Closure and Remove Closure end with `StickyDialogFooter`,
  the import dialogs' sticky idiom; Playwright checks the buttons at
  1024x640 and with the Detach body scrolling.
- **Not RS-made, fixed while there (P3-19).** The merge depth list gives a
  line's equal readings once with their count ("7.8 m (Main, 2 readings)",
  so the list still adds up to the mean), grouped in the dialog by line and
  displayed value; `formatFixed` writes a value that rounds to zero without
  a sign (every caller, not Station details only). The closure dialog's red
  error next to a refused Create is left as the capture noted.

## Deviations (package RS-FX7)

Decisions the second farm round's fixes took where this page is silent or
the code disagreed, each with its reason. The first bullet refines RS-FX1's
"Every comment an op hides is reported" (which comment counts as moved);
the others refine RS-T's "T5 under RS".

- **A comment moved only when its new holder did not hold it before (farm
  647783847b78 on 51b5191db, product).** `Project::hidden_comments` left
  out a comment "another identity shows now and did not before", by text.
  A Detach gave back a record whose own comment had the same text as a
  station the detach's sweep then archived by a promotion (two notes "sump",
  or a survey added twice with Add .stalx), so the restored station's own
  comment passed for the hidden one moved there, and the Detach preview, the
  notice and the report named nothing. The test now reads what the other
  identity stored before the op (a node's or a record's comment), not what
  it showed: a restore that moves a comment to a record with none, and
  Combine's Other and Both picks, still count as moves. A comment is a
  user's words, so the same text on two stations is two comments; the
  preview names the hidden one, which is the design's "previews tell the
  truth". No wire change: the dialogs and notices already show
  `comments_hidden` and `IdentityNotice.hidden`.
- **The metamorphic oracle compares a placement at twelve significant digits
  of the survey (farm d14345c9943b, seed 580006372, oracle).** Not an RS
  case: a range of two shots that removes no junction, with no closure.
  `settle_placements` walks the piece from its still point to the placement
  station once per edit, so the range (one walk) and the single deletes
  (two) leave the placement a few ulps of the walk's 20 m sums apart, and on
  a placement near the origin that is its own twelfth significant digit
  (`-1.74799425406e-4` against `...404` m). No view can show it (the
  Computed View agrees within 1e-6 m) and the model stores a placement as
  computed, as the move pair's docs already say. The stored survey's
  placements are compared on the projects' values within 1e-11 times the
  largest raw coordinate of either route; another station, another order or
  a real move is still reported.
- **A cut-loose piece placed on a promotion that kept its retired end
  (seeds 580002106, 580004294, oracle).** RS 5.4's name test may keep the
  retired end, so the merged node at the survivor's position carries the
  retired end's Station ID. The oracle measured the cut-loose piece's
  placement station by that id, where the retired end was (16 m and 11.6 m
  away); it now measures it as the range promotion's survivor. The
  geometry is RS 4.1 step 3's: identity and position are independent.
- **The singles' mirrored junction after a later restore (seed 580002296,
  oracle).** The singles promote a closure the other way and keep 4; the
  next single delete takes 4 off its last native line, so rule K gives back
  10 with 4 inside the junction (RS 4.3, named in that delete's notice).
  The oracle required the singles' junction to be their promotion's kept
  station, against RS-T's own trace "a restore gave it back"; it now accepts
  the record a later single delete gave back with the kept station still
  inside it. The atomic route's junction is still held to its promotion's
  kept station. This is the documented class (a promotion's kept station
  depends on its direction), now followed through a restore.
- **Pinned.** `rs_properties_hold_on_the_farm_seeds` replays the five
  rs-comment detach seeds the farm found on 51b5191db (400146250,
  400167128 at 50 steps; 480010813, 480011346, 480011587 at 200); the
  metamorphic module pins 580006372 and the three seeds above, each with
  its injection, and `a_placement_past_the_surveys_digits_is_reported`
  shows the tolerance is not vacuous.

## Deviations (package RS-FX8)

Decisions the fixes after the second real-app capture (issues N1 to N7)
took where this page is silent or the code disagreed, each with its reason.

- **A line name anchors on an own station (N1).** A branch starting on a
  junction anchored its name on the junction, where the junction's station
  label sits; both are offset in em from one point, so they overlap at every
  zoom and only one of them ever shows (the junction's label lost in the
  capture, the branch's name in the Playwright survey). Not
  RS-made, but RS makes a branch on a kept station the common shape. The
  name now anchors on the first non-reference station that is no junction
  slot (its Station ID on another slot of the SoA, `junctionSlots`) and no
  loop end; a line whose every station is shared (a one-shot tie) takes the
  middle of its first shot. The other fix the issue offered, giving station
  labels priority over line names, was not taken: a name revealed from z10
  cannot vanish at the 13.5 band (the reveal is monotone in zoom), so it
  would have to lose from the start and a branch would never be named. The
  #699 ranks (real line first) are unchanged.
- **A line-name toggle needs no rebuild (N1).** A toggle reruns the reveal
  over the emitted labels only, and a label that never clears is not
  emitted, so a station label a name beat could not come back when names
  were turned off. A build now also emits, hidden at an unreachable reveal,
  every label that overlaps a shown line name at the build zoom
  (`shadowedByKind`); the rerun then shows it. Line names are few, so this
  adds few labels; the other kinds keep the documented approximation (a
  label freed beyond the emit set waits for the next settle).
- **Remove Closure and Merge follow the answers (N2).** As Detach since
  P2-2: Remove Closure's row and sentence name the returning station as the
  chosen answer leaves it, and Merge's "will show" and loop-end sentences
  name the kept station so.
- **The Name column is 112 px wide (N3)**, enough for a three-digit name
  and "(loop end)"; a longer qualifier truncates, never the name.
- **Merge's Detach sentence names its station and comes first (N4):**
  "Detach Line from Station brings Lower / 3 back.", then what stays inside.
- **The other collision answer (N5)** reads "Rename the 0 that is on Side
  now (Station ID 29) to 0a": "Side's own 0" fit the returning station too.
- **The rename prompt (N6)** names the lines once, on its button. The grid's
  `onEdit` may return `EDIT_HELD`, and the grid then keeps its cursor on the
  edited cell; the stations grid returns it when it asks whose station a
  cell shows. A cursor that moved on with Enter pointed at the next row
  while the prompt was about this one.
- **Every station that goes with the remaining junction (N7).** When the
  detached line keeps the junction, the lines that come off take every other
  archived station with them, inside the one they get back (RS-FX4); the
  dialog names them all for either pick, not only those under the picked
  one. History records stay with the junction and are not named.

## Deviations (package RS-FX9)

Decisions the fixes of the pre-PR review took where this page is silent,
each with its reason.

- **A dialog's renames follow Set Station Name's rules.** Every rename a
  Merge, Detach or loop closure command carries is trimmed and refused when
  empty or holding a control or invisible formatting character, in Set
  Station Name's words, through the one helper both use
  (`station_naming::user_station_name`). A collision's answers are the
  backend's letters of the stations' own names, so only a name that already
  held such a character (an import's) trips it; the dialogs run Edit
  Station's check (`stationRenameProblem`) on what they would send, disable
  their action and say why, and the user renames that station first.
- **A rename answers the op's own collisions only.** A command accepts a
  rename only of a station its own collisions name, worked out as its
  preview lists them: the epilogue run with rule L on a copy of the state
  right after the mutation (both answers of each collision). Anything else
  refuses (`RenameNotAsked`) and changes nothing. The copy runs only when
  there are renames. A merge or closure that makes no collision therefore
  takes no rename at all; renaming its kept station is Set Station Name's
  job (the core test that renamed it through a merge now does so).
- **Records are looked up per line once.** Every pass that names many
  slots or lines of one station reads a record index built once
  (`RecordIndex`, a station's named lines, a home line per identity,
  `SlotTexts` for the warnings' texts), so a hub costs its lines plus its
  records. An earlier file's upgrade also asks a reader's deadline per line
  and per junction inside its steps.
- **Salvage settles an archived line named twice in one walk per station**,
  reporting each repeat in the order validation found it.
- **Detach names every station that moves.** Its lead gives a row per
  station directly inside the one that comes back, naming the stations
  still archived inside it ("Side / 0 stays archived inside it, with Twig /
  7 archived inside Side / 0"); history records are never named.
- **Records stay per slot on the wire (review nit 9, left).** Every slot of
  a junction still carries the station's record list. Sending it once per
  station means keying station-level data by Station ID instead of by slot
  in `SurveyData` (the binary envelope, the JSON `Station` that
  `get_adjusted_stations` and the dialogs read, the mock, the hidden-line
  filter that remaps slot keys), which is not cheap. It only weighs at a
  crafted hub, where each slot's `merged_with` (every other slot of the
  junction, a shape older than this plan) is quadratic too; both belong to
  one wire change, not to this package.

## Deviations (package RS-FX10)

Decisions the last review items took where this page is silent, each with
its reason.

- **Linear-time tests compare two sizes.** A wall-clock bound flakes on
  CI's debug builds and shared runners, so every test that pins a pass as
  linear times one shape at n and 4n in the same run
  (`models::linear_time::assert_linear`, test support): the runs alternate,
  the fastest of three per size counts, and 4x the input may take at most
  8x the time (a linear pass takes about 4x, the quadratic forms these
  tests were written against about 16x). A ratio over the bound gets a
  second round before it fails, since a busy runner can slow the larger
  size more (it reaches past the caches the smaller fits in); a 5-minute
  backstop covers a hang. Sizes are a few times below the old ones (1-5 s
  each here); the full caps are not run any more. Under a host load of
  about 24 on 16 cores one round of the model's hub test measured 10x, the
  reason for the second round.
- **A deadline test counts asks, not the exact one.** The upgrade's first
  step asks once per line; a deadline passing on the tenth ask must stop it
  with the deadline's error before that step asked for every line, so a
  check added between steps does not break the test.
- **The op's own letters go through verbatim.** A rename that is exactly a
  letter the command's own collisions offer for that station (rule L's or
  the other answer, `offered_letters`) is taken untrimmed: it is the
  backend's answer, free on the station's lines, and its base may be an
  imported name with padding (`" 5"` letters to `" 5a"`, which trimmed
  could repeat a `5a` of the line). Any other name follows Set Station
  Name's rules as in RS-FX9. The dialogs still run Edit Station's check on
  what they send, so a letter of a name holding a control or invisible
  formatting character still asks the user to rename that station first,
  though the backend would take it.
- **No Create Loop Closure takes a rename.** A loop end counts for no
  uniqueness (RS-FX5), creation runs no sweep, and C1 and C2 leave the
  normalisation nothing to drop, so a creation makes no collision and
  RS-FX9 refuses any rename it carries; the 24 gate seeds of the RS
  property fuzzer create 224 closures, none with a collision. The
  editing-blocked test therefore checks the creation, like every other
  command there, refused for its rename alone with editing allowed, and
  going through as its dialog sends it (no renames).
- **The Detach picker comes first.** The remaining junction's picker sits
  under the sentence it answers ("Main keeps this station, and Side and
  North come off together"), above "They get back", the archived-inside
  notes and the After Detach list, which follow the pick.
- **The prompt's every-line sentence only when the button lacks it.** "A
  new name changes it on every line it is on." is left out whenever the
  go-on button names the lines (every case but a rename with no new name
  to say).

## Deviations (package RS-PROSE)

The user's request (2026-11-24): prose should still use pills, not "Main /
12"; the pills should not affect the line spacing; check how it looks before
submitting; if pills overlap frequently, drop the pill background for
something like a radial gradient that fades towards the edges. Decisions
this package took, each with its reason; they supersede section 6's
"plain-text contexts write" for running text (its words stay the same).

- **Running text names with prose pills.** Every visible sentence that
  names a station, shot or line (the Merge, Create Loop Closure, Detach,
  Remove Closure, shown-identity, Delete Shot, Delete Shots, Delete Line,
  Split Line Here, Combine, Reverse, Swap Comments and Insert Station
  dialogs, the Station details sentences, the Warnings tab, the undo and
  redo rows, the conversion notice, the identity and removed-closure toasts,
  the map legends' "from" lines, the far-GPS-fix notice, the Plan endpoint
  sentences and the import wizards' connection lines) draws each name as a
  `ProsePill`: the link pill's dot, muted context, bare name, home-line dot,
  loop mark and superscript Station ID, inline. A screen reader reads the
  sentence exactly as plain text wrote it (the drawing is aria-hidden, the
  plain words sit beside it as sr-only text), and below the Full level the
  pill is that plain text. Plain-text contexts keep the plain label:
  aria-labels, tooltips, native dialogs, the clipboard, exports, files, logs,
  telemetry, option labels, dialog titles, button labels and backend error
  strings (`CommandResult.error` also feeds logs and the catch-all toast).
- **One token form for frontend sentences.** `useProse()` mirrors the
  plain labels (`prose.station(ref, true)` for `labels.station(ref, true)`)
  and gives a token: the part as URI-encoded JSON between U+0001 and U+0002.
  A token is a string, so it passes through `t()` interpolation, `listNames`
  and the toast store, and `<Prose text>` splits it back; en.json is
  unchanged and no node is concatenated. `proseToPlain` gives the plain words
  for a plain sink.
- **Structured parts from the backend, beside the plain string.** A
  backend text that names something carries `Vec<ProsePart>`
  (`models::prose`): `ProjectWarning.messageParts`, `ValidationWarning.parts`
  (removed closures, per-shot checks after Reverse Line and Create Shot),
  `UndoHistoryItem.descriptionParts`, `ConversionNoticeView.change_parts` and
  `detail_parts`. Builders mark names in their `format!`
  (`Project::slot_marked`, `shot_marked`, `station_marked`, `line_marked`)
  and split at the boundary, so the plain string is byte-identical to before
  and is the parts' words concatenated (tested per producer). A marked string
  never reaches a plain sink. Left plain: Create Line and import validation
  warnings (they name a chain position on a line that does not exist yet),
  the "(keeps 12)" and delete-span names inside history labels, the history
  preview popup's key-value rows, the closure refusal sentence (its dialog
  data comes from the opener) and every command error.
- **History format version 3, only where it is needed.** A history label is
  persisted, so its parts are too: `label_parts` beside `label`, without
  slots (stable ids only). An entry is written as version 3 only when it has
  parts; one whose label names nothing keeps version 2's layout, which the
  previous build reads (pre-PR fix 4). That only helps a history whose
  labels all name nothing: one version 3 entry makes the previous build drop
  the whole history, and most labels name something (all 16 of the measured
  mix below), so in practice a downgrade loses the history, as any version
  bump does. Version 2 entries are still read and
  render plain; parts that do not fit (a cut label, words that differ, a part
  drawing a name or context its own words do not hold, an unknown kind) are
  dropped on read, never the entry (fix 5). The older build drops such a
  history with its banner.
- **What the parts cost the history budget** (measured, pre-PR item 9;
  `history_labels_tests::label_parts_cost_a_measured_share_of_the_history_budget`):
  a typical mix of 16 steps (9 shot cell edits, 2 renames, a comment, an
  appended shot, a merge, a move, a delete and a reverse) on a small survey
  encodes to 8881 bytes with parts and 6214 without, +42.9%: about 160 bytes
  per entry (122 to 185; a merge, naming two stations, 300). Mean entry 555
  bytes against 388, so the 4 MiB Full-mode budget (`UNDO_BYTE_BUDGET_BYTES`)
  holds about 7556 such steps instead of 10799: 3243 fewer, 30%. The cost is
  per entry and does not grow with the survey, so larger changesets pay a
  smaller share; local projects keep a count window (20 by default), which
  the parts do not change. Accepted: thousands of steps still fit, and the
  pills are the point. If it ever matters, a part's `text` (the label's own
  words) is what could be left to be re-derived.
- **No value can forge or crash a pill** (pre-PR fixes 1 and 3). A comment, a
  name or a file name interpolated into a sentence could hold U+0001...U+0002.
  Frontend: a token carries the page load's random nonce and is read only
  with a part's full shape (`partOf`); every `t()` interpolation (the i18n
  `format` hook, run for every value before i18next measures it: an
  `escape` hook that shortened a value made i18next skip the next
  placeholder) and every raw value a sentence is built from by hand goes
  through `proseValue`, which drops the markers and other C0 controls; a pill
  whose part lacks a field draws its plain words. Backend: marks carry the
  process's random nonce, `parse` drops every other marker character (the
  one place raw interpolated text is cleaned), `mark` cleans a part's own
  fields, and `split` keeps parts only when their words are the text's (the
  check `Label::of` makes). Loaders and importers keep a name as its file has
  it (a typed name already refuses control characters); text is cleaned
  where it is worded and drawn, not by rewriting stored data.
- **A pill draws its words as recorded** (fix 2): a history row keeps the
  line names it was recorded with; the live line only colours it. Pills name
  lines by their display names (fix 6), and what a pill draws from a part is
  sanitised as displayed names are (fix 5). The conversion banner's sentence
  reads capitalised with pills as plain (fix 7); the prose types are
  generated into `src-tauri/bindings` (fix 8).
- **No pill shape: a soft radial tint.** Measured in the real app (this
  worktree's debug build through the agent harness, a synthetic survey of
  seven lines in near-white, near-black, yellow, blue, grey, magenta and
  green with long line and station names, four foreign junction slots and a
  loop end; 19 surfaces) with the first design, a bordered fill: an inline
  fill covers the font's whole content area, 19 px in a 20 px line at 14 px
  and 16 px in a 16 px line at 12 px, so of 69 pairs of pills stacked on
  adjacent lines 54 (78%) stood 1 px or less apart and 2 touched (0 px), and
  77 of 94 superscript Station IDs rose up to 2.06 px above the fill. Two
  lines of pills read as one block, so per the user's instruction the prose
  pill paints no fill shape and no border: a `radial-gradient(farthest-side)`
  of the line's tint per line fragment (`box-decoration-break: clone`), solid
  to 45% of the radius and transparent at the fragment's edge. Its half-alpha
  band is 13.8 px of a 19 px fragment and 11.6 px of a 16 px one, so the
  tints of two stacked lines stay 6.2 px (20 px lines) and 4.4 px (16 px
  lines) apart and never form an edge. The superscript now sits inside the
  text's height (at most 0.41 px above the content box, inside the
  half-leading).
- **The tint is a translucent overlay.** An opaque tint computed for the
  panel read as a dark smudge on the lighter accent and warning boxes; the
  overlay (`proseTintOf`) composites to exactly the swept tint over the
  panel and moves any other surface the same way. Its centre is the link
  pill's hover tint, and since it fades to the bare surface,
  `prosePillColors` holds text at 7:1, muted parts at 4.5:1 and the dot (or
  its ring) and the home-line dot at 3:1 on both the tint and the panel, on
  every theme over the link pill's colour sweep.
- **Line pitch, measured.** In the running app every pill-bearing block was
  measured with pills and again as plain text (highlighting off, same
  element): line tops from the text's client rects, clustered into lines.
  Dark and light themes at 1440x900 and 860x900: 103 blocks, 141 pills;
  the 38 (1440) and 37 (860) blocks that wrap and were the same element both
  ways gave 114 and 124 line gaps, every one equal to the block's
  line-height (largest deviation 0 px); the identity toast keeps 20 px. The
  blocks React rebuilt on the level change (9 and 10) were not paired.
  `e2e-playwright/prose-line-pitch.spec.ts` repeats the measurement on the
  Bulk Delete summary and a Warnings tab message (within 0.5 px); a
  superscript given `vertical-align: super` fails it (1.66 px).
- **Wrapping.** A pill wraps between its words, each line's fragment tinted
  on its own; the dot stays with the first word, a shot's arrow with its far
  end and the marks and Station ID with the last word, so no fragment is a
  lone dot or arrow. The outer span paints nothing, so the sr-only words
  never open an empty painted fragment.
- **Accessible names of labels.** Chromium puts spaces around an absolutely
  positioned sr-only span in an accessible name computed from content
  ("Keep Side / 2 , Station ID 9"); an inline technique that avoided it
  (`font-size: 0`) risks screen readers skipping the words. The sr-only
  span stays, and the radios whose label is a prose sentence (collision
  answers, Detach's lines that come off together) carry the plain sentence
  as their `aria-label`.

## Deviations (package RS-OWNERSHIP)

The user's request (2026-11-24): stations need no disambiguation dot, the
whole pill takes the colour of the line the station actually belongs to;
the dot is only needed in shots, and a shot pill is half one colour, half
the other. Decisions this package took, each with its reason; they
supersede section 6's "Pills" bullet (pills pass `homeLineId` for a dot)
and RS-FX6's home-line mark on station pills.

- **A station pill is its owner's pill.** The owner is the home line of
  the identity the slot shows, when it is not the slot's line
  (`ownerLineOf`: a foreign slot, or a loop end whose kept end lives on
  another line). The link pill's fill, border, hover and leading dot, or
  the prose pill's tint and dot, are that line's, and a shown context names
  it ("Main: 12" on Side), so no pill reads one line's name in another's
  colour. The second dot is gone. The tooltip says "Belongs to Main"; a
  button pill's aria-label keeps the plain words, a span pill reads "(Main)"
  after the name, or "(on Side)" when its context already names Main, so
  the slot's line is not lost to a screen reader. A loop end keeps its loop
  mark and takes its kept end's colour. The data table's Name column keeps
  "(Main)": its words tell the two 12s apart.
- **Parts carry the owner for loop ends too.** `ProseStation.home_line` and
  `ProseShotEnd.home_line` are now the owner line (a loop end's kept end's
  home line included), and a station part's context names it. History
  entries written by RS-PROSE builds before this package carried the slot's
  line as context and no home line for a loop end: a foreign station in them
  still renders in its owner's colour and name (the renderer names the
  owner line itself), a loop end in its slot's colour. That only affects
  unreleased development files, so the format version stays 3.
- **A two-tone shot pill, split before the arrow.** A shot with an end of
  another line paints segments instead of one pill: the from end's half and
  the to end's half (the arrow opens the second), each in its owner's
  colours or the shot's. The foreign end keeps its home-line dot, checked
  against its own half (it is that line's own pill dot). When the from end
  is the foreign one, the leading dot and the context (the shot's line)
  form a head segment in the shot's colour, so the shot's name is never
  printed on another line's colour; with both ends foreign the head, the
  from half and the to half are three colours. Link pills: the halves are
  `inline-flex` segments with no border between them, the later one
  overlapping the earlier by 1px so no hairline of the panel shows at a
  fractional seam, and a zero-width strut in the pill's font gives every
  segment, even a head holding only the dot, the text's baseline and
  height (without it the head came out 15 px tall against 23 px). Prose
  pills: one radial tint per segment, fading as one pill does.
- **The border tracks the line's lightness.** The fills of a near-white and
  a near-black line differ by about 0.03 in OKLCH lightness (the band keeps
  text legible), so in the first capture a near-white half read as a dark
  gap. The border now goes `BORDER_TRACK` (0.22) further for a line toward
  the theme's text pole: on the dark theme a near-white line's border is
  #9d9d9d against a near-black one's #626262, on the light theme #bcbcbc
  against #7e7e7e. The fill band and every text floor are unchanged;
  `twoTonePills.test.ts` sweeps ordered pairs of hard colours (near-white
  and near-black, yellow and blue, greys, primaries, a hue sweep) on every
  theme for both variants: text 7:1, muted 4.5:1, the leading dot and the
  foreign end's dot 3:1, each on its own half, at rest and on hover, and on
  the tint and the panel for prose.
- **Lists of slots keep the slot's line.** A delete's "no longer shared"
  and "loop closures that join" pairs name two slots of one station; in
  the owner's colour both read "Main: 12". They use `StationBlob`'s
  `bySlot`, the slot's own colour and name, as the "Also on" rows name a
  line, and still say whose station in a tooltip and in words.
- **The narrow-panel truncation.** A link pill's context shrinks first
  (`flex: 0 1000 auto`, ellipsized), so the station's name and Station ID
  stay in the pill; a details row's value and the "Also on" row can shrink,
  so a long line pill ellipsizes inside the panel instead of running past
  its edge.
- **Measured in the real app** (this worktree's build, a synthetic survey of
  near-white, near-black, yellow, blue and magenta lines, two 12s on one
  line, three junctions and a loop end; 13 surfaces in dark and light at
  1440x900 and 860x900): prose line pitch equal to plain text's in every
  compared block (18 to 20 blocks, 66 to 97 line gaps per combination, 0 px
  deviation), with 6 to 10 two-tone prose pills on screen; no two tint
  fragments on adjacent lines overlap (44 and 61 pairs); the Playwright
  line-pitch spec now includes a two-tone prose shot.
