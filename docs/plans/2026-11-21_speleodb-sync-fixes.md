# SpeleoDB sync fixes (2026-11-21)

Four pre-existing SpeleoDB sync bugs that the SpeleoDB protocol fuzzer found on
the survey-graph branch (and that are also on release/0.6.0), fixed in one PR
stacked on #827: S1 (F1, an open whose connection drops before the reply does not
fall back to the cached copy), S2 (F2, a crash between an edit and its save leaves
a phantom "un-uploaded" mark), S3 (F4, a stale claim of another account makes a
live session stand down from its own release) and S4 (R3, a second session of
the same account uploads over the first one's revision). Each section below is
the design note of one fix: the trace, the root cause, the decision, the tests
and what is left.

## S4 - R3, same-account overwrite

**Trace (fuzzer, directed list same-account-overwrite).** Two sessions of the same
account both hold the project's lock (SpeleoDB locks are per account, so the
server cannot tell them apart). Session A uploads. Session B, still based on the
older revision, uploads too, and its upload lands on top of A's: B's copy does not
contain A's work, so A's revision is no longer HEAD; it is only in the history.

**Root cause.** The push already fenced BASE against the server's HEAD before every
upload (`sync::plan_push`), but its #193 recovery - our own upload whose answer was
lost, which did land - adopted every commit in BASE..HEAD that our account had
authored (`classify_ancestry`, `author_email == my_email`) and uploaded on top.
The email cannot tell this session from another session of the same account, and
neither can the lock (both hold it). The status verdict had a lock-holding gate
for this (review finding lock-session#1), which does not help when both sessions
hold the lock; its suggested fix (require an authored claim in `plan_push`) would
still let the session that took the lock first overwrite the one that inherited
it.

**Decision.**

- The authorship test is what THIS session uploaded, never the email:
  `sync::OwnUploads` = the revisions this session's uploads were answered with
  (the 200 envelope's `hexsha`), and the one upload whose answer never came, with
  the BASE it sat on and its description. `classify_ancestry` takes a gap commit
  as ours only when it is one of those, or the single revision directly on the
  unanswered upload's BASE, authored by our account, whose stored description
  matches ours (`commit_message::normalize_like_server`; a history without
  descriptions, from an older server, matches on the BASE and the account). Any
  other commit between BASE and HEAD - a stranger's, or another session's of our
  own account - is `Diverged`: nothing is uploaded and the existing divergence
  prompt opens (`DoneResult` / `PushResult::DivergenceNeeded`, which Save
  Changes, Done and the close, quit and sign-out uploads already route to
  `SpeleoDivergenceDialog`, the same dialog the #202 re-verify divergence opens).
- The record lives in `AppState::speleodb_own_uploads`
  (`core/state/speleodb_own_uploads.rs`), keyed by endpoint, account, project and
  session generation; anything else reads as "nothing uploaded", which fails
  closed to the prompt. It is process-local and never persisted: a restart
  reopens through Start editing, whose reconcile has no authorship recovery, so
  persisting it would buy nothing.
- The record is set right before the upload is sent and answered on every
  definite outcome: a 200 that names its revision (kept as landed), a 304 or a
  refusal status (nothing landed), an ancestry adoption (it landed as the tip).
  It stays unanswered on a dropped connection, an exhausted 5xx, a 200 that names
  no revision and an answer body that cannot be read: those are exactly the cases
  the next push must still be able to recover.
- The status verdict uses the same record, so the SpeleoDB tab shows the conflict
  that Save Changes will report rather than "up to date" over it.
- The race the check cannot close (SpeleoDB has no conditional upload; the upload
  view takes no base, verified in SDK 0.9.0's `v2_projects_upload`): a revision
  landing between the HEAD check and the upload. After a 200 the push reads the
  history once (a direct GET, like the pre-push name fold) and
  `sync::replaced_by_upload` lists what lies between the upload's BASE and the
  new revision's parent (never SpeleoDB's creation commit; off BASE's chain, only
  the revision directly under ours). It rides the next `Released` / `Pushed` as
  `replaced_revisions` (kept in the record until a result reports it, so a
  late-edit `PushUnconfirmed` does not lose it). The frontend shows it on a
  persistent `replaced_revision` banner instead of the success toast, naming the
  latest one as the Revision History shows it and saying it is still there to
  view or restore; the exit prompt and the sign-out stop on it (like a
  divergence) so the banner is read before the project closes.
- No server change (outside this repository). A server-side conditional upload
  (refuse an upload whose parent is not the client's BASE) is the only complete
  fix for the race and is worth proposing upstream.

**Tests.** `core/state/speleodb_same_account_tests.rs`: two `AppState`s of one
account (own thread, cache root and config each) against a loopback server that
keeps a real history - the second session is refused and uploads nothing (Save
Changes and Done), an unanswered upload is no licence to adopt the other
session's revision, our own unanswered upload is still adopted, a push with HEAD
== BASE uploads, the lost race is named (Save Changes and Done) and reported
once, and the status shows the conflict. `speleodb/sync.rs` unit tests for the
gate, the description match and `replaced_by_upload`. Vitest: the refusal from
Save Changes and Done routes to the divergence prompt; the replaced report rides
the banner only; the exit (quit) and sign-out stop on it. Playwright:
`speleodb-same-account.spec.ts`. Injections, each failing the named tests:
authorship by email in `classify_ancestry` (the repro, the status test, five
sync tests and two older speleodb.rs tests); no description match (the
no-licence test and its unit test); no post-upload check (both race tests);
only the direct parent in `replaced_by_upload` (its multi-revision unit test);
no `replaced` arm in the exit/sign-out dialogs, no banner in `applyPush`, or the
success toast kept (the corresponding vitest cases).

**Behaviour changes in existing tests.** Two `speleodb.rs` status tests and one
`sync.rs` status test asserted the old shortcut (holding the lock with an
authored claim made any commit of our account "ours"); they now record an upload
of this process first, and assert `Conflict` without one.

**Left.**

- The residue of the description match: our unanswered upload and another
  session's revision with the same description on the same BASE are still told
  apart by nothing; the window needs an unanswered upload of ours AND the other
  session pushing the same words onto the same revision.
- A quit that waits for an upload started elsewhere (Save Changes, then Quit)
  quits after it; a replaced report from that upload is only in the log.
  Likewise the frontend's `released`-with-newer-edits arm replaces the banner
  with its push-failure line.
- The divergence prompt's "Keep my local version" re-runs the push, which is
  refused again while the other session's revision is HEAD (as for any
  divergence); the ways through are Take the SpeleoDB version and Save my
  version as a separate file.
- Upstream proposal: a conditional upload on SpeleoDB (`base` / `parent` on the
  upload view, refused when HEAD moved), which would close the race and make the
  post-upload report unnecessary.
