import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { formatFixed } from '../../../i18n/format';
import { MAX_LEVEL, validLevels, type IssueLevels } from '../surveyLayer/issueScale';

type Handle = 'min' | 'warn' | 'max';
const HANDLES: readonly Handle[] = ['min', 'warn', 'max'];

export interface IssueLevelsSliderProps {
  /** The levels shown, in display units (a drag in progress included). */
  levels: IssueLevels;
  /** The slider spans 0 to this (widened to `levels.max` beyond it). */
  domainMax: number;
  /** Bin counts over [0, domainMax], or null for no histogram. */
  histogram: readonly number[] | null;
  /** The map colour of a value, for the bar under the histogram. */
  colorAt: (value: number) => string;
  /** Unit suffix, e.g. "°", "m", "%". */
  unit: string;
  precision: number;
  /** The error is switched off: shown greyed, without its histogram. */
  disabled: boolean;
  /** New levels. `live` while a handle is mid-drag; the drag's last call
   *  (pointer up, or Escape restoring where it began) passes false. */
  onChange: (levels: IssueLevels, live: boolean) => void;
  histogramLabel: string;
  testIdPrefix: string;
}

const KEY_STEP = 0.01; // of the span per arrow press (x10 with Shift)
const BAR_STEPS = 48;

/**
 * A three-handle levels control for one Survey Issues error: a histogram of
 * the survey's values over the colour bar they get, with handles for Min (the
 * palette's start: at or below it the readings count as agreeing), Warn (the
 * palette's middle) and Max (the palette's end, as an error is), and the same
 * three as number fields.
 */
