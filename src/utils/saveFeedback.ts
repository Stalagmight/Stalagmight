import { useProjectStore } from '../store/projectStore';
import { useUIStore } from '../store/uiStore';
import {
  classifySaveError,
  stripSaveErrorMarker,
  IGNORES_UNREADABLE,
  type SaveErrorKind,
} from './saveErrors';
import { trackDialogComplete } from './telemetry';
import { getErrorMessage, invokeIpc } from './ipc';
import i18n from '../i18n';
import { importReceiptSummaryParts, isEmptyImportReceipt } from './importReceipt';
import { TOAST_KEYS } from './toastKeys';
import type { ValidationWarning } from '../types/ipc/ValidationWarning';
import type { ImportReceipt } from '../types/ipc/ImportReceipt';
import type { ToastKind } from '../store/uiStore';

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
 * Reads `integritySaveBlocked`, which `saveProject` refreshes (awaited) on
 * failure, so it is current by the time this runs. On an IPC-transport reject
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
  error: string | null | undefined,
  openDialog: (dialog: string, data?: Record<string, unknown>) => void,
  opts?: { skipIntegrityRouting?: boolean; title?: string },
): void {
  const routesToIntegrity =
    !opts?.skipIntegrityRouting && classifySaveError(error) !== 'ignores-unreadable';
  if (routesToIntegrity && useProjectStore.getState().integritySaveBlocked) {
    useProjectStore.getState().openIntegrityDialog();
    return;
  }
  if (!error) return;
  const kind = classifySaveError(error);
  // Already surfaced by `saveProject` through the folder-access guidance dialog.
  if (kind === 'permission') return;
  openDialog('error', {
    message: saveFailureMessage(kind, error),
    title: opts?.title ?? i18n.t('dialogs.saveFailure.title'),
  });
}

/**
 * The line a failed save gets on a user-facing surface. A marked destination
 * failure is reworded - the backend's own text for one is SQLite's "Failed to
 * open database", which tells the user nothing about the folder being gone
 * (#429) - and everything else is relayed as the backend wrote it.
 */
function saveFailureMessage(kind: SaveErrorKind, error: string): string {
  switch (kind) {
    case 'path-not-found':
      return i18n.t('dialogs.saveFailure.pathNotFound');
    case 'path-too-long':
      return i18n.t('dialogs.saveFailure.pathTooLong');
    // Already a full sentence from the backend; only its routing marker comes off.
    case 'ignores-unreadable':
      return stripSaveErrorMarker(error, IGNORES_UNREADABLE);
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
  if (state.autosaveFailureKind === 'ignores-unreadable') return true;
  return !(state.integritySaveBlocked && state.integrityIssueCount > 0);
}

/**
 * The autosave banner's line for a failed background save (banner-class, per
 * docs/notifications.md). Same rewording as the dialog, one line shorter, and
 * it must never leak a raw marker - the banner renders this string.
 */
export function autosaveFailureMessage(error?: string | null): string {
  switch (classifySaveError(error)) {
    case 'permission':
      return i18n.t('banners.autosave.permissionDenied');
    case 'path-not-found':
      return i18n.t('banners.autosave.pathNotFound');
    case 'path-too-long':
      return i18n.t('banners.autosave.pathTooLong');
    case 'ignores-unreadable':
      return stripSaveErrorMarker(error ?? '', IGNORES_UNREADABLE);
    default:
      return error ?? i18n.t('banners.autosave.genericFailure');
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
 * The subset of an import's `CommandResult.warnings` that mean "the import is only
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
  receipt?: ImportReceipt,
  stillCurrent?: () => boolean,
): Promise<void> {
  let finalReceipt: ImportReceiptWithUnanchoredCount | undefined = receipt;
  if (receipt) {
    try {
      const unanchoredCount = await invokeIpc<number>(
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
  // The count IPC above is an await: the caller's own checks ran before it.
  if (stillCurrent && !stillCurrent()) return;
  completeImportToSingleView(dialogName, ui, finalReceipt);
}

/**
 * A failed import, classified by where it failed:
 * - `command`: the backend returned `success: false` - the dialog's work is
 *   over, so it closes (with its completion telemetry) before the error dialog.
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
  | { phase: 'command'; error: string | null | undefined }
  | { phase: 'empty'; message: string }
  | { phase: 'transport'; error: unknown; format: string };

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
      // Unlike 'empty', 'command' is NOT a safe class to blanket-hide the
      // report-a-bug hint for (#447 review): execute_tml_import (and its
      // Compass/Walls/GPX siblings) return this same CommandResult::failure
      // shape for a malformed source file (`Failed to parse TML file: ...`)
      // AND for internal/invariant failures with no file-fixable cause -
      // `guard_mutation`'s "read-only, acquire the lock to edit" and
      // `execute_tml_import_sections`'s "Failed to acquire command lock" (a
      // poisoned mutex) among them (src-tauri/crates/core/src/core/state/
      // command_import.rs, src-tauri/crates/core/src/core/state/app_state.rs
      // `guard_mutation`). The backend attaches no error-kind/classification
      // to distinguish them, so hiding by phase alone would hide the hint for
      // a genuine bug too. Keep the hint here; the file case is instead
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
  receipt?: ImportReceipt,
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
  receipt?: ImportReceipt,
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
