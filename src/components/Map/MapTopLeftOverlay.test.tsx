import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import userEvent from '@testing-library/user-event';
import { MapTopLeftOverlay } from './MapTopLeftOverlay';
import { useUIStore } from '../../store/uiStore';
import type { AdjustmentReport } from '../../types/ipc/AdjustmentReport';

// `t` resolves against the REAL shipped locale file rather than a hand-copied
// literal table: the assertions below quote user-visible English, and a private
// copy of those strings would keep passing after someone reworded en.json - two
// unlinked copies of the same copy. Imported inside the factory because
// `vi.mock` is hoisted above the module's own imports.
vi.mock('react-i18next', async () => {
  const en: unknown = (await import('../../i18n/locales/en.json')).default;
  const lookup = (key: string): unknown =>
    key
      .split('.')
      .reduce<unknown>(
        (node, part) =>
          typeof node === 'object' && node !== null
            ? (node as Record<string, unknown>)[part]
            : undefined,
        en
      );
  // English plurals the way i18next picks them (`_one` / `_other` by `count`),
  // and `{{count}}` interpolation - enough for the adjustment notice (#737).
  const t = (key: string, options?: { count?: number }): string => {
    const count = options?.count;
    const plural = count === undefined ? undefined : lookup(`${key}_${count === 1 ? 'one' : 'other'}`);
    const value = typeof plural === 'string' ? plural : lookup(key);
    return typeof value === 'string' ? value.replace('{{count}}', String(count)) : key;
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

  it('stacks every notice in one column beside the fit button, in order (#737)', () => {
    render(
      <MapTopLeftOverlay
        {...baseProps}
        showSpeleoReadOnly
        showClosureReadOnly
        adjustmentReport={{
          skipped_closures: [],
          failures: [{ anchor: { section_id: 0, station_index: 0 }, section_ids: [0], reason: 'x' }],
        }}
      />
    );
    const column = screen.getByTestId('map-top-left-notices');
    expect([...column.children].map((c) => c.getAttribute('data-testid'))).toEqual([
      'speleodb-readonly-banner',
      'geometry-readonly-banner',
      'adjustment-notice',
    ]);
    // Every notice still takes clicks; the row and the column let them through.
    for (const id of ['speleodb-readonly-banner', 'geometry-readonly-banner', 'adjustment-notice']) {
      expect(screen.getByTestId(id)).toHaveClass('pointer-events-auto');
    }
    expect(screen.getByTestId('map-top-left-overlay')).toHaveClass('pointer-events-none');
  });

  describe('adjustment notice (#737)', () => {
    const skipped = (closure_id: number) => ({
      closure_id,
      station_a: { section_id: 0, station_index: 1 },
      station_b: { section_id: 0, station_index: 2 },
      shot_section_id: 0,
      shot_index: 1,
    });
    const failure = {
      anchor: { section_id: 2, station_index: 0 },
      section_ids: [2, 3],
      reason: 'loop closures tie two zero datums to the same point.',
    };
    const report = (over: Partial<AdjustmentReport>): AdjustmentReport => ({
      skipped_closures: [],
      failures: [],
      ...over,
    });

    it('shows nothing without a report, or for an empty one', () => {
      const { rerender } = render(<MapTopLeftOverlay {...baseProps} />);
      expect(screen.queryByTestId('adjustment-notice')).toBeNull();
      rerender(<MapTopLeftOverlay {...baseProps} adjustmentReport={report({})} />);
      expect(screen.queryByTestId('adjustment-notice')).toBeNull();
    });

    it('counts the skipped closures, singular and plural', () => {
      const { rerender } = render(
        <MapTopLeftOverlay {...baseProps} adjustmentReport={report({ skipped_closures: [skipped(4)] })} />
      );
      expect(screen.getByTestId('adjustment-notice-skipped')).toHaveTextContent(
        '1 loop closure skipped: it spans a single shot.'
      );
      expect(screen.queryByTestId('adjustment-notice-failed')).toBeNull();
      rerender(
        <MapTopLeftOverlay
          {...baseProps}
          adjustmentReport={report({ skipped_closures: [skipped(4), skipped(9)] })}
        />
      );
      expect(screen.getByTestId('adjustment-notice-skipped')).toHaveTextContent(
        '2 loop closures skipped: each spans a single shot.'
      );
    });

    it('says when part of the survey is drawn measured', () => {
      const { rerender } = render(
        <MapTopLeftOverlay {...baseProps} adjustmentReport={report({ failures: [failure] })} />
      );
      expect(screen.getByTestId('adjustment-notice-failed')).toHaveTextContent(
        'Part of the survey could not be adjusted and is shown as measured.'
      );
      rerender(
        <MapTopLeftOverlay
          {...baseProps}
          adjustmentReport={report({ failures: [failure, failure], skipped_closures: [skipped(1)] })}
        />
      );
      expect(screen.getByTestId('adjustment-notice-failed')).toHaveTextContent(
        '2 parts of the survey could not be adjusted and are shown as measured.'
      );
      expect(screen.getByTestId('adjustment-notice-skipped')).toBeInTheDocument();
    });

    it('opens the Warnings tab, stacked under the read-only notice rather than over it', async () => {
      const openWarningsTab = vi.fn();
      useUIStore.setState({ openWarningsTab } as never);
      render(
        <MapTopLeftOverlay
          {...baseProps}
          showClosureReadOnly
          adjustmentReport={report({ skipped_closures: [skipped(4)] })}
        />
      );
      // Fit button | one column of notices, in order: SpeleoDB, closure, adjustment.
      const row = screen.getByTestId('map-top-left-overlay');
      const column = screen.getByTestId('map-top-left-notices');
      expect(column.parentElement).toBe(row);
      expect(column).toHaveClass('flex', 'min-w-0', 'flex-col', 'items-start', 'gap-2');
      expect(column).not.toHaveClass('pointer-events-auto');
      expect([...column.children].map((c) => c.getAttribute('data-testid'))).toEqual([
        'geometry-readonly-banner',
        'adjustment-notice',
      ]);
      expect(screen.getByTestId('fit-to-view-button').nextElementSibling).toBe(column);
      expect(screen.getByTestId('adjustment-notice')).toHaveClass('pointer-events-auto', 'min-w-0');
      await userEvent.click(screen.getByTestId('adjustment-notice-show-warnings'));
      expect(openWarningsTab).toHaveBeenCalledTimes(1);
    });
  });
});
