import { useUIStore } from '../store/uiStore';
import { isUnreadableRefusal, saveErrorKind, type SaveErrorKind } from './saveErrors';
import { trackDialogComplete } from './telemetry';
import { getErrorMessage, invokeIpc } from './ipc';
import { errorCode } from './commandError';
import { isEditGateCode } from './editErrors';
import type { CommandErrorCode } from '../types/ipc/CommandErrorCode';
import i18n from '../i18n';
import { importReceiptSummaryParts, isEmptyImportReceipt } from './importReceipt';
import { TOAST_KEYS } from './toastKeys';
import { isProjectChangedRefusal } from '../store/commandResult';
import type { ValidationWarning } from '../types/ipc/ValidationWarning';
import type { ImportReceipt } from '../types/ipc/ImportReceipt';
import type { OrphanPlacementSuggestion } from '../types/commands';
import type { ToastKind } from '../store/uiStore';
import type { ProjectStore } from '../store/projectStore';

/**
 * What surfaceSaveFailure needs from the project store, passed in rather than
 * imported: projectStore calls this module, so importing projectStore back made
 * the two part of a value-import cycle (#576 D27-32(b)). Callers pass
 * `useProjectStore.getState()` (or `get()` inside the store) at the call.
 */
export type SaveFailureIntegrity = Pick<ProjectStore, 'integritySaveBlocked' | 'openIntegrityDialog'>;

/**
 * A failed save as the routers below read it (#576 C4): the code to route on (null
 * when the failure carries none) and the line to show. A refused `saveProject`
 * outcome (`CommandRefused`) is one as it stands; a caught rejection becomes one
 * through `saveFailureOf`.
 */
export interface SaveFailure {
  readonly code: CommandErrorCode | null;
  readonly error: string | null | undefined;
}

/** The `SaveFailure` a caught rejection describes; `fallback` when it has no message. */
export function saveFailureOf(e: unknown, fallback?: string): SaveFailure & { readonly error: string } {
  return { code: errorCode(e), error: getErrorMessage(e, fallback) };
}

const NO_FAILURE: SaveFailure = { code: null, error: null };

/**
 * Surface a failed (non-throwing) save to the user.
 *
 * Four classes of failure, in priority order:
 *  1. Unresolvable cross-references - NOT an error, an integrity problem the
 *     user can fix. Route them straight to the Review & Repair dialog instead
 *     of a scary "Save Failed" error window.
 *  2. Filesystem permission denial (e.g. macOS forgot folder access) - already
 *     surfaced by `saveProject` itself via the folder-access guidance dialog,
 *     so don't double up with a generic error window.
 *  3. A destination that is gone or unopenable (#429) - the error dialog, but
 *     saying what is actually wrong with the folder instead of relaying
 *     SQLite's "Failed to open database".
 *  4. Anything else (a real I/O error) - show the error dialog as before.
 *
 * Reads `integrity.integritySaveBlocked`, which `saveProject` refreshes
 * (awaited) on failure, so it is current by the time this runs. On an IPC-transport reject
 * that re-check never ran, so `integritySaveBlocked` may be stale - pass
 * `skipIntegrityRouting` there to go straight to the error dialog instead of
 * wrongly opening Review & Repair. `title` overrides the dialog title (e.g.
 * "Detach Failed") without leaving the router.
 *
 * The integrity route is SKIPPED for a failure that names its own cause and has
 * no repair in that dialog (#517's unreadable ignore list). `saveProject` runs
 * `checkProjectIntegrity()` on EVERY failure, so a project carrying both
 * problems would otherwise always show Review & Repair - which cannot fix the
 * named one - and the refusal would never render anywhere.
 */
