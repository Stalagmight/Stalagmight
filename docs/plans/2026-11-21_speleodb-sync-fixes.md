# SpeleoDB sync fixes found by the protocol fuzzer

The SpeleoDB protocol fuzzer, run against the survey-graph branch (PR #827, whose
uploads are version 2 `.stalx` files), raised four sync bugs that are also present
on `release/0.6.0`: F1, F2, F4 and R3. Each is fixed in its own stream (S1 to S4),
stacked on #827, with a test that replays the fuzzer's trace and fails without the
fix. This note records, per bug, the trace, the root cause, the fix, what was
considered and rejected, and the tests with the injections they were checked
against.

## S3: a release stands down only for a later session of its own account (F4)

### The trace

1. The owner signs in, opens a SpeleoDB project and takes its lock.
2. The owner's other session (another computer, or the website) hands the lock
   back. This process is not told, so the owner's claim stays in the claim list.
3. The collaborator signs in on the same server without signing out. Claims of
   other accounts are kept across a sign-in by design: each is the only local
   pointer to a lock that account may still hold (a lock kept over un-uploaded
   work, a release that failed).
4. The collaborator takes the lock (Start Editing; the probe finds it free).
5. The update restart releases: it came to "not held" with no request at all, and
   the collaborator's lock stayed held. SpeleoDB never expires a lock on its own,
   so everybody else stayed locked out until the lock was released from the
   held-lock prompt at the collaborator's next sign-in, or on the website.

### Root cause

Every release that can run after a later session has started (a close, a project
switch, a quit or the update restart, a delete's captured release, Start Editing's
unwind after a session boundary, the stranded-claim cleanup) is reclaim-checked:
`speleodb_release_unless_reclaimed_locked` stands down when a later session has
claimed the project meanwhile (F8 of the lock-session races plan). The reason is
that SpeleoDB treats a re-acquire by the account that already holds the lock as a
no-op, so that later session holds the very mutex the release would free.

The check asked `speleodb_claim_on_deployment(endpoint, project)`, which matched a
claim of ANY account on that deployment. Only a session of the SAME account can have
inherited the lock. Another account's claim is either live (that account holds the
mutex, so the releasing account does not, and the holder check would send nothing
anyway) or stale, as in the trace. Counting it stood the collaborator's own release
down for the owner's stale claim.

What each caller did on the trace's state:

- the close reported `NotHeld`, and the frontend closed as if the lock were handed
  back;
- a project switch, a quit and the update restart sent nothing;
- a delete reported success with no warning, over a lock left held on a project
  whose copy it had just removed;
- Start Editing's unwind after a boundary sent nothing;
- the stranded-claim cleanup sent nothing and also lost the claim: it takes the
  claim out before it releases and puts it back only on an error, so the lock was
  left held with nothing local to find it.

### Fix

`speleodb_claimed_by_releasing_account(params)` replaces the account-blind
predicate: a claim on `params.project_id` taken by the account the release is FOR
(`params.endpoint` and `params.email`, captured with its token), compared through
`LockClaim::taken_by` (normalised endpoint, email case-insensitively: the
credential layer's account identity).

- Not the account signed in when the release runs: a deferred release can outlive
  the sign-in that captured it, which is why the original check refused to ask
  about the live account. The release's own account was the question all along.
- Another account's lock stays protected by the holder check (lock#4): a release
  is sent only when SpeleoDB names the releasing account as the holder.
- `LockClaim::on_deployment` is private now. Every claim test outside the type
  names the account, so no new caller can drop it again.

The other readers of the claim list were checked for the same confusion. The old
predicate had this one caller, and every other reader already names the account:
the edit gate (`edit_gate_open`, `speleodb_commit_open_gate`), `speleodb_holds_claim`,
`speleodb_claim_is_kept`, `speleodb_lock_acquired_here`, the claim writers
(`speleodb_bind_edit_claim`, `speleodb_record_unbound_claim`,
`speleodb_keep_lock_deliberately`), the release-side clear
(`speleodb_forget_lock_acquired_inner`, the live account or the captured one), the
cleanup's choice of claim, `speleodb_take_acquisition_by_this_flow`, and the two
restores (`speleodb_restore_lock_claim` and the cleanup's), which compare the full
identity through `is_same_lock`.

### Considered and rejected

- **Ask the server instead of the claim list.** The holder read cannot tell a
  same-account successor's inherited lock from the releasing session's own: both
  read "held by this account". That is exactly why the stand-down consults the
  claims, so the account-scoped claim test is needed either way.
- **Drop other accounts' claims at sign-in.** Each may be the only pointer to a
  lock that account still holds over un-uploaded work, or after a failed release;
  dropping it strands that lock (the rule the claim list was built on).
- **Clean up the owner's stale claim when the collaborator signs in.** Only the
  owner's token can read whether the owner still holds the lock. The claim is
  harmless once it no longer stands anybody's release down. After the owner's next
  sign-in, the stranded-claim cleanup (a status refresh with no project open) reads
  the holder and ends it.

### Tests

`src-tauri/crates/core/src/core/state/speleodb_foreign_claim_tests.rs`, against a
loopback SpeleoDB that keeps the project's mutex and tells the two synthetic
accounts apart by their tokens. The trace is replayed with the app's own verbs:
token sign-ins, Start Editing, a second client's release.

- `the_update_restart_hands_back_the_collaborators_lock_over_the_owners_stale_claim`:
  the fuzzer's step, through `speleodb_release_held_lock_bounded` with a managed
  event sink, as in the app.
- `every_release_of_the_collaborators_lock_goes_out_over_the_owners_stale_claim`:
  the close (reported `Released`), the project switch and the delete (no warning).
- `start_editings_unwind_after_a_boundary_ignores_another_accounts_claim` and
  `the_stranded_claim_cleanup_hands_back_a_lock_beside_another_accounts_claim`.
- `only_a_claim_of_the_releasing_account_stands_a_release_down`: with the owner
  signed in and the collaborator's release, the owner's claim does not stand it
  down, and the collaborator's own claim (under another email case) does.

All five failed before the fix (no release request, the lock left held; the close
reported `NotHeld`; the cleanup lost its claim). Injections, each run against these
tests and the existing `speleodb_session` tests:

- the old deployment-only predicate fails all five new tests;
- asking about the account signed in now fails
  `only_a_claim_of_the_releasing_account_stands_a_release_down` and the existing
  `a_captured_release_ignores_a_same_id_session_on_another_deployment`;
- no stand-down at all fails the second half of
  `only_a_claim_of_the_releasing_account_stands_a_release_down` and the existing
  `a_captured_release_stands_down_for_an_inherited_successor_session`.
