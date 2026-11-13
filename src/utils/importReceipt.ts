import type { TFunction } from 'i18next';
import type { ImportReceipt } from '../types/ipc/ImportReceipt';

/**
 * Combine the receipts from several `import_sections` calls made during one
 * logical import (the MNemo DMP wizard batches early/segment/late/pending
 * imports, each a separate backend call) into a single summary. Counts are
 * summed, warnings are concatenated in call order, and the file-level fields
 * (`source_format`, `file_name`, `sections_found`, the first non-null
 * `unit_conversion`) are taken from whichever receipt set them first - they
 * describe the whole file, not a single batch, so summing would be wrong.
 *
 * Returns `undefined` for an empty list so a caller with no receipts (nothing
 * was actually imported) can skip opening the receipt dialog entirely.
 */
export function aggregateImportReceipts(receipts: ImportReceipt[]): ImportReceipt | undefined {
  const [first, ...rest] = receipts;
  if (!first) return undefined;

  return rest.reduce<ImportReceipt>(
    (acc, r) => ({
      source_format: acc.source_format,
      file_name: acc.file_name,
      sections_found: acc.sections_found,
      sections_imported: acc.sections_imported + r.sections_imported,
      shot_count: acc.shot_count + r.shot_count,
      closures_requested: acc.closures_requested + r.closures_requested,
      closures_created: acc.closures_created + r.closures_created,
      unit_conversion: acc.unit_conversion ?? r.unit_conversion,
      warnings: [...acc.warnings, ...r.warnings],
      // `info` is `#[serde(default)]` on the Rust side so an older cached or
      // replayed receipt can arrive without the field at all - nullish-safe
      // here for the same reason.
      info: [...(acc.info ?? []), ...(r.info ?? [])],
      // First batch that created one wins, same rule as `unit_conversion`.
      zero_datum_created: acc.zero_datum_created ?? r.zero_datum_created,
    }),
    first,
  );
}

/**
 * The three "what landed" facts shared by the receipt dialog and the receipt
 * toast (docs/notifications.md): sections imported, shots imported, and loops
 * reconstructed (only when the receipt actually requested a closure), plus
 * any `info` notice lines (#446 - e.g. "N sections are not connected to a
 * zero datum") when `includeInfo` is true. `info` rides the same summary as
 * these facts rather than a `warnings`-style tier, so it never promotes a
 * warning-free receipt from toast to dialog (`presentImportReceipt`).
 *
 * `includeInfo` defaults to true (the modal path, `ImportReceiptDialog`,
 * which has room for the full sentence); `presentImportReceipt`'s TOAST path
 * passes false and appends its own short-form line instead - the toast's
 * one-line budget (docs/notifications.md) has no room for the full sentence,
 * the same reasoning that already routes `unit_conversion` to the modal.
 *
 * `receipt.info` is nullish-safe (`#[serde(default)]` on the Rust side means
 * an older cached/replayed receipt can arrive without the field). `t` is
 * either the component `useTranslation()` hook's `t`, or `i18n.t` directly
 * when called outside React (the toast path in `saveFeedback.ts`).
 */
export function importReceiptSummaryParts(
  receipt: ImportReceipt,
  t: TFunction,
  includeInfo = true,
): string[] {
  return [
    t('dialogs.importReceipt.sections', { count: receipt.sections_imported }),
    t('dialogs.importReceipt.shots', { count: receipt.shot_count }),
    ...(receipt.closures_requested > 0
      ? [t('dialogs.importReceipt.loops', { count: receipt.closures_created })]
      : []),
    ...(includeInfo ? (receipt.info ?? []) : []),
  ];
}

/**
 * Nothing landed. #442's case: an import that silently produces nothing must
 * never be demoted to a toast, even if it happens to carry no `warnings` of
 * its own (the absence of any data IS the thing the user needs to see).
 *
 * Gated on `sections_imported` alone, NOT `shot_count`: `shot_count` is
 * summed from the sections the import *attempted* (`command_import.rs`,
 * computed before the per-section outcome is known), while
 * `sections_imported` counts what actually landed. They only agree today
 * because the import loop is all-or-nothing - the day a partial-drop path
 * exists, a zero-sections receipt could carry a nonzero `shot_count`, and a
 * `shot_count`-inclusive check would wrongly let that case through as a
 * toast.
 */
export function isEmptyImportReceipt(receipt: ImportReceipt): boolean {
  return receipt.sections_imported === 0;
}
