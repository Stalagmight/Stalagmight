# SpeleoDB lock/session races: two open P1s and the traps around them

**Status: FIXED on `feat/speleodb-session-generation` (2026-07-31).** The
mechanism is the session generation + commit guard + reclaim-checked releases
in `core/state/speleodb_session.rs` (F7: Start Editing commits through a
generation/identity check under the `project` mutex; F8: the delete clears its
own claim at snapshot time and its captured release stands down for a
successor, serialized under the new outermost `speleodb_lock_ops` mutex; the
5.4 janitor gap is closed in `speleodb_link_status`). Codex round 5 then
hardened three edges of that mechanism: the reclaim check grew a second eye
(`speleodb_live_session_accounts_for` - a successor that INHERITED the
account's still-held lock deliberately mints no acquisition record, so a
record-only check freed the mutex under exactly the sessions the stand-down
exists for); Start Editing's lock-ops span now starts BEFORE the pre-acquire
holder probe (a captured release landing between probe and acquire made
"inherited, keep on unwind" true of a mutex the release then freed, so the
abort arms stranded a lock the call had in fact taken); and the janitor keeps
its claim record through a signed-out poll and restores it after a failed
release POST (a take-first shape erased the only pointer the retry had). Two
pre-existing residuals are deliberately out of scope and noted in that
commit: the detached project-switch/quit releases and `speleodb_update`'s
fast-forward re-record. The canonical-storage change
(`feat/speleodb-canonical-storage`) separately removed the Save-As route to
two linked copies (5.7's Save-As half, D5).

The remainder of this page is the ORIGINAL problem statement, kept as the
rationale record for the fix. Section 6 describes an earlier abandoned
attempt, because the way it failed shaped the design that landed.

All line numbers are as of `6e73c8b9` (branch `refactor/remove-collab-mode`,
PR #355). Every claim below was traced in the code; where something is inferred
rather than read, it says so.

The abandoned attempt is preserved in two places:

- `git stash` entry `abandoned: SpeleoDB session-generation attempt at F7/F8`
- branch `wip/speleodb-session-generation` (an earlier, smaller checkpoint of
  the same idea)

---

## 0. Where these came from

Both findings are external review output (Codex, round 4 on PR #355), raised
against `AppState::delete_project` after that function grew a full session
teardown. Neither is caused by that PR: the underlying shape predates it. What
the PR did was make `delete_project` release a held SpeleoDB write lock, which
put a *deferred* network release next to an *unsynchronised* acquire for the
first time and made the pre-existing gap reachable and visible.

Read `docs/plans` convention note: this page is a snapshot of an investigation,
not a maintained reference.

---

## 1. The structural fact underneath both: there is no session object

This is the single most important thing on this page. Everything else follows
from it.

There is no such thing as an "edit session" value in this codebase. What the UI
and the docs call an edit session is three unrelated process-global slots hanging
off `AppState`:

| Slot | Type | Where |
|---|---|---|
| `edit_enabled` | `AtomicBool` | `app_state.rs:95` |
| `force_editing` | `AtomicBool` | `app_state.rs:102` |
| `lock_acquired_here` | `Option<String>` (a project id) | `speleodb.rs:76`, inside `SpeleoDbSession` |

`SpeleoDbSession` (`speleodb.rs:48-77`) has five fields and none of them
identifies a session: `endpoint`, `email`, `token_store`, `rehydrate_attempted`,
`lock_acquired_here`. It identifies an **account**, not an editing session. There
is no generation, epoch, nonce, or handle anywhere in it.

So the only discriminator in the entire acquire/release/forget machinery is a
**project id string**. That is enough to tell two sessions on different projects
apart. It is not enough to tell apart:

- (b) two sessions on the **same** remote project reached through different local
  files - this is F8;
- (c) the same project **before and after a teardown** - this is F7.

The codebase already has the right primitive for this, twice, in the same struct,
for exactly this class of bug:

- `unrecognised_shot_kinds: Mutex<Option<(u64, Vec<...>)>>` (`app_state.rs:140`)
  versions its slot with a `loading_id` because "a cancelled load keeps running
  until its next checkpoint, so it can write here AFTER the new project already
  did".
- `adjusted_cache_generation: AtomicU64` (`app_state.rs:148`) is "captured before
  the lock-free solve so a result computed against a since-invalidated project is
  not written back".

Neither idiom was ever applied to the SpeleoDB session. That is the gap.

### 1b. Nothing serialises SpeleoDB verbs against project lifecycle

Verified by reading the body of `speleodb_start_editing` (`speleodb.rs:1844-2135`):
it takes **neither `command_lock` nor `self.project.lock()`**, at any point, across
three network round trips.

`command_lock` (`app_state.rs:108`) serialises `execute()`/undo/redo and the
command-family mutations (`command_import.rs`, `command_delete_split.rs`,
`command_split_shot.rs`, `command_query.rs`). It is not taken by any SpeleoDB verb
and not by `delete_project`.

Every SpeleoDB command reaches the state layer through
`run_blocking` -> `tauri::async_runtime::spawn_blocking`
(`commands/speleodb.rs:41-54`; `speleodb_start_editing` at `:182-183`), so these
run on the blocking pool concurrently with each other and with the synchronous
state-layer commands. **The races are reachable.** They are not theoretical.

---

## 2. F7 - a teardown lands inside an in-flight Start Editing

### What happens

`speleodb_start_editing` (`speleodb.rs:1844`) does, in order:

1. read the open project's link (`speleodb_validated_link`)
2. read the token
3. pre-acquire provenance probe, yielding `acquired_here` / `keep_lock_on_unwind`
4. `client::acquire_lock` - **network**
5. `speleodb_server_head` - **network**
6. `sync::reconcile_verdict`
7. commit

The `UpToDate` commit arm (`speleodb.rs:2036`) does:

```rust
self.set_edit_enabled(true);
if acquired_here {
    self.speleodb_note_lock_acquired(Some(&link.project_id));
}
self.set_force_editing(false);
```

The `FastForward` arm (`speleodb.rs:2059`) does the same after re-pulling.

**Nothing between step 1 and step 7 re-validates that the project it started from
is still installed.** If `delete_project` (or `close_project`, or a project
switch) runs in that window, the commit lands on a torn-down world: the gate is
opened and an acquisition is recorded for a project that no longer exists.

The delete side cannot see it either. `delete_project` snapshots the release
params under the `project` guard (`recent_projects.rs:138`, the block around
`:253-298`); an acquire that has not yet reached step 7 has recorded nothing, so
the snapshot returns `None` and the delete releases nothing.

**Net result: a server mutex held with nothing left that can release it**, plus a
backend edit gate open with no project behind it. Other divers are blocked until
the server-side timeout.

### Why nothing cleans it up afterwards

There is a catch-all that clears a stale acquisition record: `link_status` calls
`speleodb_forget_lock_acquired_if_gate_shut()` (`speleodb.rs:747-748`) when the
gate is shut.

It cannot fire here, for two independent reasons:

1. `speleodb_link_status` (`speleodb.rs:709`) **early-returns on
   `NoRemoteProject`** at `:712-728`, before ever reaching that clear. With no
   project open - which is exactly the post-delete state - the catch-all is
   unreachable.
2. `close_open_project_locked` (`project_io.rs:658`) **resets `edit_enabled` to
   `true`** at `:684` (and `force_editing` to `false` at `:686`). That reset is
   correct on its own terms (#191: don't leave a read-only project's gate stuck
   for the next open), but it means a shut gate is not a usable signal for "no
   edit session is live" after a teardown, and an open gate does not distinguish
   "a racing Start Editing opened it" from "the teardown reset it".

So the record survives, and the only remaining backstop is the #202 20-minute
lock re-verify on the `execute()` path (`speleodb.rs:244`, `:1806`) - which needs
a project open to run at all.

---

## 3. F8 - two local files, one remote project

### What happens

`lock_acquired_here` is keyed by **project id alone** (`speleodb.rs:76`). The
compare-and-clear that a previous review round added,
`speleodb_forget_lock_acquired_for` (`speleodb_lock_release.rs:266`, comparison at
`:270`), is therefore blind between two sessions whose project ids are equal.

The interleaving:

1. Session A has project P open from local file `a.stalx`, holds P's server lock.
2. A `delete_project` on `a.stalx` snapshots the release params, tears the session
   down, drops the `project` guard, trashes the file, and **defers the HTTP
   release** to `speleodb_release_captured_lock` (`speleodb_lock_release.rs:302`).
3. Inside that deferral window, the user opens `b.stalx` - a *different* local
   file linked to the *same* remote project P - and hits Start Editing.
4. The server treats a re-acquire by the same account as a **successful no-op**
   (not a 400; only *another* holder produces the contention 400). So B's acquire
   succeeds and B becomes editable under the mutex A took.
5. A's deferred release POSTs. The mutex is freed - **B's mutex**.

The id comparison at `speleodb_lock_release.rs:270` cannot distinguish A's claim
from B's, because both are the string `P`.

### Why it is worse than a wrong bookkeeping bit

The end state is `edit_enabled == true && force_editing == false` with **no server
lock held**. That combination is precisely what the rest of the code treats as an
invalid state: `app_state.rs:99-102` documents that `force_editing == true` is the
only sanctioned "editing without a lock" mode, and the authorship shortcut at
`speleodb.rs:912` (`we_hold_the_lock`) reads gate AND record AND holder together
on the assumption that the unflagged case implies a real lock.

So session B edits believing it is protected, another diver can now take the
mutex, and both push. The only thing that eventually notices is the #202
re-verify, up to 20 minutes later.

---

## 4. Constraints any fix has to respect

These are not preferences. Each one is load-bearing and each has a test or a
review finding behind it. A fix that violates one re-opens a bug that is already
closed.

1. **No network I/O while the `project` mutex is held.** This is the entire reason
   the delete release is split into snapshot-then-release. An HTTP round trip
   inside that critical section stalls every state-layer operation for its
   duration.
2. **The release snapshot must precede the teardown.** It reads the `speleo_link`
   row through the open database handle, and `close_open_project_locked` drops
   that handle. Snapshot afterwards and you ask a `None` database for the link,
   get `NoRemoteProject`, and silently do nothing. (That was the original bug.)
3. **Teardown must precede the trash.** SQLite opens the file without
   `FILE_SHARE_DELETE` on Windows, so closing after the trash is unreachable
   there. See `close_project_if_open_at` (`project_io.rs:723`) for the same
   hazard on the pull/fast-forward replace path.
4. **Identity is decided once, under one `project` acquisition** (a TOCTOU fix).
5. **`same_file` matching, except for a symlink named directly.** Deleting a
   symlink itself must not tear down a session opened through its referent.
6. **`session_closed` is reported on both the success and failure returns**, since
   teardown happens before the trash and a denied trash still means the session is
   gone.
7. **`speleodb.rs` is frozen by the file-size ratchet** at a 1951-LOC baseline
   against a 2000 hard cap (`src-tauri/file-size-baseline.json`, `warn_loc` 1500,
   `fail_loc` 2000). New production code must go in a sibling module. This is a
   real constraint on any fix that wants to touch `start_editing`, because the
   commit arms live in the frozen file.

---

## 5. Footguns

The traps that cost the most time, in rough order of how badly they mislead.

### 5.1 "It is the same project id" does not mean "it is the same session"

The whole F8 class. Any comparison keyed on `project_id` looks correct, reads
correct, and is correct in every single-file scenario. It silently fails only when
two local `.stalx` files point at one remote project - which is a normal thing for
a user to do, since a pulled project is cached *and* can be saved elsewhere.

### 5.2 The server cannot tell you whose lock it is

`speleodb.rs:1855-1861` documents it: re-acquiring a lock the **account** already
holds is a successful server-side no-op. Only *another* holder produces the 400.
So the acquire response is not evidence of authorship, and you cannot ask the
server "did I take this, or did my other session?". This is why
`lock_acquired_here` exists as process-local provenance in the first place, and
why the fix has to be entirely client-side.

### 5.3 `edit_enabled` is reset to `true` on close, so it is not a liveness signal

`project_io.rs:684`. Intuition says a torn-down session has a shut gate. It does
not. Any fix that reads the gate to decide "is a session live" gets the wrong
answer after every close, and cannot distinguish a teardown reset from a racing
acquire.

### 5.4 The stale-record catch-all cannot run when there is no project

`speleodb_link_status` early-returns on `NoRemoteProject` (`speleodb.rs:712-728`)
*before* the `speleodb_forget_lock_acquired_if_gate_shut()` call at `:747-748`.
So the one janitor that would clean up a stranded record is unreachable in exactly
the state F7 leaves behind. It is easy to read the catch-all's existence as
covering this. It does not.

### 5.5 `speleodb_release_best_effort` blanks the record unconditionally

`speleodb.rs:3023`, clear at `:3039` (`self.speleodb_note_lock_acquired(None)`).
The comment at `:3030-3038` justifies it: every existing caller decides and clears
in one breath, so no other acquisition can have landed in between, and clearing a
record naming another project "can only cost the shortcut, never grant it".

That reasoning is sound **for its current callers** and stops being sound the
moment a new caller can fire *after* the session moved. This is the trap that
killed the attempted fix (section 6): adding refusal/unwind paths to
`start_editing` routed into this function, and those paths fire precisely when
another session has taken over. It is a correct helper with a precondition that is
documented in prose and enforced nowhere.

### 5.6 The delete target and the fast-forward destination are the same file

A pulled SpeleoDB project's `.stalx` lives at
`<cache>/speleodb/<project_id>/project.stalx` (`speleodb_cache_path`,
`speleodb.rs:1790`). `speleodb_fast_forward` (`speleodb.rs:3065`) downloads and
writes to exactly that path (`dest`, `:3079`). So for a remote project, a
`delete_project` and an in-flight fast-forward are contending for the *same file*,
not merely for the same session state. The FF's first act is
`client::download_dump` (`speleodb.rs:3077`) - seconds of network with no
re-check afterwards.

### 5.7 The `project` slot is not the only thing that identifies "which project"

If you version the `project` slot, `save_project` (Save As) is a gap: it swaps the
database handle to a **different file** and rewrites `project.file_path` without
replacing the project slot. Same for the error-recovery reopens in the same
function. A scheme that bumps only on project-slot writes will consider a Save-As
to be the same session while the database underneath has changed.

Conversely, `restore_snapshot_into` (reached from `rollback_project`) writes the
project slot and *should not* count as a new session - an undo restore is the same
open project. So "writes the slot" and "is a new session" are not the same set in
either direction.

### 5.8 Logout drops the gates without touching the project slot

`speleodb_logout_drop_gates` (`speleodb_logout.rs:345`) releases the held lock and
shuts both gates. It does not replace the project. So any slot-versioned scheme
sees no change across a logout, and an in-flight acquire can commit a gate and a
claim over a token that has just been forgotten.

Worse in the read-only case: it **early-returns at `:346`** (`if !self.edit_enabled()
{ return; }`), so it releases nothing, and a mutex a racing thread took is then
stranded with no token left in the process to release it with.

### 5.9 A refusal path can leak the very lock you are trying to protect

If Start Editing detects mid-flight that its session is gone, it has already
acquired the server lock at step 4. Abandoning quietly leaks it. Releasing it is
right in the common case and *wrong* if a replacement session has meanwhile
inherited that same mutex (5.1 again). This is not an implementation detail to
settle later; it is the hardest single decision in the problem, and getting it
wrong converts an F7 fix into an F8 instance.

### 5.10 Lock ordering, for whatever ends up being written

The established order is `project` outermost of `{project, database, undo_stack}`.
Verified during this investigation: no SpeleoDB code path holds the session mutex
across a `project.lock()`. The `project -> speleodb` edge already exists in
production (`recent_projects.rs` holds `project` across
`speleodb_abandon_lock_release_params`). Any new code must not create the reverse
edge.

Note also that `docs/undo-redo.md` documented this ordering **incorrectly** until
this branch fixed it (it claimed `command_lock -> undo_stack -> project`, which
`save_project` already contradicted). Do not trust a prose statement of lock order
without enumerating the acquisitions.

### 5.11 Test-methodology traps hit during this work

- Restoring an instrumented file with `mv` from a backup **preserves the backup's
  older mtime**, so `cargo` reuses the stale object and the next run reports a
  phantom pass or failure. Restore by rewriting the file (or `touch` it after).
  Verify restores with `sha256sum`, not by eye.
- A test named for session-based refusal may actually be exercising the
  `project.is_none()` branch instead, and still pass when the session comparison
  is deleted. Prove non-vacuity per *mechanism*, not per test name: mutate the
  specific line you believe the test covers.
- `commands::memory::tests::does_not_include_unrelated_processes` fails on a
  developer box with a dev server running (it sees WebKit processes). Environmental,
  not a regression.


---

## 6. What was attempted, and why it was abandoned

**Design:** a monotonic `ProjectSession` generation on `AppState`, bumped by every
project-slot writer, captured by anything that defers work, and compared before
the deferred work commits. `lock_acquired_here` became a `LockClaim { session,
project_id }` so a claim could be attributed to the session that took it.
Start Editing captured the session at entry and refused to commit if it had moved.

This is the right shape - it is the same idiom `adjusted_cache_generation` and
`unrecognised_shot_kinds` already use (section 1). It is not why it failed.

**It failed on the edges, not the core.** Two independent adversarial reviews both
returned BROKEN, agreeing on the primary defect:

| # | Defect |
|---|---|
| D1/D2 | The F7 refusal unwind routes into `speleodb_release_best_effort`, which has no session guard and blanks the record unconditionally (footgun 5.5). The new refusal sites fire exactly when the session moved, so the F7 fix **re-opens F8** through its own unwind path. |
| D3 | The fast-forward pre-check reads the session and *then* calls `speleodb_fast_forward`, whose first act is a multi-second `download_dump`. A delete landing in that window is unprotected, and it is deleting the FF's own destination file (footgun 5.6). |
| D4 | Logout does not bump, and early-returns in the read-only case (footgun 5.8). |
| D5 | Save-As swaps the database without bumping (footgun 5.7). |
| D6 | The session was captured *before* the link was read, so the two can name different projects; the unwind then releases the lock of the project the user actually has open. |
| D7-D10 | The source-scanning tripwire only covered three files, so a sixth installer elsewhere would be invisible; the `lock_acquired_here` doc still claimed three writers when there were four; two tests were named for session refusal but actually exercised the `project.is_none()` branch (footgun 5.11). |

A repair round addressed all ten. It was cancelled before its adversarial pass
returned a verdict, so **the repair is unvalidated** - it is not known-good and it
is not known-bad.

**The honest read:** the generation is necessary but not sufficient. The release
and unwind paths were each written with a documented precondition ("my caller
decides and clears in one breath") that a new caller silently violates, and there
is no mechanism that enforces those preconditions. Until every release path is
session-keyed by construction rather than by each caller remembering to be, adding
a guard in one place keeps pushing the same bug into the next unguarded path. That
is what four review rounds of this actually looked like.

---

## 7. Scope note

None of this is caused by the collaboration-mode removal that PR #355 is nominally
about. The chain was: collab removal touched `delete_project` (it deleted a sibling
`.tml` there) -> review found a pre-existing teardown bug -> fixing it required the
full teardown -> which required teardown-before-trash -> which required `same_file`
-> which broke frontend parity -> which needed `session_closed` -> which exposed a
TOCTOU -> then symlink semantics -> and finally the SpeleoDB locking protocol,
because the state-layer `close_project` bypasses the IPC `close_project_impl` that
releases the server lock.

Eleven review findings, all in that adjacent fix. The removal itself drew zero.
These two P1s are the tail of that chain and belong in their own change.
