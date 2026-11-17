/**
 * #275: NON_FINITE_VALUE is an error-severity warning (never ignorable), and
 * before the explicit case it fell through to `default: maybeIgnore` which is
 * EMPTY for errors - the only Error code rendering zero resolution options.
 * Pin that the panel now offers actionable options (manual fix + delete).
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

import { WarningResolution } from '../WarningResolution';
import type { ProjectWarning } from '../../../types/commands';
import type { Line, Shot } from '../../../types/project';

const line = {
  id: 3,
  name: 'A',
  shots: [],
} as unknown as Line;

const nanLengthShot = {
  id: 9,
  length: Number.NaN,
  azimuth_in: 90,
  azimuth_out: 90,
  depth_in: 0,
  depth_out: 0,
  pitch_in: null,
  pitch_out: null,
  left: null,
  right: null,
  up: null,
  down: null,
  kind: 'Normal' as const,
} as unknown as Shot;

const warning: ProjectWarning = {
  code: 'NON_FINITE_VALUE',
  message: 'Shot 3:1 has non-finite value(s) in: length',
  severity: 'error',
  lineId: 3,
  shotId: 9,
  shotIndex: 0,
  isResolved: false,
} as unknown as ProjectWarning;

describe('WarningResolution NON_FINITE_VALUE (#275)', () => {
  it('offers actionable options (manual fix + delete), never an empty panel', () => {
    render(
      <WarningResolution
        warning={warning}
        line={line}
        shot={nanLengthShot}
        onExit={vi.fn()}
        onResolved={vi.fn()}
        onSkip={vi.fn()}
      />
    );

    const radios = screen.getAllByRole('radio');
    expect(radios.length).toBeGreaterThanOrEqual(2);
    // Error severity: Ignore must NOT be offered.
    expect(screen.queryByText(/ignore/i)).not.toBeInTheDocument();
  });
});
