import type { TFunction } from 'i18next';
import type { ImportReceipt } from '../types/ipc/ImportReceipt';

/**
 * Combine the receipts from several `import_lines` calls made during one
 * logical import (the MNemo DMP wizard batches early/segment/late/pending
 * imports, each a separate backend call) into a single summary. Counts are
 * summed, warnings are concatenated in call order, and the file-level fields
 * (`source_format`, `file_name`, `lines_found`, the first non-null
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
      lines_found: acc.lines_found,
      lines_imported: acc.lines_imported + r.lines_imported,
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
      // #814: optional on the wire; kept absent when no batch had any.
      ...(acc.line_problems || r.line_problems
        ? { line_problems: [...(acc.line_problems ?? []), ...(r.line_problems ?? [])] }
        : {}),
    }),
    first,
  );
}

/**
 * The three "what landed" facts shared by the receipt dialog and the receipt
 * toast (docs/notifications.md): lines imported, shots imported, and loops
 * reconstructed (only when the receipt actually requested a closure), plus
 * any `info` notice lines (#446 - e.g. "N lines are not connected to a
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
    t('dialogs.importReceipt.sections', { count: receipt.lines_imported }),
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
 * Gated on `lines_imported` alone, NOT `shot_count`: `lines_imported`
 * is the count of what landed, which is the fact this case is about. Since
 * #814 `shot_count` also counts only the lines that landed (a refused
 * line is skipped and named in `line_problems`), but the gate stays on the
 * line count so it never depends on that sum being right; a
 * `shot_count`-inclusive check would let a zero-lines receipt through as a
 * toast.
 */
export function isEmptyImportReceipt(receipt: ImportReceipt): boolean {
  return receipt.lines_imported === 0;
}
