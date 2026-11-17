import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';
import { importReceiptSummaryParts } from '../../utils/importReceipt';
import type { ImportReceipt } from '../../types/ipc/ImportReceipt';

const MAX_VISIBLE_WARNINGS = 3;

/**
 * Post-import receipt (#391): a short "what landed" summary shown after a
 * foreign-format import (TML/Compass/Walls/GPX/DMP, plus the SpeleoDB
 * adoption pull), read from
 * `dialogData.receipt`. Tone is a receipt, not a wizard step - it never blocks
 * the import (everything it describes already happened), it just tells the
 * user what to expect before they go looking in the data table.
 *
 * Every PRODUCTION producer reaches this dialog through `presentImportReceipt`
 * (`saveFeedback.ts`, #411/#420 - tests may open it directly by injecting
 * `openDialog('import-receipt', ...)`): a receipt that carries a warning, reports zero lines
 * imported (#442), or performed a unit conversion (`unit_conversion` != null
 * - a silent transformation notice with no room in a toast's one-line budget)
 * still opens the modal here; a clean, unconverted receipt with something to
 * show instead surfaces as a toast and never reaches this file.
 *
 * `lines_found` is the whole-file line count, not a promise every
 * line was imported - DMP/Compass/Walls wizards let the user deliberately
 * deselect surveys, so a frontend-derived "N lines not imported" warning
 * from `lines_found - lines_imported` would false-alarm on every
 * partial selection. Every genuine silent-drop class (e.g. TML zero-shot
 * lines) is instead named explicitly in `receipt.warnings` by the backend.
 */
export default function ImportReceiptDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  const [expanded, setExpanded] = useState(false);

  const receipt = dialogData.receipt as ImportReceipt | undefined;
  if (!receipt) return null;

  const allWarnings = receipt.warnings;
  const visibleWarnings = allWarnings.slice(0, MAX_VISIBLE_WARNINGS);
  const hiddenWarnings = allWarnings.slice(MAX_VISIBLE_WARNINGS);
  const hasHiddenWarnings = hiddenWarnings.length > 0;

  const summaryParts = importReceiptSummaryParts(receipt, t);

  const handleContinue = () => {
    trackDialogComplete('import-receipt');
    closeDialog();
  };

  return (
    <DialogWrapper
      title={t('dialogs.importReceipt.title', {
        source: receipt.source_format,
        fileName: receipt.file_name,
      })}
      onClose={closeDialog}
      size="md"
      data-testid="import-receipt-dialog"
    >
      <div className="space-y-3">
        <ul className="space-y-1 text-sm text-text-primary">
          <li data-testid="import-receipt-summary">{summaryParts.join(' - ')}</li>
          {receipt.unit_conversion != null && (
            <li data-testid="import-receipt-unit-conversion">{receipt.unit_conversion}</li>
          )}
          {receipt.zero_datum_created != null && (
            <li data-testid="import-receipt-zero-datum-created">
              {t('dialogs.importReceipt.zeroDatumCreated', { line: receipt.zero_datum_created })}
            </li>
          )}
        </ul>

        {visibleWarnings.length > 0 && (
          <ul className="space-y-1 text-sm text-warning" data-testid="import-receipt-warnings">
            {visibleWarnings.map((w, i) => (
              <li key={i}>&#9888; {w}</li>
            ))}
          </ul>
        )}

        {expanded && hasHiddenWarnings && (
          <ul className="space-y-1 text-sm text-warning" data-testid="import-receipt-warnings-more">
            {hiddenWarnings.map((w, i) => (
              <li key={i}>&#9888; {w}</li>
            ))}
          </ul>
        )}

        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          {hasHiddenWarnings && (
            <Button
              onClick={() => setExpanded((v) => !v)}
              data-testid="import-receipt-details"
            >
              {expanded ? t('dialogs.importReceipt.hideDetails') : t('dialogs.importReceipt.viewDetails')}
            </Button>
          )}
          <Button variant="primary" onClick={handleContinue} data-testid="import-receipt-continue">
            {t('dialogs.importReceipt.continue')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