export function IssueLevelsSlider({
  levels,
  domainMax,
  histogram,
  colorAt,
  unit,
  precision,
  disabled,
  onChange,
  histogramLabel,
  testIdPrefix,
}: IssueLevelsSliderProps) {
  const { t } = useTranslation();
  const trackRef = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<Handle | null>(null);
  const dragRef = useRef<{ start: IssueLevels; moved: boolean; el: HTMLElement; pointerId: number } | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  // Escape mid-drag puts the levels back where the drag began.
  useEffect(() => {
    if (drag === null) return undefined;
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      const d = dragRef.current;
      if (!d) return;
      e.preventDefault();
      e.stopPropagation();
      dragRef.current = null;
      d.el.releasePointerCapture?.(d.pointerId);
      setDrag(null);
      if (d.moved) onChangeRef.current(d.start, false);
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [drag]);

  // Unmounted mid-drag (the popover closed, the row collapsed): end the
  // preview on the levels the drag began from.
  useEffect(
    () => () => {
      const d = dragRef.current;
      if (d?.moved) onChangeRef.current(d.start, false);
    },
    [],
  );

  const viewMax = Math.max(domainMax, levels.max) || 1;
  const pct = (v: number) => (Math.min(Math.max(v, 0), viewMax) / viewMax) * 100;
  const gap = viewMax / 1000;

  const barGradient = useMemo(() => {
    const parts: string[] = [];
    for (let i = 0; i <= BAR_STEPS; i++) {
      parts.push(`${colorAt((viewMax * i) / BAR_STEPS)} ${((i / BAR_STEPS) * 100).toFixed(2)}%`);
    }
    return `linear-gradient(to right, ${parts.join(', ')})`;
  }, [colorAt, viewMax]);

  // Bar heights on a log scale: most shots agree, so a linear scale would show
  // one spike at zero and hide the tail the levels are about.
  const logPeak = Math.log1p(histogram ? Math.max(1, ...histogram) : 1);

  const valueAt = (clientX: number): number => {
    const rect = trackRef.current?.getBoundingClientRect();
    if (!rect || rect.width <= 0) return levels.min;
    return Math.min(1, Math.max(0, (clientX - rect.left) / rect.width)) * viewMax;
  };

  // A handle stops at its neighbours. Levels that are not in order to begin
  // with (a zero project threshold gives 0 / 0 / 0) let Warn and Max push the
  // handles above them instead, so a drag can still set them apart. The editor
  // drops anything that is still not 0 <= Min < Warn < Max.
  const moved = (handle: Handle, value: number): IssueLevels => {
    const v = Math.min(Math.max(value, 0), MAX_LEVEL);
    if (handle === 'min') return { ...levels, min: Math.max(0, Math.min(v, levels.warn - gap)) };
    if (handle === 'warn') {
      const warn = Math.max(v, levels.min + gap);
      return validLevels(levels)
        ? { ...levels, warn: Math.min(warn, levels.max - gap) }
        : { ...levels, warn, max: Math.max(levels.max, warn + gap) };
    }
    const max = Math.max(v, levels.min + 2 * gap);
    return validLevels(levels)
      ? { ...levels, max: Math.max(max, levels.warn + gap) }
      : { ...levels, warn: Math.max(levels.min + gap, Math.min(levels.warn, max - gap)), max };
  };

  const onPointerDown = (handle: Handle) => (e: PointerEvent<HTMLDivElement>) => {
    if (disabled) return;
    e.preventDefault();
    e.stopPropagation();
    e.currentTarget.setPointerCapture?.(e.pointerId);
    e.currentTarget.focus();
    dragRef.current = { start: levels, moved: false, el: e.currentTarget, pointerId: e.pointerId };
    setDrag(handle);
  };
  const onPointerMove = (handle: Handle) => (e: PointerEvent<HTMLDivElement>) => {
    const d = dragRef.current;
    if (drag !== handle || !d) return;
    d.moved = true;
    onChange(moved(handle, valueAt(e.clientX)), true);
  };
  const onPointerUp = (handle: Handle) => (e: PointerEvent<HTMLDivElement>) => {
    const d = dragRef.current;
    if (drag !== handle || !d) return;
    dragRef.current = null;
    e.currentTarget.releasePointerCapture?.(e.pointerId);
    setDrag(null);
    if (d.moved) onChange(moved(handle, valueAt(e.clientX)), false);
  };

  const onKeyDown = (handle: Handle) => (e: KeyboardEvent<HTMLDivElement>) => {
    if (disabled) return;
    const step = viewMax * KEY_STEP * (e.shiftKey ? 10 : 1);
    const current = levels[handle];
    let next: number | null = null;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowDown') next = current - step;
    else if (e.key === 'ArrowRight' || e.key === 'ArrowUp') next = current + step;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = viewMax;
    if (next === null) return;
    e.preventDefault();
    e.stopPropagation();
    onChange(moved(handle, next), false);
  };

  const fmt = (v: number) => `${formatFixed(v, undefined, precision)}${unit}`;
  const handleLabel: Record<Handle, string> = {
    min: t('map.issueScale.minHandle'),
    warn: t('map.issueScale.warnHandle'),
    max: t('map.issueScale.maxHandle'),
  };

  return (
    <div
      className={`flex flex-col gap-1 ${disabled ? 'opacity-50' : ''}`}
      aria-disabled={disabled || undefined}
      data-testid={`${testIdPrefix}-slider`}
    >
      <div ref={trackRef} className="relative mx-1.5">
        {disabled || !histogram ? (
          <div
            className="h-[44px] flex items-center justify-center text-[10px] text-text-secondary"
            data-testid={`${testIdPrefix}-histogram-off`}
          >
            {disabled ? t('map.issueScale.offHint') : t('map.issueScale.noValues')}
          </div>
        ) : (
          <svg
            className="block w-full text-text-secondary"
            height={44}
            viewBox="0 0 100 44"
            preserveAspectRatio="none"
            role="img"
            aria-label={histogramLabel}
            data-testid={`${testIdPrefix}-histogram`}
          >
            {histogram.map((count, k) => {
              if (count === 0) return null;
              const binW = domainMax / histogram.length;
              const from = binW * k;
              const x = pct(from);
              const w = Math.max(0.15, pct(from + binW) - x);
              const h = Math.max(1, (Math.log1p(count) / logPeak) * 42);
              // Values at or below Min count as agreeing: drawn faint.
              return (
                <rect key={k} x={x} y={44 - h} width={w} height={h} fill="currentColor" opacity={from + binW > levels.min ? 0.85 : 0.3} />
              );
            })}
          </svg>
        )}
        <div className="h-2.5 rounded-sm border border-border" style={{ background: barGradient }} data-testid={`${testIdPrefix}-bar`} />
        <div className="relative h-3.5">
          {HANDLES.map((id) => (
            <div
              key={id}
              role="slider"
              tabIndex={disabled ? -1 : 0}
              aria-label={handleLabel[id]}
              aria-disabled={disabled || undefined}
              aria-valuemin={0}
              aria-valuemax={viewMax}
              aria-valuenow={levels[id]}
              aria-valuetext={fmt(levels[id])}
              className={`absolute top-0 -translate-x-1/2 rounded-sm focus:outline-none focus-visible:ring-1 focus-visible:ring-accent ${
                disabled ? '' : 'cursor-ew-resize'
              }`}
              style={{ left: `${pct(levels[id])}%`, touchAction: 'none' }}
              onPointerDown={onPointerDown(id)}
              onPointerMove={onPointerMove(id)}
              onPointerUp={onPointerUp(id)}
              onPointerCancel={onPointerUp(id)}
              onKeyDown={onKeyDown(id)}
              data-testid={`${testIdPrefix}-${id}-handle`}
            >
              <svg width="11" height="10" viewBox="0 0 11 10" aria-hidden="true">
                <path
                  d="M5.5 0 L11 10 L0 10 Z"
                  className={
                    id === 'min'
                      ? 'fill-panel stroke-text-primary'
                      : id === 'warn'
                        ? 'fill-text-secondary stroke-text-primary'
                        : 'fill-text-primary stroke-text-primary'
                  }
                  strokeWidth={1}
                />
              </svg>
            </div>
          ))}
        </div>
      </div>
      <IssueLevelsFields
        levels={levels}
        unit={unit}
        precision={precision}
        disabled={disabled}
        onChange={(next) => onChange(next, false)}
        testIdPrefix={testIdPrefix}
      />
    </div>
  );
}

/** Field text: plain `toFixed`, so what the field shows parses back. */
function draftOf(levels: IssueLevels, precision: number): Record<Handle, string> {
  const digits = Math.max(0, Math.min(6, Math.floor(precision)));
  return { min: levels.min.toFixed(digits), warn: levels.warn.toFixed(digits), max: levels.max.toFixed(digits) };
}

/** Min / Warn / Max number fields. Commit on Enter or blur; an entry that is
 *  not a number, below 0 or out of order (Min < Warn < Max) reverts. */
function IssueLevelsFields({
  levels,
  unit,
  precision,
  disabled,
  onChange,
  testIdPrefix,
}: {
  levels: IssueLevels;
  unit: string;
  precision: number;
  disabled: boolean;
  onChange: (levels: IssueLevels) => void;
  testIdPrefix: string;
}) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState(() => draftOf(levels, precision));
  const { min, warn, max } = levels;
  useEffect(() => {
    setDraft(draftOf({ min, warn, max }, precision));
  }, [min, warn, max, precision]);

  const commit = () => {
    const shown = draftOf(levels, precision);
    if (HANDLES.every((h) => draft[h] === shown[h])) return;
    // A field the user did not edit keeps its exact value (its text is rounded).
    const read = (h: Handle) => (draft[h] === shown[h] ? levels[h] : Number(draft[h]));
    const next = { min: read('min'), warn: read('warn'), max: read('max') };
    const ok = validLevels(next);
    if (ok && (next.min !== levels.min || next.warn !== levels.warn || next.max !== levels.max)) onChange(next);
    else setDraft(shown);
  };

  const label: Record<Handle, string> = {
    min: t('map.issueScale.min'),
    warn: t('map.issueScale.warn'),
    max: t('map.issueScale.max'),
  };

  return (
    <div className="flex gap-2">
      {HANDLES.map((h) => (
        <label key={h} className="flex flex-col gap-0.5 min-w-0 flex-1">
          <span className="text-[10px] text-text-secondary">
            {label[h]}
            {unit ? ` (${unit})` : ''}
          </span>
          <input
            type="text"
            inputMode="decimal"
            disabled={disabled}
            value={draft[h]}
            onChange={(e) => setDraft((d) => ({ ...d, [h]: e.target.value }))}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                commit();
              } else if (e.key === 'Escape' && draft[h] !== draftOf(levels, precision)[h]) {
                // Escape first reverts an unsaved edit; a second one closes the popover.
                e.preventDefault();
                e.stopPropagation();
                setDraft(draftOf(levels, precision));
              }
            }}
            className="w-full px-1.5 py-0.5 bg-background border border-border rounded text-xs text-right focus:outline-none focus:ring-1 focus:ring-accent disabled:cursor-not-allowed"
            data-testid={`${testIdPrefix}-${h}-input`}
          />
        </label>
      ))}
    </div>
  );
}
