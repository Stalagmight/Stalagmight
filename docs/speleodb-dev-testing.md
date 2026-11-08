# SpeleoDB dev/integration testing

How the SpeleoDB client (`src-tauri/crates/speleodb/src/speleodb/`) is tested against a **real**
SpeleoDB server, and how to design future multi-account / permission /
collaboration tests against the dev box.

The API contract notes below were verified against a local checkout of the SpeleoDB
server source, not against the (sometimes stale) GitHub issue text.

## The dev box

A throwaway SpeleoDB instance on the lab network. Connection details live in the
**gitignored** `.env` at the repo root (never committed):

| Env var | Meaning |
|---------|---------|
| `DEV_SPELEODB_URL` | Base URL of the dev box (e.g. `http://10.10.0.220:8000/`). The client appends the `/api/v2/...` paths. |
| `DEV_SPELEODB_EMAIL` | Login email for the primary (superuser) test account. |
| `DEV_SPELEODB_PASSWORD` | Login password for the primary test account. |
| `DEV_SPELEODB_EMAIL2` | Login email for a **second**, email-verified collaborator (for the two-account tests). Needed only by those tests, which fail naming it when it is unset. |
| `DEV_SPELEODB_PASSWORD2` | Login password for the second account. |

> `cargo test` does **not** auto-load `.env`. The live tests read these from the
> process environment, so pass them on the command line (see "Running the live
> tests"). `.env` is just the local record of the values for the running app and
> for copy-pasting into a test command.

### The configured account is a Django superuser

The account in `DEV_SPELEODB_EMAIL` (`dab@dab.com` on the current box) is a Django
**superuser**. That matters for test design: a superuser can provision *additional*
accounts, so future tests that need more than one user (permission tiers,
collaboration hand-off, lock contention) don't have to share the one login.

Provisioning caveat - **email verification is mandatory**. The server sets
`ACCOUNT_EMAIL_VERIFICATION = "mandatory"` (`config/settings/base.py`), and the
auth-token endpoint rejects accounts whose email is unverified. So a plain
allauth headless self-signup (the headless routes are mounted under
`/api/v2/auth/`) yields an account that **cannot** obtain a token until its email
is verified. To mint a usable test
account, the superuser must create it *and mark its email verified* - via the
Django admin (`/admin/`), a `manage.py shell` snippet, or a management command -
rather than relying on the self-signup flow alone.

Server-side cleanup tooling that exists for tests (run on the box, not from here):
`manage.py wipe_test_user_projects` and `manage.py wipe_test_gitlab`.

### The second account (for multi-user tests)

Some live tests need **two** identities - a project owner and a collaborator - to
exercise permission tiers and cross-user locks. That second account
(`DEV_SPELEODB_EMAIL2`/`_PASSWORD2`, `user2@dab.com` on the current box) must be
**pre-provisioned with a verified email**, because (per the caveat above) a
verified user cannot be created over HTTP with just the superuser bearer token.
Create it once, box-side, e.g.:

```bash
python manage.py shell -c "from django.contrib.auth import get_user_model; from allauth.account.models import EmailAddress; U=get_user_model(); u=U.objects.create_user(email='user2@dab.com', password='ChangeMe123!'); EmailAddress.objects.create(user=u, email='user2@dab.com', verified=True, primary=True)"
```

The two-account tests then **self-seed** the rest over HTTP: the superuser creates
a throwaway project, grants `user2` a permission level
(`POST /api/v2/projects/{id}/permission/user/detail/` `{"user","level"}`), and/or
has `user2` hold the mutex - then deletes the project on the way out. Without
`DEV_SPELEODB_EMAIL2`/`_PASSWORD2` they **fail**, naming the missing variable, so
on a box without the second account run the single-account tier and `--skip` the
two-account tests (see "Running the live tests").

## Test layers

The client follows a two-layer convention (see the plan for #183 and the epic #182):

1. **Always-run unit tests** - pure functions, no network. `dump::classify`, DTO
   parsing, Bearer/URL construction, the in-memory `TokenStore` fallback. These run
   in normal `cargo test` and in CI.
2. **Env-gated live tests** - real HTTP against the dev box. Marked `#[ignore]` so
   they never run in normal `cargo test` or CI. Run on purpose without the
   `DEV_SPELEODB_*` env vars they fail, naming the missing one (#576 E13) - a
   deliberate run must not report `ok` having tested nothing. They live in the
   `live_tests` module of `src-tauri/crates/speleodb/src/speleodb/mod.rs`.

### Self-seeding, self-cleaning (no shared fixtures)

The live round-trip owns all of its own state - there is **no** pre-seeded
`DEV_SPELEODB_PROJECT_ID` to keep alive:

- Each run **creates a throwaway project** with a UUID-suffixed name
  (`stalagmight-spike-<uuid>`) via `client::create_project`.
- A `ProjectCleanup` guard (`Drop`) **soft-deletes** that project on the way out -
  even if an assertion panics partway through - so runs don't leak projects.
- Seeding a known-good `.stalx` goes through the real single-writer dance:
  `acquire_lock → upload_auto → release_lock`. (Upload requires holding the
  project mutex; the server enforces `UserOwnsProjectMutex`.)
- The pull half then exercises `download_dump → classify → extract →
  Database::open_existing`, proving the dump opens self-contained.

This is the pattern to copy for new live tests: **one project per run, created and
destroyed by the test**, keyed by a UUID so parallel/repeat runs never collide.
For multi-user scenarios, have the superuser provision the extra accounts (see
above), grant them the project permission under test, and tear everything down in a
`Drop` guard the same way.

### Poking at the cache: linked files open only from their cache path

A `.stalx` that carries a `speleo_link` row can only be opened from
`<cache>/speleodb/<project_id>/project.stalx` (canonical storage - see
`core/state/speleodb_foreign.rs`). Copy one elsewhere and the open is refused
with the managed-file dialog, which offers exactly two ways forward: open the
managed copy from SpeleoDB, or strip the file's link
(`speleodb_remove_link_at`) and open it as a plain local project. To hand-craft
a foreign-file repro, copy `<cache>/speleodb/<id>/project.stalx` to any other
path and open it.

## Running the live tests

```bash
DEV_SPELEODB_URL=http://10.10.0.220:8000/ \
DEV_SPELEODB_EMAIL=dab@dab.com \
DEV_SPELEODB_PASSWORD=dab \
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-speleodb \
  speleodb::live_tests -- --ignored
```

- `--ignored` is required - the live tests are `#[ignore]`d by default.
- No `--features telemetry` (the speleodb member has no such feature since
  #576 B7) and so no `TELEMETRY_ENDPOINT`, which the telemetry member's
  `build.rs` requires only under that feature.
- Drop a `DEV_SPELEODB_*` var and every test that needs it fails, naming it. On a
  box without the second account, leave the two-account tier out with
  `--skip live_collaborator_sees_read_only_label --skip live_web_viewer_cannot_download
  --skip live_lock_holder_reflects_other_user --skip live_no_permission_field_absent
  --skip live_acquire_contended_returns_400 --skip live_acquire_no_perm_returns_403`
  (and, for the orchestration suite, `--skip live_orch_start_editing_reports_locked_by_other_account
  --skip live_orch_admin_collaborators_teams_and_delete_unlink`).
- `live_bad_credentials_returns_400` needs only `DEV_SPELEODB_URL` (it sends junk
  credentials and asserts the 400 contract); the full round-trip needs all three.

## Verified API contract (for writing more tests)

Checked against the SpeleoDB server source; these override the stale issue text:

- **Base path `/api/v2/`** (`api_router.py`); v1 is a deprecated alias. v2 responses
  are **bare** JSON (the `DRFWrapResponseMiddleware` only wraps `/api/v1/`).
- **Auth header `Authorization: Bearer <token>`** (not `Token`); the token is
  non-rotating (`get_or_create`).
- **Bad credentials → HTTP 400** (`{"errors":{"non_field_errors":[…]}}`), *not* 401.
  A `401` means "not authenticated" on a protected endpoint (e.g. listing with a
  dead token); a missing auth header yields `403`.
- **`permission` is a STRING label** - `"WEB_VIEWER" | "READ_ONLY" |
  "READ_AND_WRITE" | "ADMIN"` - and the field is **absent** (not null) when the user
  has no permission. (Issues #188/#203 say "parse the integer" - wrong.)
- **Project `type`** is a string enum: `ariane | compass | stalagmight | stickmaps |
  therion | walls | other` (`stalagmight` confirmed in the production `/api/schema/`,
  2026-11-03). Stalagmight writes `stalagmight`; `other` is the legacy managed type.
- **The `.stalx` lands at `misc/project.stalx`** in the dump ZIP. Locate it by the
  `.stalx` **suffix**, never the hard-coded `misc/` path (survives a server
  `TARGET_FOLDER` change).
- **Upload requires the mutex** - `PUT /projects/{id}/upload/auto/` is gated by
  `UserOwnsProjectMutex`, so seed via `acquire_lock` first, then `release_lock`.

## Concurrency model - the account's lock is the user's (#193, #576 SDB-ACCT-D1)

The single-writer lock (mutex) is the only concurrency control; the server itself
is last-writer-wins. The ambiguous-push self-heal (guardrail 7, `sync::classify_ancestry`)
recovers a lost push confirmation by asking *"is the commit now on top of BASE
authored by me?"* - it identifies our own landed commits **purely** by
`author_email == my_email`.

**Rule (#576 SDB-ACCT-D1):** a write lock held by the user's **account** is the
user's, however they acquired it - in another Stalagmight session, on another
machine, or on the SpeleoDB website. How the lock was acquired is the user's
responsibility. A session that finds the account holding the lock of the project it
has open edits under it (the status poll's inherited-lock reopen, or Start editing),
and that lock is then released like one this session took - once an edit has been
made under it here (#576 SDB-2MACHINE-D1, below). Editing the same project
from two sessions of one account at the same time is the user's call; the guard
below keeps the other session's commits from being read as our own push, so they
show up as a divergence rather than being uploaded over.

**Guard (the licence, #576 B32):** the self-heal only runs for a session that
provably took the lock itself - an acquisition recorded by this process's own
Start editing, the gate open, not force-editing, and the server naming our
account as the holder (`speleodb_authorship_licence`). The status poll
(`sync::poll_verdict`) and Done / Push (`sync::plan_push`) take that same
licence, so without it a same-account commit in the gap reads as a divergence on
the button AND on the click: the conflict dialog opens instead of the copy being
uploaded over the other session's revision. The cost: after a restart in the
middle of a push whose reply was lost, your own landed upload also reads as a
conflict (the claim is in memory only); resolve it with "Take the SpeleoDB
version" or "Save my version as a local copy".

**Release (#576 SDB-ACCT-D1, SDB-QUIT-D1, SDB-2MACHINE-D1):** every release hands
back the lock the session edits under, whether this session took it or ADOPTED it
from the account (an adopted one once an edit has been made under it here, see the
two-computer rule below):
Done, close, project switch, logout, a sign-in that replaces the session, deleting
the open project, and the quit - an update restart included: it exits through the
same `release_held_lock_on_exit` (`RunEvent::ExitRequested`, or on Windows the
updater's `on_before_exit` hook, which exits the process itself), so it releases
under the same rule, once (#579). When the status poll's inherited-lock reopen or a
Start editing (plain, or the held-lock prompt's "Continue editing") opens the gate
over a lock the account already held, the session records an adopted claim for it,
and Done and the silent releases (close, switch, quit) release whenever the edit gate
is open (not force-editing), claim or not - except over an adopted claim with no edit
made here (below) - and so do logout and a replacing sign-in when the sign-out prompt
is answered "Release and sign out"; the un-pushed-edits veto still keeps a lock over
edits not on the server (Done pushes them first).

**Two computers on one account (#576 SDB-2MACHINE-D1):** computer A edits under the
lock, and on computer B the status poll sees the account's lock and adopts it. Close,
switch, quit and Done on B hand an ADOPTED lock back only once B has made an edit
under it - a survey or settings mutation accepted while editing, or an upload of the
copy's edits the server confirmed (`LockClaim::edited`, in memory; the gate's
`speleodb_lock_adopted_unused`). A lock B merely saw on the account stays held, so A
keeps it: no release request, and the claim ends with B's session, so the
stranded-claim janitor does not hand it back after a close either (it drops such a
claim unreleased, one a 401 drop stranded behind a shut gate included). Done there
finishes editing on B and answers `LockHeldElsewhere` ("Editing finished here; the
lock stays in use elsewhere."), and the status poll does not adopt the lock again
until a new open or an explicit Start editing (`speleodb_finished_here`). A lock B
took itself is released as before, and so is a gate open with no claim at all. The
sign-out prompt (below) still lists an adopted lock: one with no edit made here
apart, as in use elsewhere (`in_use_elsewhere`), kept by "Keep locks and sign out"
and released only when the user picks "Release and sign out". Deleting the open
project still releases it: the delete is a deliberate, confirmed act that destroys
the copy and its link, like unlink and take-remote. One cost: a lock this app took
before a restart reads as adopted afterwards (the claim is in memory), so a quit with
no edit made since leaves it held; the held-lock prompt at the next sign-in offers
to release it.

A revision View is refused while the gate is open over such a lock as
over one taken here (FZ-SDB-4). This replaces decision RV2-R-13, under which only a
lock this session took was released and a session whose gate the reopen had opened
left it with the account (Done's `LockHeldElsewhere` answer, which SDB-2MACHINE-D1
brings back with the poll's "finished here" guard it needs, for an adopted lock with
no edit made here only). A logout whose release the server refuses
(or does not answer) still signs out, and its answer names the project, which the
frontend warns about with how to release it (#576 Q-W61-3-D1); a sign-in that replaces
the session does the same for its hand-back: it signs in, and its answer names the
project (#576 NF-W72-1). Logout and a replacing sign-in ASK FIRST when the session
holds a lock they would hand back (#576 SIGNOUT-PROMPT-D1; `speleodb_sign_out_locks`,
`speleodb_sign_in_hand_back_locks`): "Release and sign out" releases as above, "Keep
locks and sign out" (`release_locks: false`) sends no release and marks the lock kept
as the close dialog's "Keep lock" does, and Cancel does nothing. A lock over un-pushed
edits is not offered for release - the veto keeps it whatever the answer - but it is
named, as staying held until the edits are pushed or discarded (`unpushed`, #576
SIGNOUT-PROMPT-D2); when such locks are all there is, the prompt still appears, with
only "Sign out" and Cancel. An acquire whose reply was lost and whose
holder re-read failed is marked unconfirmed in every case and settled by the next
poll (#576 SDB-POLL-D1). An adopted claim is marked as resumed: it licenses the
release only, not the authorship self-heal above (#576 RV4-R-S3), since the other
session may still be editing under the lock, and its commits must read as a
divergence there. A failed Start editing still never hands back a lock it did not
take (the unwind).

**Mitigation (the notice):** `speleodb_open_project` checks the project's
`active_mutex` at open time; if the lock is already held by **our own** account, a
concurrent same-account session is live, and the open result carries
`concurrent_session = true` (`sync::concurrent_session_notice`) so the UI warns the
user. A lock held by a *different* account is an ordinary contended-lock case, not
this notice. (Detection is verified by the `live_orch_open_flags_concurrent_*`
tests; the pure predicate by `concurrent_session_flagged_only_for_a_same_account_lock_hold`.)

## Render-compat verification (#184 - the P0.5 gate)

The premise of the whole `.tml` render sidecar is that SpeleoDB's `openspeleo_lib`
accepts **our exporter's** `.tml` and regenerates a `ProjectGeoJSON`. #184 proves
that against the live box. Three `#[ignore]` tests live in `speleodb::live_tests`
(single-account tier):

- `live_render_compat_georef_generates_geojson` - **the gate.** Builds an in-memory
  georeferenced project (a `ZeroDatum` with real lat/long), `export_tml`s it, pushes
  it via the production two-`artifact` `upload/auto`, then **reads the render back**
  and asserts features exist.
- `live_render_compat_no_anchor_records_geojson_state` - pushes an un-georeferenced
  project (0/0 START) and records that no new render is generated (the D2 path).
- `live_open1_bare_tml_mime_outcome` - resolves OPEN-1 (below).

Key server facts these depend on (checked against the SpeleoDB server source):

- **A 200 upload does NOT prove a render.** `survey_to_geojson` runs best-effort
  *after* the commit; `api/v2/views/file.py` catches `NoKnownAnchorError` /
  `EmptySurveyError` / any `Exception` and `continue`s, so the push still 200s with
  no `ProjectGeoJSON`. The render must be read back to be verified.
- **Read the render back via OGC API - Features** (no `Authorization` header - the
  token authenticates via the URL path; the `user_token` converter requires a
  **40-hex** key, which the DRF token is):

  ```
  GET /api/v2/gis-ogc/user/<token>/collections/<commit_sha>_points/items
  GET /api/v2/gis-ogc/user/<token>/collections/<commit_sha>_lines/items
  ```

  Returns a **bare** `FeatureCollection` (`{"type":"FeatureCollection","features":[…],
  "numberMatched":N,…}`, a `StreamingHttpResponse`) on 200, and **404** when no
  `ProjectGeoJSON` exists for that commit + geometry group. The collection id is
  `<sha>_(points|lines)` (`ogc_typed_id` converter). A lighter
  `GET /api/v2/projects/{id}/geojson/` (Bearer, ≥ READ_ONLY) lists the commits that
  have a render.
- **No-anchor → "may be stale", not blank.** On `NoKnownAnchorError` the server keeps
  the previous commit's `ProjectGeoJSON` row (it only skips generating a *new* one),
  which is why the D2 banner copy says the map *may be stale* rather than missing.

### OPEN-1 - is explicit per-part MIME required for the `.tml`?

The server selects the upload processor by **extension** (`processors/auto_selector.py`:
`.tml` → `ArianeTMLFileProcessor`), then validates the multipart part's `Content-Type`
against `ArianeTMLFileProcessor.ALLOWED_MIMETYPES = ["application/octet-stream",
"application/zip"]` (enforced in `processors/artifact.py::assert_valid`). **`text/plain`
is not allowed**, so a part whose Content-Type is `text/plain` (the multipart default
when none is set, or `mime_guess` on the unknown `.tml` extension) is rejected →
`ValidationError` → `ATOMIC_REQUESTS` rollback → **400**. Our production
`client::upload_auto` always sends `application/octet-stream`, so it passes. This means
the `client.rs::mime_for` comment ("explicit MIME is robustness, not a hard
requirement") is **wrong on the server contract**: an `octet-stream`/`zip` Content-Type
is genuinely required for the `.tml` part. `live_open1_bare_tml_mime_outcome` confirms
empirically what Content-Type `ureq`'s bare `Part::file()` actually sends (and thus
whether *our client specifically* could omit the explicit MIME); it prints the verdict.

### Live-run results (2026-06-27, box `http://10.10.0.220:8000/`)

Ran `live_render_compat_georef_generates_geojson`, `live_render_compat_no_anchor_records_geojson_state`,
and `live_open1_bare_tml_mime_outcome` against the box - all pass:

- **🟢 GATE GREEN.** A Stalagmight-exported **georeferenced** `.tml` regenerated a
  `ProjectGeoJSON`: the OGC `<sha>_points` layer returned HTTP 200 with **1** feature
  and `<sha>_lines` returned 200 with **2** features (the 2 shot segments). So
  `openspeleo_lib` accepts our exporter output - the P1 render/push half is de-risked;
  no exporter fix / #219 fast-track is needed to proceed.
- **No-anchor → no render.** A push of the same project with **no** zero datum (0/0
  START) still 200'd, but both `<sha>_points` and `<sha>_lines` returned **404** - i.e.
  no new `ProjectGeoJSON` for that commit (the D2 "may be stale" banner path). 0/0 IS
  treated as "no known anchor".
- **OPEN-1 resolved → explicit MIME REQUIRED.** A `.tml` part sent **without** an
  explicit `Content-Type` (bare `ureq` `Part::file()`) was rejected with **HTTP 400** -
  ureq's default part Content-Type is not in `ArianeTMLFileProcessor`'s
  `{octet-stream, zip}` allow-list. So `client::mime_for`'s explicit
  `application/octet-stream` for `.tml` is load-bearing, not defensive (the old "not a
  hard requirement" comment was wrong and has been corrected).

## Self-conflict coverage (#260)

Three `#[ignore]` orchestration tests in `core::state::speleodb`, added while
tracing the "pushing twice conflicts with yourself" report. The middle one is the
repro; the other two guard the routes found alongside it.

- `live_orch_push_then_edit_then_done_does_not_self_conflict` - the user's exact
  sequence (start editing -> edit -> push keeping the lock -> edit -> Done). It
  passed from the start, which is what **ruled the push path out** and pointed at
  the passive poll instead. Keep it: it pins that BASE advances to the topological
  tip at each push and never to the empty-repo sentinel.
- `live_orch_stale_poll_snapshot_does_not_report_our_own_push_as_conflict` - the
  actual reproduction. It hands `speleodb_remote_state` the exact link snapshot an
  in-flight poll is holding (BASE captured before a push, resolved after it) and
  asserts the verdict is not `Conflict`. It failed with "got Conflict" before the
  fix. There is no command-level lock, so that interleaving is ordinary, not
  exotic.
- `live_orch_release_current_lock_closes_the_edit_gate` - releasing your own write
  lock from the admin surface must also drop the local edit gate, or the shell
  stays editable over a lock that no longer exists. Confirmed to fail without the
  fix; the revert flow (#8) hands a lock back through this path.

The pure half is offline in `speleodb::sync::poll_verdict` (five arms), so CI
covers the policy without the box.

## Loop-UI client coverage (#198/#200/#201/#202)

The single-writer loop UI added three client methods that had no live coverage.
Each now has a `#[ignore]` single-account live test in `speleodb::live_tests`:

- `live_get_project_reflects_holder_and_tip` (**#202**) - pins the two fields the
  lazy re-verify (`speleodb_verify_lock_if_expired`) branches on: while we hold the
  mutex `client::get_project` reports `active_mutex.user == me` and carries a
  `latest_commit`; after `release_lock`, `active_mutex` goes **absent** (the
  lost-lock signal that routes to the divergence prompt). It deliberately does NOT
  assert `latest_commit.id` equals the pushed hexsha - that field is
  `max(authored_date)` and documented unreliable as a tip, which is why #202's
  tip-moved check is best-effort.
- `live_release_all_locks_releases_held` (**#201**) - `client::release_all_locks`
  (`DELETE /user/release_all_locks/`) drops **every** lock the token holds in one
  request; the test locks two throwaway projects, releases all, asserts both are
  freed, then re-calls to confirm the empty-handed no-op still 2xxes (idempotent).
  Caveat: it clears **all** locks for the configured account, so run it only against
  the throwaway box.
- `live_patch_exclude_geojson_roundtrips` (**#200**) - `client::patch_project`
  (`PATCH /projects/{id}/`, write permission, no lock) round-trips
  `exclude_geojson`: flip to `true`, confirm the PATCH response and a follow-up GET
  both report `true`, then flip back to `false` (the shipped Enable-rendering
  action) and confirm the cleared state.

### Live-run results (2026-07-02, box `http://10.10.0.220:8000/`)

Full suite green: `cargo test … speleodb::live_tests -- --ignored --test-threads=1`
ran **all 25** live tests (22 existing + the 3 above) against the box - **25 passed,
0 failed** in ~50s, with both `DEV_SPELEODB_EMAIL2`/`_PASSWORD2` set (so the
two-account tier ran too). This also confirms the loop-UI DTO additions
(`Project.exclude_geojson`, `ProjectMetadata.speleodb_project_id`) still parse real
server responses. Run single-threaded because `live_release_all_locks_releases_held`
clears account-wide locks and must not race lock-holding tests.

## F2 live-suite acceptance coverage (#212)

The env-gated live round-trip / render-compat / negative suite (F2) is the
`#[ignore]` + `DEV_SPELEODB_*` model above (the issue's `SPELEODB_TEST_URL` name
predates the adopted `DEV_SPELEODB_URL` convention; the CI/offline-green
guarantee comes from `#[ignore]`, so those runs never touch the box). Each #212
acceptance item maps to concrete tests across `speleodb::live_tests` (client) and
`core::state::speleodb` orchestration live tests:

| #212 criterion | Covering test(s) |
|----------------|------------------|
| CI/offline stay green | `#[ignore]` on every live test; a deliberate run without `DEV_SPELEODB_*` fails in `live_env()`/`orch_env()`, naming the unset variable (#576 E13) |
| Round-trip tree shape + 2nd-checkout pull matches | `live_round_trip_seed_pull_open` (seed → pull a fresh checkout → open self-contained), `live_orch_start_editing_edit_done_advances_base` (full start→edit→done loop, BASE advances), `live_orch_pushed_dump_is_self_contained_without_sidecars` (server dump = one `.stalx` located by suffix + `ariane.tml` at root + no sqlite sidecar; re-opens carrying the pushed edit) |
| Render-compat: `ProjectGeoJSON` + OGC feature counts; no-anchor → no render + D2 | `live_render_compat_georef_generates_geojson` (OGC `<sha>_points`=1 / `<sha>_lines`=2), `live_render_compat_no_anchor_records_geojson_state` (0/0 START → 404 render → D2 "may be stale") |
| Negative: 403-no-mutex | `live_upload_without_mutex_returns_403` (push while `active_mutex` is unheld → `UserOwnsProjectMutex` → 403) |
| Negative: 400-contended, locked-by via re-GET (not string parse) | `live_acquire_contended_returns_400`, `live_lock_holder_reflects_other_user`, `live_orch_start_editing_reports_locked_by_other_account` |
| Negative: stale push → fence trips, no clobber | `live_orch_update_diverges_when_dirty_and_tip_moved` (dirty + moved tip → `DivergenceNeeded`, dirty bit preserved), `live_orch_start_editing_diverges_on_unreconcilable_base` (unreconcilable BASE → divergence prompt + lock released, no overwrite) |

Note on "regenerated GeoJSON": the render is a server-side `ProjectGeoJSON`
served via the **OGC API** (`gis-ogc/.../collections/<sha>_(points\|lines)/items`),
**not** a file committed to the dump tree - so it is asserted by the render-compat
tests reading the render back, not by inspecting the ZIP. (This is the mechanism
#219/#220 aim to retire.) The self-contained-dump test locates the `.stalx` by its
suffix, never the hard-coded `misc/project.stalx` path, per the verified contract.

## Adopting a `.tml`-only project (C2, #214)

When a SpeleoDB project's dump holds **exactly one** `ariane.tml` and **no
`.stalx`**, opening it routes to the one-time adoption bootstrap (the single
sanctioned server-`.tml` read; never an ongoing reconcile). Two or more
`ariane.tml` is an *ambiguity* rather than an adoption (#258): nothing in the
archive says which file is the project, so it routes to the dump picker, as more
than one `.stalx` does. The verbs that can neither ask nor replay an answer -
Start editing, Update, Take remote - refuse with directions instead. `speleodb_open_project` surfaces the typed
`SpeleoDbAction::Adopt` (not a hard error); the browser then offers a confirm
dialog, and on confirm `speleodb_adopt_ariane` runs the whole hand-over:

1. **Lock first.** The write mutex is taken before the head resolve and the
   download, so no other writer can land a revision while the `.tml` is being
   converted - the import is the slow part, and it used to sit in an open window.
   The pre-read lock holder decides this: a mutex held by anybody, including our
   own account in another window, stops the push (never the adoption).
2. **Convert.** The `ariane.tml` is imported through the normal TML importer
   (`include_excluded=false`, `comments_on_stations=true` - the same settings a
   plain `.tml` import/export round-trip uses) and saved as a new `.stalx` at the
   pull cache path, with the `speleo_link` stamped (`base = server tip`,
   `has_unpushed_edits = true`) through the live handle.
3. **Push.** The ordinary Start-editing → Done pair uploads the `.stalx` with the
   `export_tml_for_speleodb` sidecar and releases the lock, which clears the dirty
   bit and advances `base` to the new commit.
4. **Flip the type**, ariane→stalagmight, and only once the `.stalx` has landed - a
   project typed `stalagmight` whose only file is the Ariane one that type says to
   disregard is openable by nobody.
5. **Close**, so the frontend's `loadProject` is a real load. Leaving it open made
   the backend answer that load with `ALREADY_OPEN`, which the frontend reads as
   "already on screen" - true after a re-open, false after an adopt, so adoption
   finished with the view flipped and nothing rendered.

Every step after the lock is best-effort in one direction only: a lock that cannot
be taken, or a push that does not land, still leaves the project adopted on disk
and stamped dirty. `AdoptUpload` reports which happened, and an `AdoptLockGuard`
hands the mutex back if the conversion fails before the push can use it.

Offline coverage: the typed-signal seams are unit-tested (`dto_for_error(
AdoptAriane) → Adopt`; the dump classifier's adopt arm carries the `ariane.tml`
path), the ordering above is enforced by the `adopt_order` proof types in
`speleodb_adopt.rs` (lock before read, guard spanning the conversion, flip after
push, close last): each step takes a value only the step before it returns, so a
reorder does not compile. The guard's arm/disarm behaviour is a unit test, and the FE
confirm→adopt→reload flow has a component test
(`OpenFromSpeleoDbDialog.test.tsx`). The full create→import→push orchestration
needs an app handle + a live box, so it is verified manually on the dev box:

1. Seed / pick an `ariane`-type project whose dump contains only `ariane.tml`.
2. Open-from-SpeleoDB → pick it → the confirm dialog spells out the consequences,
   including that adopting uploads and briefly takes the write lock.
3. Confirm → the survey imports, a `.stalx` lands in the cache **and on the
   server**, and the UI opens it **read-only** (remote, lock already handed back);
   the success toast says it was uploaded.
4. DB-inspect the cached `.stalx`: one `speleo_link` row, `base` = the commit the
   adoption just pushed, `has_unpushed_edits = 0`.
5. On the server the project `type` is now `stalagmight`, the repo holds both
   `ariane.tml` and `misc/project.stalx`, and a GeoJSON row exists for the new
   commit when the survey carries a GPS zero datum.
6. A later fresh pull classifies as `Stalx` (normal open), confirming the
   bootstrap is one-time.
7. Negative: with a READ_ONLY account the lock cannot be taken, so adoption still
   opens the project locally, stamped `has_unpushed_edits = 1`, the server `type`
   stays `ariane`, and the toast says it was not uploaded.
8. Negative: with the write lock held by another account, same as (7) - the
   adoption never pushes through somebody else's mutex.

## Optimistic force-edit - editing without the lock (E2, #213)

Force-edit lets an **opted-in** user edit a remote project **without** the write
lock (to survey offline, or while someone else holds it), behind a persistent
big-red "this `.stalx` cannot be merged" warning. It introduces a transient
`force_editing` flag distinct from `edit_enabled`: `edit_enabled && force_editing`
= editing WITHOUT the lock; `edit_enabled && !force_editing` = the normal lock-held
mode. Force-edit changes nothing else in the state model - edits set the existing
dirty bit, BASE never moves (no acquire), and reconnect runs the SAME acquire+fence
as Start-editing (it folds into `speleodb_start_editing`, no new verb).

The opt-in is `SpeleoDbConfig.allow_force_edit` (default false); without it the
"Edit without the write lock" affordance stays hidden and `speleodb_force_edit` refuses.

Offline coverage is the pure-state seam (no network): `speleodb_force_edit` flips
both gates only when opted-in + remote + read-only; with `force_editing` set,
`speleodb_held_lock_release_params` returns `None` (close/exit/switch never release
a non-existent lock), `speleodb_verify_lock_if_expired` is a no-op (#202 exempt),
`speleodb_done`/`speleodb_push_only` return the coded reconnect error, and
`speleodb_logout` drops both gates without a release. FE coverage: the confirm
dialog, banner + Reconnect, the settings toggle, and the menu gating each have a
Vitest test. The reconnect handshake needs a live box, so verify manually:

1. Settings → enable "Allow editing without the write lock"; open a remote project
   (opens read-only).
2. Menu (or the status button) → "Edit without the write lock" → confirm the big-red dialog →
   the persistent red banner appears and the survey becomes editable; make an edit.
3. DB-inspect the cached `.stalx`: `speleo_link.base_commit_sha` is **unchanged**
   (no acquire) and `has_unpushed_edits = 1`.
4. Confirm the #202 re-verify never fires while force-editing (no 20-min lock check).
5. Close + reopen → opens read-only again (force-edit is transient; the edits
   persist via the `.stalx` + dirty bit - re-enter force-edit to continue).
6. Reconnect (the banner action, = Start-editing) while the server is unchanged →
   UpToDate → normal lock-held editing (banner clears, Done available).
7. Reconnect while another account holds the lock → 400 → "locked by {holder}", and
   the force-edit banner persists (keep editing offline).
8. Reconnect after the server tip moved with local edits → the divergence prompt.
9. Opt-out account: no "Edit without the write lock" affordance; only read-only + the existing
   "Save a local copy" detach.

## Protocol fuzzer (#576 fuzzers)

A stateful fuzzer drives the app's SpeleoDB verbs against a **private** SpeleoDB
instance: `src-tauri/crates/core/src/core/state/speleodb_fuzz/`. Each run builds a
fresh world on the instance (an owner project shared read-write with a collaborator,
a collaborator project shared with the owner, an Ariane-only project), signs an
`AppState` in as the owner through an in-process fault proxy, and plays a seeded
sequence of the verbs the frontend calls (open with its unpushed / adopt /
ambiguous branches, start editing, edits, autosave, push, Done, update, take remote,
discard, revision View / Restore / Unwind / Rewind, the release verbs, close and
quit with their dialogs, crash, unlink, save a local copy, delete, publish, the
status poll, logout, sign-in to another deployment). Between them a second client
(the collaborator, or another session of the owner) takes and drops the lock, pushes
competing or newer-format revisions, renames or deletes the project and changes the
owner's access; the proxy injects 4xx / 5xx answers, a status after the server
applied the request, latency, a dropped upload, a lost reply, a truncated or corrupt
body, and a hang past the client timeout. After every step an oracle with superuser
eyes checks the invariants documented in `speleodb_fuzz/oracle.rs` (I1 no silent
loss of an un-pushed edit ... I11 editing means holding the lock). Each run writes a
JSONL trace; a failing run is delta-debugged to a short step list.

It refuses to start unless every condition below holds, before it sends anything:

- `STALAGMIGHT_SPELEODB_FUZZ_URL` is `http://<loopback IP literal>:<port>/` (no
  name is resolved, so no public SpeleoDB can be reached);
- every account email is under the reserved `.invalid` domain;
- `DEV_SPELEODB_URL` is unset (the app would prefer it over its own endpoint).

| Env var | Meaning |
|---------|---------|
| `STALAGMIGHT_SPELEODB_FUZZ_URL` | The instance base URL (loopback only). |
| `STALAGMIGHT_SPELEODB_FUZZ_EMAIL` / `_PASSWORD` | The app's account (a normal user). |
| `STALAGMIGHT_SPELEODB_FUZZ_EMAIL2` / `_PASSWORD2` | The second client (a normal user). |
| `STALAGMIGHT_SPELEODB_FUZZ_ADMIN_EMAIL` / `_ADMIN_PASSWORD` | A superuser: the oracle's view of locks, history and every commit's content. |
| `STALAGMIGHT_SPELEODB_FUZZ_SEEDS` | Seed range `a..b` (default `1..4`). |
| `STALAGMIGHT_SPELEODB_FUZZ_STEPS` | Steps per seed (default 40). |
| `STALAGMIGHT_SPELEODB_FUZZ_FAULT_PCT` | Percent of app steps with a fault (default 25). |
| `STALAGMIGHT_SPELEODB_FUZZ_MINUTES` | Stop starting new seeds after this long (default: no limit). |
| `STALAGMIGHT_SPELEODB_FUZZ_SHRINK` / `_SHRINK_BUDGET` | Shrink failures (default on) / at most this many replays (40). |
| `STALAGMIGHT_SPELEODB_FUZZ_DIR` | The app's folders for the run (default: a temp dir per process). |
| `STALAGMIGHT_SPELEODB_FUZZ_TRACE_DIR` | Where traces and step lists go (default: a private temp dir per process, kept after the run). |
| `STALAGMIGHT_SPELEODB_FUZZ_REPLAY` | A saved step list for the replay entry point. |

```bash
cargo test --manifest-path src-tauri/crates/core/Cargo.toml --lib \
  core::state::speleodb_fuzz::speleodb_fuzz_campaign -- --exact --ignored --nocapture
```

Run it alone (`--exact`): it sets the process-wide app folders. Parallel workers
need one account pair each, since `release_all_locks` is account-wide. The replay
entry point is `speleodb_fuzz_replay`; `speleodb_fuzz_regression_seeds` re-runs
every seed that once failed (`REGRESSION_SEEDS` in `speleodb_fuzz/mod.rs`; each
is a fixed bug or a since-fixed oracle false positive, so all of them pass; a seed
that pins a still-open question goes in with a comment and fails until it is
decided), then the directed step lists beside them (`directed_steps`): shapes the
generator cannot draw because it arms one fault per step, such as Start Editing's
acquire losing its reply while every attempt of the holder re-read after it fails
(#576 SDB-POLL-D1). A directed step arms its further faults (`then`) after its
`fault`, in order; the first armed rule that matches a request fires. Every load in
a run ends in the status poll, as the frontend's
`loadProject` does. Normal
`cargo test` runs only the offline unit tests (the allowlist, generator, proxy,
stub and oracle); the three entry points are
`#[ignore]` and fail naming the missing variable when run without an instance.

The instance itself (deployment, accounts, reset to a clean snapshot) is tooling
on the maintainer's harness host and is not part of this repo.
