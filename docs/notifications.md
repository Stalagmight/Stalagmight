# Notification policy

Which surface an event gets - toast, banner, dialog, or log - and the rules each
surface carries. This is normative: new code follows it, and review holds changes to
it. It synthesizes the Carbon notification pattern, Apple's HIG interruption levels,
and Material's notification content guidance, applied to this codebase's surfaces.

Some existing code predates this policy; the cleanup pass was tracked in issue
#415 (closed). Do not copy an existing callsite's behavior without checking it
against this page first.

## Adding a notification? Answer these first

1. Is it an ongoing condition, or a one-time event? Ongoing -> banner/indicator,
   never a repeating toast.
2. Must the user acknowledge or decide before continuing, or is data at risk?
   Yes -> modal dialog. No -> keep going.
3. Is the outcome already visible where the user is looking? Yes -> nothing.
   Direct feedback beats a notification.
4. Is it recoverable, self-healing, or internal? Yes -> log only, no user surface.
5. Otherwise it is a toast: pick the severity honestly (see table), register a
   dedup key, and localize the string.
6. One event, one surface. If a dialog or banner already reports it, do not also
   toast it.

## Principles

- **Notify only when it changes what the user knows or does next.** Confirmation of
  an action whose result is visibly on screen is noise (Apple, Material).
- **Disruption proportional to urgency.** The ladder, most to least disruptive:
  modal dialog > banner > toast > status indicator > log. Pick the lowest rung that
  still does the job (Carbon's high/low-contrast rule, Apple's interruption levels).
- **No silent state changes.** Anything that mutates, fails to mutate, or blocks a
  mutation of user data must reach a surface. "Silent state" is a named root cause
  in the 2026-08 UX field report.
- **Never notify for**: self-healing or retried errors, operations needing no user
  involvement, progress the UI already shows, or anything promotional.

## The four surfaces

