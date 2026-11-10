import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { formatFixed } from '../../../i18n/format';
import {
  autoScale,
  clampGamma,
  gammaFromFraction,
  midValue,
  scaleHex,
  type ColorScaleInput,
  type LevelsScale,
} from '../../../utils/palettes';

type Handle = 'min' | 'mid' | 'max';

export interface LevelsSliderProps {
  /** The data's own extent: the auto range and the slider's base domain. */
  domain: { min: number; max: number };
  /** The explicit range, or null for auto (the data extent, linear). */
  scale: LevelsScale | null;
  /** Palette + scale, for the colour bar under the histogram. */
  colorScale: ColorScaleInput;
  /** Bin counts over `domain`, or null while there is nothing to bin. */
  histogram: readonly number[] | null;
  /** Unit suffix for the value read-outs, e.g. "m". */
  unit: string;
  precision: number;
  /** A new range. `live` is true while a handle is being dragged; the final
   *  call of a drag (pointer up) passes false. */
  onChange: (scale: LevelsScale, live: boolean) => void;
  /** Accessible name for the histogram, e.g. "Histogram of depth values". */
  histogramLabel: string;
  testIdPrefix: string;
}

const KEY_STEP = 0.01; // of the domain per arrow press (x10 with Shift)

/**
 * A Photoshop-Levels-style range control: a histogram of the values over a
 * colour bar, with three triangle handles underneath - range start, midpoint
 * (gamma) and range end - and the same three as number fields. Values outside
 * the range take the palette's end colours, as Levels clips to black / white.
 */
