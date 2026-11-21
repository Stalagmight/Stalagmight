# SpeleoDB sync fixes (2026-11-21)

Four pre-existing SpeleoDB sync bugs, found by the SpeleoDB protocol fuzzer on
the station-graph branch (PR #827) and present on `release/0.6.0` too, fixed in
one PR stacked on #827: F1 (S1), F2 (S2), F4 (S3) and R3 (S4). Each section
below is the design note of one fix: the trace, the mechanism, the design and
what it rules out, and the tests that fail without it.

## S2: F2-phantom-unpushed (a stale un-uploaded mark)

**Trace** (fuzzer seed 900426): Edit on the open SpeleoDB copy; a crash that
loses the unsaved edit; another session pushes; the next open of the copy
refuses with `UnpushedLocalEdits` ("this project has local edits that were
never pushed"), although the copy holds exactly its BASE revision. A close with
"Don't save" after an edit does the same.

**Mechanism.** `mark_remote_dirty` makes the dirty flip durable at the edit
with a local-only save, so that a crash after an edit still knows there was
work; the edit itself reaches the file only with the next save. A crash or a
discard in between leaves the mark over BASE content, and every reader took
the mark at its word: the replace guard, the fast-forward arm, the listing
behind the sign-out and held-lock prompts, the open link behind the exit
prompt. 0.6.0 had the same (one UPDATE of `speleo_link` per edit).

**Design: decide by content, keep the crash safety.**

- *A digest of BASE's survey.* `stalx_v2::survey_digest(bytes, base)` hashes,
  from the bytes as written, every member an upload carries that is survey
  content: everything but `format`, `version`, `solver`, `generator`,
  `summary` (derived), `local`, `history` (cut per push) and `end`, with
  `project.modified` (an edit and the edit back change it and nothing else)
  and the link's `endpoint` and `name` (rewritten from the session and the
  server's name without an edit) left out. The BASE id is hashed first, so a
  digest kept over another BASE never matches. Byte-level, so a link reader
  answers without decoding the survey (plan 6.1).
- *Recorded where the copy is known to hold BASE*, as
  `local.speleodb.base_survey`: every install under a clean link
  (`PreparedDownload::cache_bytes`) and every confirmed upload, from the bytes
  that went up (the 200 arm when it names the new revision, the 304 arm).
  `set_speleo_link` drops it when BASE or the project changes. A Restore
  marks the copy over a BASE it does not hold and records none.
- *Read everywhere the app asks.* The shared link probe (`probe_link` and
  `read_cache_file`, through `speleodb_base_survey::probe_settled`) reads a
  marked file with a digest whole and reports it clean when its survey has
  the digest; nothing is written. The guard, the fast-forward arm, the
  offline open, the listing (sign-out, held-lock prompt, Home) and logout's
  forget-recents all read through it. The open project answers from its live
  link, which the open and the save correct.
- *Cleared durably.* An open clears a stale mark (`settle_on_open`) and the
  open's local-only save writes it. A save whose survey is BASE's again (an
  edit taken back, by hand or by Undo) clears it in the file it writes
  (`clear_stale_mark`) and reports `CommandResult.speleodb_unpushed_cleared`,
  which the frontend mirrors into `hasUnpushedEdits` (the exit prompt's
  input) unless an edit landed meanwhile.
- *The push fence.* Only the mark is cleared; `last_edit_at` stays. The 200
  and 304 arms now SET the mark when an edit came after the push started,
  instead of leaving it: a save that clears a stale mark during the upload
  (the survey back at the old BASE) must not leave the copy clean over the new
  BASE it does not hold.

**Ruled out.** A copy without a digest (a 0.6.0 copy, a version 2 copy
installed before this) keeps the conservative answer, as does any survey that
differs in a byte the digest covers (an ignore kept only for a shot the history
can bring back, another writer's layout). Trusting a digest the file states
for its own body was rejected: a writer that keeps unknown `local` keys
verbatim while changing the body (an older build, a third-party tool) would
make it lie; the body is always re-hashed.

**Tests** (`core/state/speleodb_base_survey_tests.rs`, loopback SpeleoDB):
the fuzzer trace; a crash after an edit reopens clean (Recents and the
SpeleoDB browser); a saved edit keeps its mark; an edit taken back and saved
counts as clean (by hand, by Undo); a confirmed upload records the digest; an
edit taken back during an upload stays un-uploaded; a copy without a digest
keeps its mark (0.6.0 and version 2). Five fail on the code before the fix.
Injections, each caught: the probe returning the header as read, no digest at
the install, the open's clear off, the save's clear off, no digest at the
landing, the landing's `has_unpushed_edits = true` removed. The digest is
pinned in `persistence/stalx_v2/format_v2_digest_tests.rs`; the frontend
mirror in `projectSaveActions.test.ts` and `speleoStore.test.ts`.
