# SpeleoDB project workspace

The project editor has one left dock. Linked projects expose Sections and
SpeleoDB tabs in Survey; Plan and Export expose SpeleoDB on request. Opening a
project selects Sections. Tab navigation never changes lock ownership.

## Action inventory and destinations

| Existing action | Destination |
| --- | --- |
| Status icon and its menu | Logo + SpeleoDB toolbar button, workspace |
| Upload, release/resume, refresh, web link | Persistent everyday controls |
| Revision history, inspect, restore, revert, unwind/rewind | History accordion |
| Remote metadata, rendering, undo policy, lock recovery | Project settings accordion |
| Open remote project, refresh/search, create project | Projects accordion |
| Sign in/out, server and connection preferences | Account & connection accordion |
| Remote actions in hamburger, Project Details, undo menu | Workspace navigation |
| Home sign-in, browsing, creation, local publishing | Retained on Home |
| Explicit Save and keyboard Save | Shared local-save/remote-upload dispatcher |
| Autosave and autosave retry | Local save only |

## State matrix

| State | Navigation | Local mutations | Upload |
| --- | --- | --- | --- |
| Local project | Sections in Survey | Survey only | Local Save |
| Linked, acquired lock | All modes | Survey only | Verify ownership, require description |
| Linked, read-only or another editor | All modes | Blocked | Explain missing lock |
| Open offline | All modes | Blocked | Connection error |
| Transient offline after acquisition | All modes | Survey only | Connection error |
| Signed out, permission lost, confirmed lock loss | All modes | Blocked immediately | Recovery first, confirm afterward |
| Busy | Navigation available | Operation gate applies | Disabled during operation |
| Already uploaded | All modes | According to lock | No changes to upload |

Save and Close closes only after a confirmed upload and successful release, with
no newer changes. Cancel, conflict, uncertain response and release failure leave
the project open. Ordinary upload retains the lock. Plain Close, switch and Quit
retain automatic release and Discard changes / Cancel protection.

## Sidebar wireframe

```text
+--------------------------------+
| Sections | SpeleoDB             |
| Project name                   |
| Editing/read-only, owner, online|
| Upload changes                 |
| Release lock / Resume editing  |
| Get latest | Open on SpeleoDB   |
| > History                      |
| > Project settings             |
| > Projects                     |
| > Account & connection         |
+--------------------------------+
```

One accordion opens at a time. Collapsing or hiding content preserves its drafts
and scroll position. Project/account replacement protects drafts independently
of validity. Requests are bound to project identity and session generation.
Programmatic navigation focuses the selected tab without changing the saved
pin/hover preference. SpeleoDB needs 320px within the existing 400px maximum;
constrained layouts use the existing overlay dock.

## Verification record

Completed on 2026-11-09. No push, PR, version tag, project-file format change or
fixture migration was performed. All test data introduced for this work is synthetic.

| Check | Result |
| --- | --- |
| Main, agent and Playwright TypeScript checks | Passed |
| IPC mock/handler contract | 7 passed; removed command absent |
| Frontend unit/component suite | 5,684 passed across 408 files |
| Targeted headless Chromium cases | 48 unique passed (28 baseline cases plus 26 additional, with 6 overlapping) |
| Complete Rust workspace with telemetry | 3,593 unit/doc tests passed; 41 intentionally ignored |
| Vendored compass_data suite | Passed, including doctests |
| Root/member rustfmt and workspace all-target Clippy | Passed; existing advisory Clippy warnings remain |
| Read-only, test-ID, Unicode-dash gates and their fixtures | Passed |
| Parallel-safety, timing-reporter, file-size and private-leak gates | Passed; existing file-size baseline warnings remain |

Browser coverage includes automatic acquisition/contention, tab and accordion draft
retention, keyboard focus, all modes, fullscreen-table exit, narrow toolbar/resize,
upload retaining the lock, blocked upload, invalid-draft close/switch/server-change
cancellation, account/browser flows, and existing close protection.

The native Tauri walkthrough used an isolated profile and two controlled projects
on the configured test server. It verified Home sign-in and creation, Sections-first
navigation, missing-lock recovery without automatic upload, invalid metadata draft
retention, ordinary upload with a new server revision and retained lock, and Save
and Close with a new server revision, released lock and closed project. It found a
publication relocation/automatic-acquisition bug; the fix passed a fresh native
creation/publish walkthrough. Both controlled projects were deleted and their
absence verified. The user's existing projects, locks and running dev app were
preserved. Test app, bridge and driver processes were stopped.

Light/dark baseline layouts were inspected. Native capture uses synthetic WebKit
input and DOM screenshots, with equivalent inline CSS for loaded stylesheet rules;
WebGL and external PNG masks are omitted. Hidden macOS WebKit suspended resize/frame
callbacks during the requested size/zoom matrix. Two unexplained ordinary native
app exits prevented final native mode/fullscreen/hover/trusted-shortcut checks.
Those cases pass in Chromium; full native responsive and trusted-input acceptance
remains limited. Live destructive offline/auth-loss/conflict/failure variants were
not exercised; deterministic backend and frontend tests cover them.

Local, ignored evidence is under
`.agent-artifacts/reports/speleodb-workspace-20261108/`: `report.md`,
`screenshots/INDEX.md` (14 inspected captures), `size-matrix.json` and `cleanup.json`.
The report distinguishes completed workflows from harness-limited attempts.
