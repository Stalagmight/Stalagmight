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
