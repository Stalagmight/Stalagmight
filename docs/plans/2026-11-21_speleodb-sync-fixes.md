# SpeleoDB sync fixes - F1, F2, F4, R3 (stacked on #827)

Four pre-existing SpeleoDB sync bugs that the SpeleoDB protocol fuzzer found,
present on release/0.6.0 as well as on #827's new survey model and `.stalx`
v2: F1, an
open that refuses instead of opening the cached copy when the reply is lost;
F2, an "un-uploaded" mark left over BASE content by a crash between an edit
and its save; F4, an update restart's lock release that stands down for
another account's claim; R3, a same-account second session whose push
silently replaces the other session's revision. Each is fixed at its root
cause on its own branch, reproduced first by a test that fails on the code
before the fix, with regression tests whose injection is stated, and the
user and developer docs updated; one section per bug below.

## S1 / F1: an open whose reply is lost opens the cached copy offline

**Trace** (seed 900366, step 10): an open of a project with a clean cached
copy, with every GET under `/api/v2/projects/` dropped before any reply,
refused with Retry ("Network error talking to SpeleoDB: error sending
request") instead of opening the cached copy offline.

**Root cause.** Two classifiers. The SDK's send failures reach the app as
`HttpError::Middleware`, whose arm in `client::api_error` knew only a timeout
and a TLS failure; everything else (a refused connection, a DNS failure of an
`https://` host, a reset, and hyper's "connection closed before message
completed", which has no I/O error under it at all) was
`TransportCause::Unknown`. Body reads went through `network_error`, which
walked the I/O kinds but not `UnexpectedEof`, so a reply cut short was
`Unknown` too. `offline_open_result` falls back only on `Refused | Timeout |
Dns`, so in practice an open fell back only on a timeout (the docs carried it
as "Known gap F1, #669"). Reproduced first: the new client tests saw `Unknown`
for a closed, reset, refused and DNS-failed request, and `Refused` for a reset
in the body; the new open test got the fuzzer's exact refusal.

**Fix.**
- One typed classifier, `client::transport_cause(&reqwest::Error)`, for the
  send (`Middleware(Reqwest(e))`) and the body reads alike, in this order: a
  timeout anywhere; DNS; TLS (`rustls::Error`); the connector failed or an I/O
  error says refused or unreachable (`Refused`); hyper's incomplete message,
  canceled request, closed channel or aborted body write, or an I/O reset,
  abort, broken pipe, not-connected or early end of file (`Dropped`); else
  `Unknown`. Typed throughout (`hyper` and `reqwest-middleware` are named as
  dependencies only for their error types, the way `rustls` already was).
- A new `TransportCause::Dropped`: the connection was made, then closed or
  reset before the whole reply arrived. `Refused` now means the connection
  could not be made, so nothing was sent (a blackholed address's EINVAL is a
  connect failure, so it is `Refused` now, not `Unknown`). A reset while the
  reply is read was `Refused` and is `Dropped` now.
- `TransportCause::got_no_reply()` = `Refused | Dropped | Timeout | Dns`: the
  one definition of "a read that heard nothing from the server".
  `offline_open_result` falls back exactly then. `Tls` (the peer did not prove
  it is the server) and `Unknown` (for instance a reply that is not HTTP: the
  server was reached) still report the error.
- Writes are unchanged on purpose: every write treats every transport failure
  as an unknown outcome whatever the cause (the push's `Unconfirmed`, the
  acquire's holder check, sdk#4), and mutations are never retried. The cause
  matters to a write only for its wording, which is why `Dropped` gets its own
  line ("The connection to SpeleoDB at <server> was lost before it finished
  answering") instead of `Refused`'s "Could not connect".
- The offline-open rules stay: only the cached copy of this project on this
  instance; this account's un-uploaded edits open still marked as not
  uploaded (never passed off as the server's state); another account's open
  read-only naming whose they are; a discard never falls back; no owner record
  is written by an offline open.

**Tests** (offline, loopback listeners that lose replies on purpose; nothing
contacts a SpeleoDB server).
- `speleodb::client::tests::transport_causes` (speleodb crate): closed before
  the reply, reset before it, head or body cut short, reset in the body (each
  on the project, history and both download reads, tried three times), a port
  nothing listens on (`Refused`), an `https://` name under `.invalid` (`Dns`),
  a reply that is not HTTP (stays `Unknown`), the `got_no_reply` table, and
  five writes (acquire, release, create, patch, upload) whose reply is lost:
  `Dropped`, sent once.
- `core/state/speleodb_offline_open_tests.rs`: the whole open path for each
  way of losing every GET under `/api/v2/projects/` (clean copy opens offline,
  untouched, no owner record), un-uploaded work (mine still marked, theirs
  read-only), not-HTTP and discard never fall back, and a push whose upload
  reply is lost (closed, reset, body cut short) stays unconfirmed, dirty at
  BASE and locked, sent once.
- `speleodb_pull::tests::offline_fallback_requires_connection_failure_and_matching_cache_identity`
  now includes `Dropped`; `src/utils/speleoError.test.tsx` checks the new line
  differs from every other cause's and never says "could not connect".

**Injections** (each made, run, reverted):
1. `offline_open_result` back to the `Refused | Timeout | Dns` filter: the two
   open tests and the pull unit test fail.
2. The send arm back to its own timeout/TLS classifier: six client tests
   (closed, reset, cut short, refused, DNS, writes) and three open tests fail.
3. hyper's incomplete message not recognised: closed, cut short and the writes
   fail (client), and the open tests fail.
4. `UnexpectedEof` not recognised: cut short and the writes fail, and the open
   tests fail.
5. `got_no_reply` also counting `Unknown`: the not-HTTP tests (client and
   open), the `got_no_reply` table and the pull unit test fail.
6. The push treating a no-reply transport failure as not landed: the upload
   test fails (writes must stay uncertain).
7. A reset mapped to `Refused` again: both reset tests fail.

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

**Cost.** Only a copy marked un-uploaded with a recorded digest is hashed: a
save, an open, a link read of it. On the format bench (release, one member
scan and SHA-256) the digest takes about a third of an encode: 1.4 ms for
5,000 shots (0.6 MB), 6.2 ms for 20,000 (2.4 MB), 29 ms for 100,000 (12 MB),
against 2.8, 14 and 87 ms to encode. Hashing during the encode instead would
save the scan; not worth its change of the encoder here.

**Ruled out.** A copy without a digest (a 0.6.0 copy, a version 2 copy
installed before this) keeps the conservative answer, as does any survey that
differs in a byte the digest covers (an ignore kept only for a shot the history
can bring back, another writer's layout, a project name the server-name fold
(#216) changed on a clean copy, until its next download or upload). Trusting a digest the file states
for its own body was rejected: a writer that keeps unknown `local` keys
verbatim while changing the body (an older build, a third-party tool) would
make it lie; the body is always re-hashed.

**Tests** (`core/state/speleodb_base_survey_tests.rs`, loopback SpeleoDB):
the fuzzer trace; a crash after an edit reopens clean (Recents and the
SpeleoDB browser); a saved edit keeps its mark; an edit taken back and saved
counts as clean (by hand, by Undo); a confirmed upload records the digest; an
edit taken back during an upload stays un-uploaded; a copy without a digest
keeps its mark (0.6.0 and version 2); the digest goes with its BASE and its
project. Five fail on the code before the fix.
Injections, each caught: the probe returning the header as read, no digest at
the install, the open's clear off, the save's clear off, no digest at the
landing, the landing's `has_unpushed_edits = true` removed, the clear in
`set_speleo_link` removed. The digest is pinned in `persistence/stalx_v2/format_v2_digest_tests.rs`; the frontend
mirror in `projectSaveActions.test.ts` and `speleoStore.test.ts`.