export function surfaceSaveFailure(
  failure: SaveFailure | null | undefined,
  openDialog: (dialog: string, data?: Record<string, unknown>) => void,
  integrity: SaveFailureIntegrity,
  opts?: { skipIntegrityRouting?: boolean; title?: string },
): void {
  // [#576 FZ-FZ-INT-4] A save refused because the project changed - or was being switched
  // - under it is not a failure: the switch is the user's own action. Nothing to show.
  if (isProjectChangedRefusal(failure)) return;
  const { code, error } = failure ?? NO_FAILURE;
  const kind = saveErrorKind(code);
  // [#576 B6-D1] A project saved by a newer version refuses every save. That is a
  // blocked action on a read-only project (the banner already says why), so it is
  // a warning toast (docs/notifications.md) - not an error dialog, and never
  // Review & Repair, whose repairs this build may not make either.
  if (kind === 'newer-file') {
    useUIStore
      .getState()
      .showToast(i18n.t('banners.newerFile.saveRefused'), TOAST_KEYS.NEWER_FILE_READ_ONLY, 'warning');
    return;
  }
  // [#576 B15] Stored settings the open could not read refuse every save, and the
  // banner already says which and why: a blocked action, so a warning toast - not
  // an error dialog, and never Review & Repair, which cannot read them either.
  if (kind === 'settings-unreadable') {
    useUIStore
      .getState()
      .showToast(
        i18n.t('banners.settingsUnreadable.saveRefused'),
        TOAST_KEYS.SETTINGS_UNREADABLE,
        'warning',
      );
    return;
  }
  const routesToIntegrity = !opts?.skipIntegrityRouting && !isUnreadableRefusal(kind);
  if (routesToIntegrity && integrity.integritySaveBlocked) {
    integrity.openIntegrityDialog();
    return;
  }
  if (!error) return;
  // Already surfaced by `saveProject` through the folder-access guidance dialog.
  if (kind === 'permission') return;
  openDialog('error', {
    message: saveFailureMessage(kind, error),
    title: opts?.title ?? i18n.t('dialogs.saveFailure.title'),
  });
}

/**
 * The line a failed save gets on a user-facing surface. A destination failure is
 * reworded - the backend's own text for one is SQLite's "Failed to open database",
 * which tells the user nothing about the folder being gone (#429) - and everything
 * else (an unreadable side table's refusal included, already a full sentence) is
 * relayed as the backend wrote it.
 */
function saveFailureMessage(kind: SaveErrorKind, error: string): string {
  switch (kind) {
    case 'path-not-found':
      return i18n.t('dialogs.saveFailure.pathNotFound');
    case 'path-too-long':
      return i18n.t('dialogs.saveFailure.pathTooLong');
    default:
      return error;
  }
}

/**
 * Whether the autosave-failure banner should render, given the integrity
 * banner's state (`App.tsx`).
 *
 * The default is to YIELD: when saving is integrity-blocked, the integrity
 * banner already tells the "saving is paused, here is the Repair button" story,
 * and docs/notifications.md asks for one surface per event.
 *
 * The exception is a failure Review & Repair cannot fix (#517's unreadable
 * ignore list). That is a DIFFERENT ongoing condition with a different remedy,
 * so the integrity banner is not telling its story - it is hiding it, and the
 * user would be left repairing cross-references while every save is refused for
 * an unrelated reason they were never shown.
 */
export function shouldShowAutosaveBanner(state: {
  autosaveFailure: string | null;
  autosaveFailureKind: SaveErrorKind | null;
  integritySaveBlocked: boolean;
  integrityIssueCount: number;
}): boolean {
  if (state.autosaveFailure === null) return false;
  if (isUnreadableRefusal(state.autosaveFailureKind)) return true;
  return !(state.integritySaveBlocked && state.integrityIssueCount > 0);
}

/**
 * The autosave banner's line for a failed background save (banner-class, per
 * docs/notifications.md). Same rewording as the dialog, one line shorter; the
 * banner renders this string.
 */
export function autosaveFailureMessage(failure: SaveFailure | null | undefined): string {
  const { code, error } = failure ?? NO_FAILURE;
  switch (saveErrorKind(code)) {
    case 'permission':
      return i18n.t('banners.autosave.permissionDenied');
    case 'path-not-found':
      return i18n.t('banners.autosave.pathNotFound');
    case 'path-too-long':
      return i18n.t('banners.autosave.pathTooLong');
    // [#576 B6-D1] Unreachable through autosave, which never runs on such a
    // project (useAutoSave), but a refused save still gets its own line.
    case 'newer-file':
      return i18n.t('banners.newerFile.saveRefused');
    // [#576 B15] Unreachable through autosave, which never runs while settings
    // are unread (useAutoSave), but a refused save still gets its own line.
    case 'settings-unreadable':
      return i18n.t('banners.settingsUnreadable.saveRefused');
    default:
      return error ?? i18n.t('banners.autosave.genericFailure');
  }
}

/**
 * What a SUCCESSFUL save could not keep, by the code the backend stamps on
 * `SaveOutcome.warnings` (the `*_CODE` consts in `save_sidecars.rs`). Exported
 * for saveWarningCodes.parity.test.ts, which holds it to those consts.
 */
