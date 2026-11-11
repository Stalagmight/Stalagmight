import { useId, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { getMapThemeColors, getThemeColor } from '../../utils/themeColors';
import { useUIStore } from '../../store/uiStore';
import { issueScoreColor } from './surveyLayer/issueColor';
import { LENGTH_DELTA_MIN_INCLINATION_DEG, SCORE_MAX, SCORE_WARNING } from './surveyLayer/shotQuality';

interface IssueLegendProps {
  /** Survey Issues paints the map: show the colour ramp. */
  showRamp: boolean;
  /** The Errors labels are on: show what their abbreviations mean. */
  showKey: boolean;
  /** CSS selector of another bottom-left legend currently on screen (depth /
   *  distance); the key then sits to its right instead of on top of it. */
  besideSelector?: string | null;
}

const RAMP_STOPS = [0, 0.5, 1, 2, 3];
const BAR_HEIGHT = 90;

/** Vertical position (px from the top of the bar) of a score; 3 is at the top. */
const yOf = (score: number) => BAR_HEIGHT * (1 - score / SCORE_MAX);

/**
 * The Survey Issues legend, bottom-left like the depth / distance legends:
 *  - the ramp: green where a shot's readings agree, yellow at a warning
 *    threshold, red at three times it or for an error;
 *  - the Errors labels key: what Δaz, Δinc, Δi/d, ΔL and the closure label mean.
 */
export function IssueLegend({ showRamp, showKey, besideSelector = null }: IssueLegendProps) {
  const gradientId = useId();
  const { t } = useTranslation();
  // Re-render on theme change so text colours stay correct.
  useUIStore((s) => s.resolvedTheme);
  const textColor = getThemeColor('--cc-depth-text');
  const textHaloAlt = getThemeColor('--cc-text-halo-alt');
  const left = useBesideOffset(besideSelector);

  if (!showRamp && !showKey) return null;
  const theme = getMapThemeColors();
  const abbrev = (key: string) => t(`map.errorLabels.abbrev.${key}`);
  const keyRows: [string, string][] = [
    [abbrev('azimuth'), t('map.issueLegend.key.azimuth')],
    [abbrev('inclination'), t('map.issueLegend.key.inclination')],
    [abbrev('inclinationDepth'), t('map.issueLegend.key.inclinationDepth')],
    [abbrev('length'), t('map.issueLegend.key.length', { degrees: LENGTH_DELTA_MIN_INCLINATION_DEG })],
    [abbrev('closure'), t('map.issueLegend.key.closure')],
  ];

  return (
    <div
      data-testid="issue-legend"
      style={{
        position: 'absolute',
        bottom: 50,
        left: 10 + left,
        background: textHaloAlt,
        borderRadius: 4,
        padding: 8,
        zIndex: 1,
        display: 'flex',
        flexDirection: 'column',
        gap: 8,
        maxWidth: 200,
        color: textColor,
      }}
    >
      {showRamp && (
        <div data-testid="issue-legend-ramp" style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
          <span style={{ fontSize: 11, fontWeight: 600 }}>{t('map.issueLegend.title')}</span>
          <div style={{ display: 'flex', gap: 6 }}>
            <svg width="16" height={BAR_HEIGHT} role="img" aria-label={t('map.issueLegend.aria')}>
              <defs>
                <linearGradient id={gradientId} x1="0%" y1="0%" x2="0%" y2="100%">
                  {RAMP_STOPS.slice()
                    .reverse()
                    .map((s) => (
                      <stop key={s} offset={`${(1 - s / SCORE_MAX) * 100}%`} stopColor={issueScoreColor(theme, s)} />
                    ))}
                </linearGradient>
              </defs>
              <rect width="16" height={BAR_HEIGHT} fill={`url(#${gradientId})`} rx="2" />
            </svg>
            <div style={{ position: 'relative', height: BAR_HEIGHT, minWidth: 112, fontSize: 10 }}>
              {[
                [SCORE_MAX, t('map.issueLegend.error'), 'issue-legend-error'],
                [SCORE_WARNING, t('map.issueLegend.warning'), 'issue-legend-warning'],
                [0, t('map.issueLegend.clear'), 'issue-legend-clear'],
              ].map(([score, label, testId]) => (
                <span
                  key={testId as string}
                  data-testid={testId as string}
                  style={{
                    position: 'absolute',
                    left: 0,
                    top: Math.min(Math.max(yOf(score as number) - 7, 0), BAR_HEIGHT - 14),
                    whiteSpace: 'nowrap',
                    textShadow: `0 1px 2px ${textHaloAlt}`,
                  }}
                >
                  {label}
                </span>
              ))}
            </div>
          </div>
        </div>
      )}
      {showKey && (
        <div data-testid="issue-legend-key" style={{ display: 'flex', flexDirection: 'column', gap: 2, fontSize: 10 }}>
          <span style={{ fontSize: 11, fontWeight: 600 }}>{t('map.issueLegend.keyTitle')}</span>
          {keyRows.map(([abbr, meaning]) => (
            <span key={abbr}>
              <span style={{ fontWeight: 600 }}>{abbr}</span> {meaning}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * The selector of the other bottom-left legend on screen, if any, for
 * `besideSelector`. Each state names its exact element (the loading cue and the
 * finished legend are different nodes), so a swap between them changes the
 * selector and the offset is measured again.
 */
export function otherLegendSelector(on: {
  depth: boolean;
  distance: boolean;
  distanceReady: boolean;
  gas: boolean;
  gasReady: boolean;
}): string | null {
  if (on.gas) return on.gasReady ? '[data-testid="gas-legend"]' : '[data-testid="gas-legend-loading"]';
  if (on.distance) return on.distanceReady ? '[data-testid="distance-legend"]' : '[data-testid="distance-legend-loading"]';
  if (on.depth) return '[data-testid="depth-legend"]';
  return null;
}

/** Width (+ gap) of the legend matched by `selector`, kept current while it is
 *  on screen; 0 when there is none. */
function useBesideOffset(selector: string | null): number {
  const [offset, setOffset] = useState(0);
  const observer = useRef<ResizeObserver | null>(null);
  useLayoutEffect(() => {
    const el = selector ? document.querySelector<HTMLElement>(selector) : null;
    const measure = () => setOffset(el ? el.offsetWidth + 8 : 0);
    measure();
    if (!el || typeof ResizeObserver === 'undefined') return undefined;
    observer.current = new ResizeObserver(measure);
    observer.current.observe(el);
    return () => observer.current?.disconnect();
  }, [selector]);
  return offset;
}
