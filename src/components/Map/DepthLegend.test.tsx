import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
// The real English strings (not a stub `t`), so the label and aria-label checks
// below read what a user and a screen reader get.
import '../../i18n';
import { DepthLegend } from './DepthLegend';
import { getDepthColor } from '../../utils/colors';

/**
 * Guard against the legend drifting from the map: a CSS/SVG gradient with only
 * two stops interpolates in sRGB, but `getDepthColor` interpolates in HSL along
 * the SHORTEST hue path (src/utils/colors.ts:179). Those two only agree at the
 * endpoints - any theme whose shallow/deep hues are more than 180deg apart (or
 * whose saturation/lightness differ) will show a visibly different gradient
 * mid-ramp. This asserts every rendered <stop> matches `getDepthColor` exactly,
 * so a future rewrite back to a naive 2-stop gradient fails loudly here instead
 * of shipping a legend that lies about what the map drew.
 */
describe('DepthLegend', () => {
  it('every gradient stop matches getDepthColor at the same t (hue-path parity)', () => {
    const shallowColor = '#a89060'; // hue ~35 (tan)
    const deepColor = '#b85048'; // hue ~5 (salmon) - a case where naive RGB and
    // HSL-shortest-path interpolation diverge visibly in the interior.
    const minDepth = 0;
    const maxDepth = 50;

    const { container } = render(
      <DepthLegend
        minDepth={minDepth}
        maxDepth={maxDepth}
        shallowColor={shallowColor}
        deepColor={deepColor}
        visible
      />
    );

    const stops = Array.from(container.querySelectorAll('stop'));
    expect(stops.length).toBeGreaterThan(2);

    for (const stop of stops) {
      const offsetPct = parseFloat(stop.getAttribute('offset') ?? '0');
      const t = offsetPct / 100;
      const depth = minDepth + t * (maxDepth - minDepth);
      const expected = getDepthColor(depth, shallowColor, deepColor, minDepth, maxDepth);
      expect(stop.getAttribute('stop-color')).toBe(expected);
    }
  });

  // Ported from e2e-playwright/depth-gradient.spec.ts (#576 E17-D1): the labels and the
  // accessible name are the component's own rendering, so they are pinned here, exactly,
  // where the browser spec could only pattern-match them. The spec keeps the in-app
  // show/hide and the painted-pixel checks.
  it('labels the shallow and the deep end with their depths in metres', () => {
    render(<DepthLegend minDepth={2} maxDepth={12.5} shallowColor="#a89060" deepColor="#b85048" visible />);

    expect(screen.getByTestId('depth-legend-min')).toHaveTextContent(/^2\.0m$/);
    expect(screen.getByTestId('depth-legend-max')).toHaveTextContent(/^12\.5m$/);
    // A non-flat range has no "Flat" note between the two.
    expect(screen.getByTestId('depth-legend')).not.toHaveTextContent('Flat');
  });

  it('is an image whose accessible name states the depth range', () => {
    render(<DepthLegend minDepth={2} maxDepth={12.5} shallowColor="#a89060" deepColor="#b85048" visible />);

    expect(screen.getByRole('img')).toHaveAccessibleName('Depth scale from 2.0m (shallow) to 12.5m (deep)');
  });

  it('marks a flat range and renders nothing while hidden', () => {
    const { rerender } = render(
      <DepthLegend minDepth={4} maxDepth={4} shallowColor="#a89060" deepColor="#b85048" visible />
    );
    expect(screen.getByTestId('depth-legend')).toHaveTextContent('Flat');

    rerender(<DepthLegend minDepth={4} maxDepth={4} shallowColor="#a89060" deepColor="#b85048" visible={false} />);
    expect(screen.queryByTestId('depth-legend')).toBeNull();
  });
});
