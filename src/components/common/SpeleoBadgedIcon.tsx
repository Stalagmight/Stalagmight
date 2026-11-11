import type { ReactNode } from 'react';
import { SpeleoDbLogo } from './SpeleoDbLogo';

/**
 * The SpeleoDB mark with a small state/action badge composited onto its corner
 * (shared by the toolbar status button and the hamburger SpeleoDB submenu, so
 * both speak the same visual language). The badge is a dark disc holding a
 * `currentColor` glyph, coloured by `badgeColor`.
 */
export function SpeleoBadgedIcon({
  badge,
  badgeColor,
  size = 20,
  tint,
  testIdPrefix,
}: {
  /**
   * Omit for the bare mark. "No badge" is a real state, not an absence: an
   * unlocked SpeleoDB project is the resting case, and a resting case should not
   * be decorated. Expressing it here keeps callers on one code path instead of
   * special-casing a bare `SpeleoDbLogo`.
   */
  badge?: ReactNode;
  badgeColor?: string | undefined;
  size?: number;
  /** Recolour the MARK itself (#270: yellow while there are un-pushed local edits). */
  tint?: string | undefined;
  /** Stable prefix that keeps the nested logo and badge IDs unique at every call site. */
  testIdPrefix: string;
}) {
  return (
    <span className="relative inline-flex">
      <SpeleoDbLogo
        size={size}
        tint={tint}
        testId={`${testIdPrefix}-logo`}
      />
      {/* The disc is the panel BACKGROUND colour (like the DisplayMenu badge-bg,
          fill: var(--cc-panel)) so it knocks the icon out around the badge - a
          cutout, not a coloured circle. */}
      {badge != null && (
        <span
          className={`absolute -bottom-1 -right-1 flex items-center justify-center rounded-full bg-panel p-[1px] ${badgeColor ?? ''}`}
          data-testid={`${testIdPrefix}-state-badge`}
        >
          {badge}
        </span>
      )}
    </span>
  );
}

/**
 * Filled "open on the web" glyph: a window with an arrow breaking out of it.
 *
 * Deliberately tighter than `ExternalLinkIcon`, which is drawn for a 20-unit menu
 * slot: at badge scale its thin open box and separate arrow bars collapse into
 * mush. This compresses the frame, knocks its interior out in the panel colour
 * rather than outlining it, and haloes the arrow in the panel colour, so the two
 * shapes stay legible at 9px.
 */
export function ExternalLinkGlyph({ size = 9 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
      {/* Window, as a solid block with its interior knocked out. */}
      <path d="M1 6h9v9H1z" />
      <path d="M2.6 7.6h5.8v5.8H2.6z" style={{ fill: 'var(--cc-panel)' }} />
      {/* Arrow breaking out top-right, haloed so it reads clear of the frame. */}
      <g
        style={{ stroke: 'var(--cc-panel)' }}
        strokeWidth={1.4}
        strokeLinejoin="round"
        paintOrder="stroke"
      >
        <path d="M9 1h6v6h-2V4.4L8.4 9 7 7.6 11.6 3H9z" />
      </g>
    </svg>
  );
}

export function PenGlyph({ size = 11 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
      {/* barrel, running top-right to bottom-left */}
      <path d="M10.6 1.4l2.6 2.6-7.4 7.4-2.6-2.6z" />
      {/* nib, tapering to the writing tip */}
      <path d="M3.2 8.8l2.6 2.6-4 2z" />
      {/* ferrule band, knocked out in the panel colour */}
      <path d="M8.8 3.2l2.6 2.6-0.7 0.7-2.6-2.6z" style={{ fill: 'var(--cc-panel)' }} />
    </svg>
  );
}

/** Filled hourglass glyph (someone else is editing). */
export function HourglassGlyph({ size = 10 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
      <path d="M4 2h8v3L8 8 4 5V2zM4 14h8v-3L8 8l-4 3v3z" />
    </svg>
  );
}

/** Filled two-arrow circular glyph (refresh / sync with the server). */
export function RefreshGlyph({ size = 10 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M12 4V1L8 5l4 4V6c3.31 0 6 2.69 6 6 0 1.01-.25 1.97-.7 2.8l1.46 1.46C19.54 15.03 20 13.57 20 12c0-4.42-3.58-8-8-8zm0 14c-3.31 0-6-2.69-6-6 0-1.01.25-1.97.7-2.8L5.24 7.74C4.46 8.97 4 10.43 4 12c0 4.42 3.58 8 8 8v3l4-4-4-4v3z" />
    </svg>
  );
}

/** Filled up-arrow glyph (push to the server). */
export function PushGlyph({ size = 10 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
      <path d="M8 2l5 6h-3v6H6V8H3l5-6z" />
    </svg>
  );
}

/**
 * Filled pencil with a slash through it (read-only / web-viewer: you can view
 * but not edit). The slash carries a panel-coloured under-stroke so it reads as
 * a distinct diagonal cut across the pencil.
 */
export function CrossedPencilGlyph({ size = 11 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
      {/* vertical pencil, tip pointing down */}
      <path d="M5.4 2h3.2v9l-1.6 3-1.6-3z" />
      {/* diagonal "no" slash: a wider panel-coloured line underneath cuts a gap so
          the slash reads as separate from the pencil, then the slash on top. */}
      <path d="M2.5 13.5L13.5 2.5" style={{ stroke: 'var(--cc-panel)' }} strokeWidth={3.4} />
      <path d="M2.5 13.5L13.5 2.5" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" />
    </svg>
  );
}

/** Filled question-mark glyph (an unexpected SpeleoDB server error). */
export function QuestionMarkGlyph({ size = 11 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
      {/* Filled hook (not a stroke - WebKit fattens strokes under the UI zoom), plus
          a filled dot. */}
      <path d="M5 5.4a3 3 0 015.95.6c0 1.5-1.05 2.15-1.75 2.75-.55.48-.7.95-.7 1.75H6.7c0-1.15.2-1.75.9-2.4.6-.56 1.5-1 1.5-1.95a1.35 1.35 0 00-2.7-.05z" />
      <circle cx="8" cy="12.4" r="1.5" />
    </svg>
  );
}
