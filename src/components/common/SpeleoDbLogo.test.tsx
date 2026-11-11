/**
 * Component test (#357 item 11): the untinted SpeleoDB mark must stay legible
 * on light chrome, not just dark chrome.
 *
 * Before this fix, an untinted mark rendered as a bare `<img>` of white line
 * art on a transparent PNG, which vanished against any light theme
 * (`kaleido_eink`, and PR #361's `light`). No prior test pinned the untinted
 * colour, which is exactly why that shipped. This asserts the untinted mark
 * takes the SAME mask-over-solid-colour path as the tinted mark, with the
 * solid colour set to `currentColor` so it follows the surrounding text
 * colour on any theme, instead of being a bare `<img>` whose colour cannot
 * follow the theme at all.
 */
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';

import { SpeleoDbLogo, speleoLogoMask } from './SpeleoDbLogo';

describe('SpeleoDbLogo untinted mark (#357 item 11)', () => {
  it('renders via the mask path and inherits currentColor, not a bare <img>', () => {
    render(<SpeleoDbLogo />);
    // A bare <img> (the old, invisible-on-light path) must not be present.
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
    const mark = document.querySelector('[style*="mask-image"]') as HTMLElement | null;
    expect(mark).not.toBeNull();
    // Check the authored inline style, not the computed style: jsdom RESOLVES
    // `currentcolor` to an actual rgb() in `getComputedStyle`, which would pass
    // for any solid colour and defeat the point of this assertion.
    expect(mark?.style.backgroundColor).toBe('currentcolor');
  });

  it('has no tinted testid/attribute when untinted, preserving the #270 contract', () => {
    render(<SpeleoDbLogo />);
    expect(screen.queryByTestId('speleodb-logo-tinted')).not.toBeInTheDocument();
  });

  it('still takes the tinted colour and testid/attribute when a tint is given', () => {
    render(<SpeleoDbLogo tint="#ff0" />);
    const mark = screen.getByTestId('speleodb-logo-tinted');
    expect(mark).toHaveAttribute('data-tint', '#ff0');
    expect(mark).toHaveStyle({ backgroundColor: '#ff0' });
  });

  it('uses a caller-qualified testid when supplied', () => {
    render(<SpeleoDbLogo tint="#ff0" testId="recent-project-logo" />);
    expect(screen.getByTestId('recent-project-logo')).toHaveAttribute('data-tint', '#ff0');
    expect(screen.queryByTestId('speleodb-logo-tinted')).not.toBeInTheDocument();
  });
});

/**
 * The 128px line art shrunk 8x by the browser's mask sampling rendered as noise at
 * tab and button sizes. The mark now draws from the smallest pre-downscaled mask
 * that still covers the device pixels it occupies (size x devicePixelRatio, which
 * the UI zoom and the display both move).
 */
describe('SpeleoDbLogo mask size', () => {
  const maskOf = () => (document.querySelector('[style*="mask-image"]') as HTMLElement).style.maskImage;

  it.each([
    [16, 1, 'speleodb-logo-16'],
    [18, 1, 'speleodb-logo-24'],
    [16, 1.27, 'speleodb-logo-24'],
    [16, 0.8, 'speleodb-logo-16'],
    [16, 2, 'speleodb-logo-32'],
    [56, 1, 'speleodb-logo-64'],
    [56, 2, 'speleodb-logo.png'],
    [56, 3, 'speleodb-logo.png'],
  ])('size %i at %s device px per CSS px draws from %s', (size, dpr, asset) => {
    expect(speleoLogoMask(size * dpr)).toContain(asset);
  });

  it('follows the current device pixel ratio when it renders', () => {
    const original = window.devicePixelRatio;
    Object.defineProperty(window, 'devicePixelRatio', { configurable: true, value: 2 });
    try {
      render(<SpeleoDbLogo size={16} />);
      expect(maskOf()).toContain('speleodb-logo-32');
    } finally {
      Object.defineProperty(window, 'devicePixelRatio', { configurable: true, value: original });
    }
  });

  it('every small mask is the original art, box-filtered: same aspect and a sane size', async () => {
    const fs = await import('node:fs');
    const path = await import('node:path');
    for (const px of [16, 24, 32, 48, 64]) {
      const file = path.resolve(__dirname, `../../assets/speleodb-logo-${px}.png`);
      const png = fs.readFileSync(file);
      // PNG IHDR: width and height, big-endian, at bytes 16..24.
      expect([png.readUInt32BE(16), png.readUInt32BE(20)]).toEqual([px, px]);
      expect(png.length).toBeLessThan(8 * 1024);
    }
  });
});
