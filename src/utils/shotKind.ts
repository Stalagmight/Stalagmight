/**
 * Shared presentation helpers for the 3-way {@link ShotKind} (#27).
 *
 * The data table, the context menu, the detail panel and the draw-shot dialog
 * all render the same three states, so the glyphs, the cycle order and the
 * i18n key mapping live here rather than being restated at each call site.
 */
import type { ShotKind } from '../types/ipc/ShotKind';

/**
 * Cycle order for the data-table cell: permanent, then the two virtual kinds.
 *
 * Jump comes before Reference because it is the more common virtual shot in
 * practice (issue #27 q2), so the usual edit is one click rather than two.
 */
export const SHOT_KIND_CYCLE: readonly ShotKind[] = ['Normal', 'Jump', 'Reference'];

/**
 * Single-character cell glyph. A permanent shot renders blank so the column
 * stays quiet on a normal survey and virtual shots stand out at a glance,
 * which is the survey-doneness cue the issue asks for.
 */
export function shotKindGlyph(kind: ShotKind): string {
  switch (kind) {
    case 'Jump':
      return 'J';
    case 'Reference':
      return 'R';
    default:
      return '';
  }
}

/**
 * Next kind in the cycle. `direction` of `-1` walks backwards (shift-click), so
 * overshooting a three-state ring costs one click instead of two.
 */
export function nextShotKind(kind: ShotKind, direction: 1 | -1 = 1): ShotKind {
  const current = SHOT_KIND_CYCLE.indexOf(kind);
  const length = SHOT_KIND_CYCLE.length;
  // `+ length` keeps the modulo non-negative when walking backwards from index 0.
  const next = ((current < 0 ? 0 : current) + direction + length) % length;
  return SHOT_KIND_CYCLE[next] ?? 'Normal';
}

/** True for the two virtual kinds. Mirrors Rust's `ShotKind::is_virtual`. */
export function isVirtualShotKind(kind: ShotKind): boolean {
  return kind !== 'Normal';
}

/** i18n key for each kind's user-facing name. */
export const SHOT_KIND_LABEL_KEY: Record<ShotKind, string> = {
  Normal: 'shotKind.normal',
  Jump: 'shotKind.jump',
  Reference: 'shotKind.reference',
};

/**
 * Tailwind text colour per kind, shared by the table cell and any other glyph
 * surface. Jump keeps the warm accent the old reference dot used, so existing
 * surveys read the same; Reference gets a cooler, dimmer tone to mark that it
 * is not a swimmable leg.
 */
export const SHOT_KIND_CLASS: Record<ShotKind, string> = {
  Normal: '',
  Jump: 'text-amber-400',
  Reference: 'text-sky-400/70',
};