export function LevelsSlider({
  domain,
  scale,
  colorScale,
  histogram,
  unit,
  precision,
  onChange,
  histogramLabel,
  testIdPrefix,
}: LevelsSliderProps) {
  const { t } = useTranslation();
  const trackRef = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<Handle | null>(null);
  const eff = scale ?? autoScale(domain);

  // The slider shows the data extent, widened to an explicit range beyond it.
  let viewMin = Math.min(domain.min, eff.min);
  let viewMax = Math.max(domain.max, eff.max);
  if (!(viewMax > viewMin)) {
    viewMin -= 0.5;
    viewMax += 0.5;
  }
  const span = viewMax - viewMin;
  const pct = (v: number) => ((v - viewMin) / span) * 100;
  const minGap = span / 1000;

  const barGradient = useMemo(() => {
    const parts: string[] = [];
    const n = 48;
    for (let i = 0; i <= n; i++) {
      const v = viewMin + (span * i) / n;
      parts.push(`${scaleHex(v, colorScale, domain) ?? '#808080'} ${((i / n) * 100).toFixed(2)}%`);
    }
    return `linear-gradient(to right, ${parts.join(', ')})`;
  }, [viewMin, span, colorScale, domain]);

  const peak = histogram ? Math.max(1, ...histogram) : 1;

  const valueAt = (clientX: number): number => {
    const rect = trackRef.current?.getBoundingClientRect();
    if (!rect || rect.width <= 0) return eff.min;
    const f = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
    return viewMin + f * span;
  };

  const moved = (handle: Handle, value: number): LevelsScale => {
    if (handle === 'min') return { ...eff, min: Math.min(value, eff.max - minGap) };
    if (handle === 'max') return { ...eff, max: Math.max(value, eff.min + minGap) };
    return { ...eff, gamma: gammaFromFraction((value - eff.min) / (eff.max - eff.min)) };
  };

  const onPointerDown = (handle: Handle) => (e: PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.stopPropagation();
    e.currentTarget.setPointerCapture?.(e.pointerId);
    e.currentTarget.focus();
    setDrag(handle);
    onChange(eff, true);
  };
  const onPointerMove = (handle: Handle) => (e: PointerEvent<HTMLDivElement>) => {
    if (drag !== handle) return;
    onChange(moved(handle, valueAt(e.clientX)), true);
  };
  const onPointerUp = (handle: Handle) => (e: PointerEvent<HTMLDivElement>) => {
    if (drag !== handle) return;
    e.currentTarget.releasePointerCapture?.(e.pointerId);
    setDrag(null);
    onChange(moved(handle, valueAt(e.clientX)), false);
  };

  const onKeyDown = (handle: Handle) => (e: KeyboardEvent<HTMLDivElement>) => {
    const step = span * KEY_STEP * (e.shiftKey ? 10 : 1);
    const current = handle === 'min' ? eff.min : handle === 'max' ? eff.max : midValue(eff);
    let next: number | null = null;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowDown') next = current - step;
    else if (e.key === 'ArrowRight' || e.key === 'ArrowUp') next = current + step;
    else if (e.key === 'Home') next = handle === 'max' ? eff.min : viewMin;
    else if (e.key === 'End') next = handle === 'min' ? eff.max : viewMax;
    if (next === null) return;
    e.preventDefault();
    e.stopPropagation();
    onChange(moved(handle, next), false);
  };

  const fmt = (v: number) => formatFixed(v, undefined, precision);
  const handles: Array<{ id: Handle; value: number; label: string; valueText: string }> = [
    { id: 'min', value: eff.min, label: t('map.colorScale.minHandle'), valueText: `${fmt(eff.min)}${unit}` },
    { id: 'mid', value: midValue(eff), label: t('map.colorScale.midHandle'), valueText: eff.gamma.toFixed(2) },
    { id: 'max', value: eff.max, label: t('map.colorScale.maxHandle'), valueText: `${fmt(eff.max)}${unit}` },
  ];

  return (
    <div className="flex flex-col gap-1" data-testid={`${testIdPrefix}-slider`}>
      <div ref={trackRef} className="relative mx-1.5">
        <svg
          className="block w-full text-text-secondary"
          height={44}
          viewBox="0 0 100 44"
          preserveAspectRatio="none"
          role="img"
          aria-label={histogramLabel}
          data-testid={`${testIdPrefix}-histogram`}
        >
          {histogram?.map((count, k) => {
            if (count === 0) return null;
            const binW = (domain.max - domain.min) / histogram.length;
            const from = domain.min + binW * k;
            const x = pct(from);
            const w = Math.max(0.15, pct(from + binW) - x);
            const h = Math.max(1, (count / peak) * 42);
            const inside = from + binW > eff.min && from < eff.max;
            return (
              <rect
                key={k}
                x={x}
                y={44 - h}
                width={w}
                height={h}
                fill="currentColor"
                opacity={inside ? 0.85 : 0.3}
              />
            );
          })}
        </svg>
        <div
          className="h-2.5 rounded-sm border border-border"
          style={{ background: barGradient }}
          data-testid={`${testIdPrefix}-bar`}
        />
        <div className="relative h-3.5">
          {handles.map((h) => (
            <div
              key={h.id}
              role="slider"
              tabIndex={0}
              aria-label={h.label}
              aria-valuemin={viewMin}
              aria-valuemax={viewMax}
              aria-valuenow={h.value}
              aria-valuetext={h.valueText}
              className="absolute top-0 -translate-x-1/2 cursor-ew-resize focus:outline-none focus-visible:ring-1 focus-visible:ring-accent rounded-sm"
              style={{ left: `${pct(h.value)}%`, touchAction: 'none' }}
              onPointerDown={onPointerDown(h.id)}
              onPointerMove={onPointerMove(h.id)}
              onPointerUp={onPointerUp(h.id)}
              onPointerCancel={onPointerUp(h.id)}
              onKeyDown={onKeyDown(h.id)}
              data-testid={`${testIdPrefix}-${h.id}-handle`}
            >
              <svg width="11" height="10" viewBox="0 0 11 10" aria-hidden="true">
                <path
                  d="M5.5 0 L11 10 L0 10 Z"
                  className={
                    h.id === 'min'
                      ? 'fill-text-primary stroke-text-primary'
                      : h.id === 'mid'
                        ? 'fill-text-secondary stroke-text-primary'
                        : 'fill-panel stroke-text-primary'
                  }
                  strokeWidth={1}
                />
              </svg>
            </div>
          ))}
        </div>
      </div>
      <LevelsFields
        scale={eff}
        unit={unit}
        precision={precision}
        onChange={(next) => onChange(next, false)}
        testIdPrefix={testIdPrefix}
      />
      <span className="text-[10px] text-text-secondary self-center" data-testid={`${testIdPrefix}-mid-value`}>
        {t('map.colorScale.midValue', { value: `${fmt(midValue(eff))}${unit}` })}
      </span>
    </div>
  );
}

