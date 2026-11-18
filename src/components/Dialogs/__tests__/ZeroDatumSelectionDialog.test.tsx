/**
 * The Skip action on the zero-datum line picker (#F5): it must read as
 * secondary to the line cards, not compete with them, and it must carry a
 * visible, accurate cost line so skipping is an informed choice rather than
 * the path of least resistance. The dialog also must not close on a
 * backdrop click (#F9) - a click meant for the map behind it would otherwise
 * silently drop the wizard.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('react-i18next', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react-i18next')>()),
  useTranslation: () => ({ t: (k: string) => k }),
}));

vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ config: null }),
}));

export const mockDialogWrapperSpy = vi.fn();
vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({
    children,
    'data-testid': testId,
    ...props
  }: {
    children: React.ReactNode;
    'data-testid'?: string;
    [k: string]: unknown;
  }) => {
    mockDialogWrapperSpy({ 'data-testid': testId, ...props });
    return <div data-testid={testId}>{children}</div>;
  },
}));

import ZeroDatumSelectionDialog from '../ZeroDatumSelectionDialog';
import type { DmpFile, DmpSection, DmpShot } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import en from '../../../i18n/locales/en.json';

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
  const line: DmpSection = {
    version: 1,
    name: 'Anchor',
    direction: 'in',
    date: '2026-01-01',
    shots: Array.from({ length: 3 }, dmpShot),
    content_hash: 'hash-anchor',
  };
  return { version: 1, lines: [line] } as unknown as DmpFile;
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
      onSelectLine={vi.fn()}
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
    // The line cards are hand-rolled <button>s that never render through
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

  it('the skip-cost copy is accurate: skipping still places lines, it just leaves no GPS anchor', () => {
    // `useTranslation` is mocked to echo keys, so this pins the real string
    // directly rather than through the rendered (mocked) DOM - an empty
    // string, or the earlier draft's wrong "every line will land
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
