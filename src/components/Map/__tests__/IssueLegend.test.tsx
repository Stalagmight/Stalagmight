/**
 * The Survey Issues legend: the colour ramp while Survey Issues paints the map,
 * the Errors labels key while those labels are on, nothing otherwise.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key }),
  initReactI18next: { type: '3rdParty', init: () => undefined },
}));

import { IssueLegend } from '../IssueLegend';

describe('IssueLegend', () => {
  it('renders nothing when neither part is wanted', () => {
    render(<IssueLegend showRamp={false} showKey={false} />);
    expect(screen.queryByTestId('issue-legend')).toBeNull();
  });

  it('shows the ramp from agree (bottom) through the warning threshold to error (top)', () => {
    render(<IssueLegend showRamp showKey={false} />);
    expect(screen.getByTestId('issue-legend-ramp')).toBeInTheDocument();
    expect(screen.queryByTestId('issue-legend-key')).toBeNull();
    const top = (id: string) => parseFloat(screen.getByTestId(id).style.top);
    expect(top('issue-legend-error')).toBeLessThan(top('issue-legend-warning'));
    expect(top('issue-legend-warning')).toBeLessThan(top('issue-legend-clear'));
    const stops = [...document.querySelectorAll('stop')].map((s) => s.getAttribute('stop-color'));
    expect(stops[0]).toBe('#ef4444');
    expect(stops[stops.length - 1]).toBe('#22c55e');
  });

  it('keys every Errors label abbreviation, and the closure label', () => {
    render(<IssueLegend showRamp={false} showKey />);
    const key = screen.getByTestId('issue-legend-key');
    for (const k of ['azimuth', 'inclination', 'inclinationDepth', 'length']) {
      expect(key).toHaveTextContent(`map.errorLabels.abbrev.${k}`);
      expect(key).toHaveTextContent(`map.issueLegend.key.${k}`);
    }
    expect(key).toHaveTextContent('map.errorLabels.abbrev.closure');
    expect(key).toHaveTextContent('map.issueLegend.key.closure');
  });
});
