import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { MapTopLeftOverlay } from './MapTopLeftOverlay';

// `t` resolves against the REAL shipped locale file rather than a hand-copied
// literal table: the assertions below quote user-visible English, and a private
// copy of those strings would keep passing after someone reworded en.json - two
// unlinked copies of the same copy. Imported inside the factory because
// `vi.mock` is hoisted above the module's own imports.
vi.mock('react-i18next', async () => {
  const en: unknown = (await import('../../i18n/locales/en.json')).default;
  const t = (key: string): string => {
    const value = key
      .split('.')
      .reduce<unknown>(
        (node, part) =>
          typeof node === 'object' && node !== null
            ? (node as Record<string, unknown>)[part]
            : undefined,
        en
      );
    return typeof value === 'string' ? value : key;
  };
  // The pills' Resume buttons pull in the SpeleoDB action hook, whose imports
  // initialise i18n - hand it a no-op plugin so that init stays inert here.
  return { useTranslation: () => ({ t }), initReactI18next: { type: '3rdParty', init: () => {} } };
});
vi.mock('../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));

const baseProps = {
  focused: false,
  onFitOrExitFocus: vi.fn(),
  showSpeleoReadOnly: false,
  showClosureReadOnly: false,
  showClosureComputing: false,
};

describe('MapTopLeftOverlay', () => {
  it('replaces the closure read-only label with a unified computing status', () => {
    render(
      <MapTopLeftOverlay
        {...baseProps}
        showClosureReadOnly
        showClosureComputing
      />
    );

    expect(screen.getByRole('status')).toHaveTextContent('Computing Closures - Read Only');
    expect(screen.getByTestId('geometry-computing-spinner')).toHaveClass('animate-spin');
    expect(screen.queryByText('Closures Computed View · read-only')).not.toBeInTheDocument();
    // The role must NOT ride on the chip: the chip is mounted by the very state
    // change it would have to announce, which is the unreliable case.
    expect(screen.getByTestId('geometry-readonly-banner')).not.toHaveAttribute('role');
  });

  it('keeps the status region mounted and empty before a solve starts', () => {
    render(<MapTopLeftOverlay {...baseProps} />);

    // An announcement channel that appears together with its text is unreliably
    // announced, so it has to pre-exist the solve - empty, but present.
    expect(screen.getByRole('status')).toBeEmptyDOMElement();
  });

  it('keeps the static read-only badge when no solve is running', () => {
    render(<MapTopLeftOverlay {...baseProps} showClosureReadOnly />);

    expect(screen.getByTestId('geometry-readonly-banner')).toHaveTextContent(
      'Closures Computed View · read-only'
    );
    expect(screen.queryByTestId('geometry-computing-spinner')).not.toBeInTheDocument();
  });

  it('shows computing progress without read-only wording in Plan and Export modes', () => {
    render(<MapTopLeftOverlay {...baseProps} showClosureComputing />);

    expect(screen.getByTestId('geometry-computing-banner')).toHaveTextContent(
      'Computing closures…'
    );
    expect(screen.getByRole('status')).not.toHaveTextContent('read-only');
    expect(screen.getByTestId('geometry-computing-spinner')).toHaveClass('animate-spin');
  });

  it('shows no closure status when the map is editable and idle', () => {
    render(<MapTopLeftOverlay {...baseProps} />);

    expect(screen.queryByTestId('geometry-readonly-banner')).not.toBeInTheDocument();
    expect(screen.queryByTestId('geometry-computing-banner')).not.toBeInTheDocument();
  });

  // jsdom has no layout: this pins the class contract, and
  // e2e-playwright/map-readonly-pill.spec.ts measures the real thing at MAP_MIN_WIDTH.
  it('bounds the row clear of the zoom control, lets the notices wrap, and passes clicks through', () => {
    render(<MapTopLeftOverlay {...baseProps} showClosureReadOnly />);

    const row = screen.getByTestId('map-top-left-overlay');
    expect(row).toHaveClass('right-14', 'pointer-events-none');
    expect(screen.getByTestId('fit-to-view-button')).toHaveClass('pointer-events-auto', 'shrink-0');
    // The pill never wraps its glyph away from the words: the glyph has its own
    // slot, and only the body (words + action) wraps.
    const pill = screen.getByTestId('geometry-readonly-banner');
    expect(pill).toHaveClass('pointer-events-auto', 'min-w-0', 'flex', 'items-start');
    expect(pill).not.toHaveClass('flex-wrap');
    expect(screen.getByTestId('geometry-readonly-banner-glyph')).toHaveClass('shrink-0');
    expect(screen.getByTestId('geometry-readonly-banner-body')).toHaveClass('flex-wrap', 'min-w-0', 'flex-1');
    expect(screen.getByTestId('geometry-readonly-banner-body')).toContainElement(screen.getByTestId('geometry-readonly-resume'));
    expect(screen.getByTestId('geometry-readonly-resume')).toHaveClass('shrink-0', 'whitespace-nowrap');
  });

  it('gives the SpeleoDB notice the same glyph / body structure', () => {
    render(<MapTopLeftOverlay {...baseProps} showSpeleoReadOnly />);
    const pill = screen.getByTestId('speleodb-readonly-banner');
    expect(pill).not.toHaveClass('flex-wrap');
    expect(screen.getByTestId('speleodb-readonly-banner-glyph').querySelector('svg')).not.toBeNull();
    expect(screen.getByTestId('speleodb-readonly-banner-body')).toContainElement(screen.getByTestId('speleodb-readonly-reason'));
    // The words keep a minimum column before the action drops under them.
    expect(screen.getByTestId('speleodb-readonly-reason')).toHaveClass('flex-[1_1_5rem]', 'max-w-max');
  });
});
