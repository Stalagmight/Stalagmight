import { useId, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { getMapThemeColors, getThemeColor } from '../../utils/themeColors';
import { useUIStore } from '../../store/uiStore';
import { useColorScaleStore } from '../../store/colorScaleStore';
import { useEffectivePalette, useHiddenIssues } from '../../store/colorScalePalette';
import type { SurveyData } from '../../types/survey-data';
import type { LoopClosure } from '../../types/project';
import { issueScoreColor } from './surveyLayer/issueColor';
import { LENGTH_DELTA_MIN_INCLINATION_DEG, SCORE_MAX, SCORE_WARNING } from './surveyLayer/shotQuality';
import { ISSUE_METRICS, positionToScore, scoreToPosition } from './surveyLayer/issueScale';
import { issuePaletteFor } from './colorScale/useIssueScale';
import { LegendCogPopover } from './colorScale/LegendCogPopover';
import { IssueScaleEditor } from './colorScale/IssueScaleEditor';

interface IssueLegendProps {
  /** Survey Issues paints the map: show the colour ramp. */
  showRamp: boolean;
  /** The Errors labels are on: show what their abbreviations mean. */
  showKey: boolean;
  /** CSS selector of another bottom-left legend currently on screen (depth /
   *  distance); the key then sits to its right instead of on top of it. */
  besideSelector?: string | null;
  /** The survey and its loop closures, for the cog's histograms. */
  surveyData?: SurveyData | null;
  closures?: readonly LoopClosure[] | undefined;
}

const BAR_HEIGHT = 90;
const RAMP_STOPS = 16;
const POPOVER_WIDTH = 340;
const NO_CLOSURES: readonly LoopClosure[] = [];

/** Vertical position (px from the top of the bar) of a score: the palette's
 *  end (3) at the top, its middle (the warn level) halfway. */
const yOf = (score: number) => BAR_HEIGHT * (1 - scoreToPosition(score));

/**
 * The Survey Issues legend, bottom-left like the depth / distance legends,
 * with the cog that opens the Survey Issues colour settings:
 *  - the ramp: the palette's start where a shot's readings agree, its middle at
 *    an error's warn level, its end at the max level or for an error;
 *  - the Errors labels key: what Δaz, Δinc, Δi/d, ΔL and the closure label mean
 *    (only the errors that are switched on).
 */
export function IssueLegend({
  showRamp,
  showKey,
  besideSelector = null,
  surveyData = null,
  closures = NO_CLOSURES,
}: IssueLegendProps) {
  const gradientId = useId();
  const { t } = useTranslation();
  // Re-render on theme change so text colours stay correct.
  useUIStore((s) => s.resolvedTheme);
  const choice = useEffectivePalette('issues');
  const hidden = useHiddenIssues();
  const setPalettePreview = useColorScaleStore((s) => s.setPalettePreview);
  const setIssueLevels = useColorScaleStore((s) => s.setIssueLevels);
  const textColor = getThemeColor('--cc-depth-text');
  const textHaloAlt = getThemeColor('--cc-text-halo-alt');
  const left = useBesideOffset(besideSelector);

  if (!showRamp && !showKey) return null;
  const theme = getMapThemeColors();
  const palette = issuePaletteFor(choice);
  const abbrev = (key: string) => t(`map.errorLabels.abbrev.${key}`);
  const keyRows = ISSUE_METRICS.filter((id) => !hidden.includes(id)).map(
    (id): [string, string, string] => [id, abbrev(id), t(`map.issueLegend.key.${id}`, { degrees: LENGTH_DELTA_MIN_INCLINATION_DEG })],
  );
  const stops = Array.from({ length: RAMP_STOPS + 1 }, (_, k) => k / RAMP_STOPS);

  // End a preview the popover started (a hovered palette, a dragged handle).
  const endPreview = () => {
    if (useColorScaleStore.getState().palettePreview?.kind === 'issues') setPalettePreview(null);
    const drag = useColorScaleStore.getState().issuePreview;
    if (drag) setIssueLevels(drag.id, useColorScaleStore.getState().issueLevels[drag.id] ?? null, false);
  };

  return (
    <div
      data-testid="issue-legend-slot"
      data-legend-slot
      className="flex items-start gap-1"
      style={{ position: 'absolute', bottom: 50, left: 10 + left, zIndex: 1 }}
    >
      <div
        data-testid="issue-legend"
        style={{
          background: textHaloAlt,
          borderRadius: 4,
          padding: 8,
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
                    {stops.map((p) => (
                      <stop
                        key={p}
                        offset={`${(p * 100).toFixed(2)}%`}
                        stopColor={issueScoreColor(theme, positionToScore(1 - p), palette)}
                      />
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
            {keyRows.length === 0 && <span data-testid="issue-legend-key-none">{t('map.issueLegend.allOff')}</span>}
            {keyRows.map(([id, abbr, meaning]) => (
              <span key={id} data-testid={`issue-legend-key-${id}`}>
                <span style={{ fontWeight: 600 }}>{abbr}</span> {meaning}
              </span>
            ))}
          </div>
        )}
      </div>
      <LegendCogPopover id="issues" title={t('map.issueScale.title')} width={POPOVER_WIDTH} onClose={endPreview}>
        {(popRef) => <IssueScaleEditor surveyData={surveyData} closures={closures} pickerPortalInto={popRef} />}
      </LegendCogPopover>
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
    const found = selector ? document.querySelector<HTMLElement>(selector) : null;
    // The legend's whole slot when it has one, so its cog is cleared too.
    const el = found?.closest<HTMLElement>('[data-legend-slot]') ?? found;
    const measure = () => setOffset(el ? el.offsetWidth + 8 : 0);
    measure();
    if (!el || typeof ResizeObserver === 'undefined') return undefined;
    observer.current = new ResizeObserver(measure);
    observer.current.observe(el);
    return () => observer.current?.disconnect();
  }, [selector]);
  return offset;
}
