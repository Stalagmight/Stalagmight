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
| Banner / indicator | Ongoing conditions | Until the condition clears | integrity banner (`App.tsx`), `SpeleoBanner`, `OfflineIndicator`, read-only badges (`MapTopLeftOverlay`), `UpdateBanner` (#579: available/downloading/ready for an in-app update; a failed check gets no BANNER and goes quietly to idle - but a manual check from About's Updates section reports failure inline in the dialog itself, `SettingsDialog.tsx`) |
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
  is where policy (integrity-blocked routing, permission-denied suppression,
  receipt aggregation) lives.
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
  `TOAST_KEYS.IMPORT_RECEIPT`) instead of opening `ImportReceiptDialog`. Four
  things keep the modal:
  - It carries even one warning: a warning means something was lost or needs
    a look, and #411 exists precisely so that never rides a dismissible toast.
  - `sections_imported` is zero (#442) - nothing landing is itself the fact
    the user needs to see, never inferred from `shot_count` (which is summed
    from the sections the import *attempted*, not what landed - the two only
    agree while the import path is all-or-nothing).
  - `unit_conversion` is set - a length-unit conversion silently changes every
    number in the data. It is a transformation notice, not a decision, but it
    does not fit the toast's one-line budget, so it keeps the modal rather
    than being dropped or given a second, lesser surface.
  - `zero_datum_created` is set (F12c) - the import created a zero datum on a
    section, and naming that section does not fit the toast's one-line budget
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
- Genuine IPC rejections (transport errors) re-sync state and re-throw; the caller
  that initiated the action owns surfacing them under the rules above.

## Copy

- Lead with what happened, then what to do: "Export failed - choose a folder with
  write access", not "An error occurred".
- Name things the user recognizes (project, section, shot), active voice, no
  internal jargon or error-code prefixes in the message body.
- Sentence case, ASCII hyphens only (repo-wide rule), tone per
  [style-guide.md](style-guide.md).
- Dialog = title (what happened) + body (why / what next) + one primary action.

## Accessibility

Comes free from the shared components - which is why new surfaces must reuse them,
never re-implement: `ToastStack` announces via polite (info/success) and assertive
(warning/error) live regions; `dialogStack` makes the app inert behind any dialog;
banners carry `role="status"`. A hand-rolled floating div has none of this.
