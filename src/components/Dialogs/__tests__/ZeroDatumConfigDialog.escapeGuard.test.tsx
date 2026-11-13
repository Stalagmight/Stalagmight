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

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () => [],
    getTotalSize: () => count * 33,
    scrollToIndex: () => {},
  }),
}));

vi.mock('react-i18next', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react-i18next')>()),
  useTranslation: () => ({ t: (k: string) => k }),
}));

vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../SectionPreview', () => ({ default: () => null }));
vi.mock('../../common/SectionMetadataForm', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../common/SectionMetadataForm')>()),
  SectionMetadataForm: () => null,
}));

vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ project: null, config: null }),
}));

import ZeroDatumConfigDialog from '../ZeroDatumConfigDialog';
import type { DmpFile, DmpSection, DmpShot } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';

function dmpShot(): DmpShot {
  return {
    shot_type: 'Standard',
    heading_in: 10,
    heading_out: 10,
    length: 5,
    depth_in: 0,
    depth_out: 0,
    pitch_in: 0,
    pitch_out: 0,
    left: 0,
    right: 0,
    up: 0,
    down: 0,
    temperature: 0,
    hour: 0,
    minute: 0,
    second: 0,
    marker_index: 0,
  };
}

function dmpFile(): DmpFile {
  const section: DmpSection = {
    version: 1,
    name: 'Anchor',
    direction: 'in',
    date: '2026-01-01',
    shots: Array.from({ length: 2 }, dmpShot),
    content_hash: 'hash-anchor',
  };
  return { version: 1, sections: [section] } as unknown as DmpFile;
}

const selectionConfig = {
  selectedIndices: new Set([0]),
  defaultSurveyors: [{ name: 'S' }],
  defaultExplorers: [],
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