export const SAVE_WARNING_TEXT: Readonly<Record<string, string>> = {
  // The undo history could not be written and was removed from the saved file.
  UNDO_HISTORY_DROPPED: 'saveWarnings.undoHistoryDropped',
  // The project's display settings could not be carried into the saved file.
  DISPLAY_SETTINGS_NOT_SAVED: 'saveWarnings.displaySettingsNotSaved',
  // Written, but the file could not be reopened afterwards.
  DATABASE_NOT_REOPENED: 'saveWarnings.databaseNotReopened',
  // A Save As away from an open file that could not be read: the new file has no
  // ignored warnings, SpeleoDB link or undo-history markers (#576 RV2-R-7).
  SIDE_TABLES_NOT_CARRIED: 'saveWarnings.sideTablesNotCarried',
};

/**
 * Tell the user what a SUCCESSFUL save went ahead without (#576 A3-D2, NF-AN2-1).
 *
 * The backend reports it on `SaveOutcome.warnings`, which nothing read before -
 * so a save that dropped the undo history said only "saved". Each one is a
 * warning toast (docs/notifications.md: the action succeeded with a caveat): not
 * a dialog, because there is nothing to decide, and not a banner, because it is
 * not ongoing - the next save starts clean. One dedup key per code, so a burst of
 * autosaves collapses into one notice and two different losses never replace
 * each other. A code this build has no words for still reaches the user, with the
 * backend's own sentence.
 */
export function surfaceSaveWarnings(warnings: ValidationWarning[] | undefined): void {
  for (const warning of warnings ?? []) {
    const textKey = SAVE_WARNING_TEXT[warning.code];
    const message = textKey
      ? i18n.t(textKey)
      : i18n.t('saveWarnings.other', { message: warning.message });
    useUIStore
      .getState()
      .showToast(message, `${TOAST_KEYS.SAVE_WARNING}:${warning.code}`, 'warning');
  }
}

/**
 * The backend stamps the deferred-save partial-success warning
 * (`finalize_deferred_import` → `CommandResult::with_warning`, src-tauri) with the
 * generic "SYSTEM" code, and `with_warning` is the ONLY caller of that code in the
 * crate. So this reliably distinguishes "imported but couldn't save to disk" from
 * ordinary per-shot validation warnings (LONG_SHOT, AZIMUTH_DEVIATION, …): those
 * carry their own codes, ride along on a *successful, fully-saved* import, and are
 * already surfaced through the data-table warning UI - they must NOT pop a modal
 * here (nor suppress the orphan-placement step). (#110)
 */
const DEFERRED_SAVE_WARNING_CODE = 'SYSTEM';

/**
 * The subset of an import's `ImportOutcome.warnings` that mean "the import is only
 * in memory - the save failed". Empty for a fully-persisted import, even one that
 * carries routine validation warnings. (#110)
 */
export function deferredSaveWarnings(
  warnings: ValidationWarning[] | undefined,
): ValidationWarning[] {
  return (warnings ?? []).filter((w) => w.code === DEFERRED_SAVE_WARNING_CODE);
}

/** The UI store actions a new-project import dialog uses to finish: close the
 *  dialog, return to the fitted single-system view, and surface follow-ups. */
export interface ImportDialogUi {
  closeDialog: () => void;
  setView: (view: 'single') => void;
  triggerFitToBounds: () => void;
  openDialog: (dialog: string, data?: Record<string, unknown>) => void;
  showToast: (message: string, key: string, kind: ToastKind) => void;
}

/**
 * A receipt carrying the raw count backing the toast's short-form
 * "N sections unanchored" line (#446). NOT part of the IPC contract - stamped
 * locally by `finishImportWithoutOrphanSuggestions` alongside the full
 * sentence it pushes onto `info` (that one is for the modal path, which has
 * room for it). Optional, so a plain `ImportReceipt` from any other producer
 * is still assignable here.
 */
export interface ImportReceiptWithUnanchoredCount extends ImportReceipt {
  unanchoredCount?: number;
}

