# SpeleoDB sync fixes: F1, F2, F4, R3 (stacked on #827)

Four pre-existing SpeleoDB sync bugs found by the SpeleoDB protocol fuzzer,
present on `release/0.6.0` as well as on #827's station-graph model and `.stalx`
version 2:

- **F1**: an open whose reply is lost refuses with Retry instead of opening the
  cached copy offline.
- **F2**: an "un-uploaded" mark left over a copy that holds exactly its BASE
  revision (an edit lost in a crash or closed with Don't save) makes every reader
  treat it as work to protect.
- **F4**: a lock release stands down for another account's stale claim, so the
  releasing account's lock stays held on the server.
- **R3**: a second session of the same account uploads over the other session's
  revision, which is then only in the history.

Each was fixed at its root cause on its own branch, reproduced first by a test
that fails on the code before the fix, with regression tests whose injections are
stated, and merged into one PR stacked on #827. The sections below are one per
bug: the trace, the mechanism, the decision, the tests. [How the fixes
meet](#how-the-fixes-meet) checks them against each other (one interaction needed
a fix of its own), and [What users notice](#what-users-notice) lists the visible
changes. Everything is tested offline: loopback listeners stand in for SpeleoDB,
and no test or manual check here contacts a shared or real server.

## F1: an open whose reply is lost opens the cached copy offline

### Trace

Seed 900366, step 10: an open of a project with a clean cached copy, with every
GET under `/api/v2/projects/` dropped before any reply, refused with Retry
("Network error talking to SpeleoDB: error sending request") instead of opening
the cached copy offline.

### Mechanism

There were two classifiers. The SDK's send failures reach the app as
`HttpError::Middleware`, whose arm in `client::api_error` knew only a timeout and
a TLS failure. Everything else was `TransportCause::Unknown`: a refused
connection, a DNS failure of an `https://` host, a reset, and hyper's "connection
closed before message completed", which has no I/O error under it at all. Body
reads went through `network_error`, which walked the I/O kinds but not
`UnexpectedEof`, so a reply cut short was `Unknown` too. `offline_open_result`
falls back only on `Refused | Timeout | Dns`, so in practice an open fell back
only on a timeout (the docs carried it as "Known gap F1, #669").

### Decision

- One typed classifier, `client::transport_cause(&reqwest::Error)`, for the send
  (`Middleware(Reqwest(e))`) and the body reads alike, in this order: a timeout
  anywhere; DNS; TLS (`rustls::Error`); the connector failed (`Refused`); hyper's
  incomplete message, canceled request, closed channel or aborted body write, or
  an I/O reset, abort, broken pipe, not-connected, early end of file, or a refused
  or unreachable error on a connection already made (`Dropped`); else `Unknown`.
  (Review F1-transport-6: the refused and unreachable kinds used to read as
  `Refused` anywhere in the tree, against "nothing was sent".) Typed
  throughout, never the error's text: `hyper` and `reqwest-middleware` are named
  as dependencies only for their error types, the way `rustls` already was.
- A new `TransportCause::Dropped`: the connection was made, then closed or reset
  before the whole reply arrived. `Refused` now means the connection could not be
  made, so nothing was sent. A blackholed address's EINVAL is a connect failure,
  so it is `Refused` now, not `Unknown`; a reset while the reply is read was
  `Refused` and is `Dropped` now.
- `TransportCause::got_no_reply()` = `Refused | Dropped | Timeout | Dns`, the one
  definition of "a read that heard nothing from the server", and
  `offline_open_result` falls back exactly then. A refusal whose body is cut short
  is `Dropped` on a JSON read (the SDK reads the body before the status, so the
  status is lost) and a reply (`Unknown`) on the download, whose request step reads
  a body only for a status that is not 200 (review F1-transport-2). The offline
  copy opens read-only and the open runs no automatic Start Editing (review
  F1-transport-1); when the history read answered, the result says whether
  SpeleoDB has a newer revision (`newer_on_server`, review F1-transport-5). `Tls` (the peer did not prove it
  is the server) and `Unknown` (for instance a reply that is not HTTP: the server
  was reached) still report the error.
- Writes are unchanged on purpose: every write treats every transport failure as
  an unknown outcome whatever the cause (the push's `Unconfirmed`, the acquire's
  holder check), and no mutation is replayed. The cause matters to a write only
  for its wording, which is why `Dropped` has its own line ("The connection to
  SpeleoDB at <server> was lost before it finished answering") instead of
  `Refused`'s "Could not connect". (Amended after the real-app demo, item 3: an
  acquire that could not connect or resolve the name never left the computer, so
  it is no unknown outcome; see [Review](#real-app-demo-items-3-to-7).)
- The offline-open rules stay: only the cached copy of this project on this
  instance; this account's un-uploaded edits open still marked as not uploaded;
  another account's open read-only, naming whose they are; a discard never falls
  back; an offline open writes no owner record.

### Tests

- `speleodb::client::tests::transport_causes` (speleodb crate): closed before the
  reply, reset before it, head or body cut short, reset in the body (each on the
  project, history and both download reads, tried three times), a port nothing
  listens on (`Refused`), an `https://` name under `.invalid` (`Dns`, one lookup,
  never a server), a reply that is not HTTP (stays `Unknown`), the `got_no_reply`
  table, and five writes (acquire, release, create, patch, upload) whose reply is
  lost: `Dropped`, sent once.
- `core/state/speleodb_offline_open_tests.rs`: the whole open path for each way
  of losing every GET under `/api/v2/projects/` (a clean copy opens offline,
  untouched, no owner record); un-uploaded work (mine still marked, theirs
  read-only); not-HTTP and a discard never fall back; a push whose upload reply
  is lost (closed, reset, body cut short) stays unconfirmed, dirty at BASE and
  locked, sent once.
- `speleodb_pull::tests::offline_fallback_requires_connection_failure_and_matching_cache_identity`
  includes `Dropped`; `src/utils/speleoError.test.tsx` checks the new line differs
  from every other cause's and never says "could not connect".
- The automatic Start Editing after an offline open loses its reply the same way.
  Its refusal keeps the raw diagnostic in `message` ("Network error talking to
  SpeleoDB: error sending request", for the log) and carries `transport: Dropped`
  (`the_automatic_start_editing_after_an_offline_open_names_the_lost_reply`); both
  Start Editing surfaces (the acquire after an open, and the Start editing buttons)
  word the read-only notice from `transport`, so the user reads the Dropped line,
  never the raw text, and no toast repeats it (`autoAcquireSpeleoLock.test.ts`,
  `useSpeleoActions.test.tsx`, and the offline-open case of
  `speleodb-home-cache.spec.ts`).

Before the fix, 7 of the 9 client tests and 3 of the 4 open tests failed, the
open test with the fuzzer's exact refusal. Injections, each made, run and
reverted:

1. `offline_open_result` back to the `Refused | Timeout | Dns` filter: the two
   open tests and the pull unit test fail.
2. The send arm back to its own timeout/TLS classifier: six client tests and
   three open tests fail.
3. hyper's incomplete message not recognised: closed, cut short and the writes
   fail (client), and the open tests fail.
4. `UnexpectedEof` not recognised: cut short and the writes fail, and the open
   tests fail.
5. `got_no_reply` also counting `Unknown`: the not-HTTP tests, the
   `got_no_reply` table and the pull unit test fail.
6. The push treating a no-reply failure as not landed: the upload test fails.
7. A reset mapped to `Refused` again: both reset tests fail.

## F2: un-uploaded work is decided by the survey a copy holds

### Trace

Seed 900426: an edit on the open SpeleoDB copy; a crash that loses the unsaved
edit; another session pushes; the next open of the copy refuses with
`UnpushedLocalEdits` ("this project has local edits that were never pushed"),
although the copy holds exactly its BASE revision. A close with "Don't save" after
an edit does the same.

### Mechanism

`mark_remote_dirty` makes the mark durable at the edit with a local-only save, so
that a crash after an edit still knows there was work; the edit itself reaches the
file only with the next save. A crash or a discard in between leaves the mark over
BASE content, and every reader took the mark at its word: the pull's replace
guard, the fast-forward arm, the listing behind the sign-out and held-lock
prompts, and the open link behind the exit prompt. 0.6.0 had the same (one UPDATE
of `speleo_link` per edit).

### Decision: decide by content, keep the crash safety

- *A digest of BASE's survey.* `stalx_v2::survey_digest(bytes, base)` hashes,
  from the bytes as written (SHA-256, a fixed domain, length-prefixed fields),
  every member an upload carries that is survey content: everything but
  `format`, `version`, `solver`, `generator`, `summary` (derived), `local`,
  `history` (cut per push), `end` and `counters` (allocation state; amended
  after the X3 pre-PR review, should-fix 2: an add and its Undo leave them
  advanced), with `project.modified` (an edit and the edit back change it and
  nothing else) and the link's `endpoint` and `name` (rewritten without an
  edit) left out. The BASE id is hashed first, so a digest
  kept over another BASE never matches. Byte-level, so a link reader answers
  without decoding the survey.
- *Recorded where the copy is known to hold BASE*, as
  `local.speleodb.base_survey`: every install of bytes named as the BASE
  revision's (`PreparedDownload::holding_revision` then `cache_bytes`: pull,
  fast-forward, Take remote, View, a Restore of the tip) and every confirmed
  upload, from the bytes that went up (the 200 arm when it names the new
  revision, the 304 arm). `set_speleo_link` drops it when BASE or the project
  changes. A Restore of an older revision records none (review F2-digest-1: this
  used to rest on the dirty bit alone).
  [How the fixes meet](#how-the-fixes-meet) adds the one place it is dropped
  besides: every upload.
- *Read everywhere the app asks.* The shared link probe (`probe_link` and
  `read_cache_file`, through `speleodb_base_survey::probe_settled`) reads a marked
  file with a digest whole and reports it clean when its survey has the digest;
  nothing is written. The guard, the fast-forward arm, the offline and signed-out
  opens, the listing (sign-out, held-lock prompt, Home) and logout's
  forget-recents all read through it.
- *Cleared durably.* An open clears a stale mark (`settle_on_open`) and the
  open's existing local-only save writes it. A save whose survey is BASE's again
  (an edit taken back, by hand or by Undo) clears it in the one file it writes
  (`clear_stale_mark`) and reports `CommandResult.speleodb_unpushed_cleared`,
  which the frontend mirrors into `hasUnpushedEdits` (the exit prompt's input)
  unless an edit landed while the save ran.
- *The push fence.* Only the mark is cleared; `last_edit_at` stays. The 200 and
  304 arms SET the mark when an edit came after the push started, instead of
  leaving it as it was.
- *No digest, no change.* A 0.6.0 copy and a version 2 copy installed before the
  digest existed keep the conservative answer, as does any survey that differs
  in a byte the digest covers: an ignore kept only for a shot the history can
  bring back, another writer's layout, a project name the server-name fold (#216)
  changed inside an upload that then failed before it dropped the digest (the
  fold runs only in an upload, so only over a marked copy), until its next
  download or upload.
- *Rejected:* trusting a digest the file states for its own body. A writer that
  keeps unknown `local` keys verbatim while changing the body (an older build, a
  third-party tool) would make it lie, so the body is always re-hashed. Such a
  writer also keeps the digest through its own upload, so `base_survey_edit`
  binds the digest to the edits this build recorded (review F2-digest-2).

**Cost.** Only a copy marked un-uploaded with a recorded digest is hashed: a save,
an open, a link read of it. A copy with real saved work keeps both for as long as
the work stays un-uploaded, so its link reads (every listing fetch) were each a
whole read and a hash; the probe now remembers "real work" per unchanged file and
hashes it once (review F2-digest-5). On the format bench (release) the digest takes about a
third of an encode: 1.4 ms for 5,000 shots (0.6 MB), 6.2 ms for 20,000 (2.4 MB), 29
ms for 100,000 (12 MB), against 2.8, 14 and 87 ms to encode.

### Tests

`core/state/speleodb_base_survey_tests.rs`, against a loopback SpeleoDB, through
the real pull, open, edit, save and push:
`an_edit_lost_in_a_crash_leaves_no_un_uploaded_work_behind` (the fuzzer trace),
`a_crash_after_an_edit_reopens_clean` (Recents and the SpeleoDB browser),
`a_saved_edit_keeps_its_mark`, `an_edit_taken_back_and_saved_counts_as_clean` (by
hand, by Undo), `after_a_confirmed_upload_a_lost_edit_leaves_no_un_uploaded_work`,
`an_edit_taken_back_during_an_upload_stays_un_uploaded`,
`a_copy_without_a_digest_keeps_its_mark` (0.6.0 and version 2) and
`the_digest_goes_with_its_base`. Five failed on the code before the fix. The
digest is pinned in `persistence/stalx_v2/format_v2_digest_tests.rs`; the frontend
mirror in `projectSaveActions.test.ts` and `speleoStore.test.ts`.

Injections, each caught: the probe returning the header as read; no digest at the
install; the open's clear off; the save's clear off; no digest at the landing; the
clear in `set_speleo_link` removed; the digest kept when an upload is sent (see
[How the fixes meet](#how-the-fixes-meet)). The landing's late-edit
`has_unpushed_edits = true` is defence in depth since the digest goes with every
upload: a save during an upload has no digest to clear the mark with.

### Left

- A close with "Don't save" after an edit still keeps the write lock: the release
  veto reads the in-memory mark, and the session is dirty at that point. The next
  open resumes the lock and the copy reads as clean. Data-safe; a follow-up for
  the lock-release code.
- Every save of a SpeleoDB copy that is marked and has a digest hashes the survey
  once (6 ms at 20,000 shots). Hashing during the encode would remove the member
  scan; not done.

## F4: a release stands down only for a later session of its own account

### Trace

1. The owner signs in, opens a SpeleoDB project and takes its lock.
2. The owner's other session (another computer, or the website) hands the lock
   back. This process is not told, so the owner's claim stays in the claim list.
3. The collaborator signs in on the same server without signing out. Claims of
   other accounts are kept across a sign-in by design: each is the only local
   pointer to a lock that account may still hold.
4. The collaborator takes the lock (Start Editing; the probe finds it free).
5. The update restart releases (seed 900380, step 32): it came to "not held" with
   no request at all, and the collaborator's lock stayed held. SpeleoDB never
   expires a lock on its own.

### Mechanism

Every release that can run after a later session has started (a close, a project
switch, a quit or the update restart, a delete's captured release, Start Editing's
unwind after a session boundary, the stranded-claim cleanup) is reclaim-checked:
`speleodb_release_unless_reclaimed_locked` stands down when a later session has
claimed the project meanwhile (F8 of the lock-session races plan), because
SpeleoDB treats a re-acquire by the account that already holds the lock as a
no-op, so that later session holds the very lock the release would free.

The check asked `speleodb_claim_on_deployment(endpoint, project)`, which matched a
claim of ANY account on that deployment. Only a session of the SAME account can
have inherited the lock. Another account's claim is either live (that account
holds the lock, and the holder check would send nothing anyway) or stale, as in
the trace. On the trace's state, the close reported `NotHeld` and the frontend
closed as if the lock were handed back; a project switch, a quit and the update
restart sent nothing; a delete reported success with no warning; Start Editing's
unwind sent nothing; the stranded-claim cleanup sent nothing and also lost the
claim (it takes the claim out first and puts it back only on an error).

### Decision

- `speleodb_claimed_by_releasing_account(params)` replaces the account-blind
  predicate: a claim on `params.project_id` taken by the account the release is
  FOR (`params.endpoint` and `params.email`, captured with its token), compared
  through `LockClaim::taken_by` (normalised endpoint, email compared
  case-insensitively).
- Not the account signed in when the release runs: a deferred release can outlive
  the sign-in that captured it. The release's own account was the question all
  along.
- Another account's lock stays protected by the holder check (lock#4): a release
  is sent only when SpeleoDB names the releasing account as the holder.
- `LockClaim::on_deployment` is private now, so no claim test outside the type can
  drop the account again. Every other reader of the claim list (the edit gate,
  `speleodb_holds_claim`, `speleodb_claim_is_kept`, the claim writers, the
  release-side clear, the cleanup's choice of claim and the two restores) already
  names the account.
- *Rejected:* asking the server instead (a holder read cannot tell a same-account
  successor's inherited lock from the releasing session's own); dropping other
  accounts' claims at sign-in (each may be the only pointer to a lock that account
  still holds over un-uploaded work); cleaning up the owner's stale claim at the
  collaborator's sign-in (only the owner's token can read the holder; the owner's
  next sign-in ends it through the stranded-claim cleanup).

### Tests

`core/state/speleodb_foreign_claim_tests.rs`, against a loopback SpeleoDB that
keeps the project's lock and tells two synthetic accounts apart by their tokens,
replaying the trace with the app's own verbs (token sign-ins, Start Editing, a
second client's release):

- `the_update_restart_hands_back_the_collaborators_lock_over_the_owners_stale_claim`:
  the fuzzer's step, through `speleodb_release_held_lock_bounded` with a managed
  event sink, as in the app (without a sink the reclaim check is skipped and the
  bug does not show).
- `every_release_of_the_collaborators_lock_goes_out_over_the_owners_stale_claim`:
  the close (reported `Released`), the project switch and the delete (no warning).
- `start_editings_unwind_after_a_boundary_ignores_another_accounts_claim` and
  `the_stranded_claim_cleanup_hands_back_a_lock_beside_another_accounts_claim`.
- `only_a_claim_of_the_releasing_account_stands_a_release_down`: with the owner
  signed in and the collaborator's release, the owner's claim does not stand it
  down, and the collaborator's own claim (under another email case) does.

All five failed before the fix. Injections, each run against these tests and the
existing `speleodb_session` tests: the old deployment-only predicate fails all
five; asking about the account signed in now fails
`only_a_claim_of_the_releasing_account_stands_a_release_down` and
`a_captured_release_ignores_a_same_id_session_on_another_deployment`; no
stand-down at all fails the second half of the former and
`a_captured_release_stands_down_for_an_inherited_successor_session`.

## R3: never upload over a revision this session did not make

### Trace

Two sessions of the same account both hold the project's lock (SpeleoDB locks are
per account, so the server cannot tell them apart). Session A uploads. Session B,
still based on the older revision, uploads too, and its upload lands on top of
A's: B's copy does not contain A's work, so A's revision is no longer HEAD; it is
only in the history.

### Mechanism

The push already fenced BASE against the server's HEAD before every upload
(`sync::plan_push`), but its #193 recovery (our own upload whose answer was lost,
which did land) adopted every commit in BASE..HEAD that our account had authored
(`classify_ancestry`, `author_email == my_email`) and uploaded on top. The email
cannot tell this session from another session of the same account, and neither
can the lock (both hold it). The fix the review findings suggested (require an
authored lock claim in `plan_push`) would still let the session that took the
lock first overwrite the one that inherited it.

### Decision

- The authorship test is what THIS session uploaded, never the email:
  `sync::OwnUploads` holds the revisions this session's uploads were answered
  with (the 200 envelope's `hexsha`), and every upload whose answer never came
  (until a landing or an adoption settles them; a retry sent while an earlier
  one's fate is unknown keeps both, review F4R3-locks-push-5), each with the BASE
  it sat on and its description. `classify_ancestry` takes a gap commit as ours
  only when it is one of those, or a revision directly on an unanswered upload's
  BASE (each accounts for one), by our account, whose stored description matches
  ours (`normalize_like_server`; a history without any descriptions matches on
  BASE and account, and an undescribed revision in a history that has them is
  not ours: amended after the X3 pre-PR review, should-fix 4). Anything else
  between BASE and HEAD is `Diverged`: nothing is
  uploaded and the existing divergence prompt opens (`DivergenceNeeded`, already
  routed from Save Changes, Done and the close, quit and sign-out uploads).
- The record lives in `AppState::speleodb_own_uploads`
  (`core/state/speleodb_own_uploads.rs`), keyed by endpoint, account, project and
  session generation; any mismatch reads as "nothing uploaded", which fails closed
  to the prompt. Process-local and never persisted: a restart reopens through
  Start editing, whose reconcile has no authorship recovery.
- It is set right before the upload is sent and answered on every definite
  outcome: a 200 that names its revision (kept as landed; settles every
  unanswered upload), a 304 or a refusal (nothing landed from this upload, which
  alone is settled), an ancestry adoption. It stays unanswered on a dropped
  connection, an exhausted 5xx, a 200 that names no revision and an answer body
  that cannot be read: exactly the cases the next push must still recover.
- The status verdict uses the same record, so the SpeleoDB tab shows the conflict
  that Save Changes will report.
- The race the check cannot close: SpeleoDB has no conditional upload (SDK 0.9.0's
  `v2_projects_upload` takes no base), so a revision can land between the HEAD
  check and the upload. After a 200 the push reads the history once (a direct
  GET) and `sync::replaced_by_upload` lists what lies between the upload's BASE and
  the new revision's parent. It rides the next `Released` / `Pushed` as
  `replaced_revisions` and is shown on a persistent `replaced_revision` banner
  instead of the success toast (notifications.md: data at risk never rides a
  toast), naming the latest revision as the Revision History shows it. Every
  close that ran the upload stops on it so the banner is read before the project
  closes: the exit prompt and the sign-out (`ExitUploadResult` `'replaced'`, like
  `'diverged'`), Save and Close, and the older-version confirmation finishing a
  close or a quit, all through one predicate
  (`landedOverReplacedRevisions`). The lock is back and nothing is left to
  upload, so the next close asks nothing.
- No server change. A conditional upload on SpeleoDB (a `base` on the upload view,
  refused when HEAD moved) is the only complete fix for the race, and is worth
  proposing upstream.

### Tests

`core/state/speleodb_same_account_tests.rs`: two `AppState`s of one account (own
thread, cache root and config each) against a loopback server that keeps a real
history. The second session is refused and uploads nothing (Save Changes and
Done); an unanswered upload is no licence to adopt the other session's revision;
our own unanswered upload is still adopted; a push with HEAD == BASE uploads; the
lost race is named (Save Changes and Done) and reported once; the status shows the
conflict. Unit tests in `speleodb/sync.rs` (ancestry, `plan_push`,
`status_verdict`, `replaced_by_upload`) and `models.rs`. Vitest: the refusal
routes to the divergence prompt; the replaced report rides the banner only; the
exit and sign-out dialogs, Save and Close and the older-version confirmation
(close and quit) stop on it. Playwright: `speleodb-same-account.spec.ts` (Save
Changes, and Save and Close followed by a close that asks nothing).
Three older tests asserted the old shortcut (holding the lock made any commit of
the account ours); they now record an upload of this process first and assert
`Conflict` without one.

On the code before the fix, B's push answered `Pushed`. Injections, each failing
the named tests: authorship by email in `classify_ancestry` (the repro, the status
test, five sync tests); no description match (the no-licence test and its unit
test); no post-upload check (both race tests); only the direct parent in
`replaced_by_upload` (its multi-revision unit test); no `replaced` arm in the exit
and sign-out dialogs, no banner in `applyPush`, or the success toast kept (five
vitest cases); no stop in Save and Close (its vitest case and the Playwright case)
or in the older-version confirmation (its close and quit cases).

### Left

- Our unanswered upload and another session's revision with the same description
  on the same BASE are told apart by nothing, and neither is anything on our BASE
  by our account on a server that keeps no descriptions at all.
- A quit that waits for an upload started elsewhere (Save Changes, then Quit)
  quits after it; a replaced report from that upload is only in the log. (The
  `released`-with-newer-edits arm no longer replaces the report: it has its own
  banner slot since review F4R3-locks-push-4.)
- The divergence prompt's "Keep my local version" re-runs the push, which is
  refused again while the other session's revision is HEAD; the ways through are
  Take the SpeleoDB version and Save my version as a separate file. Its help now
  says so (review F4R3-locks-push-7); an explicit "upload on top of the latest
  revision" is a product decision, not taken here.

## How the fixes meet

The four branches were merged in order (F1, F2, F4, R3). The code conflicts were
in the push's 200 arm (F2's landed bytes and R3's post-upload check, both kept) and
the list of SpeleoDB sources the tripwire tests scan (F2 and R3 each added a
module). Each pair that decides the same outcome was then checked, and
`core/state/speleodb_fix_interactions_tests.rs` pins every one against a loopback
server that keeps a real history and lock holder and can lose its replies.

- **F1 x F2: an offline open of a copy whose mark is stale.** The read fallback
  opens the copy as the shared probe reads it, so the content rule applies
  offline too: an edit lost in a crash opens clean, its own account's or another
  account's (there is no work of anyone's in it, so it is not opened read-only on
  their behalf), while a saved edit still opens marked, and another account's
  read-only. The offline open writes nothing; the open of the file then clears
  the stale mark as any open does
  (`an_offline_open_decides_un_uploaded_work_by_content`; caught by the probe
  returning the header as read, and by the open's clear off).
- **F2 x the #193 recovery that R3 narrowed: an edit taken back after an
  unconfirmed upload. This one needed a fix.** An upload whose answer is lost may
  have landed, so SpeleoDB's latest revision may be our own, past BASE. F2's
  digest still described BASE, so taking the edit back and saving read as clean:
  the save cleared the mark, Done released without uploading, and SpeleoDB's
  latest revision kept the edit the user had taken back (reproduced: the save
  reported the mark cleared, one upload instead of two). Before F2 the mark stayed,
  and Done adopted our own landed upload and uploaded the take-back on top. The
  digest now goes with every upload: `speleodb_upload` drops it in the save the
  push starts with (so a crash during the upload cannot bring it back), and only a
  confirmed landing (a 200 naming the revision, or a 304) or a new install records
  one again. Until then the mark is taken at its word, as on any copy without a
  digest; a refused upload (nothing landed) leaves the copy without a digest
  until its next confirmed upload or download, which is only conservative
  (`an_edit_taken_back_after_an_unconfirmed_upload_is_still_uploaded`; F2's
  `an_edit_taken_back_during_an_upload_stays_un_uploaded` now asserts that a save
  during an upload clears nothing; caught by keeping the digest at the upload).
- **F2 x R3: a stale mark when another session of the account pushed.** The copy
  holds exactly BASE, so another session's revision on top leaves it behind, not
  in conflict: the status reads `RemoteAhead`, Save Changes and Done upload
  nothing and refuse nothing, and Done hands the lock back
  (`a_stale_mark_is_no_conflict_when_another_session_of_the_account_pushed`;
  caught by the open's clear off, which reads `Conflict`).
- **F4 x R3 on the exit and sign-out paths.** An exit upload (Done, which the
  close, quit and sign-out prompts run) refused because another session's
  revision is HEAD uploads nothing; the quit's bounded release and the sign-out
  that follow send no release, the lock stays the account's, its claim is kept on
  purpose and the copy stays marked at BASE
  (`an_exit_upload_refused_for_another_sessions_revision_keeps_the_lock`; caught
  by dropping the release veto). An exit upload that landed over a revision pushed
  meanwhile is reported by name, and Done itself hands the lock back and ends the
  claim, so the quit and the sign-out send nothing more and the stranded-claim
  cleanup has nothing to hand back
  (`an_exit_upload_that_replaced_a_revision_hands_the_lock_back_once`; caught by
  keeping the claim after Done). F4's account-scoped stand-down and R3's
  generation-scoped upload record agree on the account: both compare the
  normalised endpoint and the email case-insensitively.
- **F1 x R3.** An upload whose reply is lost is `Dropped` (F1) and still an
  unknown outcome; R3 keeps it as the unanswered upload, so the retry adopts it
  when it landed (`our_own_unanswered_upload_is_adopted_on_the_retry` loses the
  reply by closing the connection after landing, which F1 classifies as
  `Dropped`).
- **F1 x F4, F2 x F4.** No shared decision: F4 only changes which claim stands a
  release down, and the release veto over un-uploaded work, which F2's mark feeds,
  runs before it.

## What users notice

- A SpeleoDB project with a copy on this computer opens offline (read-only, "SpeleoDB
  is unreachable") when the connection is refused, the server's name cannot be
  found or the connection drops before the reply, not only after a timeout. It
  takes no lock by itself; Start editing does. When SpeleoDB answered and only the
  download was cut, the notice says whether SpeleoDB has a newer revision. A
  connection lost mid-reply now reads "The connection to SpeleoDB at <server> was
  lost before it finished answering", never "Could not connect".
- An edit lost in a crash or closed with Don't save, or an edit taken back and
  saved, no longer leaves the project "with local edits that were never pushed":
  opening it updates it as usual, and the close, quit and sign-out prompts ask
  nothing. After an upload whose answer was lost, an edit taken back is still
  uploaded, in case that upload reached SpeleoDB.
- After another account's lock on the same project was handed back elsewhere,
  closing, switching, quitting, the update restart or deleting the copy hands
  your own lock back again.
- With the same account on two computers, an upload no longer goes over the
  other computer's newer revision: the conflict prompt opens and nothing is
  uploaded. If the other revision lands in the moment between the check and the
  upload, a banner names the revision that was replaced and says it is still in
  the Revision History, stays until dismissed, and closing, quitting or signing
  out waits until it has been seen.

## Review

An adversarial review of the four fixes raised the findings below; a verifier
reproduced each one (CONFIRMED) or found it plausible from the code (PLAUSIBLE).
Each was fixed with a regression test that fails without the fix (the injection
is named in the test). Nothing here contacted a SpeleoDB server.

### F1: the offline open and the transport causes

- **F1-transport-1 (P2, CONFIRMED): an offline open ran Start Editing at once,
  and an acquire that landed while every read around it was lost left the lock
  held with nothing pointing at it.** Two fixes. Start Editing's unwind runs only
  after an acquire that landed, so when its holder-checked release cannot tell
  (the holder read or the POST failed) and there was no claim to put back, it
  now leaves an unbound claim (`speleodb_unwind_release`), as the acquire-unknown
  arm already did; the next close or the janitor hands the lock back once
  SpeleoDB answers. This hole predated F1 and was reachable from a manual Start
  Editing too. And an open the backend fell back to the cached copy for no longer
  runs the automatic Start Editing (`finishOpen`'s `takeLock: false`): the copy
  opens read-only, as the CHANGELOG said; "Open my local copy" still takes the
  lock. Tests: `a_start_editing_whose_acquire_landed_leaves_a_pointer_to_the_lock`
  (lost GETs, answered POSTs, a server that keeps the lock), the vitest offline
  case, and the offline case of `speleodb-home-cache.spec.ts`.
- **F1-transport-2 (P3, CONFIRMED): a refusal whose body was cut short opened the
  cached copy offline.** The SDK reads a JSON reply's body before it looks at the
  status, so there the status is lost with the body and nothing can tell a cut
  403 from a cut 200: documented (SpeleoDB.md, SPEC.md, `TransportCause::Dropped`),
  and a request to the SDK to return the status with a body-read error is left
  open. The download can tell: the SDK reads a body in its request step only for
  a status that is not 200, so a body cut there is a reply (`Unknown`, no
  fallback; `client::download_head_error`). Test:
  `a_refusal_cut_short_is_a_reply_to_the_download_only`.
- **F1-transport-3 (P3, CONFIRMED): "sent once" could not fail.** The fault
  listener keeps accepting for a grace after the expected connections, so a
  second send is counted: `a_write_sent_twice_is_counted_twice` (two sends, one
  still in flight, both seen).
- **F1-transport-4 (P3, CONFIRMED): the TLS shape of `Dropped` was untested.** No
  test here can serve TLS without key material, so the classifier is
  `cause_of_tree` over any error tree, and the TLS close (rustls's early end of
  file without `close_notify`, wrapped as hyper wraps it) and a TLS failure
  inside an I/O error are pinned on hand-built trees:
  `a_tls_connection_closed_before_the_reply_is_dropped`.
- **F1-transport-5 (P3, CONFIRMED): an open whose history read answered and whose
  download was cut said "SpeleoDB is unreachable".** The tip the history named
  goes into `offline_open_result`, the result carries `newer_on_server`, and the
  toast says whether SpeleoDB has a newer revision than the copy that opened:
  `an_open_whose_download_alone_was_lost_says_what_the_history_told_it` and the
  vitest toast cases.
- **F1-transport-6 (P3, PLAUSIBLE): `Refused` said "nothing was sent" but also
  matched a refused or unreachable error anywhere in the tree.** `Refused` is now
  only a connector failure; the same kinds on a connection already made are
  `Dropped`, and `Tls` joins the causes after which the request may have reached
  the server. `refused_is_only_a_connection_that_was_never_made`.
- **F1-transport-7 (P3, CONFIRMED): `tokio ={`** in the workspace manifest:
  restored, and the hyper comment there now says it holds over plain HTTP only.

### F2: the BASE survey digest

- **F2-digest-1 (P2, CONFIRMED): nothing pinned that a Restore records no
  digest.** Structural now: `PreparedDownload::holding_revision` names the
  revision the bytes are (the pull and the fast-forward their pinned tip, a View
  or a Restore the revision it fetched), and `cache_bytes` records the digest
  only when that revision is the link's BASE, never from the dirty bit.
  `a_restore_of_an_older_revision_records_no_digest_and_stays_un_uploaded` (the
  mark survives a save, a crash and the reopen, and the push uploads it) and
  `a_restore_of_the_tip_records_its_digest`.
- **F2-digest-2 (P3, CONFIRMED): a writer that keeps unknown keys carried BASE's
  digest past its own upload.** `local.speleodb.base_survey_edit` is the
  `last_edit` this build vouched for: each edit it records moves both while they
  were equal, and drops the digest otherwise. The digest counts only while the
  two are equal, so an edit by a writer that does not know the digest ends it.
  The spec now says a writer that uploads without computing it should remove it.
  `a_digest_another_writer_carried_past_its_edit_is_not_trusted` and
  `the_digest_counts_only_while_this_build_recorded_every_edit_since`.
- **F2-digest-3 (P3, CONFIRMED): the 304 arm's digest was untested.** The
  loopback server answers 304 on request:
  `a_304_landing_records_the_digest_and_keeps_a_late_edit_marked`. Its late-edit
  mark is defence in depth (the digest goes when the upload starts), pinned by
  the combined injection the test names.
- **F2-digest-4 (P3, PLAUSIBLE): the docs said a Don't-save close closes clean,
  and gave a fold case that cannot happen on a clean copy.** SPEC.md now says the
  Don't-save close keeps the lock and the copy reads clean from its next open;
  the fold example says the fold runs only inside an upload.
- **F2-digest-5 (P3, CONFIRMED): a copy with real un-uploaded work was read whole
  and hashed on every link read.** `probe_settled` remembers the answer "real
  work" per file (length, modification time, digest, vouched edit time); only
  that answer, so a stale entry can keep a mark, never clear one.
  `a_copy_with_real_work_is_hashed_once_while_it_is_unchanged`.

### R3 and the locks

- **F4R3-locks-push-1 (P2, CONFIRMED): Save and Close closed over a replacing
  upload.** Fixed before this review section was written (branch `sdbc/S5-gaps`,
  merged): Save and Close and the older-version confirmation stop on
  `landedOverReplacedRevisions`, like the exit and sign-out prompts.
- **F4R3-locks-push-2 (P3, CONFIRMED): the frontend's downgrade of a push dropped
  the report.** The `push_unconfirmed` it reports keeps `replaced_revisions`.
  vitest `a push downgraded for an edit during the reply keeps what it replaced`.
  (A reply that arrives after the user left the project is still dropped by the
  frontend; the project's banner is gone with it.)
- **F4R3-locks-push-3 (P3, CONFIRMED): a report waited for a result the session
  might never reach.** Every result built after a landing carries it:
  `PushUnconfirmed` (a late edit) and Done's release failure
  (`SpeleoErrorDto.replaced_revisions`). The frontend raises it from a failure,
  and the exit and sign-out prompts stop on it (`'replaced'`).
  `a_release_that_fails_after_a_landed_race_still_names_the_replaced_revision`,
  and the vitest failure and exit cases.
- **F4R3-locks-push-4 (P3, CONFIRMED): the replaced banner was overwritten by the
  next outcome.** It has a slot of its own (`speleoStore.replacedRevisions`,
  `speleo-replaced-banner`), cleared only by its Dismiss or by leaving the
  project; later reports fold in. `SpeleoBanner.test.tsx` (stays through a clean
  push and release, Start editing, an unconfirmed or failed push, a notice).
- **F4R3-locks-push-5 (P3, PLAUSIBLE): one unanswered upload was remembered.**
  `OwnUploads.unanswered` keeps every one until a landing or an adoption settles
  them (a 304 or a refusal settles only the upload just sent), and a revision one
  of them made is not reported as replaced (`sync::not_our_own`).
  `a_retry_does_not_report_our_own_late_upload_as_replaced`,
  `a_retry_answered_304_over_our_own_late_upload_leaves_it_adoptable` and the
  `sync` unit tests. Whether SpeleoDB ever lands an upload after the client gave
  up on it was not checked against a server.
- **F4R3-locks-push-7 (P3, CONFIRMED): "Keep my local version" promised an upload
  that is refused again.** Its help now says it comes back to the prompt while
  SpeleoDB has a revision the copy does not contain, and points at Save as an
  unmanaged file (`SpeleoDivergenceDialog.test.tsx`). An explicit "upload on top
  of the latest revision" is left as a product decision.
- **F4R3-locks-push-8 (P3, CONFIRMED): the undo-history notice was lost beside a
  replaced report.** Its warning toast is shown beside the banner (a separate
  fact): vitest `... that replaced a revision and left its undo history out says
  both`.

### Real-app demo, items 3 to 7

A demo of the four fixes in the real app against an internal test server (two
sessions, a fault proxy; never speleodb.org) found no P0 or P1. Items 3 to 7 of
its list, and the review's leftover (every SpeleoDB failure surface worded by
cause), are fixed here; each was reproduced first by a test that fails on the
code before the fix. Items 1 and 2 are in the next subsection; items 8 and 9
(the memory warning, the replaced report's date format) are not addressed.

- **Item 4 (P2) and the leftover: the release-failure dialog showed the raw
  diagnostic** ("What went wrong: The SpeleoDB lock could not be handed back:
  Network error talking to SpeleoDB: error sending request"), and so did
  Discard changes, the older-version prompt's discard, Switch to Read Only
  Mode, the sign-out prompt's re-check and discard (each `getErrorMessage` of a
  rejected SpeleoDB call), the cache purge and the delete's and the cached-copy
  removal's lock warnings. A failed release now carries its cause
  (`CommandResult.lock_release_transport`), which the dialog words with the
  line every read-only notice uses (`describeTransportCause`); the other
  surfaces use `describeSpeleoError` (the self-framed line, else the error as
  it came); the backend's own sentences name the cause in plain words
  (`SpeleoDbError::user_reason`). The style guide has no details-line pattern,
  so the raw text is only in the log. Tests:
  `a_release_that_got_no_reply_carries_its_cause_not_the_raw_diagnostic`, the
  raw-free assertions of `deleting_the_open_remote_project_releases_its_held_lock`
  and `an_unreachable_server_removes_the_copy_and_says_the_lock_may_stay`,
  `a_transport_failure_reads_in_plain_words_and_only_a_failed_connection_sent_nothing`,
  and vitest (`speleoReleaseFailure.routes`, `releaseOpenProjectLock`, both
  discards in `SpeleoUnwoundPushDialog`, `SpeleoSignOutDialog`, `speleoError`),
  plus `speleodb-release-failure.spec.ts`. Injection: `release_failure` without
  the cause, or `{e}` back in a warning, fails them.
- **Item 3 (P2): a lost acquire was explained only at Close.** Two parts. An
  acquire whose connection could not be made or whose server name did not
  resolve (`TransportCause::nothing_sent`) sent nothing, but read as an unknown
  outcome: its holder check failed the same way, it left an unbound claim, and
  Close asked "Couldn't hand back the lock" about a request SpeleoDB never saw.
  It is a plain failure now (`acquire_outcome_unknown`). And a Start Editing
  refusal after which the process keeps a claim to a lock that may be this
  account's says so (`SpeleoErrorDto.lock_may_be_held`, read from the claim the
  flow left, so the acquire-unknown arm and the unwind that could not tell are
  both covered, and an acquire SpeleoDB says did not land is not); the notice
  adds "The lock request may have reached SpeleoDB; if it did, Stalagmight hands
  the lock back when you close the project, or you can try again." The flag is
  read from the claim rather than the cause the task proposed (`Dropped`,
  `Timeout`, `Unknown`), since a 5xx or an unreadable reply leaves the same
  claim and a lost reply that SpeleoDB says did not land leaves none; `Tls`
  counts as may-have-landed, as `TransportCause` documents. Tests:
  `a_start_editing_that_never_connected_leaves_no_pointer_to_a_lock` (injection:
  every `Transport` an unknown outcome again),
  `a_lost_lock_request_says_the_lock_may_be_held_only_while_nothing_can_tell`
  (injection: the flag set from the cause alone),
  `an_acquire_that_never_left_this_computer_is_no_unknown_outcome`, the flag
  assertion in `a_start_editing_whose_acquire_landed_leaves_a_pointer_to_the_lock`,
  vitest (`autoAcquireSpeleoLock`, `useSpeleoActions`, `speleoError`) and
  `speleodb-home-cache.spec.ts`.
- **Item 7 (P3): the notice after Resume editing was titled "Project opened
  read-only".** The notice has a kind: the automatic Start Editing after an
  open keeps that title; Resume editing and Start editing on an open project
  (and a discard that re-takes the lock) say "Still read-only". Vitest
  `SpeleoReadOnlyNotice`, `useSpeleoActions`, and the e2e above.
- **Item 6 (P3): "Ready for editing" beside the read-only badge offline.** A
  status that cannot reach SpeleoDB reads no lock holder, which the line took
  for "free". While the connection is unavailable it says "Offline - ..." with
  what the last lock scan saw (`remoteLocks`: free, someone, this account, or not
  known). Vitest `SpeleoWorkspaceHeader` and the e2e above.
- **Item 5 (P3): nothing persistent said the copy was behind.** The tab says
  "SpeleoDB has a newer revision than this copy" whenever the status knows it
  (`remote_ahead`), seeded at an offline open whose history named a newer
  revision (`setLinkOnOpen`'s `newerOnServer`), and where Resume editing is
  offered adds "Resume editing downloads it first". Checked, not changed: Start
  Editing on a clean copy behind SpeleoDB fast-forwards (downloads the newer
  revision) before it opens the edit gate, and over un-uploaded work returns
  the divergence prompt and downloads nothing:
  `resume_editing_on_a_copy_behind_downloads_the_newer_revision_first`
  (injection: `reconcile_verdict` answering `UpToDate` for a fast-forward). A
  status that then cannot reach SpeleoDB says "Connection unavailable" and drops
  the line, which is what is known. Vitest `SpeleoWorkspaceHeader`,
  `speleoStore`, `useOpenRemoteProject`, and `speleodb-home-cache.spec.ts`.

### The real-app demo: two sessions of one account

A real-app run of `sdb/core` at cb85ce496 (two app instances with separate
profiles, like two computers, against a test server; synthetic accounts and
survey) listed what confused it. Its items 1 and 2 are the same root: SpeleoDB's
lock belongs to the account, so the server cannot tell two sessions of one account
apart, and the client did not say so or act on it.

- **Demo item 1 (P2): a second live session was invited to release the lock the
  first was editing under.** At sign-in the held-lock prompt listed the account's
  lock, said "perhaps from a session that did not exit cleanly" and offered a bare
  Release, which also took the initial focus (DialogWrapper focuses the first
  enabled button: the first row's Release once the rows have loaded). The prompt is
  built from the sign-in scan (`scanLocksOnOpen`: every project SpeleoDB names this
  account as holding, minus the open one) joined with the local states, and nothing
  in either can tell a live session of the account from a crashed one: the lock is
  per account, and lock claims are process-local, so this computer's own crashed
  session leaves no claim after the restart either. The stranded-claim cleanup is
  not a prompt: it hands back only a lock THIS run claimed and never shows anything.
  The sign-out survey, however, handed back every clean lock the account holds with
  no question, which is the same hazard.
  - Fix: the body says the lock may belong to Stalagmight open on another computer
    with this account or to a session that did not exit cleanly, and that releasing
    it while that session edits lets others change the project under it. The
    release is the worded "Release anyway", never hidden (SpeleoDB never expires a
    lock, so a crashed session's lock must stay releasable), and Not now is the
    default focus (`data-autofocus`).
  - A row says "Held by this account, not by this computer" only on a positive
    answer (`lockNotTakenHere`): this run holds no claim on the lock
    (`SpeleoLocalProjectState.lock_claimed_here`, new, joined by the backend from the
    signed-in account's claims, whatever the disk says), there is no local copy, the
    project is not open, its copy is not unreadable, and no recent file is linked to
    the project (a published copy kept outside the cache). A clean copy here is not
    enough: a session that crashed over it leaves exactly that.
  - The sign-out leaves such a lock alone (`surveySignOut`): it strands nothing of
    this computer's, and it may be the lock another session edits under.
  - *Rejected:* a persisted "this computer took the lock" marker (a key in the
    copy's `local.speleodb`, or the claims mirrored to a file in the config
    directory), which would let the clean-copy case say "not by this computer" too.
    Every release path (close, switch, quit, the update restart, delete, Done,
    unlink, sign-out, the cleanup, the per-project Release of a project that is not
    open) would have to clear it, and a missed clear makes the prompt say "this
    computer" wrongly while a missed set makes it say "not by this computer"
    wrongly; the per-project Release would also have to write a file it does not
    have open. *Rejected:* withholding Release on rows that might be live (above).
  - Tests: `a_row_says_whether_this_run_holds_a_claim_on_the_lock` (Rust: this
    account's claim with no copy, another account's claim, a copy alone, the
    single-file form), `HeldLockResumeDialog.sameAccount.test.tsx` (the body, Release
    anyway and its release, the default focus with DialogWrapper's initial-focus
    frame run by hand, each origin case including a failed read), the
    `surveySignOut` case, and `speleodb-held-lock-same-account.spec.ts`
    (Playwright: Enter keeps the lock; Release anyway releases). All failed on
    cb85ce496 (the Rust one does not compile there: the field is new).
    Injections: drop the claim join at the end of the listing (the Rust case);
    drop `data-autofocus` from Not now (the focus case: focus lands on Release
    anyway); drop the `lockNotTakenHere` term from the sign-out survey (its case).
- **Demo item 2 (P2): Take the SpeleoDB version released the lock another session
  of the account edits under.** Both sessions edit under the account's lock;
  session 1 is refused (R3) and takes the SpeleoDB version. `speleodb_take_remote`
  shut the gate and released the lock (abandoning, holder-checked: the holder is the
  account, so the release went out), and SpeleoDB showed no lock while session 2
  still said "You are editing".
  - Decision: Take the SpeleoDB version releases nothing and leaves the claim as it
    was (bound, unbound or kept). Replacing the copy is a fast-forward with
    `LocalEditsPolicy::Discard` and needs nothing from the lock, so it has none to
    give up. An edit session carries over the reload (data#3 is unchanged: only a
    claim still bound carries) and goes on editing the server version; a read-only
    session stays read-only with whatever pointer it had. The confirmation says
    which ("You keep the write lock and go on editing the SpeleoDB version" / "The
    write lock is left as it is, and the project stays read-only"). Amended by X6
    (below): the session goes on only while SpeleoDB still records this account's
    lock, read once before the reload.
  - *Rejected:* releasing and re-acquiring. The project is unlocked in between, and
    an acquire that fails there leaves exactly the bug. *Rejected:* a re-acquire on
    top (an acquire by the account that holds the lock is a no-op, so it proves
    nothing). *Rejected:* clearing a kept claim's mark because the work behind it
    was discarded: the next close would then release a lock that may be another
    session's; it stays for the next sign-in's prompt, which now says so.
  - Found on the way: the gate used to shut only after the history read, and the
    fast-forward took its consent after that read, so an edit made during it was
    discarded with the rest although the user had confirmed the discard before it
    (reproduced on cb85ce496). The gate now stays open, so the revision read on entry
    is the consent: an edit made since refuses the reload (`SessionMoved`, Retry)
    and is kept.
  - Tests: `taking_the_speleodb_version_keeps_the_lock_another_session_edits_under`
    and `taking_the_speleodb_version_read_only_leaves_the_lock_as_it_was` (two
    sessions of one account on the R3 loopback server, which now serves downloads
    and mints hex revision ids), `take_remote_keeps_the_lock_and_the_edit_session`
    (was `take_remote_reopens_the_server_version_read_only`) and
    `an_edit_during_take_remotes_history_read_refuses_it` (integration,
    `speleodb_revision_swap.rs`), the divergence dialog's two confirmation cases,
    the `takeRemote` hook case, and `speleodb-take-remote.spec.ts` (Playwright,
    editing and read-only). All failed on cb85ce496. Injections: the release put
    back before the reload (both R3 cases and the integration keep-the-lock case);
    no entry revision for the fast-forward (the edit-during-the-read case); the lock
    line dropped from the confirmation (both dialog cases).
- **Left**
  - Every other release still cannot tell this session's lock from another
    session's of the same account: a close, quit, Done or Switch to Read Only in one
    session hands back the lock the other edits under (the holder check passes: the
    holder is the account), and the stranded-claim cleanup hands back this run's
    pointer even if another computer has since inherited the lock. Only a lock
    SpeleoDB keeps per session (or a token the acquire returns and the release must
    present) can fix that; worth proposing upstream with the conditional upload.
    (Kept by the second capture's issue 4, below; the session that loses its lock
    now says so.)
  - The prompt cannot say "not by this computer" over a clean copy here (the
    rejected marker above). The Home card, the open and the sign-out now use the
    same rule (the second capture's issues 3 and 5, below).
  - The demo's items 3 to 7 (a lost acquire explained only at Close, the raw
    diagnostic in "Couldn't hand back the lock", nothing persistent saying the copy
    is behind, "Ready for editing" on an offline copy, the read-only notice's title
    after Resume editing) are in the subsection above; items 8 and 9 (the memory
    warning, the replaced report's date format) are not addressed.

### The second real-app capture: promises the UI made and the app did not keep

A second real-app run of `sdb/core` at e9df1b642 (the same two-instance setup and
fault proxy against the internal test server, synthetic account and survey; never
speleodb.org) found no P0 or P1. Its five P2s and six of its P3s are fixed here,
each reproduced first by a test that fails on e9df1b642 (named with each item).

- **Issues 1 and 2 (P2, screenshots 17, 18, 18b, 23 to 25): an in-place reload
  ended the edit session it had just kept or started.** "Take the SpeleoDB version"
  said "You keep the write lock and go on editing" and the session ended read-only
  with Resume editing offered; Resume editing on a copy behind SpeleoDB downloaded
  the newer revision and took the lock, stayed read-only with no message, and a
  second click started editing. The backend was right both times: Take remote's
  fast-forward carried the bound claim into the generation its reload bumps to, and
  Start Editing's fast-forward arm bound the claim to it
  (`take_remote_keeps_the_lock_and_the_edit_session` passed). The frontend then
  reconciled the reload through `loadProject` (`useSpeleoActions.ts:174` Start
  Editing's fast-forward, `:305` Update's, `:590` Take remote's on e9df1b642), which
  is a project switch: its first step, `cancelLoading` (`projectStore.ts:1001`),
  sends `cancel_loading`, which moves the SpeleoDB session generation on
  (`app_state.rs:512`, so that a confirmed switch invalidates older downloads). The
  claim stayed bound to the reload's generation, the gate predicate compares it with
  the current one (`speleodb_edit_gate.rs:85`), and the gate was shut while the lock
  stayed held on SpeleoDB. `load_project_full` then short-circuited on the open path
  (`project_load.rs:631`), so nothing else happened. The Rust tests never sent
  `cancel_loading` and the Playwright specs ran on a mock that did not model it,
  which is why both passed.
  - Fix: the three arms reconcile through `applySpeleoTreeReplacement` alone, as the
    automatic Start Editing after an open and Restore already did (its doc says why
    `loadProject` is not called). Keeping editing is safe in every branch the backend
    carries it in (only a claim still bound to the session carries; data#3), so the
    confirmation's words stay. Where the re-read status still says read-only after a
    reload that should have kept or started editing (a lost-lock signal in between),
    the "Still read-only" notice says so ("The SpeleoDB version is open, but editing
    did not start. Resume editing tries again.") instead of a silent badge, and a
    one-click Resume editing that downloaded first says "editing enabled" like the
    plain one.
  - Tests: vitest "an in-place reload keeps the edit session" (Take remote while
    editing, from read-only, coming back read-only; one-click Resume editing;
    a Resume editing whose reload ends read-only), four of five failing on
    e9df1b642, and the three `loadProject` assertions inverted;
    `speleodb-take-remote.spec.ts` now models `cancel_loading` ending the bound
    session, and its editing case and the new one-click Resume editing case fail on
    e9df1b642.
- **Issue 3 (P2, screenshots 11, 12, 15): a second session of the account
  contradicted itself.** The held-lock prompt said "Held by this account, not by
  this computer" while the Home card behind it said "You are editing"
  (`SpeleoProjectInfo.tsx:20`, from `lockedByMe` alone, `SpeleoLockBadge.tsx:52`);
  opening the card then took the lock over in silence: the automatic Start Editing
  probed the holder, found this account and inherited the lock
  (`speleodb.rs:1019`; re-acquiring the account's own lock is a no-op on SpeleoDB).
  - Fix, the Home card: the card, the recents line and the badge tooltip use the
    prompt's rule (`speleoLockOrigin`, beside `lockNotTakenHere`): "You are editing"
    only for a lock this run claims (`lock_claimed_here`), "Held by this account,
    not by this computer" where this computer can positively not have taken it, and
    "Held by this account" where it cannot tell. The tab of a read-only session
    under the account's lock says "Read-only - your account holds the write lock".
  - Fix, the open: the automatic Start Editing after an open
    (`speleodb_start_editing` with `afterOpen`,
    `AppState::speleodb_start_editing_after_open`) stops short of a lock the account
    holds while this run holds no claim on it, before anything but the holder read
    is sent, and answers `locked_by` with the account's own email. The project opens
    read-only and the notice says, in the prompt's words, that the lock may be
    Stalagmight open on another computer with this account or a session that did
    not exit cleanly, and that Resume editing edits it here too. A lock this run
    claims (taken here, kept by a close) is resumed as before, and Resume editing
    and the held-lock prompt's Continue editing take the plain verb, so resuming a
    crashed session's lock costs one click after the notice. Continue editing loads
    the project without the automatic acquire (its explicit Start Editing is the
    resume), so the notice never lands over the session it resumes.
  - *Rejected:* a new `StartEditingResult` variant (the existing `locked_by` with
    the account's own email says the same, and every exhaustive match would have had
    to change); refusing the plain verb too (a crashed session's lock must stay
    resumable from Resume editing).
  - Tests: `opening_does_not_take_over_a_lock_another_session_of_the_account_edits_under`
    and `opening_resumes_a_lock_this_run_claims` (two sessions on the R3 loopback
    server; the new verb does not exist on e9df1b642; injection: drop the
    `resume_held` early return in the "already ours" probe arm and the first
    fails), the autoAcquire vitest case,
    `SpeleoProjectInfo` (four origin cases fail on e9df1b642), the header case, and
    `speleodb-held-lock-same-account.spec.ts` asserting the card behind the prompt
    (e9df1b642: "You are editing").
- **Issue 4 (P2, screenshot 21): a session that lost its lock to another session
  of the account turned read-only in silence.** Session 2's Close handed back the
  account's lock (the Left list above: no release can tell the account's sessions
  apart), and session 1's next status refresh switched to "Ready for editing" and
  read-only with no surface (`speleoStore.ts:927` seeds the status and says
  nothing).
  - Decision: the release stays. X2's sign-out rule (`lockNotTakenHere`) never
    applies to an open project, which always has a copy here, and an inherited claim
    cannot stand in for "another session's" either: Continue editing after a crash
    inherits too, and a close that kept that lock would strand it. The same holds
    for the update restart's release. What changes is that the status refresh that
    ends an edit session because SpeleoDB no longer gives this account the lock says
    so (`speleoLockLostMessage`): the "Editing stopped" notice, worded by what the
    reply knows (handed back elsewhere, perhaps by Stalagmight on another computer
    with this account; held by someone else; access that no longer allows editing),
    and that the changes are kept on this computer. A reply that did not hear from
    SpeleoDB, a refused project (its own surface) or one that still names this
    account says nothing here.
  - Tests: speleoStore "a refresh that ends the edit session says why" (the three
    worded cases fail on e9df1b642, four quiet cases), and the new case of
    `speleodb-same-account.spec.ts` (e9df1b642: no notice).
- **Issue 5 (P2, screenshots 27, 28): Disconnect handed back a lock the held-lock
  prompt had warned about.** With a copy here, the prompt warned that the lock may
  be another computer's and worded its button "Release anyway"; the sign-out survey
  listed the same lock under "These projects will be handed back" and released it
  behind a plain Disconnect (`speleoSignOut.ts:121` released every account lock but
  one this computer can positively not have taken; `SpeleoSignOutDialog.tsx:101`).
  - Fix: Disconnect hands back only locks this run claims (`lock_claimed_here`).
    The account's other locks over a copy or a recent file here (`heldUnclaimed`)
    are listed in the prompt's words with "Release anyway" per project, Disconnect
    leaves them held, and Cancel keeps the initial focus. The "hands back every
    project you are editing" line is said only when there is something of this
    computer's to hand back. A lock this computer can positively not have taken is
    still left alone without a dialog (X2, screenshot 13). A crashed session's lock
    over a copy here is now left held by a plain Disconnect too, with the dialog
    saying so; the next sign-in's prompt offers it again, as for any lock left held.
  - Tests: the survey and dialog vitest cases (six fail on e9df1b642) and the new
    case of `speleodb-exit-prompt.spec.ts`; that spec's local-state fixture now says
    `lock_claimed_here`, which the backend always reports, for the locks its
    scenarios took here.
- **P3s, each small enough to fix here:**
  - *06, the release-failure prompt stated a lock it could not vouch for*
    (`SpeleoReleaseFailedDialog.tsx:64`): only an edit session knows it holds the
    lock, so the prompt gets whether the session was editing when it tried to leave
    (Close, switch, quit) and otherwise says it could not check whether the account
    holds the lock, "if your account holds it ...". Tests: the routes cases and the
    dialog's wording case.
  - *07, "free when last checked" after a landed acquire*: a Start Editing refusal
    that says the lock may be held forgets the last scan's entry for the project, so
    the offline tab says "Offline - lock state not known". Tests: the
    `lock_may_be_held` case of `useSpeleoActions` and an autoAcquire case.
  - *16b, Cancel after a refused upload left no sign of the conflict*
    (`speleoStore.ts:1405` and the other three divergence arms recorded only the
    prompt's origin): every `divergence_needed` now marks the link as in conflict,
    as the #202 re-verify's arm already did, so the tab keeps "Resolve
    synchronization conflict". Tests: the four apply* divergence cases.
  - *20, the behind line while editing named no next step*
    (`SpeleoWorkspaceHeader.tsx:77`): while editing it says "Reload Project
    downloads it, and you go on editing" (remote_ahead is reported only for a clean
    copy, and the reload carries the session). Test: the header's editing case.
  - *26, the first Disconnect check failure gave no cause* (`speleoSignOut.ts:194`
    dropped the error): the guard words it by cause and the dialog shows it at once.
    Tests: the guard and dialog cases.
  - *The "newer revision ... which is older" toast outlived the download*
    (`useOpenRemoteProject.ts:380`, an unregistered key nothing dismissed):
    `applySpeleoTreeReplacement` dismisses the offline-open toasts when a revision
    from SpeleoDB replaced the copy. Test: `speleoTreeReplacement.test.ts`.
- **Left**
  - The capture's P3 10 (the conflict prompt does not say what moved SpeleoDB) and
    P3 12 (Settings > Map's satellite default has no effect; outside the sync work)
    were not in this round.
  - Issue 4's release is unchanged (above), so two live sessions of one account can
    still pull the lock from under each other; the loser now says so, and the open
    no longer starts a second edit session without a word.
  - "Couldn't hand back the lock" decides certain or not from the frontend's edit
    state when the user leaves. A backend flag on the release result (the claim was
    unbound before the release) would also cover a session that reached the prompt
    some other way; not needed by any path today.

### The X3 pre-PR review

The pre-PR review of 54db7c016..438676e91 (the stack through the second
capture's round) asked for one must-fix, three should-fixes and two nits. Each
fix has a test that fails on 438676e91 (or, where the test needs a new API, an
injection that puts the old rule back).

- **Must-fix 1: a release this session started showed "Editing stopped".**
  "Discard and hand back the lock" (`SpeleoDiscardChangesDialog` ->
  `discardChanges(false)`): the backend released, shut the gate
  (`speleodb_push.rs:582-587`) and returned null; the frontend skipped
  `routeStartEditingResult`, so the link stayed 'editing', and the tree
  replacement's status re-read (no holder, editing off) went through
  `speleoLockLostMessage` beside the success toast. Any status reply that
  overlaps a release this session makes reads the same way.
  - Fix: every release the frontend starts runs under an own-release marker
    (`ownSpeleoRelease`: Discard, Done, Switch to Read Only, the held-lock
    prompt's and the sign-out's per-project releases, the revert's upload and
    unwind, the switch and quit's settle, Close), and `refreshLinkStatus` raises
    no lost notice for a reply that overlapped one of the same project's lock
    (keyed by project after the X4 delta review, below). Where this session's own
    release of the open project's lock lands (Discard's null, the held-lock
    prompt's or the sign-out's release of the open project), the session is
    marked read-only (`markOwnReleaseEnded`), so a later refresh has no edit
    session to lose; Done and Switch to Read Only already did.
  - *Rejected:* setting read-only before the release (a failed Switch to Read
    Only returns the same session to editing, and a failed release keeps the
    lock), and a flag on the backend's status (the status cannot tell whose
    release freed the lock either).
  - Tests: `speleoOwnRelease.test.tsx`, one case per path with the reply the
    review saw; all ten fail on 438676e91, and a real loss (the control) still
    says "Editing stopped".
- **Should-fix 2: an add taken back with Undo did not count as clean.** The
  CHANGELOG, saving.md and SpeleoDB.md said an edit taken back by hand or with
  Undo and saved counts as clean, but the digest hashed `counters`, and Undo never
  hands an id back. The claim is now true: the digest leaves `counters` out (rule
  2, its own domain string, so a rule-1 digest never matches and the copy keeps
  its mark until its next download or upload). Safe for the question the digest
  answers: every id the body uses is below its counter, so counters ahead of
  BASE's over BASE's body name ids nothing uses, and the copy's next download
  installs BASE's counters with BASE's body. Test:
  `an_added_section_undone_and_saved_counts_as_clean` (fails on 438676e91: the
  mark stays).
- **Should-fix 3: the open over this account's lock forgot the kept-changes
  sentence.** The own-account `locked_by` branch
  (`autoAcquireSpeleoLock.ts:103-106`) ran before the un-uploaded check; it now
  has a variant that says the changes are kept. Test: the autoAcquire case over
  un-uploaded work.
- **Should-fix 4: an undescribed revision on our BASE counted as our upload.**
  `UnansweredUpload::made` matched a revision with no description, by our account,
  on our BASE, on any server, and could clear the dirty mark over work that never
  reached SpeleoDB. Our upload always carries a description (the upload verbs
  put in a default), and a server that stores descriptions shows it, so an
  undescribed revision now matches only where the history carries no
  descriptions at all (`history_records_descriptions`): an older server, the
  documented residue. Such a server sends an empty description, which the history
  client reads as none (amended after the X4 delta review, nit 3: the client
  used to keep it as `Some("")`, so the fallback never ran outside tests). `classify_ancestry` and `not_our_own` (the replaced report)
  both read it from the full history. Test:
  `an_undescribed_revision_is_ours_only_where_the_server_keeps_no_descriptions`
  (the mixed-history case is Landed on 438676e91; the residue is pinned).
- **Nit 5:** `stalx_v2.rs`'s module doc said nothing in the app used it; it is
  the app's format, and a 0.6.0 SQLite file is only read.
- **Nit 6:** `api_error`'s middleware arm kept the error text, which may carry
  the request URL; it keeps a fixed detail now (no middleware is installed).
  Test: `a_middleware_failure_keeps_no_url`.

### The X4 delta review

- **Should-fix 1: the own-release marker was project-blind.** Releasing a stale
  lock on another project (the held-lock prompt, the sign-out loop) while a
  refresh of the open project was in flight swallowed that project's genuine
  "Editing stopped", for good once the reply turned it read-only. The marker is
  keyed by the project whose lock is released (`ownSpeleoRelease(projectId,
  work)`), and the refresh asks about the project its status is for. Close, the
  settle, Done, Discard, Switch to Read Only and the revert pass the open (or
  captured) project's id. A release of an unknown project still overlaps every
  refresh. Test: "a release of another project does not swallow the open
  project's real loss".
- **Nit 2: the sign-out's marker ended at the bound.** It wrapped `bounded(...)`,
  whose race gives up on the reply while the release still runs; a late landing
  then read as a loss. The marker sits inside the bound around the release, and
  the open project's session is marked as ended here when the release lands.
  Test: the late-landing case (fake timers).
- **Nit 3: the undescribed-history fallback never ran in production.** The
  history client kept an empty description as `Some("")`; it reads an empty or
  blank one as none now (above, should-fix 4). Test:
  `an_empty_revision_description_reaches_the_history_as_none`.

### X6: a reload carried an edit session over a lock SpeleoDB no longer recorded

Farm finding P2-take-remote-editable on `sdb/core` dff90754d (seed 950411, step
12): the app opened a project and took its lock (step 7), edited (8), an upload
landed over another revision (9), an actor of the same account released the lock
on the server the way another session's Close does (10), and `take_remote`
returned with the edit gate open while SpeleoDB recorded no lock on the project
(12). A regression of X3's demo item 2: before it, Take the SpeleoDB version
released the lock and ended read-only.

- **Cause.** `speleodb_take_remote` (`speleodb.rs:1405`) hands its fast-forward
  (`:1445`) the decision to carry the edit session, and `speleodb_fast_forward`
  takes it from the session's own claim alone (`speleodb_pull.rs:433`,
  `was_editing = self.edit_enabled()`, passed to `open_project_download` at
  `:475`, carried by `speleodb_edit_gate.rs:147`). The claim is this process's
  record; SpeleoDB's lock is the account's, so another session of the account can
  hand it back without this one hearing. Reload Project's fast-forward
  (`speleodb_push.rs:616` `speleodb_reload_locked`, `:687`) carried it the same
  way. Resume editing on a copy behind does not: Start Editing acquires first, and
  the acquire is SpeleoDB's own answer. A Restore with `restore: true` carries it
  too, but its only path from the UI (the revert) runs Start Editing right before
  it; the standalone Restore dialog is registered but nothing opens it.
- **Fix.** `speleodb_confirm_lock_for_reload` reads the holder once (the project
  read every holder check uses; the history read carries the project too, but the
  client keeps only its commits, and no test server keeps its holder honest), right
  before the reload commits, and only when there is an edit session to carry.
  This account: the session goes on. No holder, or another account: the gate is
  shut (the claim stays as the pointer a later holder-checked release needs), the
  reload opens read-only, and the result says why: `TakeRemoteResult.editing_stopped`
  and `UpdateResult::FastForwarded.editing_stopped` (`EditingStopped::Released` or
  `HeldBy { email }`). The frontend marks the session read-only before the tree
  replacement (so the status re-read raises no generic lost notice, which would
  say the just-discarded changes are kept) and shows "Editing stopped" worded for
  the SpeleoDB version now on screen: handed back elsewhere, Resume editing takes
  the lock again; or held by someone else now.
- **No reply.** A holder read that gets no reply (or a 5xx, or no email to compare
  with) keeps the session, as before the check. Shutting it instead would end a
  live session on every network blip, and Resume editing could not take it back
  offline; carrying it loses nothing, because every upload reads the holder again
  before it sends (`speleodb_require_owned_lock_locked`) and shuts the gate then,
  and the next status refresh that hears from SpeleoDB raises "Editing stopped".
- **Tests.** Same-account loopback server (which can now drop project reads):
  `taking_the_speleodb_version_after_the_lock_was_handed_back_elsewhere_stops_editing`
  (the trace shape; fails on dff90754d with the gate open), `..._over_a_lock_someone_else_holds_now_says_who`,
  `..._when_the_holder_read_gets_no_reply_goes_on_editing`, and
  `reload_project_after_the_lock_was_handed_back_elsewhere_stops_editing` (injection:
  drop the check from `speleodb_reload_locked`); the existing keep-the-lock tests
  assert `editing_stopped` is none. Vitest (Take remote released and held_by,
  Reload Project released; fail on the old hook) and the new case of
  `speleodb-take-remote.spec.ts` (fails on the old hook: "editing did not start").
- **Left.** The window between the holder read and the reload's commit (the
  download) is not closed: a release landing in it is caught by the next upload's
  holder read or status refresh. A Restore over a lock released elsewhere is not
  checked (unreachable from the UI today, above).

### X7: a Revert's upload after the Revert (an oracle finding)

Farm finding R3-same-account-overwrite on `sdb/core` 411227301 (seed 950443,
step 8): one account opened its project and edited (steps 1-2), Save Changes
uploaded the edit (4), Revert back one restored the revision before it (5),
and Save and Close (8) uploaded the restored copy over the app's own upload.
The oracle read that as a revision superseded without a prompt.

- **Cause: the fuzzer's model, not the app.** In step 5 the restore answered,
  and the fuzzer then reloaded the project through its model of `loadProject`,
  which runs `cancel_loading`. That is a project switch: it moves the backend's
  session on and ends the edit session the restore had carried. The trace shows
  this as `link_status edit=false` after the swap. Done then answered
  `NotEditing` with nothing uploaded, and the restored copy stayed as
  un-uploaded work until step 8. The real Revert (`useRevertRevision.ts:231`
  Start Editing, `:304` the restore, `:123` Done) restores through
  `useRestoreRevision.ts:77`. It reconciles only through
  `applySpeleoTreeReplacement` (`:103`), never through `loadProject` or
  `cancel_loading`. The backend carries the session through the swap
  (`speleodb_revisions.rs:529`, `restore` passed as the carry). So the app's
  own Done uploads the Revert. This does not reproduce in the product on
  411227301 or on `sg/core` 54db7c016 (the same lines). It is not X3 issue 1
  either: the restore paths never went through `loadProject`. The same model
  error sat at the other in-place reloads (a View, an Unwind, the
  fast-forwards of Start Editing and Reload Project, Take the SpeleoDB
  version).
- **Is the later upload an overwrite?** No. The restore installs the older
  revision as un-uploaded work on top of the tip it rode on, here the app's own
  upload. Uploading it later (after a failed Done, from Retry, Save Changes or
  Save and Close) drops exactly what the user asked the Revert to drop, and
  only on the copy's own BASE. The oracle now excuses such an upload when its
  parent is the BASE the restore rode on and every line it drops is one the
  restored revision does not have either. Anything else is judged as before.
  The product needs no change.
- **Fuzzer fixes** (on the fuzzer's own branch, not here): every in-place
  reload is modelled as `applySpeleoTreeReplacement` (file status, link status,
  no `cancel_loading`). A Revert's restore is remembered until an upload of it
  lands, and the overwrite check takes that claim with the two conditions
  above. Its test (`a_later_upload_of_a_restored_revision_is_the_reverts`)
  fails when the claim is ignored.
- **Guards on this branch** (both pass on 411227301; there is no product bug
  to fail first):
  - `a_revert_keeps_its_edit_session_and_uploads_the_restored_revision`
    (same-account loopback server: two uploads, Revert back one, the gate
    stays open, Done releases with the restored survey uploaded). Injection:
    the swap carrying no session fails it at the gate.
  - The vitest case "calls neither loadProject nor cancel_loading" in
    `useRestoreRevision.test.tsx`.