interface LevelsFieldsProps {
  scale: LevelsScale;
  unit: string;
  precision: number;
  onChange: (scale: LevelsScale) => void;
  testIdPrefix: string;
}

/** Min / Mid (gamma) / Max number fields. Commit on Enter or blur; an invalid
 *  entry (min not below max, not a number) reverts. */
/** Field text: plain `toFixed`, not locale formatting, so what the field shows
 *  always parses back with `Number()` (no thousands separators, `.` decimal). */
function draftOf(scale: LevelsScale, precision: number) {
  const digits = Math.max(0, Math.min(6, Math.floor(precision)));
  return {
    min: scale.min.toFixed(digits),
    gamma: scale.gamma.toFixed(2),
    max: scale.max.toFixed(digits),
  };
}

function LevelsFields({ scale, unit, precision, onChange, testIdPrefix }: LevelsFieldsProps) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState(() => draftOf(scale, precision));
  // Keyed on the numbers, not the object: an auto range is a fresh object every
  // render, and resetting the draft on each would eat the user's typing.
  const { min, max, gamma } = scale;
  useEffect(() => {
    setDraft(draftOf({ min, max, gamma }, precision));
  }, [min, max, gamma, precision]);

  const commit = () => {
    const min = Number(draft.min);
    const max = Number(draft.max);
    const gamma = Number(draft.gamma);
    if (!Number.isFinite(min) || !Number.isFinite(max) || !Number.isFinite(gamma) || !(max > min)) {
      setDraft(draftOf(scale, precision));
      return;
    }
    const next = { min, max, gamma: clampGamma(gamma) };
    if (next.min !== scale.min || next.max !== scale.max || next.gamma !== scale.gamma) onChange(next);
    else setDraft(draftOf(scale, precision));
  };

  const field = (key: 'min' | 'gamma' | 'max', label: string, title?: string) => (
    <label className="flex flex-col gap-0.5 min-w-0 flex-1" title={title}>
      <span className="text-[10px] text-text-secondary">
        {label}
        {key !== 'gamma' && unit ? ` (${unit})` : ''}
      </span>
      <input
        type="text"
        inputMode="decimal"
        value={draft[key]}
        onChange={(e) => setDraft((d) => ({ ...d, [key]: e.target.value }))}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault();
            commit();
          } else if (e.key === 'Escape' && draft[key] !== draftOf(scale, precision)[key]) {
            // Escape first reverts an unsaved edit; a second Escape closes the popover.
            e.preventDefault();
            e.stopPropagation();
            setDraft(draftOf(scale, precision));
          }
        }}
        className="w-full px-1.5 py-0.5 bg-background border border-border rounded text-xs text-right focus:outline-none focus:ring-1 focus:ring-accent"
        data-testid={`${testIdPrefix}-${key}-input`}
      />
    </label>
  );

  return (
    <div className="flex gap-2">
      {field('min', t('map.colorScale.min'))}
      {field('gamma', t('map.colorScale.gamma'), t('map.colorScale.gammaHelp'))}
      {field('max', t('map.colorScale.max'))}
    </div>
  );
}