/**
 * The surface for a post-import receipt (docs/notifications.md, #411/#420): a
 * receipt with no decision in it - no warnings, and something actually
 * landed - is a toast, not a modal (there's nothing to acknowledge, just a
 * click to continue). Three things keep the modal:
 *  - `warnings` is non-empty (something needs a look).
 *  - Nothing landed (#442, `isEmptyImportReceipt`) - the absence of data IS
 *    the fact the user needs to see.
 *  - `unit_conversion` is set: a length-unit conversion (e.g. feet -> meters)
 *    silently changes every number in the imported data. That is a
 *    transformation notice, not a decision, but the toast auto-dismisses in
 *    5-9s and its one-line budget (docs/notifications.md) has no room for it
 *    without truncating the summary - so it rides the modal instead of being
 *    dropped or getting a second, second-class surface.
 *  - `zero_datum_created` is set (F12c): same reasoning as `unit_conversion`
 *    - naming the GPS-anchored section is a fact the toast's one-line budget
 *    has no room for, so it rides the modal too.
 */
export function presentImportReceipt(
  receipt: ImportReceiptWithUnanchoredCount,
  ui: Pick<ImportDialogUi, 'openDialog' | 'showToast'>,
): void {
  if (
    receipt.warnings.length > 0 ||
    isEmptyImportReceipt(receipt) ||
    receipt.unit_conversion != null ||
    receipt.zero_datum_created != null
  ) {
    ui.openDialog('import-receipt', { receipt });
    return;
  }
  // Toast path: skip the full `info` sentence(s) (`includeInfo: false`) and
  // append the short "N sections unanchored" scale instead - the toast's
  // one-line budget has no room for the full sentence (the same reasoning
  // that already routes `unit_conversion` to the modal above). The modal
  // path (`ImportReceiptDialog`) calls `importReceiptSummaryParts` directly
  // with the default `includeInfo: true` and shows the full sentence.
  const parts = importReceiptSummaryParts(receipt, i18n.t, false);
  if (receipt.unanchoredCount) {
    parts.push(i18n.t('dialogs.importReceipt.unanchoredInfoShort', { count: receipt.unanchoredCount }));
  }
  const summary = parts.join(' - ');
  ui.showToast(
    i18n.t('dialogs.importReceipt.toast', { source: receipt.source_format, summary }),
    TOAST_KEYS.IMPORT_RECEIPT,
    'success',
  );
}

/**
 * Ask where the sections an import just created could attach
 * (`suggest_orphan_placements`), for the TML, Compass and Walls dialogs.
 *
 * A failed lookup is skipped, but not silently (#576 NF-DMPS-1, as the DMP
 * wizard's NF-AN4-N4): the import has landed, so it completes as before - this
 * answers `[]` and the caller goes on to `finishImportWithoutOrphanSuggestions`
 * - and the user is told the step that offers to attach unanchored sections did
 * not run, and why. A warning toast with the DMP wording: the importer's dialog
 * is closing over an import that succeeded, so there is nothing to decide.
 */
export async function suggestOrphanPlacementsAfterImport(
  newlyImportedSectionIds: number[],
  purpose: string,
  ui: Pick<ImportDialogUi, 'showToast'>,
): Promise<OrphanPlacementSuggestion[]> {
  try {
    return await invokeIpc(
      'suggest_orphan_placements',
      { newlyImportedSectionIds },
      { purpose },
    );
  } catch (e) {
    const reason = getErrorMessage(e);
    console.warn('[import] orphan placement lookup failed:', reason);
    ui.showToast(
      i18n.t('dialogs.importReceipt.orphanLookupFailed', { reason }),
      TOAST_KEYS.ORPHAN_LOOKUP_FAILED,
      'warning',
    );
    return [];
  }
}

/**
 * Finish an import after the orphan-placement pass produced no suggestions.
 * If the import still left sections unanchored, appends the "N sections are
 * not connected to a zero datum" notice to the receipt as an INFO line
 * (#446) before running the normal exit sequence - an info line, not a
 * warning, so it rides `presentImportReceipt`'s existing routing rather than
 * promoting an otherwise clean receipt from toast to dialog. Non-fatal: if
 * the check itself fails, the import still finishes, just without the line.
 *
 * `newlyImportedSectionIds` scopes the unanchored-count check to components
 * THIS import touched - otherwise a clean import into a project that already
 * had unrelated orphans would blame this receipt for orphans it didn't
 * create.
 *
 * Callers must invoke this at most once per import and never ALSO fall back
 * to a bare `completeImportToSingleView` on the same path (double-completion
 * would double-stamp `trackDialogComplete`/close/toast) - it already calls
 * `completeImportToSingleView` itself exactly once, so it should sit outside
 * any try/catch whose catch also completes the import.
 */
