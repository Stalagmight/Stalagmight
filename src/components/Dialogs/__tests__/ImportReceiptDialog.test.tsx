/**
 * #391: the post-import receipt shown after a foreign-format import so the
 * user sees what landed without having to open the data table.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import type { ImportReceipt } from '../../../types/ipc/ImportReceipt';

const closeDialog = vi.fn();

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).translationStub('keyAndOptions'));

const dialogData: { receipt?: ImportReceipt | undefined } = {};

vi.mock('../../../store/uiStore', () => ({
  useUIStore: (sel: (s: unknown) => unknown) => sel({ closeDialog, dialogData }),
}));

vi.mock('../../../utils/telemetry', async () => (await import('../../../test/factories/telemetry')).telemetryStubs());

vi.mock('../DialogWrapper', async () =>
  (await import('../../../test/factories/components')).dialogWrapperStub({ title: 'testid' }),
);

vi.mock('../../common/Button', async () => (await import('../../../test/factories/components')).buttonStub());

import { trackDialogComplete } from '../../../utils/telemetry';
import ImportReceiptDialog from '../ImportReceiptDialog';

const baseReceipt = (overrides: Partial<ImportReceipt> = {}): ImportReceipt => ({
  source_format: 'Ariane',
  file_name: 'cave.tml',
  sections_found: 3,
  sections_imported: 3,
  shot_count: 42,
  closures_requested: 0,
  closures_created: 0,
  unit_conversion: null,
  warnings: [],
  info: [],
  zero_datum_created: null,
  ...overrides,
});

describe('ImportReceiptDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    dialogData.receipt = undefined;
  });

  it('renders nothing when there is no receipt', () => {
    const { container } = render(<ImportReceiptDialog />);
    expect(container).toBeEmptyDOMElement();
  });

  it('shows the section/shot counts', () => {
    dialogData.receipt = baseReceipt();
    render(<ImportReceiptDialog />);
    const summary = screen.getByTestId('import-receipt-summary').textContent ?? '';
    expect(summary).toContain('"count":3');
    expect(summary).toContain('"count":42');
  });

  it('shows the loops segment only when closures were requested', () => {
    dialogData.receipt = baseReceipt();
    const { rerender } = render(<ImportReceiptDialog />);
    expect(screen.getByTestId('import-receipt-summary').textContent).not.toContain('loops');

    dialogData.receipt = baseReceipt({ closures_requested: 2, closures_created: 2 });
    rerender(<ImportReceiptDialog />);
    expect(screen.getByTestId('import-receipt-summary').textContent).toContain(
      'dialogs.importReceipt.loops',
    );
  });

  it('shows the unit conversion line only when present, as the exact backend sentence', () => {
    dialogData.receipt = baseReceipt();
    const { rerender } = render(<ImportReceiptDialog />);
    expect(screen.queryByTestId('import-receipt-unit-conversion')).not.toBeInTheDocument();

    dialogData.receipt = baseReceipt({ unit_conversion: 'Lengths converted from feet to meters' });
    rerender(<ImportReceiptDialog />);
    expect(screen.getByTestId('import-receipt-unit-conversion').textContent).toBe(
      'Lengths converted from feet to meters',
    );
  });

  it('shows the zero-datum-created line only when present, naming the section (F12c)', () => {
    dialogData.receipt = baseReceipt();
    const { rerender } = render(<ImportReceiptDialog />);
    expect(screen.queryByTestId('import-receipt-zero-datum-created')).not.toBeInTheDocument();

    dialogData.receipt = baseReceipt({ zero_datum_created: 'GPS Anchor' });
    rerender(<ImportReceiptDialog />);
    expect(screen.getByTestId('import-receipt-zero-datum-created').textContent).toContain('GPS Anchor');
  });

  it('renders the full info sentence in the summary, not just a short scale (#446)', () => {
    dialogData.receipt = baseReceipt({ info: ['3 sections are not connected to a zero datum'] });
    render(<ImportReceiptDialog />);
    expect(screen.getByTestId('import-receipt-summary').textContent).toContain(
      '3 sections are not connected to a zero datum',
    );
  });

  it('does NOT derive a "not imported" warning from sections_found/sections_imported', () => {
    dialogData.receipt = baseReceipt({ sections_found: 5, sections_imported: 3, warnings: [] });
    render(<ImportReceiptDialog />);
    expect(screen.queryByTestId('import-receipt-warnings')).not.toBeInTheDocument();
  });

  it('hides "View details" when there are no extra warnings to expand', () => {
    dialogData.receipt = baseReceipt({ warnings: ['one'] });
    render(<ImportReceiptDialog />);
    expect(screen.queryByTestId('import-receipt-details')).not.toBeInTheDocument();
  });

  it('toggles the rest of the warnings behind "View details", and its label flips to "Hide details"', () => {
    dialogData.receipt = baseReceipt({
      warnings: ['w1', 'w2', 'w3', 'w4', 'w5'],
    });
    render(<ImportReceiptDialog />);
    expect(screen.queryByTestId('import-receipt-warnings-more')).not.toBeInTheDocument();
    const detailsButton = screen.getByTestId('import-receipt-details');
    expect(detailsButton.textContent).toBe('dialogs.importReceipt.viewDetails');
    fireEvent.click(detailsButton);
    expect(screen.getByTestId('import-receipt-warnings-more').textContent).toContain('w5');
    expect(detailsButton.textContent).toBe('dialogs.importReceipt.hideDetails');
    fireEvent.click(detailsButton);
    expect(screen.queryByTestId('import-receipt-warnings-more')).not.toBeInTheDocument();
    expect(detailsButton.textContent).toBe('dialogs.importReceipt.viewDetails');
  });

  it('continuing tracks completion and closes', () => {
    dialogData.receipt = baseReceipt();
    render(<ImportReceiptDialog />);
    fireEvent.click(screen.getByTestId('import-receipt-continue'));
    expect(trackDialogComplete).toHaveBeenCalledWith('import-receipt');
    expect(closeDialog).toHaveBeenCalled();
  });
});
