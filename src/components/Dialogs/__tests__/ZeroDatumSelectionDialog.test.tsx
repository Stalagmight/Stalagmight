/**
 * The Skip action on the zero-datum section picker (#F5): it must read as
 * secondary to the section cards, not compete with them, and it must carry a
 * visible, accurate cost line so skipping is an informed choice rather than
 * the path of least resistance. The dialog also must not close on a
 * backdrop click (#F9) - a click meant for the map behind it would otherwise
 * silently drop the wizard.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys());

vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ config: null }),
}));

export const mockDialogWrapperSpy = vi.fn();
vi.mock('../DialogWrapper', async () =>
  (await import('../../../test/factories/components')).dialogWrapperStub({
    testId: true,
    spy: (props) => mockDialogWrapperSpy(props),
  }),
);

import ZeroDatumSelectionDialog from '../ZeroDatumSelectionDialog';
import type { DmpFile, DmpSection } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import en from '../../../i18n/locales/en.json';
import { makeDmpSection, makeDmpShots } from '../../../test/factories/dmp';

function dmpFile(): DmpFile {
  const section: DmpSection = makeDmpSection({ shots: makeDmpShots(3) });
  return { version: 1, sections: [section] } as unknown as DmpFile;
}

const selectionConfig = {
  selectedIndices: new Set([0]),
  defaultSurveyor: 'S',
  defaultDate: '2026-01-01',
} as unknown as DmpSelectionConfig;

function renderDialog() {
  render(
    <ZeroDatumSelectionDialog
      dmpFile={dmpFile()}
      selectionConfig={selectionConfig}
      filePath="/tmp/anchor.dmp"
      onCancel={vi.fn()}
      onBack={vi.fn()}
      onSkip={vi.fn()}
      onSelectSection={vi.fn()}
    />,
  );
}

describe('ZeroDatumSelectionDialog skip button', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders Skip as a secondary action, not the primary one', () => {
    renderDialog();
    const skip = screen.getByTestId('zero-datum-selection-skip');
    // `data-primary-action` is only ever set on `variant="primary"` (Button.tsx) -
    // a component that regressed Skip back to primary would carry it again.
    expect(skip).not.toHaveAttribute('data-primary-action');
  });

  it('has no primary-action element anywhere in the dialog', () => {
    // The section cards are hand-rolled <button>s that never render through
    // Button.tsx, so asserting on one of them alone cannot fail - this
    // queries the whole dialog body so a regression anywhere (Skip, a card,
    // a future footer button) is caught.
    renderDialog();
    const dialog = screen.getByTestId('zero-datum-selection-dialog');
    expect(dialog.querySelectorAll('[data-primary-action]')).toHaveLength(0);
  });

  it('renders the cost line next to the button', () => {
    renderDialog();
    expect(screen.getByTestId('zero-datum-selection-skip-cost')).toBeInTheDocument();
  });

  it('the skip-cost copy is accurate: skipping still places sections, it just leaves no GPS anchor', () => {
    // `useTranslation` is mocked to echo keys, so this pins the real string
    // directly rather than through the rendered (mocked) DOM - an empty
    // string, or the earlier draft's wrong "every section will land
    // unpositioned" claim (onSkip still runs the normal placement wizard),
    // fails this.
    expect(en.dialogs.zeroDatumSelection.skipCost).toContain('no real-world GPS anchor');
    expect(en.dialogs.zeroDatumSelection.skipCost).not.toContain('unpositioned');
  });
});

describe('ZeroDatumSelectionDialog backdrop click', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('passes closeOnBackdropClick={false} to DialogWrapper', () => {
    renderDialog();
    const calls = mockDialogWrapperSpy.mock.calls;
    const last = calls[calls.length - 1]?.[0] as { closeOnBackdropClick?: boolean } | undefined;
    expect(last?.closeOnBackdropClick).toBe(false);
  });
});
