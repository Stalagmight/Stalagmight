import { useId, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { formatFixed } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { getDepthColor } from '../../utils/colors';
import { getThemeColor } from '../../utils/themeColors';
import { useUIStore } from '../../store/uiStore';
import { effectiveScale, legendGradientStops, type ColorScaleInput } from '../../utils/palettes/scale';

interface DepthLegendProps {
  minDepth: number;
  maxDepth: number;
  shallowColor?: string;
  deepColor?: string;
  visible: boolean;
  /** The user's palette + Levels range. Null / absent = the theme ramp over
   *  min..max (the original stops below). The bar then spans the scale's range. */
  colorScale?: ColorScaleInput | null;
  /** Laid out by a wrapper (MapColorLegends) instead of positioning itself. */
  embedded?: boolean;
}

/**
 * Number of color stops to generate for the gradient.
 * More stops = smoother gradient that matches HSL interpolation.
 */
const GRADIENT_STOPS = 10;

/**
 * Shows a depth color gradient legend with min/max labels.
 * Positioned in the bottom-left corner of the map, above the scale bar.
 * Uses the same HSL color interpolation as the map display.
 */
export function DepthLegend({
  minDepth,
  maxDepth,
  shallowColor,
  deepColor,
  visible,
  colorScale = null,
  embedded = false,
}: DepthLegendProps) {
  const gradientId = useId();
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  // Subscribe to theme for reactive color updates (value triggers re-render)
  useUIStore((s) => s.resolvedTheme);
  // Fall back to the LIVE token, not the dark one: the legend has to show the
  // gradient the canvas actually drew, and getMapThemeColors now reads these per
  // theme. A hardcoded dark fallback would make the legend disagree with the map
  // on any theme that declares its own endpoints.
  const effectiveShallow = shallowColor ?? getThemeColor('--cc-depth-shallow');
  const effectiveDeep = deepColor ?? getThemeColor('--cc-depth-deep');
  const depthTextColor = getThemeColor('--cc-depth-text');
  const textHaloAlt = getThemeColor('--cc-text-halo-alt');

  // With a user scale the bar spans that scale's range and its palette.
  const scaled = colorScale ? effectiveScale(colorScale, { min: minDepth, max: maxDepth }) : null;
  const shownMin = scaled ? scaled.min : minDepth;
  const shownMax = scaled ? scaled.max : maxDepth;

  // Generate gradient stops using the same getDepthColor function as the map
  const gradientStops = useMemo(() => {
    if (colorScale) return legendGradientStops(colorScale, { min: minDepth, max: maxDepth });
    const isFlat = minDepth === maxDepth;
    if (isFlat) {
      return [{ offset: 0, color: effectiveShallow }, { offset: 100, color: effectiveShallow }];
    }

    const stops: { offset: number; color: string }[] = [];
    for (let i = 0; i <= GRADIENT_STOPS; i++) {
      const t = i / GRADIENT_STOPS;
      const depth = minDepth + t * (maxDepth - minDepth);
      const color = getDepthColor(depth, effectiveShallow, effectiveDeep, minDepth, maxDepth) ?? effectiveShallow;
      stops.push({ offset: t * 100, color });
    }
    return stops;
  }, [minDepth, maxDepth, effectiveShallow, effectiveDeep, colorScale]);

  if (!visible) return null;

  const isFlat = shownMin === shownMax;

  return (
    <div
      data-testid="depth-legend"
      className="depth-legend"
      style={{
        ...(embedded ? {} : { position: 'absolute' as const, bottom: 50, left: 10, zIndex: 1 }),
        background: textHaloAlt,
        borderRadius: 4,
        padding: 8,
        display: 'flex',
        gap: 6,
      }}
    >
      <svg
        width="16"
        height="80"
        role="img"
        aria-label={t('map.depthLegend.aria', {
          min: `${formatFixed(shownMin, undefined, precision.depth)}${t('common.units.meters')}`,
          max: `${formatFixed(shownMax, undefined, precision.depth)}${t('common.units.meters')}`,
        })}
      >
        <defs>
          <linearGradient id={gradientId} x1="0%" y1="0%" x2="0%" y2="100%">
            {gradientStops.map((stop, i) => (
              <stop key={i} offset={`${stop.offset}%`} stopColor={stop.color} />
            ))}
          </linearGradient>
        </defs>
        <rect width="16" height="80" fill={`url(#${gradientId})`} rx="2" />
      </svg>
      <div
        className="legend-labels"
        style={{
          display: 'flex',
          flexDirection: 'column',
          justifyContent: 'space-between',
          fontSize: 11,
          color: depthTextColor,
          textShadow: `0 1px 2px ${textHaloAlt}`,
        }}
      >
        <span data-testid="depth-legend-min">{`${formatFixed(shownMin, undefined, precision.depth)}${t('common.units.meters')}`}</span>
        {isFlat && (
          <span style={{ fontSize: 10, opacity: 0.7 }}>
            {t('map.depthLegend.flat')}
          </span>
        )}
        <span data-testid="depth-legend-max">{`${formatFixed(shownMax, undefined, precision.depth)}${t('common.units.meters')}`}</span>
      </div>
    </div>
  );
}
