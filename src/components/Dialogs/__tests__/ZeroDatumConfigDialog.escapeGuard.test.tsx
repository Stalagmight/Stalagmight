/**
 * Escape while the Back-discard confirm is open (#F9 review fix round #2,
 * claim 4). `DialogWrapper`'s own Escape handling lives on its outer
 * container and has no idea the dialog also renders its own, unrelated
 * Back-discard overlay - left alone, an Escape press bubbles straight past it
 * and opens DialogWrapper's "Unsaved Changes" prompt ON TOP of the Back one.
 *
 * The REAL `DialogWrapper` is used here (not the lighter mock the sibling
 * test files use) because this is exactly the bubbling path a mock would
 * have to reimplement to be worth anything - `#dialog-root` is provisioned
 * globally by `src/test/setup.ts`.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', async () =>
  (await import('../../../test/factories/virtualizer')).fakeVirtualizer({ renderRows: false }),
);

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys());

vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../SectionPreview', () => ({ default: () => null }));
vi.mock('../../common/SectionMetadataForm', () => ({ SectionMetadataForm: () => null }));

vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ project: null, config: null }),
}));

import ZeroDatumConfigDialog from '../ZeroDatumConfigDialog';
import type { DmpFile, DmpSection } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import { makeDmpSection, makeDmpShots } from '../../../test/factories/dmp';

function dmpFile(): DmpFile {
  const section: DmpSection = makeDmpSection({ shots: makeDmpShots(2) });
  return { version: 1, sections: [section] } as unknown as DmpFile;
}

const selectionConfig = {
  selectedIndices: new Set([0]),
  defaultSurveyor: 'S',
  defaultDate: '2026-01-01',
} as unknown as DmpSelectionConfig;

function renderDialog(onBack = vi.fn()) {
  render(
    <ZeroDatumConfigDialog
      dmpFile={dmpFile()}
      selectionConfig={selectionConfig}
      sectionIndex={0}
      filePath="/tmp/anchor.dmp"
      onCancel={vi.fn()}
      onBack={onBack}
      onConfirm={vi.fn()}
    />,
  );
}

describe('ZeroDatumConfigDialog Escape while the Back confirm is open', () => {
  beforeAll(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('Escape dismisses ONLY the Back confirm, not DialogWrapper\'s own overlay too', () => {
    const onBack = vi.fn();
    renderDialog(onBack);

    // Make the section dirty, then open the Back confirm.
    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), { target: { value: '2' } });
    fireEvent.click(screen.getByTestId('zero-datum-config-back'));
    expect(screen.getByTestId('zero-datum-config-back-confirm-discard')).toBeInTheDocument();

    fireEvent.keyDown(screen.getByTestId('zero-datum-config-back-confirm-discard'), { key: 'Escape' });

    // The Back confirm is gone...
    expect(screen.queryByTestId('zero-datum-config-back-confirm-discard')).not.toBeInTheDocument();
    // ...and DialogWrapper's OWN discard overlay never opened on top of it.
    expect(screen.queryByTestId('dirty-dialog-discard')).not.toBeInTheDocument();
    expect(screen.queryByText('Unsaved Changes')).not.toBeInTheDocument();
    // Neither Back nor Cancel actually fired - Escape here only closed the prompt.
    expect(onBack).not.toHaveBeenCalled();
  });

  it('sanity: Escape WITHOUT the Back confirm open still raises DialogWrapper\'s own guard', () => {
    // The positive control for the test above: DialogWrapper's Escape
    // handling is not disabled wholesale, only skipped while the Back
    // confirm owns the keystroke.
    renderDialog();
    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), { target: { value: '2' } });
    fireEvent.keyDown(screen.getByTestId('zero-datum-config-dialog'), { key: 'Escape' });
    expect(screen.getByTestId('dirty-dialog-discard')).toBeInTheDocument();
  });
});
