# Undo/Redo System

## Overview

Every state-changing operation is a `Command` enum variant or a recipe that
runs through the same pipeline. The pipeline snapshots the survey before the
edit, runs it, and records the difference as one id-keyed **changeset**
(`models::survey::changeset`, API in
[survey-model.md](survey-model.md#undo-engine-changeset)). Undo steps the
changeset back, redo forward; the same entry moves between the two stacks. Any
new edit clears the redo stack (standard diverging-timeline semantics).

Design: [plan section 8](plans/2026-11-20_station-graph-model.md#8-undo-and-redo)
and its stream U and C0 Deviations.

## The changeset engine

### One payload type

Every undoable step records one `Changeset`: what changed, as before and
after images keyed by stable ids (stations by Station ID, lines by line id,
shots by uuid through per-line runs, closures, datums, placements,
Sections and import hashes as list hunks, the project fields, the line
order, the counters). There is no per-command capture code: the command runs,
then `capture(&before, &after)` diffs the two model states after the whole
epilogue (sweep, anchor settle, validate).

- **One entry serves both stacks.** A changeset is symmetric: undo steps it
  from its after state to its before state, redo the other way. Nothing is
  re-encoded on a step, and an entry is self-describing rather than relative
  to its neighbour.
- **Preconditions instead of re-derivation.** A step refuses, and changes
  nothing, unless the live project is exactly the recorded state on
  everything the changeset touches: every recorded field, every station
  image, every list's length and replaced stretches, the line order, and
  the content hash of every line whose shots it rebuilds (before the step
  for the state it reads, after the step for the state it builds). The
  refusal names what differs (`Refusal`). A refused entry stays on its
  stack.
- **Nothing derived is stored**, so nothing is re-measured or healed after a
  step: positions, closure metrics, alignment, gauges and datum roles are
  recomputed from the model. `undo_to` / `redo_to` step N changesets and
  bump the revision once.
- **Counters** go to `max(live, recorded)`: an undo of what minted an id
  leaves the counter standing, so a redo reinstates exactly the ids it
  removed and nothing hands an id out twice. `next_line_id` is derived
  (`max + 1`); a stale entry that names a reused line id fails its
  preconditions.
- **Session shot ids** are never persisted (plan 1.1). An entry captured in
  the session carries each touched line's ids in memory, so a step gives
  every shot its id back; a decoded entry mints ids for the shots it
  re-creates (copied shots keep theirs).
- **Bit-exact.** Undo and redo land on the recorded state to the bit:
  canonical model text and the `.stalx` v2 bytes are identical, the
  counters may stand higher. Pinned by the property tests, the size gates
  and the undo fuzzer.

**How an entry is held** (C0 decision): an `UndoEntry` (`core/undo.rs`) keeps
the decoded `HistoryEntry` (time, kind, record-time label, refs, recorded
preview rows, changeset; session shot ids exact for an in-session capture) AND
its canonical text (`encode_entry`, once at push or as read from the file).
The byte budget counts the text, which is what a save writes and a push
carries; a save copies it rather than re-encoding the history.

### The execute pipeline

One pipeline for `execute()`, a `Batch`, and every recipe
(`core/state/commands.rs`, `AppState::run_recorded`; plan 9.1):

1. `command_lock` (the caller holds it), the read-only gates (the SpeleoDB
   edit session through `guard_mutation`; a file made with a newer
   Stalagmight);
2. the `Record`: kind, label worded now, stable refs, the preview rows the
   changeset cannot say (`core/state/labels.rs`);
3. `let before = project.clone()`;
4. the model operation(s); a refusal restores `before`;
5. `validate()`; a violation restores `before` and refuses;
6. `let changes = capture(&before, &project)`; an empty changeset is a
   provable no-op: nothing is pushed and the redo branch is kept;
7. stamp `modified_at` (the engine never touches it), bump the revision, push
   the entry (redo cleared), mark the file dirty, set the SpeleoDB dirty bit
   and drop the Unwind position.

If the entry of a successful edit cannot be encoded (a model bug: `validate`
refuses non-finite numbers), the edit stands and both stacks are cleared, so
undo never steps over a missing entry.

**Recipes.** Split Line Here, Combine Lines, the delete-and-split family,
split shot at distance and Insert Station, imports and create-and-merge, Add
.stalx, project settings and surface elevation are `AppState` methods rather
than `Command` variants, because one user action is several model steps
(Split may insert a station first; Combine may reverse the other line first).
Each takes `command_lock`, calls `guard_mutation(..)`, and runs `run_recorded`
with its own `Record`, so it is one entry like any command.

**Batches** (`Command::Batch { commands }`) are atomic: every sub-command runs
through its family's `apply` on the same project and the same record, and the
pipeline's one snapshot covers them all; any refusal rolls the whole batch
back. Only the commands `AppState::is_batch_allowed` lists may be batched (it
also keeps a path-bearing lifecycle command out of an uploaded history). A
batch that moves several slots of one station (the map's co-located drag)
moves that station once.

**Add .stalx** (#674) is one entry on top of the project's history. Its
changeset holds what the add brought (the copied lines whole, their stations,
datums, placements, closures and Sections, the import hashes, the project
fields it set), never the cave it was added into. The command's `path` is
never recorded; only the sanitized file name is in the label.

**Non-undoable commands** are never pushed: `OpenProject`, `SaveProject`,
`CloseProject`, `CreateProject`, `MergeProjects`. Display toggles, SpeleoDB
state and Ignore / Unignore of a warning are not undo steps either (the
ignores ride the next save as survey content, but are not in the history).
An ignore keyed by a shot the survey no longer has stays in the file while a
saved undo or redo step can bring that shot back (file-format.md 5.13), so an
undo after a reopen finds it, as within the session.

### Undo, redo, jumps

`undo()` / `redo()` (`core/state/undo.rs`) take `command_lock`, pass the same
gates (a read-only SpeleoDB session or a read-only file refuses), pop the
entry and step it (`changeset::step`):

- on a refusal the entry goes back where it was and the refusal is the error;
- an entry decoded from a file (no session ids) is stepped on a project kept
  aside first and validated after its step; on a violation the project is put
  back from that copy and the step is refused. Not the inverse step: a step
  checks only the side it reads, so a crafted entry's other side (its runs,
  edits or hash) can be refused, and the survey must never be left invalid;
- on success it moves to the other stack, `modified_at` is stamped, the
  revision is bumped, the file is marked dirty and, on a linked project, the
  SpeleoDB dirty bit is set (an undo is not a forward edit: the Unwind
  position stays).

`undo_to(i)` / `redo_to(i)` step `i + 1` entries and bump the revision once. A
refusal mid-jump stops there: the steps already applied stay, the failing entry
goes back, and the error says how far it got (`Undo failed at step 3 of 5:
...`). `reversed_lines` (the lines a step renumbered, for the frontend's pin
mirroring) comes from each applied entry's digest. With no project open the
answer is "No project open"; with an empty stack "Nothing to undo" / "Nothing
to redo".

### Labels and previews

Everything an entry says about itself is worded **once, at record time**, with
the names before the edit (`core/state/labels.rs`): the label (0.6.0's
`describe` rules), the refs, and the preview rows the changeset cannot say
(a batch's first 8 sub-commands with `more`, a move's offset and affected
lines, merge, closure and datum rows worded by slot). Everything else the
hover shows is read off the changeset's digest when the history is listed
(`core/undo/history_item.rs`): a shot edit's cells, a kind change, a created
shot, a created or deleted line, a line metadata edit, a credit edit, a
station comment. An entry therefore never changes after later edits: a
reverse, an insert or a rename does not make an older entry name another
station, and a deleted line stays named. Batch rows name lines and stations
as they were before the batch. K's characterisation dump was the oracle for
the wording when it was ported (compared with 0.6.0's once, plan C18); in
CI the wording is pinned by `core/state/history_labels_tests.rs` and
`batch_label_tests.rs` (the dump, `characterization_dump_tests.rs`, runs on
demand).

### Persisted entries

`history: {"undo": [...], "redo": [...]}` in the v2 file
([file-format.md 5.15](file-format.md#515-history)), one entry per line, each a
stack whose last element is the next step:

```text
{"v":1,"t":"2026-11-20T12:59:00.000000000Z","k":"update_shot","label":"...",
 "refs":{...},"preview":{...},"ops":{...}}
```

`encode_entry` writes it (already in the format's canonical form;
`RawJson::parse` leaves it unchanged); `decode_entry` reads it as untrusted
input (size, version, strict layout, `EntryLimits`, then the structural rules
that also run before every step: ids in their spaces, runs inside what they
read, each source shot copied at most once, columns and hunks in order and in
range). The `Command` is never persisted: its payload holds session shot ids
and positional slots.

- **Save** (`AppState::history_to_persist`): the newest
  `general.undo_persist_limit` (default 20) entries of each stack on a count
  window (a local project, or a SpeleoDB project in Disabled mode); both
  stacks whole in Full mode, where the byte budget bounds them (below).
- **Load**: every entry is decoded; one that fails drops the whole history
  (both stacks), because an entry is only meaningful on the state the one
  before it left. `EntryError::Version` of an older version is the "earlier
  version" banner, anything else "could not be read". A legacy file's history
  is always dropped (plan 7.2.8).
- **What bounds a load**: the file's own size cap and count limits (a history
  of more than 65,536 entries is dropped, not refused) and `EntryLimits` per
  entry (64 MiB of text, 4,096 refs of each kind, 64 preview rows, 4,096
  characters per string). An entry decodes in time linear in its size
  (`changeset/entry_tests.rs` pins hostile entries).
- **The banner.** `AppState::history_discarded` (and
  `history_discarded_from_earlier_version`) is what `history-discarded-banner`
  in `App.tsx` reads. It is written under the same `project` acquisition that
  commits the project it describes, and every project-slot commit writes it,
  loads and creates alike, so a clean load clears a previous project's flag. A
  discarded history is never silent ([notifications.md](notifications.md)).

### The size gate

`persistence/src/persistence/history_size_gate_tests.rs` builds the gate
surveys with the survey API (a tree of 100 or 400 lines of 50 shots on
junctions, a snap closure every 25 lines, a datum, a Section, an island and
a leaf; with a 2,000-shot line for the credit gates), runs every gated
action, and fails if any entry (the persisted JSON, label included) is over
1% of the survey's v2 file (`stalx_v2::encode`, empty history), or if any
undo, redo or second undo misses the recorded state to the bit, in the
session and through the persisted entry (on 5k shots also by the v2
bytes). A paste costs at most 4,000 bytes plus 40 per changed cell. Run it
with `--nocapture`, or `STALAGMIGHT_UNDO_SIZE_REPORT=<path>`, for the table.
`core/state/undo_size_gate_tests.rs` gates what the pipeline adds on top (the
label, refs and recorded rows of each command kind).

| Action | Old engine (SQLite file) | Changeset (v2 file) |
|---|---|---|
| Delete line, 5k shots | 0.73% | 5,104 B, 0.80% of 640,894 B |
| 1,000 station names, 20k | 0.86% | 14,809 B, 0.57% of 2,577,031 B |
| 1,000-cell paste, 20k | 0.77% (33-36 KB) | 10,679 B, 0.41% |
| Paste, per changed cell | at most 40 B | 10.8-11.2 B |
| Credit over a 2,000-shot line | 331 B | 381 B |
| Credit over 1,000 shots in 40 lines | 2.4 KB | 7,153 B (0.79%) |
| Reverse a 2,000-shot line | - | 295 B |

Every other action is under 0.4% on 5k shots and 0.1% on 20k (full table in
the stream U report). The denominator is the v2 file, about half the SQLite
file, so the 1% cap is about twice as tight as before (R2 H6); every gate
still passes with room.

### Station identity across undo and redo

Stations are entities with stable ids, so identity needs no epilogue: a
changeset records a created or deleted station as its whole image (id, name,
comment, aliases, external id) and a changed one by its changed fields, so an
undone delete restores the same id and name and a redone insert re-creates
the station with the id and name it had. The station counter goes to
`max(live, recorded)`, so a later edit never takes an id a redo needs.

### Shot ID display positions

Shot IDs are internal and opaque: `line_id * 100_000 + sequence_order`
(0-indexed) after a load, plain sequential integers before the first save.
`shot_display_position(shot_id, line_id)` in `models/src/shot.rs` converts
them to 1-indexed display positions for both forms. User-facing text never
shows the raw id: labels name shots by their stations (`Update shot Main
Line: 4 → 5`).

## Hybrid mode for SpeleoDB projects (#303)

A SpeleoDB-linked project's file is committed to a git repo on every push, so
what its undo stack holds at push time is what that commit stores forever. Two
policies, resolved by `AppState::undo_policy` (`core/state/undo_policy.rs`)
from the link's `undo_mode` (per project, in the file's `speleodb` member, so it
rides to the server) then `SpeleoDbConfig.undo_mode` (the application
default). A missing (legacy NULL) mode is materialized with the default on
open and saved at once by a local-only save that copies the file's history
byte for byte (the open never cuts it to the persisted window; a version 2
file only, a legacy file keeps it for its upgrading save; see
[saving.md](saving.md#what-an-open-writes)). Existing per-project values continue to load and apply; the workspace
no longer offers a policy editor:

| | `Full` (default) | `Disabled` | Local project |
|---|---|---|---|
| In-memory depth | byte budget | `undo_history_limit` | `undo_history_limit` |
| Persisted (save) | both stacks whole | newest `undo_persist_limit` per stack | newest `undo_persist_limit` per stack |
| Rides the push | undo only, cut (below) | no | n/a |

**The byte budget** (`UNDO_BYTE_BUDGET_BYTES` in `core/undo.rs`, 4 MiB of
persisted JSON, both stacks together; plan 8.3 lowered it from 32 MiB, which
was sized for gzip in SQLite) replaces the entry-count window rather than
joining it. Count is the wrong unit here: an entry is as large as what its
command changed - a few hundred bytes for most, the deleted data for a delete,
the whole added file for an Add or an import. Past the budget the oldest undo
entries go first, then the furthest redo entries.
`AppState::sync_undo_depth_limit` re-arms the bound wherever the policy can
change (an open, a close, a mode change, a config reload).

**What a push carries** (`push_snapshot`, `Mode::ForPush` in the encoder): no
`local` and no redo, ever; in Full mode the undo entries newer than the
inherited-history watermark (`local.speleodb.inherited_through`), then the
oldest of those cut until the whole file fits the 5 MiB upload limit (the
newest kept). An Unwind position recorded without a watermark ships no undo
at all (the inherited extent is unknown). "Uploaded without undo history" is
reported only when every entry was cut; a partial cut is logged. **Redo is
never uploaded**, in any mode: a redo branch replays FORWARD onto a state the
server has now committed past.

New entries are stamped strictly after the newest undo entry and after the
watermark (`UndoStack::next_stamp`), so timestamps strictly increase and a
push's watermark cut never takes the user's own work.

### When the stack is cleared

| Event | Effect | Why |
|---|---|---|
| Confirmed push | undo entries stamped at or before the push cutoff dropped, redo cleared, the saved history cut the same way, the Unwind position and watermark cleared; one local-only save (`speleodb_clear_undo_after_push`) | they are in the commit now, so keeping them would duplicate them in the next one. Bounded by the cutoff because an edit recorded after the snapshot keeps its dirty bit, so its history has to survive with it |
| A server copy becoming the local cache file | the download's history emptied before it is written (`InheritedUndo::Drop` in `prepare_download`: pull, fast-forward and revision swap) | the history describes another session's operations |
| Lock acquired over a copy that equals git (`UpToDate` + clean) | stacks cleared, made durable at once (`speleodb_clear_inherited_undo`) | the history describes already-committed operations. The clean check is what lets lock, edit, unlock-without-pushing, re-lock keep the user's own history |

The download is cleared in its bytes, not only in memory, because the open
reads the file's history into the live stack. The reason it is cleared at all
is #194 b3: an adopting collaborator pressing Ctrl-Z would replay the pusher's
operation against a tree they have not looked at.

### Unwind: undoing past a commit

> **No UI.** #668 removed the Unwind / Redo-into-next-version entries, and #671 removed
> the target commands and the confirmation dialog that only they opened. What is left is
> the backend path: `speleodb_restore_revision` with `keep_undo_history: true` still
> enforces every rule in this section, but no frontend surface sends that flag.

`sync::unwind_target` walks BASE first, then first-parents: after a push the
working copy IS the new commit and its history was just cleared, so the first
Unwind loads that same commit (the undo list refills with the operations just
pushed); undo through those and the next Unwind loads the parent, and so on
back to the root. `sync::rewind_target` is the mirror, finding the child on
BASE's own first-parent chain. Mechanically it is the revision swap with
`keep_undo_history: true`: the download keeps its undo entries (never redo).
A revision an earlier Stalagmight pushed is a SQLite file whose history is
never converted (plan 7.2.8), so a step onto one is refused after the download
and before the swap, with nothing replaced and no position recorded.

- **The position** is `local.speleodb.unwound_from` (per machine, never
  uploaded), on disk so an Unwind interrupted by a restart resumes. It is
  cleared by a confirmed push, a lock acquisition, a fast-forward, a plain View
  or Restore, and every forward edit (`mark_remote_forward_edit`).
- **The guard** (`speleodb_guard_unwind`): the stack that must be empty is the
  undo stack backward, the redo stack forward; dirt must be a previous Unwind's
  own doing, proven by the position, unless the caller acknowledged discarding
  un-pushed work (`unwind_discards_unpushed`).
- **Loaded history does not ride into the next commit.** The loaded entries
  stay in the stack so the user can keep undoing, and the push cuts them by the
  watermark: `local.speleodb.inherited_through`, the instant through which
  everything was inherited, stamped as `max(now, newest loaded entry)`. A
  timestamp, not a stack position, so it survives undo, redo and truncation. It
  outlives a forward edit (that is the point) and is cleared where the stack
  legitimately becomes all-this-session again (confirmed push, lock
  acquisition, fast-forward, any `InheritedUndo::Drop`). Unwinding to an
  ancestor and pushing without editing yields a commit carrying no undo
  history at all.

The swap's follow-up records the carried display settings, the Unwind position
and the watermark in one local-only save (`speleodb_after_swap`).

## Lock Ordering

`command_lock` is always outermost for edits, undo, redo and the recipes.
Below it the order is `project` -> `file` -> `undo_stack` -> `derived` (`file`
took the place the database handle had; `derived` is a leaf). Where more than
one is held they are taken in that order and nothing holds a later one across
an earlier one's `lock()`:

- `run_recorded` holds `project` through the mutation, takes `file` to mark it
  dirty and `undo_stack` to push, then drops `project` before the SpeleoDB
  dirty bit (`mark_remote_forward_edit` makes a local-only save, which takes
  `project` itself);
- `save_with` holds `project`, then `file`, then `undo_stack` for the window
  it writes;
- the local-only durable save runs under the `project` guard (so a toggle
  racing an undo can never write the pre-undo body with the post-undo stacks),
  then takes `file` and `undo_stack`; a caller that already holds `project`
  uses `persist_local_state_at` / `speleodb_mutate_link_held` instead, because
  `std::sync::Mutex` is not reentrant;
- the derived-view cache is read and written under `project`, and the
  Computed View is solved on a clone outside every lock;
- the close teardown `close_open_project_locked` runs under its caller's
  `project` guard, so `delete_project` decides *whether* to close and closes in
  one acquisition (a concurrent load cannot install a different project
  between the check and the teardown).

A project load does not take `command_lock`: it commits under one `project`
acquisition (`commit_opened` / `commit_opened_locked`), and the full load keys
the raw layout it seeds by the revision its commit made, so an edit racing the
load cannot leave a stale cache.

One lock sits OUTSIDE even `command_lock`: `speleodb_lock_ops`
(`core/state/speleodb_session.rs`), which serializes every SpeleoDB **lock
operation** against every other and is deliberately held across network calls.
Start Editing takes it before its first read (generation, link, token, holder)
and holds it through the commit; so do the reclaim-checked releases, the status
refresh's short lock-holder probe, logout, and a settings save or reload that
moves to another SpeleoDB instance (taken before `config_write_lock`) - each of
which can invalidate a fact another one of them is mid-way through acting on.
Nothing holding `project` / `file` / `undo_stack` / `config_write_lock` may
block on it - its users decide under `project`, drop that guard, and only then
take it. No synchronous Tauri command may block on it either: those run on the
main thread, and the span lasts as long as an upload or download. The partial
config writes never take it, and Switch to Read Only Mode drops `command_lock`
and `project` before its release request. The SpeleoDB session generation
(`speleodb_session_generation`) is likewise only ever bumped while holding
`project`; see the module doc in `speleodb_session.rs` for the rules.

The read-only edit gate that every mutation entry (`guard_mutation`) checks is
not a stored flag but a function of the lock claim bound to the editing session
(`core/state/speleodb_edit_gate.rs`): a local project is always editable, a
SpeleoDB project only while the signed-in account holds a claim on it bound to the
current session generation. Reading it takes the open-project gate record (the
innermost lock of all) and then, separately, the SpeleoDB session mutex, under
which nothing but that record is ever taken - so `edit_enabled()` must never be
called while holding the session mutex. An in-place reload bumps the generation and carries the binding in one
acquisition of the session mutex, so a reader never sees the gate of a session
that kept editing flicker shut. Undo and redo on a SpeleoDB project are therefore
refused exactly when there is no edit session, including right after Take remote,
whose reload carries no claim. A Restore that loads a revision's undo history keeps
its edit session by carrying the claim through the swap; a View carries none.

`core/state/concurrency_tests.rs` drives one `AppState` from several threads
over all of these paths and checks liveness, no poison, a coherent derived
cache and a guard that describes the file on disk.

## History UI

SpeleoDB revision inspection and restore/revert live in the left workspace History
section; Unwind/Rewind has no UI (see above). They are separate from the local undo/redo dropdown,
which closes when its stack becomes empty. Revision editing requires Survey mode
and the editing lock.

The IPC (`src-tauri/src/commands/history.rs`) is unchanged: `undo`, `redo`,
`undo_to`, `redo_to`, `can_undo`, `can_redo`, `get_undo_stack`,
`get_redo_stack`, each item an `UndoHistoryItem` (`index`, `description`,
`timestamp`, `commandType`, `previewData`). `get_undo_stack` /
`get_redo_stack` build each item from its `HistoryEntry` alone
(`history_item`): the record-time label and kind, the time, and the preview.
Never from a `Command` (the entry keeps none) and never from the live project,
so an item reads the same deep in the stack, after an undo, a redo, a reverse,
an insert or a reopen.

### Dropdown (`UndoHistoryDropdown.tsx`)

- Opens on ~300ms hover over the undo/redo toolbar button
- Shows up to 5 items with icon, truncated description (40 chars), and relative timestamp
- **Click item**: jumps to that stack position (`undo_to`/`redo_to`) and closes.
  A jump lands exactly where the same number of single steps lands: each step
  is the same changeset step `undo()`/`redo()` takes, and nothing derived is
  re-measured in between (the undo fuzzer's JUMPS oracle pins it)
- **Click button while open**: performs single undo/redo, refreshes the list, stays open
- **Keyboard**: Arrow keys navigate, Enter selects, Escape closes
- Actions are queued and processed sequentially (`actionQueueRef`)

### Detail Preview Popup

Appears on item hover, positioned to the right of the dropdown (flips left if near viewport edge).

**Rendering patterns:**
- **Both old + new value**: `Field: old → new` (e.g., `Length: 10.00 → 15.50`)
- **New value only** (info display): `Field: value` (e.g., `Offset: 5.3m NNW`)
- **Old value only** (removed): `Field: ~~value~~` (strikethrough)
- **Batch sub-commands**: Description as gray header, details on indented line below

### Name Resolution

Labels and previews never show the positional ids (`line:index`, shot ids):
lines, stations and shots are named the way the user reads them -
`Update shot Main Line: 4 → 5`, `Merge stations Main Line / 0 and Side / 1`,
`Delete line "Main Line"`. Each line shows a junction by the name that line
knows it by, and a label names a station by the name its slot's line shows; a
detach names the line that leaves and the station once, in the menu's words
(`Detach line "Side" from station 0`, by Side's name for it).

The names are worded **at record time**, with the names before the edit
(`core/state/labels.rs`), and stored in the entry. A metadata edit that renames
a line is labelled by its old name, with the row `Name: old -> new`; a
station rename is labelled with the name the station had. A batch row names
its lines and stations as they were before the batch; a line created earlier
in the same batch is `-`. 0.6.0 resolved single-command labels and batch rows
when the history was listed; the record-time rule makes every entry stable
(declared in the plan's core Deviations).

**Recipe labels.** A recipe words its own label: `Insert station Main Line /
4a`, `Combine lines "Main Line" and "Downstream"`, `Create line "Side Passage"
at Main Line / Junction` (a New Line snapped onto a station), `Split line
"Main Line" at 5`, `Delete shot 4 → 5 and split line "Main Line"`, `Delete 3
shots (1 → 4) from line "Side"`, `Delete line "Stub" with its last shot`,
`Import 5 lines ("A", "B", "C" and 2 more)`, `Add 'cave.stalx' to project`. A
batch the frontend sends (a data-table paste, Reverse Lines, a move snapped
onto a station) is summarized from its sub-commands (`Update 2 shots in line
"Main"`, `Reverse lines "Main" and "Side"`, `Create line "Grid 01" and 9 more
changes`). A batch's preview lists its first 8 sub-commands, then `+N more`
counting every sub-command.

### Preview Data (`core/undo/history_item.rs`)

Per kind, in this order:

- **recorded rows** win: what the record worded by a slot or from the command
  (merges, closures, datums, moves, a batch's sub-commands) and what a recipe
  recorded;
- otherwise the **digest**: a shot edit's changed cells, old -> new, at most 5
  (`SHOT_EDIT_ROWS`; only the cells that changed); a kind change; a created
  shot's length and azimuth; a created or deleted line's name and shots; a line
  metadata edit's fields, at most 3 (`LINE_EDIT_ROWS`; Surveyor and Date read
  off the shot credits on a line with shots); a credit edit's names and dates
  over the shots it changed; a station comment, old to new, 80 characters,
  `(removed)`;
- otherwise one `Action` row repeating the label, so a hover is never empty
  (`previewData` is never null).

## Key Files

| File | Contents |
|------|----------|
| `src-tauri/crates/models/src/survey/changeset.rs`, `changeset/` | The engine: `Changeset`, `capture`, `step`, history entries (`entry.rs`), the digest, the test kit |
| `src-tauri/crates/core/src/core/undo.rs` | `UndoStack`, `UndoEntry`, `DepthLimit` and the byte budget, `HistoryDropped` |
| `src-tauri/crates/core/src/core/undo/history_item.rs` | `UndoHistoryItem` and its preview, built from an entry |
| `src-tauri/crates/core/src/core/state/undo.rs` | `undo()`, `redo()`, `undo_to()`, `redo_to()` |
| `src-tauri/crates/core/src/core/state/commands.rs` | `execute()`, `run_recorded`, `apply_command`, the batch allowlist |
| `src-tauri/crates/core/src/core/state/labels.rs` | Record-time labels, refs and preview rows |
| `src-tauri/crates/core/src/core/state/undo_policy.rs` | `undo_policy()` (the Full/Disabled resolver), the depth bound |
| `src-tauri/crates/models/src/undo_mode.rs` | `UndoMode`, `UndoPolicy` |
| `src-tauri/crates/core/src/core/state/speleodb_push.rs` | The push snapshot, clear-on-push |
| `src-tauri/crates/core/src/core/state/speleodb_cache.rs` | `InheritedUndo` and the drop-on-adopt chokepoint |
| `src-tauri/crates/core/src/core/state/speleodb_revisions.rs` | Unwind and rewind targets, the guard, the watermark |
| `src-tauri/crates/speleodb/src/speleodb/sync.rs` | `unwind_target` / `rewind_target` - the BASE-then-first-parents chain |
| `src-tauri/crates/persistence/src/persistence/history_size_gate_tests.rs` | The 1% size gate on the v2 file |
| `src-tauri/crates/core/src/core/command.rs` | `Command` enum and `description()` method |
| `src-tauri/src/commands/history.rs` | Tauri commands for stacks |
| `src/components/SingleSystemView/UndoHistoryDropdown.tsx` | Frontend dropdown + preview popup |
| `src-tauri/crates/models/src/shot.rs` | `shot_display_position()` helper |
