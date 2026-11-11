# SpeleoDB review and fixes - handoff (2026-11-09)

## Status (integrated, 2026-11-10)

- The fix branches are integrated on `speleodb/review-fixes`. The owner's
  decisions 1-14 and 16-18 under "To implement" are implemented there (6 and 11
  in the form the #668 reconciliation below gives them). Decision 15 is #670.
- PR #668 (SpeleoDB UX) landed on master first. This branch was reconciled with
  it per four owner decisions:
  1. The insecure-HTTP option exists only in development builds. Release builds
     use #668's automatic rule: HTTPS, with plain HTTP only to `localhost`,
     `127.0.0.1` and `192.168.x.x` as written (no DNS lookup).
  2. Settings uses #668's Connect / Disconnect. Disconnect runs the sign-out flow
     first (un-uploaded work, lock hand-back).
  3. A close, project switch or app-driven quit whose lock release fails or is
     uncertain no longer closes silently. An in-app "Couldn't hand back the lock"
     dialog offers Retry / Close and keep the lock / Cancel.
  4. The SpeleoDB workspace keeps #668's actions and "Revision History" naming,
     but its buttons are the app's ordinary buttons (standard size and variants,
     one primary per state). The colour-by-action variants were tried and
     rejected by the owner, so they are gone, along with the yellow `highlight`
     variant: Home's "Open project" is a standard primary button.
- After the owner's UI review: every SpeleoDB dialog keeps all of its buttons in
  the footer row, like the rest of the app; un-uploaded changes are shown by a
  yellow tab icon and label only (no "!" badge); the in-project menu no longer
  has a SpeleoDB row; Settings uses the standard reset icon.
- The pre-PR review findings are fixed: the owner checks on the fast-forward open
  and on purging, removing and discarding cached copies; Quit now; the download
  deadline and its ceiling; the redaction of server values in error detail; the
  quit gate counting every command the backend exit waits for; the backend guard
  on restart-to-update.
- The yellow `highlight` variant and the "Yellow is overloaded" follow-up below
  are therefore obsolete.
- The fuzzer-found bugs F1-F3 are in #669. The remaining follow-ups and the
  undecided items are in #671. The upstream SpeleoDB reports are drafted
  separately.
- `docs/SpeleoDB.md` describes the resulting behaviour.

The rest of this document is the original hand-off, kept as the design record.

## Original hand-off