| Surface | Use for | Persistence | Examples in the codebase |
|---|---|---|---|
| Modal dialog | User must acknowledge/decide before continuing; data at risk | Until dismissed; app inert behind it | `ErrorDialog`, `ImportReceiptDialog` (only when the receipt has a warning, is empty, converted units, or created a zero datum, #411), corruption recovery |
| Banner / indicator | Ongoing conditions | Until the condition clears | `SpeleoBanner`, `OfflineIndicator`, read-only badges and the adjustment notice (`MapTopLeftOverlay`; #737: loop closures the Closures Computed View skips because they span a single shot, and parts of the survey it shows measured, with a Show warnings action), `UpdateBanner` (#579, #631: an update is available, downloading, or ready. A background download (from a launch, periodic or track-switch check) shows no banner until it is ready; one started from About or the banner's Download shows progress. A failed check shows nothing and goes quietly to idle, except that About's manual check says so inline in the dialog. A failed Download from the banner, a failed Restart, and a restart the backend refuses because a SpeleoDB transfer started are toasts; a failed background download just drops back to the banner's Download, and a failed install on quit is only logged) |
| Toast | Transient outcome of an action whose result is off-screen or non-obvious | Auto-dismiss (5-9 s by kind) | save confirmations, blocked-edit notice, export results, warning-free non-converted import receipt (#411) |
| Log only | Internal, recoverable, debug | n/a | `console.warn`/`console.error` |

### Decision tree

```
ongoing condition? ──yes──> banner / persistent indicator
   │no
must the user act or decide before continuing,
or is data at risk? ──yes──> modal dialog
   │no
outcome already visible where the user is looking? ──yes──> nothing
   │no
user-relevant at all? ──no──> log only
   │yes
toast (explicit kind, registered key, localized)
```

## Toast rules

The one toast system is `showToast(message, key, kind)` on the UI store
(`src/store/uiStore.ts`), rendered by `src/components/common/ToastStack.tsx`
(bottom-left stack, capacity-managed, hover pauses the stack, sr-only live
regions). Read the comment blocks in those two files plus
`src/utils/toastCapacity.ts` for mechanics; this section is about usage.

- **`kind` is mandatory.** Never rely on the `info` default. Severity mapping:
  - `error` - the action failed and will not self-heal.
  - `warning` - the action succeeded with caveats, or was blocked (read-only,
    validation).
  - `success` - the action succeeded and its result is off-screen or non-obvious.
  - `info` - FYI with no outcome attached. Rare; question whether it belongs at all.
- **Dedup `key`s come from `src/utils/toastKeys.ts`, never ad-hoc literals.** One
  key per event class; callsites reporting the same event share the same key so the
  dedup window collapses them. Register a new key there first.
- **All toast strings go through i18n** (`t()` / `en.json`), like any other UI
  string. Hardcoded English in a `showToast` call is a defect.
- **Nothing critical or must-act rides a toast.** Toasts auto-dismiss; a message the
  user cannot afford to miss, or that carries a required action, is a dialog or
  banner (Carbon: actionable notifications must persist). Toasts here have no action
  buttons by design - do not add them per-callsite.
- **Keep it to one line** (~60 characters): what happened, then what to do if
  anything. Details belong in a dialog.
- Keyboard-shortcut confirmation toasts are additionally gated by the
  `general.shortcut_feedback` preference - go through the existing path in
  `useKeyboardShortcuts.ts`, not a raw `showToast`.

## Dialog rules

- **Outcome-reporting goes through the routers in `src/utils/saveFeedback.ts`**
  (`surfaceSaveFailure`, `surfaceImportFailure`, `finalizeDeferredSaveWarnings`,
  `completeImportToSingleView`) when one owns the event class. A bespoke
  `openDialog('error', ...)` for save or import outcomes is a defect - the router
  is where policy (the save prompt for a file changed on disk, permission-denied
  suppression, receipt aggregation) lives.
- **Modal means blocking.** Open a dialog only when the user must read or decide
  before continuing. Reporting a success that needs no decision is a receipt-style
  dialog only when there are multiple facts to review (import receipt); otherwise
  it is a toast.
- **A receipt with no warnings has no decision in it, and is not a modal
  (#411/#420).** The import receipt (TML/Compass/Walls/GPX/DMP and the SpeleoDB
  adoption pull, all routed
  through `presentImportReceipt` in `saveFeedback.ts` - a receipt producer that
  bypasses it is a policy defect) reports facts the user
  already asked for by importing - counts, not a choice. When it carries no
  `warnings`, no unit conversion, no created zero datum, and something actually
  landed, it surfaces as a `success` toast (counts only, dedup key
  `TOAST_KEYS.IMPORT_RECEIPT`) instead of opening `ImportReceiptDialog`. Five
  things keep the modal:
  - It carries even one warning: a warning means something was lost or needs
    a look, and #411 exists precisely so that never rides a dismissible toast.
  - It carries even one `line_problems` entry (#814): a line the import
    left out or could not connect. The dialog shows every such entry above
    the warnings and never folds them behind Details, since they are what the
    user most needs. A line left unconnected is also listed afterwards in the
    Warnings tab (code `SECTION_NOT_CONNECTED`, the pre-rename spelling the
    stored Ignores key on) while the project holds a zero datum; in a project
    with none, the receipt is its only record.
  - `lines_imported` is zero (#442) - nothing landing is itself the fact
    the user needs to see. Since #814 both it and `shot_count` count only the
    lines that landed: a line the project refuses is skipped and named in
    the receipt's `line_problems`, which keep the modal anyway.
  - `unit_conversion` is set - a length-unit conversion silently changes every
    number in the data. It is a transformation notice, not a decision, but it
    does not fit the toast's one-line budget, so it keeps the modal rather
    than being dropped or given a second, lesser surface.
  - `zero_datum_created` is set (F12c) - the import created a zero datum on a
    line, and naming that line does not fit the toast's one-line budget
    either, so it rides the modal for the same reason as `unit_conversion`.

  The toast path emits no dialog telemetry, deliberately (toasts carry no
  events and the registry is untouched) - so from this change on,
  `dwell.dialog.import-receipt` counts only warning/empty/converted/zero-datum
  imports, not "imports happened". Read dashboards accordingly.
- **Warnings are not errors.** Do not dress warning-class content in `ErrorDialog`
  with `hideReportHint` as a costume; if no warning-capable surface fits, that is a
  gap to raise, not to paper over.
- **Every outcome dialog carries telemetry**: `trackDialogComplete` /
  `checkDialogCancel`, plus the registry entries in `src-tauri` `registry.rs` and
  `src/utils/telemetry.ts`, including the `active-dwell.dialog.*` mirror for every
  `dwell.dialog.*` (the registry test enforces the count).
- Autosave failure is banner-class, not modal: it must not interrupt, but it must
  not be silent either (an unsaved-changes condition is ongoing). Its Dismiss is a
  deliberate override - the banner re-raises on the next failing autosave rather
  than tracking the condition continuously; the close guard remains the backstop.
- **A file another program changed is a modal decision, but only on a manual
  save.** A save refused with `EXTERNALLY_MODIFIED` (the `.stalx` guard,
  [saving.md](saving.md#the-external-modification-guard)) puts data at risk
  either way, so `surfaceSaveFailure` opens `SaveConflictDialog`: Keep my
  version, Save as a new file, Use the file's version (not for a SpeleoDB copy,
  whose reload belongs to the sync, nor for a file that was moved or deleted:
  the refusal carries `FILE_MISSING`, and the prompt says "moved or deleted";
  nor for a file that appeared where a first save goes: `FILE_EXISTS`, and the
  prompt says "A file named ... appeared at this location"), or Decide later.
  An autosave refused the same way raises no modal (a background save never
  does): it goes to the autosave banner ("changed by another program", "moved
  or deleted by another program", or "A different file appeared where the
  project is saved") and autosave
  pauses until a save lands; the banner's Save opens the prompt.
- **A read-only file and a converted file are banners, not dialogs.** A file
  made with a newer Stalagmight opens read-only with a banner naming the reason
  (`read-only-file-banner`, no button: there is nothing to do here but update),
  and the status bar reads "Read-only". Anything it turns away - a typed key,
  Ctrl+Z, Save, Ctrl+S, Save As, Import, a click on a disabled menu row, a
  save the backend refused (`surfaceSaveFailure`) - is the `warning` toast
  `edit-disabled` with the read-only notice, once while it is on screen,
  never the error dialog: it is a blocked action, not a failure, and the
  report-a-bug footer would be wrong advice (demo issue N-1). An old
  file whose conversion moved stations, changed a depth, could not place a
  station, showed a station under another name or joined comments shows the
  one-time conversion notice (`conversion-notice-banner`) until dismissed: one
  lead, the changes the backend words with their counts (a long list of
  renamed stations behind Details), and one sentence on saving that is true
  now ("the next time you save", or "Saved in the new format"; "the original
  file is kept": Recent Projects, right-click, Restore an earlier copy; for a
  legacy SpeleoDB revision converted on download, "the original revision is
  unchanged on SpeleoDB" instead). The converting save moves that sentence on;
  with no banner showing it is a success toast instead (one event, one
  surface). Until that save the status bar reads "Old format: converted when
  you save", not "Saved", banner or not (N-6). The history-discarded banner
  (`history-discarded-banner`) is unchanged: it also covers an old file's
  history, which the conversion drops.
- **A choice that may free someone else's lock is worded, and never the default.**
  SpeleoDB's lock belongs to the account, so a lock the held-lock prompt
  (`HeldLockResumeDialog`) lists may be the one the same account is editing under on
  another computer right now; the app cannot tell that from a lock a crashed session
  left. The prompt says both are possible and what releasing does (others can then
  change the project under the session still editing), offers the release as
  "Release anyway" rather than hiding it (a crashed session's lock must stay
  releasable), and focuses Not now, so Enter keeps every lock. A row it can prove is
  not this computer's says "Held by this account, not by this computer". The same
  goes for any confirmation whose action reaches past this computer: say what
  happens to the lock (Take the SpeleoDB version: "You keep the write lock").
- **An upload that replaced someone's revision is a banner, and stops an exit.**
  When a SpeleoDB upload lands on top of a revision another session uploaded
  between the push's check and the upload (R3), the upload succeeded but that
  revision's work is no longer the latest: data at risk of being overlooked, so
  never a toast. The replaced-revision banner (`speleo-replaced-banner`, its own
  slot, `speleoStore.replacedRevisions`) names the revision (description, short
  id, author), says its changes are not in the upload and that it is still in the
  Revision History to view or restore, and stays until dismissed: no later push,
  release, Start editing, failure or notice takes its place; those show in the
  ordinary banner beside it. It replaces the success toast (one event, one
  surface); an upload that also left its undo history out still says so in its
  warning toast, a separate fact. A failure after the landing (the release that
  followed failed, or a late edit is still to upload) carries the report too. Every close
  that ran the upload stops instead of closing, so the banner is read first: the
  exit and sign-out prompts, Save and Close, and the older-version confirmation
  finishing a close or a quit. The next close asks nothing. An upload refused
  because another session uploaded first is the
  divergence prompt, like any divergence.

## Command-failure contract

- **`success: false` never dies silently.** Precedence: a specific handler first
  (read-only `EDIT_DISABLED` notice, divergence prompt, the save/import routers);
  when nothing specific claims the failure, `executeCommandInner`'s catch-all
  error toast (key `command-failed`) surfaces `result.error`. The catch-all
  yields while ANY dialog is open - dialogs own their inline errors and are open
  when they issue commands - so a panel or map flow must NOT add its own
  failure toast for an `executeCommand`-backed action; the catch-all is the
  surface. The context menu is the one dialog that does not suppress it: it
  issues commands but renders no errors. Known accepted loss: the gate is
  global, so a non-dialog command failing while an unrelated dialog happens to
  be open is still silent - the cost of never double-messaging a dialog.
  Undo/redo keep their own equivalent fallback (same localized default).
- **`CommandResult::with_warning` (code `"SYSTEM"`) is exclusive to the deferred-
  save contract** (#92/#104): "imported but could not save" keeps
  `success: true` + warning so data survives exit. Never route ordinary warnings
  through it; import-quality warnings ride `CommandResult.import_receipt`.
- **A side effect the user did not ask for gets its own warning code.** A delete
  with shift that removes a loop closure it left spanning a single shot (#737)
  reports each removal as `CLOSURE_REMOVED_ACROSS_ONE_SHOT` in
  `CommandResult.warnings`; `notifyRemovedClosures` (`utils/closureRemovalToast.ts`,
  called by the `deleteShot` / `deleteShotRangeShift` store actions) turns them
  into ONE warning toast (key `closure-removed`) in the backend's words, the way
  the `command-failed` catch-all shows `result.error`. A toast, not a dialog: the
  delete did what was asked, nothing needs deciding, and one Undo restores the
  closure with the shots.
- Genuine IPC rejections (transport errors) re-sync state and re-throw; the caller
  that initiated the action owns surfacing them under the rules above.

## Copy

- Lead with what happened, then what to do: "Export failed - choose a folder with
  write access", not "An error occurred".
- **A SpeleoDB failure is worded by its cause, never shown raw.** A rejected
  SpeleoDB call carries its transport cause typed (`SpeleoErrorDto.transport`;
  a failed lock release `CommandResult.lock_release_transport`), and every surface
  words it from that: `humanizeSpeleoError` with an operation prefix,
  `describeSpeleoError` where the surface frames the failure itself (a dialog with
  its own title, an error line under a prompt), `describeTransportCause` for a
  cause alone. The line names the server and says what to check. The backend's
  "Network error talking to SpeleoDB: <raw cause>" is for the log only: no
  details line shows it. Sentences the backend builds itself (a lock left held
  after a delete) name the cause in plain words (`SpeleoDbError::user_reason`).
- **A notice is titled for what happened.** The SpeleoDB read-only notice says
  "Project opened read-only" only after an open; after Resume editing or Start
  editing on a project already open it says "Still read-only". When the lock
  request may have reached SpeleoDB without anything confirming it, the notice
  says so in one sentence, so the close that later hands the lock back (or asks
  "Couldn't hand back the lock") is no surprise.
- Name things the user recognizes (project, line, shot), active voice, no
  internal jargon or error-code prefixes in the message body.
- Sentence case, ASCII hyphens only (repo-wide rule), tone per
  [style-guide.md](style-guide.md).
- Dialog = title (what happened) + body (why / what next) + one primary action.

## Accessibility

Comes free from the shared components - which is why new surfaces must reuse them,
never re-implement: `ToastStack` announces via polite (info/success) and assertive
(warning/error) live regions; `dialogStack` makes the app inert behind any dialog;
banners carry `role="status"`. A hand-rolled floating div has none of this.