export async function finishImportWithoutOrphanSuggestions(
  dialogName: string,
  ui: ImportDialogUi,
  newlyImportedSectionIds: number[],
  receipt?: ImportReceipt | null,
): Promise<void> {
  let finalReceipt: ImportReceiptWithUnanchoredCount | undefined = receipt ?? undefined;
  if (receipt) {
    try {
      const unanchoredCount = await invokeIpc(
        'count_unanchored_sections',
        { newlyImportedSectionIds },
        { purpose: 'check for unanchored sections after import' },
      );
      if (unanchoredCount > 0) {
        finalReceipt = {
          ...receipt,
          info: [
            ...receipt.info,
            i18n.t('dialogs.importReceipt.unanchoredInfo', { count: unanchoredCount }),
          ],
          unanchoredCount,
        };
      }
    } catch {
      // Non-fatal: proceed without the info line.
    }
  }
  completeImportToSingleView(dialogName, ui, finalReceipt);
}

/**
 * A failed import, classified by where it failed:
 * - `command`: the backend refused the import (the store's refused outcome: its
 *   line and its code) - the dialog's work is
 *   over, so it closes (with its completion telemetry) before the error dialog.
 *   An edit-gate refusal gets no error dialog: the store already showed the
 *   read-only notice (#576 NF-W7E-2).
 *   It does NOT run the rest of the exit sequence: `setView`/fit dropped the
 *   user into the empty project the import had just created, fitted the map to
 *   nothing, and only THEN said why (#411 C2).
 * - `empty`: the backend succeeded but imported nothing (e.g. a `.wpj` whose
 *   `.srv` files are missing) - closes the dialog and says why, because a
 *   silent "success" with an empty map reads as breakage.
 * - `transport`: the IPC call itself threw - unlike the other two, the dialog is
 *   not closed and no completion telemetry is stamped; the error dialog takes
 *   the single dialog slot, so dismissing it returns the user to where they
 *   were. `format` names the source format in the fallback message.
 */
export type ImportFailure =
  | { phase: 'command'; error: string | null | undefined; code: CommandErrorCode | null }
  | { phase: 'empty'; message: string }
  | { phase: 'transport'; error: unknown; format: string };

/**
 * Whether an import command's failure says the user cancelled it: the loading
 * overlay's Cancel stopped it before anything was applied (code `IMPORT_CANCELLED`,
 * the backend's import runner; #576 NF-FIX3A-2), so nothing was imported.
 */
export function isImportCancelled(code: CommandErrorCode | null | undefined): boolean {
  return code === 'IMPORT_CANCELLED';
}

/**
 * The user-facing message for a failed import. Exported so a dialog that reports
 * the failure on a surface of its own (TmlImportDialog's empty-project prompt,
 * which has to ask about the project the failed import created) words it exactly
 * as this router would, instead of minting a second set of strings.
 */
export function importFailureMessage(failure: ImportFailure): string {
  switch (failure.phase) {
    case 'transport':
      return getErrorMessage(
        failure.error,
        i18n.t('dialogs.importFailure.transportFallback', { format: failure.format }),
      );
    case 'empty':
      return failure.message;
    case 'command':
      return failure.error ?? i18n.t('dialogs.importFailure.unknown');
  }
}

/**
 * The one router for the TML/Compass/Walls/GPX import dialogs' failure paths
 * (docs/notifications.md) - bespoke `openDialog('error', ...)` calls in import
 * dialogs are a policy defect.
 *
 * Every caller returns straight after calling this, in every phase, so reaching
 * here ends the import flow - which is why `importContext` is dropped here too
 * (see `completeImportToSingleView` for what a stale one costs).
 */
