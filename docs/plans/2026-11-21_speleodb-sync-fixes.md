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
  `Refused`'s "Could not connect".
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
  `history` (cut per push) and `end`, with `project.modified` (an edit and the
  edit back change it and nothing else) and the link's `endpoint` and `name`
  (rewritten without an edit) left out. The BASE id is hashed first, so a digest
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
  ours (`normalize_like_server`; a history without descriptions matches on BASE
  and account). Anything else between BASE and HEAD is `Diverged`: nothing is
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
  on the same BASE are told apart by nothing.
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
