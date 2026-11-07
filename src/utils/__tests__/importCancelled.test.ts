/**
 * #576 NF-FIX3A-2 (decided: "Imports register the cancel token; Cancel stops them
 * between phases"). A file import the loading overlay's Cancel stopped is refused
 * with code IMPORT_CANCELLED (nothing was imported). The import dialogs route
 * every failure through `surfaceImportFailure`; a cancelled one is a quiet note,
 * not an import-error dialog.
 */
import { describe, it, expect, vi } from 'vitest';

import { isImportCancelled, surfaceImportFailure, type ImportDialogUi } from '../saveFeedback';
import i18n from '../../i18n';

const mockUi = (): ImportDialogUi & { [K in keyof ImportDialogUi]: ReturnType<typeof vi.fn> } => ({
  closeDialog: vi.fn(),
  setView: vi.fn(),
  triggerFitToBounds: vi.fn(),
  openDialog: vi.fn(),
  showToast: vi.fn(),
});

const CANCELLED = 'the Compass import was cancelled; nothing was imported';

describe('a cancelled import (#576 NF-FIX3A-2)', () => {
  it('is recognised by its code, and only by it', () => {
    expect(isImportCancelled('IMPORT_CANCELLED')).toBe(true);
    expect(isImportCancelled('FAILED')).toBe(false);
    expect(isImportCancelled('CANCELLED')).toBe(false);
    expect(isImportCancelled(null)).toBe(false);
  });

  it('closes the dialog with a quiet note instead of an import-error dialog', () => {
    const ui = mockUi();

    surfaceImportFailure('compass-import', ui, { phase: 'command', error: CANCELLED, code: 'IMPORT_CANCELLED' });

    expect(ui.closeDialog).toHaveBeenCalled();
    expect(ui.openDialog).not.toHaveBeenCalled();
    expect(ui.showToast).toHaveBeenCalledWith(
      i18n.t('dialogs.importFailure.cancelled'),
      'import-cancelled',
      'info',
    );
  });
});