export function surfaceImportFailure(
  dialogName: string,
  ui: ImportDialogUi,
  failure: ImportFailure,
): void {
  const message = importFailureMessage(failure);
  useUIStore.getState().setImportContext(null);
  switch (failure.phase) {
    case 'transport':
      ui.openDialog('error', {
        title: i18n.t('dialogs.importFailure.transportTitle'),
        message,
      });
      return;
    case 'empty':
      trackDialogComplete(dialogName);
      ui.closeDialog();
      // Nothing survived the chosen options - the user's file/toggles, not a
      // Stalagmight defect, so the report-a-bug hint would be wrong advice (#447).
      ui.openDialog('error', {
        title: i18n.t('dialogs.importFailure.emptyTitle'),
        message,
        hideReportHint: true,
      });
      return;
    case 'command':
      // Report BEFORE anything moves: no setView, no fit-to-bounds.
      trackDialogComplete(dialogName);
      ui.closeDialog();
      // The user cancelled it (#576 NF-FIX3A-2): nothing was imported and nothing
      // went wrong, so a quiet note rather than an import-error dialog.
      if (isImportCancelled(failure.code)) {
        ui.showToast(i18n.t('dialogs.importFailure.cancelled'), TOAST_KEYS.IMPORT_CANCELLED, 'info');
        return;
      }
      // [#576 NF-W7E-2] Refused by the edit gate (a read-only SpeleoDB project, the
      // Plan/Export workspace, a newer file, an adoption hand-over): the store's edit
      // path already showed the read-only notice, the surface the command-failure
      // contract gives those codes (docs/notifications.md). One event, one surface -
      // and a blocked edit is not an "Import failed" error with a bug-report hint.
      if (isEditGateCode(failure.code)) return;
      // Unlike 'empty', 'command' is NOT a safe class to blanket-hide the
      // report-a-bug hint for (#447 review): execute_tml_import (and its
      // Compass/Walls/GPX siblings) refuse with the same uncoded `FAILED` for a
      // malformed source file (`Failed to parse TML file: ...`) AND for
      // internal/invariant failures with no file-fixable cause, so hiding by
      // phase alone would hide the hint for a genuine bug too. Keep the hint here; the file case is instead
      // caught before it ever reaches this router, at the parse step
      // (`parseForImport` in openOrImport.ts).
      ui.openDialog('error', {
        title: i18n.t('dialogs.importFailure.commandTitle'),
        message,
      });
  }
}

/**
 * Close the import dialog and return to the fitted single-system view, stamping
 * the dialog-complete telemetry event first. Shared exit sequence for the TML,
 * Compass, Walls and GPX new-project import dialogs.
 *
 * When `receipt` is present (a successful import that produced one), surfaces
 * the post-import receipt (#391) right after so the user sees what landed
 * without having to open the data table - as a toast when it carries no
 * decision, or the modal when it does (#411/#420, `presentImportReceipt`).
 *
 * This is also where the import flow ENDS, so it is where `importContext` is
 * dropped. Left set, it outlives its flow: `saveAfterImport` reads it, and a
 * later import whose own route sets no context (GPX/Compass/Walls) would inherit
 * a stale 'new-project' and take the full-save branch, committing the user's
 * unrelated pending edits.
 */
export function completeImportToSingleView(
  dialogName: string,
  ui: ImportDialogUi,
  receipt?: ImportReceipt | null,
): void {
  trackDialogComplete(dialogName);
  useUIStore.getState().setImportContext(null);
  ui.closeDialog();
  ui.setView('single');
  ui.triggerFitToBounds();
  if (receipt) {
    presentImportReceipt(receipt, ui);
  }
}

/**
 * Handle the deferred-save partial-success case shared by the import dialogs:
 * when a new-project import couldn't be saved to disk, it lives only in
 * memory. Close the dialog, return to the fitted view, and surface the "save
 * manually" warning as the LEADING row of the import receipt ("unsaved"
 * outranks format caveats) - returning true so the caller bails before the
 * orphan-placement step. Returns false (and does nothing) for a fully-saved
 * import, even one carrying routine per-shot validation warnings. (#110)
 *
 * Every import command attaches a receipt on success, so the no-receipt
 * fallback (a plain warning dialog) is defensive only.
 */
export function finalizeDeferredSaveWarnings(
  warnings: ValidationWarning[] | undefined,
  dialogName: string,
  ui: ImportDialogUi,
  receipt?: ImportReceipt | null,
): boolean {
  const saveWarnings = deferredSaveWarnings(warnings);
  if (saveWarnings.length === 0) return false;
  const messages = saveWarnings.map((w) => w.message);
  if (receipt) {
    completeImportToSingleView(dialogName, ui, {
      ...receipt,
      warnings: [...messages, ...receipt.warnings],
    });
  } else {
    completeImportToSingleView(dialogName, ui);
    ui.openDialog('error', {
      title: i18n.t('dialogs.importFailure.deferredSaveTitle'),
      message: messages.join('\n\n'),
      hideReportHint: true,
    });
  }
  return true;
}
