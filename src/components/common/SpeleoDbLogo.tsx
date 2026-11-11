import { useSyncExternalStore } from 'react';
import logo16 from '../../assets/speleodb-logo-16.png';
import logo24 from '../../assets/speleodb-logo-24.png';
import logo32 from '../../assets/speleodb-logo-32.png';
import logo48 from '../../assets/speleodb-logo-48.png';
import logo64 from '../../assets/speleodb-logo-64.png';
import logo128 from '../../assets/speleodb-logo.png';
import { watchDevicePixelRatio } from '../../utils/dprWatcher';

/**
 * The mark's mask at the sizes it is drawn at, smallest first. The 128px original
 * is detailed line art; shrunk 8x by the browser's mask sampling (no mipmaps) at
 * the 16-18px of a tab or a button it came out as speckled noise. The smaller
 * ones are the same art box-filtered down offline (exact pixel coverage, no new
 * drawing), so the browser only ever scales a mask by a small factor.
 */
const LOGO_MASKS: ReadonlyArray<readonly [px: number, url: string]> = [
  [16, logo16], [24, logo24], [32, logo32], [48, logo48], [64, logo64], [128, logo128],
];

/** The smallest mask at least `devicePx` device pixels wide (the largest past 128). */
export function speleoLogoMask(devicePx: number): string {
  const fit = LOGO_MASKS.find(([px]) => px >= devicePx - 0.01);
  return fit ? fit[1] : logo128;
}

const readDpr = (): number => (typeof window === 'undefined' ? 1 : window.devicePixelRatio || 1);

/**
 * The official SpeleoDB brand mark (the white line-art cave scene), from the
 * SpeleoDB repo's app icon. It's raster line art (no vector source exists), so it
 * ships pre-downscaled masks and picks the one closest above the device pixels it
 * covers (see `LOGO_MASKS`). Decorative - callers provide the accessible label.
 *
 * The asset is a PNG - white pixels on a fully transparent background - so it
 * is always recoloured by using its ALPHA as a mask over a solid colour block,
 * which reproduces the line art exactly in the chosen colour. A CSS `filter`
 * chain would only approximate it, and there is no vector source to
 * `currentColor` directly, hence the mask. `tint` recolours the mark (#270:
 * yellow when the project has changes that are only on this computer); when
 * omitted, the mark inherits `currentColor` so it stays legible against
 * whatever chrome (light or dark) it's placed on.
 */
export function SpeleoDbLogo({
  size = 18,
  className,
  tint,
  testId,
}: {
  size?: number;
  className?: string;
  /** CSS colour to paint the mark in (e.g. `UNPUSHED_ACCENT`); `currentColor` when omitted. */
  tint?: string | undefined;
  /** Caller-qualified ID for repeated instances; tinted standalone marks keep the legacy default. */
  testId?: string | undefined;
}) {
  const resolvedTestId = testId ?? (tint ? 'speleodb-logo-tinted' : undefined);
  // The UI zoom and the display both change the device pixels per CSS pixel.
  const dpr = useSyncExternalStore(watchDevicePixelRatio, readDpr, () => 1);
  const logoUrl = speleoLogoMask(size * dpr);

  return (
    <span
      className={className}
      aria-hidden="true"
      {...(resolvedTestId ? { 'data-testid': resolvedTestId } : {})}
      {...(tint ? { 'data-tint': tint } : {})}
      style={{
        display: 'inline-block',
        width: size,
        height: size,
        backgroundColor: tint ?? 'currentColor',
        maskImage: `url(${logoUrl})`,
        WebkitMaskImage: `url(${logoUrl})`,
        maskSize: 'contain',
        WebkitMaskSize: 'contain',
        maskRepeat: 'no-repeat',
        WebkitMaskRepeat: 'no-repeat',
        maskPosition: 'center',
        WebkitMaskPosition: 'center',
      }}
    />
  );
}
