# SpeleoDB workspace adversarial review

Static review of the workspace redesign from `68437dafb` through `9343bcf96`,
followed by review of the corrective changes. The review-agent skill was used.

## Findings and corrections

| Severity | Source | Concrete failure | Smallest correction applied |
| --- | --- | --- | --- |
| P1 | `speleodb.rs`, `speleodb_session.rs` | A status probe could pair an old link with a new generation or close a newly acquired editing gate. An inherited lock could reopen editing without a current write grant. | Capture link and generation together, serialize the short ownership probe with lock operations, close gates under the project mutex, and require a fresh permission and holder check before reopening. |
| P1 | `speleodb_write_guard.rs`, `speleodb_project.rs` | Metadata verification and PATCH could use different account credentials. A failed permission probe could leave editing enabled; a delayed name fold could rename a reopened project. | Retain captured credentials and lock-operation ownership through PATCH, reject session changes, close the current gate on confirmed denial, and scope name folds to the original generation. |
| P1 | `speleodb_push.rs`, `project_io.rs` | An upload reply could advance a replacement project's BASE, clear its dirty flag or undo history, or close its editing gate. A save could install its reopened database after a replacement project had taken the slot. | Carry the original generation through upload, save and snapshot; check it under the project mutex for local bookkeeping and gate changes. Publish the saved database while still holding the project mutex. |
| P1 | `speleodb_credentials.rs`, `speleoStore.ts` | A delayed login could resurrect credentials after logout; replacing an identity could retain the previous remote editing gate. | Refuse authentication publication after a session boundary, fence frontend auth completions, and require fresh remote ownership after login. |
| P1 | `uploadContinuation.ts`, `SpeleoDoneDialog.tsx` | A saved description and Save-and-Close intent could target a reopened project or different account. The dialog could bind to whichever project was current at submission. | Bind drafts to the observed project lifetime and account, capture ownership at dialog mount, and invalidate stale continuations. Same-account recovery retains the draft. |
| P1 | `useSpeleoActions.ts`, `SpeleoDoneDialog.tsx`, `SpeleoUnwoundPushDialog.tsx`, `quit.ts` | A late release reply could clear newer edits and let Save-and-Close close prematurely. | Fence replies by project operation, account and tree generation; retain newer work and recheck cleanliness after draft confirmation immediately before close. |
| P1 | `protectAccountTransition.ts`, `projectStore.ts`, `SpeleoWorkspaceAccount.tsx` | Same-account recovery could discard pending survey changes; account transition could reopen an old project after a newer open; New Project bypassed form protection. A session-key remount discarded connection drafts. | Preserve known same-account recovery, carry one transition request through close/reload, add draft protection to creation, scope confirmation callbacks, and preserve dirty forms while refreshing clean forms. |
| P2 | `speleoStore.ts`, `SpeleoDbSettings.tsx` | A successful login from one mounted account form could erase credentials or connection edits entered in another form during the request. | Clear only the submitted credentials and avoid overwriting a newer connection draft. |
| P2 | `OpenFromSpeleoDbDialog.tsx` | The persistent browser stayed busy after an unuploaded-work refusal or successful same-path open. A failed account refresh could retain another account's project list. | Release busy state in the owning request's finally block and reset cached results on account changes; ignore stale responses. |
| P2 | `usePanelReveal.ts` | A pending hover timeout could hide the dock after focus entered a form. | Cancel the timeout on focus and consult current focus when it fires. |
| P2 | `SpeleoBanner.tsx`, `dispatchProjectSave.ts` | Retry could bypass Save confirmation or lose Save-and-Close intent. A clean retry could stop at the no-changes toast after an upload succeeded but release failed. | Return to confirmation with the retained description and valid close intent; retain explicit lock-release retries and honor pending close intent before the no-changes shortcut. |

## Corrective plan and static assessment

The corrective plan was reviewed before implementation: establish backend session
and credential ownership first, fence upload and close continuations, then repair
persistent forms, browser state, retries and focus. The resulting diffs were read
again for stale callbacks, lock ordering and unintended project retargeting.

Rust sources were formatted and `git diff --check` was used for whitespace review.
No tests, builds, typechecks, lint gates or application sessions were
executed for these corrections. Existing test call sites and a synthetic response
were adjusted for the tightened internal contracts; no tests were added. Runtime
behavior and compatibility with the existing suites remain unverified for this
review's changes. Earlier acceptance results do not validate these corrections.

## Follow-up static validation

The user subsequently requested Clippy, linting and typechecking without tests.
The two denied unnecessary qualifications in the status probe were replaced with
the existing `normalize_endpoint` import, and the affected Rust source was formatted.

Passed: workspace Clippy with telemetry and all targets; main, agent and Playwright
typechecks; all nine lint gates without their test scripts; Rust formatting for the
root package and every workspace member; and whitespace review. Clippy retains
existing advisory warnings. No tests or application sessions were run.