Pick-up notes for the follow-up work on the SpeleoDB integration that landed in
master `0977878a2` (external PRs #633 SDK migration, #663 live-test timeout,
#635 workspace UX). Everything below is on separate, unmerged branches. None of
them has had `/pre-pr` run yet, and none has been merged together.

The full adversarially verified review (49 findings with file:line, failure
scenario, proof and fix direction) is in
[2026-11-09_speleodb-review-findings.md](2026-11-09_speleodb-review-findings.md).
Finding ids below (e.g. "lock#1") refer to that file.

## Branches

All fix branches start from `eb399b266` = master `0977878a2` + one commit.

| Branch | Tip | State | Content |
|---|---|---|---|
| `feat/speleodb-home-button-swap` | `eb399b266` | done | Home screen: the SpeleoDB button sits in the Refresh slot while signed out; Refresh replaces it once signed in. Header SpeleoDB button removed. |
| `feat/speleodb-ui-polish` | `d2114372e` | done, gates green | 16 UI items (see below). |
| `feat/speleodb-exit-prompt` | `b163ba7b1` | done; full vitest re-run pending | Exit / logout / restart prompts and the lock-release invariant. |
| `fix/speleodb-install-guards` | `500e23fe0` | done, gates green | Stale SQLite sidecars on cache install; revision View/Restore race. |
| `feat/speleodb-nonblocking` | `34e185488` | done, gates green | Main thread never waits on SpeleoDB network I/O; busy spinner on the tab icon. |
| `fuzz/sdb-sdk` | see branch | WIP | Protocol fuzzer port to the SDK client (test-only). See "Fuzzer" below. |
| `speleodb/review-handoff` | this doc | - | This handoff and the review findings. |

### Suggested merge order and known conflicts

Merge into one integration branch in this order: ui-polish, exit-prompt,
install-guards, nonblocking. Then run the full pre-push checklist from CLAUDE.md
and `/pre-pr`.

- `src/components/SingleSystemView/LeftWorkspace.tsx`: ui-polish (tab icons and
  badges) vs nonblocking (busy spinner). Resolution: wrap the tab icon, i.e.
  `<SpeleoNetworkBusyIndicator><SpeleoTabIcon .../></SpeleoNetworkBusyIndicator>`.
  The spinner overlays the icon's top-left so it adds no width at the 230 px
  minimum; the badges sit bottom-right.
- `src/i18n/locales/en.json`: ui-polish and exit-prompt both add strings, in
  separate sub-objects (`map.readOnlyPill`, `speleodb.workspaceTab`,
  `speleodb.reloadConfirm`, `speleodb.lockState`, the exit/sign-out prompts).
  The conflicts should be textual only.
- `speleodb_pull.rs`: exit-prompt adds a 9-line base==tip hook in
  `speleodb_pull_project`; install-guards changes the install paths further
  down. Keep both.
- `close_project` / exit release: exit-prompt made `close_project` async;
  nonblocking deliberately left close/exit to it. After merging, confirm that
  no SpeleoDB network call runs on the main thread.

## What was fixed

### Exit flow (`feat/speleodb-exit-prompt`)
- Fixes frontend#1 / removed#1 (P1): saved-but-unuploaded work was stranded
  after close/quit.
- On close, project switch (file open, New Project, open from SpeleoDB) and quit
  (menu, Ctrl+Q, window X): when this account holds the lock and there are
  changes not yet uploaded, ONE in-app prompt offers:
  - Upload;
  - Keep the lock;
  - Discard, followed by a second "lost forever" confirmation;
  - Cancel.

  A clean project releases the lock silently. A read-only project gets the
  normal unsaved-changes dialog.
- Backend invariant: lifecycle release (close, switch, quit, restart, logout)
  never releases the lock while `has_unpushed_edits` is set, unless the caller
  carries the user's explicit discard. `close_project` is async and takes
  `discard_local`; the keep-lock close path is repaired.
- Logout and account switch: one dialog lists
  - the open project's changes (Upload / Discard);
  - other cached projects with changes (Open to upload / Discard);
  - the locks to hand back.

  There is no keep-lock option on sign-out (that is what strands locks).
  Releases are bounded; on failure the user gets Try again / Sign out anyway.
  Logout no longer forgets recents whose copy has un-uploaded changes.
  This also fixes lock#5 (account switch stranded the old account's lock).
- Restart-to-update (lock#3): no prompt. A clean project releases the lock
  (bounded) before install. With changes, it saves, closes keeping the lock and
  restarts; the next open resumes editing.
- Open when the local base equals the server tip: the local copy opens directly
  (no download, no prompt). The refusal dialog for a real divergence also offers
  a non-destructive "Open my local copy".
- Global shortcuts no longer fire behind overlay dialogs.

### Data integrity (`fix/speleodb-install-guards`)
- data#1 (P1): new `speleodb_sidecars.rs`. All three cache renames
  (`speleodb_cache_install.rs`, `speleodb_cache.rs`, `speleodb_reload.rs`) move
  `-journal`/`-wal`/`-shm` aside, fsync the directory, rename, then delete; on a
  failed install the sidecars are restored. Under Protect, a hot journal counts
  as unknown local state (refuse, leave untouched). Under Discard, the journal is
  removed.
- data#2 (P1): new `speleodb_revision_swap.rs`. Revision View/Restore snapshots
  the generation, link and in-memory project at entry, then re-checks all three
  inside one atomic reload. A late reply returns `SessionMoved` and never closes
  another project. On the frontend a stale reply ends quietly.
- Deliberate deviation: a snapshot rather than a download-generation
  reservation, because reserving would abort an in-flight Start Editing or
  upload.

### Non-blocking network (`feat/speleodb-nonblocking`)
- Fixes lock#2 / sdk#5 (P1 freezes).
- Config writes take `speleodb_lock_ops` only on an instance change.
- `update_config`, `reload_config` and `delete_project` run off the main thread.
- Switch to Read Only drops the command and project locks before its POSTs.
- The 20-minute lock re-verify runs in the background.
- Conflicting SpeleoDB controls are disabled while an operation runs.
- A spinner overlays the SpeleoDB tab icon.
- A throttled-upload check showed config writes at <= 4 ms on the branch, vs 26 s on base.

### UI polish (`feat/speleodb-ui-polish`)
- **Workspace and toolbar**
  - Toolbar SpeleoDB button removed. A "SpeleoDB" row was added to the
    in-project menu, because Plan, Export and the fullscreen table had no other
    entry point.
  - Tab icons: Sections, and the SpeleoDB logo. The SpeleoDB icon gets a lock
    badge when read-only, and a yellow "!" plus warning styling when there are
    un-uploaded changes.
  - Minimum left-panel width is 230 px for both tabs, and switching tabs no
    longer resizes the panel.
  - Icons on all SpeleoDB-tab buttons.
  - The read-only banner is removed.
- **Read-only map pill:** "Resume editing" for SpeleoDB (re-acquire) and for the
  Closures Computed View (back to Raw Data View).
- **Home screen**
  - The refresh spinner shows only on the Refresh button.
  - Plain-language lock copy: "Ready for editing", "Currently being edited by
    X", "You are editing".
  - The Open button is removed from SpeleoDB cards and table rows (the row or
    card opens on click).
  - New yellow `highlight` Button variant on the local "Open project" button,
    WCAG AA in all themes.
- **Menus and dialogs**
  - Debug Logs is in the Debug submenu.
  - The Reload confirmation is an in-app dialog.
  - The startup held-locks dialog says "Not now" instead of "Keep working".

## Owner decisions on what is still open (NOT implemented)

### Accepted, no action
- sdk#1 / removed#2: projects stored as `misc/project.stalx`. Only dev builds
  produced that layout.
- lock#1 / removed#3: same account in two sessions. Accepted for now.
- Credential leftovers (old keychain token, legacy plaintext token file). That
  version was effectively unused.

### To implement
1. **Lifecycle release holder check (lock#4).** Send the release request only
   when the server says the lock is held by this account. Never release someone
   else's lock, even as an admin.
2. **Edit mode married to the lock (data#3, Take remote reopens editable).**
   Make "editing without a held lock" structurally impossible: one source of
   truth, where the edit gate is derived from the held lock of this account and
   session. Do not patch Take remote alone.
3. **Quit during an in-flight upload or acquire (lock#6).** Let in-flight
   network actions finish, so slow connections do not lose work. Quit waits with
   visible progress, then exits.
4. **Ambiguous acquire failure (sdk#4).** On a transport, 5xx or parse failure,
   re-read the lock holder. If it is us, adopt the claim (or release it). Remove
   the false "server lock times out" comments: the server has no lock TTL.
5. **Cached SpeleoDB projects on Home (frontend#5).**
   - When logged in, show only the signed-in account's cached SpeleoDB
     projects.
   - When logged out, show all of them.
   - Right-click on a SpeleoDB card or row opens the proper context menu,
     including removing the local cached copy.
   - SpeleoDB settings get "Purge all cached files". It must keep, or ask about,
     copies with un-uploaded edits.
   - Projects the user cannot edit show clear prose both in recents and in the
     opened project, e.g. "Read-only copy - log in to edit".
6. **http endpoints (sdk#6).** `http://` local/private endpoints must behave
   exactly like `https://` endpoints, including the offline cache fallback. A
   DNS failure must not become an `InsecureEndpoint` fatal.
7. **Upload result after the 120 s notice (frontend#2).** Apply the outcome
   (link refresh, success, failure banner) whether or not the dialog is still
   open, and toast the user.
8. **Escape during open (frontend#3).** Block closing the Open-from-SpeleoDB
   browser while an open is in flight, as `SpeleoUnpushedLocalDialog` does.
9. **Window X / OS quit during the Saving/Publishing overlay (frontend#4).**
   Guard it. Toast that a save or upload is in progress and that the app will
   close when it finishes.
10. **Ctrl+S when a push is impossible (frontend#6).**
    - If the lock can be acquired, acquire it and push.
    - If pushing is not possible, save to the local cache.
    - Never leave edits that are only discardable.
11. **"Log in now" in Settings (security#4).**
    - Log in with the values on screen, including insecure-http.
    - Apply the SpeleoDB settings automatically on success.
    - Do not close Settings when settings outside the SpeleoDB tab were
      modified, so they are not discarded.
12. **Upload size (sdk#3).**
    - Check the file size before uploading.
    - If it exceeds the server limit, drop the undo history from the uploaded
      copy. This is acceptable for huge projects; the existing undo policy
      already has an upload-without-history path.
    - If it is still too large, tell the user.
    - The server limit is currently 24,657,920 bytes, caused by a 1204 typo in
      a server setting. Report that upstream.
13. **Commit messages over 1024 chars (sdk#2).** Reproduce on a private test
    instance and report upstream, as described in "Upstream SpeleoDB reports"
    below. Cap at 1000 characters client-side for now, measured after the
    server's newline normalisation.
14. **Revision timestamps (sdk#7).** Show local time and state the GMT offset.
    The server returns New York wall time with no zone. Also report upstream.
15. **`speleodb_set_undo_mode` (removed#6, frontend#10).** Non-critical. Open an
    issue: the store action calls a retired IPC command, and
    `speleodb_get_project_metadata` has no caller.
16. **Workspace-draft guards (frontend#10).** Unify them with the new
    exit/logout/switch prompts. All of these flows must use in-app dialogs only,
    never native/system dialogs (`workspaceDrafts.ts`,
    `protectAccountTransition.ts` and other leftovers).
17. **"Web rendering disabled" banner (frontend#7).** Remove it. It is expected
    to be retired from SpeleoDB.
18. **Error and metrics cleanup.** Fix the `handBackProject` "[object Object]"
    error (frontend#8) and the metrics (sdk#8: Parse counted as Ok, api_error
    drops the SDK parse detail, Tls never produced).

### Not yet decided
- data#4: a publish retry can duplicate the server project (plausible).
- data#5: the fast-forward check ignores link-only dirtying during a download
  (plausible).
- The remaining P3s in the findings file: doc/comment drift, the
  SystemTable copy per row, e2e mock drift, and the CI live-test secret
  exposure / leaked locked project on cancel.

### Follow-ups found during the fixes
- **Un-uploaded marker hidden:** the "!" is not visible while the left panel is
  closed, because the dock bar has no badge.
- **Yellow is overloaded:** it means both "main action" (home) and
  "un-uploaded" (tab).
- **Sidecar replay on Save As:** saving over an existing file with a stale hot
  journal may hit the same sidecar replay (outside install-guards' scope).
- **Dead install path:** `install_linked_download_with_limits` without a
  generation, and `replace_cache_stalx_with`, have no production caller. Delete
  them, or put them under one lock.
- **WAL-mode downloads:** these are accepted; forcing the journal mode would be
  extra hardening.
- **Stale copy:** the en.json keys `speleodb.lock.*` / `speleodb.button.*` look
  unused, and many comments still mention "the status button".
- **UI not run in the real app:** the 230 px minimum includes about 6 px
  estimated headroom for Mac/Windows fonts, and the WebKit UI zoom was not
  exercised.
- **Live API tests:** they went from about 40 to 1
  (`live_used_api_calls_with_email_and_token`). Consider restoring lock,
  permission and two-account coverage.

## Upstream SpeleoDB reports to file
- Upload limit: about 24.6 MB because of a `1204` typo in a size setting.
- `ProjectCommit.message` is `CharField(max_length=1024)`, while git accepts
  longer messages. To reproduce on a private instance:
  1. Create a throwaway project and acquire the lock.
  2. `PUT /api/v2/projects/<id>/upload/stalagmight/` with a ~1100-char
     `message`. Expect HTTP 500: the git push lands, then the DB insert raises
     `DataError`, and `ATOMIC_REQUESTS` rolls back only the DB.
  3. Upload again with a short message. Expect 500 again, because the history
     rebuild hits the same row.
  4. Delete the project.

  Report it with the server log of step 2.
- Revision `authored_date` is New York wall time without a zone (e.g. "in 4
  hours" for a just-made commit in CET).

## How to test
- Pre-push checklist: CLAUDE.md. Rust must run with `--workspace --features
  telemetry` and `TELEMETRY_ENDPOINT` set.
- On Node 25+, vitest needs `NODE_OPTIONS=--no-experimental-webstorage`. Node
  26's built-in `localStorage` hides jsdom's and breaks 8 unrelated tests. The
  repo pins Node 24.
- Run Playwright on a private port when a dev server is up:
  `PW_PORT=1450 pnpm exec playwright test ...`. `reuseExistingServer` otherwise
  attaches to whatever holds 1420.
- `scripts/dev.sh` runs `pkill -f "stalagmight"`, which also kills any process
  whose path contains `stalagmight` (e.g. a review worktree under
  `~/.cache/stalagmight-*`).
- The only live API test now is
  `speleodb::client::tests::live_used_api_calls_with_email_and_token`. Set
  `SPELEODB_API_TEST_ENABLED=true` and `SPELEODB_INSTANCE_URL` /
  `SPELEODB_OAUTH_TOKEN` / `SPELEODB_EMAIL` / `SPELEODB_PASSWORD`. Live
  orchestration tests added on the exit-prompt branch are `#[ignore]` and
  env-gated.

## Fuzzer
`fuzz/sdb-sdk` ports the SpeleoDB protocol fuzzer to the SDK client
(test-only code). It runs only against a private SpeleoDB instance, never
against speleodb.org. Its state at hand-off is in the branch's latest commit
message. Findings it reproduced map onto the review ids above.

Two NEW bugs found by the fuzzing track, both confirmed by probe tests, not in
the review and not yet fixed:

- **F1 (regression): opening offline never falls back to the local copy when
  the server is down.**
  - Cause: `client.rs:238-245` classifies every SDK send failure that is not a
    timeout as an unknown error, including connection refused, reset before
    reply, and DNS failure. `offline_open_result` (`speleodb_pull.rs:52-60`)
    only accepts refused, timeout or DNS, so only timeouts fall back.
  - Before the merge, Home opened a downloaded project's file directly.
  - Fix direction: in `api_error`, unwrap the middleware error to its source
    and reuse the existing `network_error` classification.
  - Test it through the SDK client. Today's DNS test calls `network_error`
    directly and bypasses the SDK.
  - Related to decision 6 (http endpoints).
- **F2: closing with "Discard changes" leaves the copy flagged as
  un-uploaded.**
  - Every edit writes `has_unpushed_edits=1` to the file immediately
    (`app_state.rs:404-423`).
  - Close with discard saves nothing and never resets the flag, although the
    content still equals the server version. The next online open refuses with
    the un-uploaded dialog.
  - Reload-with-discard and `speleodb_discard_changes` do clear it.
  - First check whether `feat/speleodb-exit-prompt` already covers this: its
    discard path removes the cached copy. Also cover the plain unsaved-changes
    Discard on a read-only or clean-lock project.
  - Fix direction: persist the flag together with the content, or restore it
    from the last save when closing without saving.
