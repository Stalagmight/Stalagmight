import { useLayoutEffect, useMemo } from 'react';
import { useProjectStore } from '../store/projectStore';
import { useUIStore } from '../store/uiStore';
import type { SyntacticHighlightLevel } from '../types/config';
import { defaultConfig } from '../types/config';
import {
  SYNTACTIC_ROLES,
  resolveSyntacticColors,
  syntacticVarName,
} from '../theme/syntacticPalette';

/** Neutral fallback used in the "off" level so icons render monochrome. */
const OFF_NEUTRAL = 'var(--cc-text-primary)';

/** Coerce the generated level (which includes "unknown") to a concrete level. */
function normalizeLevel(level: SyntacticHighlightLevel): SyntacticHighlightLevel {
  return level === 'unknown' ? 'full' : level;
}

/**
 * Resolve the syntactic-highlighting config (preview-aware), apply the role
 * colors as `--sh-<role>` custom properties on the document root, set the
 * `data-sh-level` attribute, and mirror the resolved level into uiStore so
 * components can gate Full-only UI (blobs).
 *
 * Mirrors {@link useTheme}: a single `useLayoutEffect` keyed on the resolved
 * config so changes (including Settings live-preview) repaint before paint.
 *
 * Call once at the top level of the App component.
 */
export function useSyntacticHighlighting(): SyntacticHighlightLevel {
  const configHighlighting = useProjectStore(
    (s) => s.config?.syntactic_highlighting ?? null,
  );
  const preview = useUIStore((s) => s.syntacticHighlightPreview);
  const setSyntacticLevel = useUIStore((s) => s.setSyntacticLevel);

  const active = preview ?? configHighlighting ?? defaultConfig.syntactic_highlighting;
  const level = normalizeLevel(active.level);
  const preset = active.preset;
  const overrides = active.overrides;

  // `overrides` is a stable object from the store except during Settings live
  // preview, so serialize it only when its reference changes - not on every
  // App-root render (this hook runs at the top of App, which re-renders often).
  const overridesKey = useMemo(() => JSON.stringify(overrides), [overrides]);
  const colors = useMemo(
    () => resolveSyntacticColors(preset, overrides),
    // overridesKey stands in for `overrides` so we re-resolve on preset/override
    // changes only, not on unrelated renders.
    // eslint-disable-next-line react-hooks/exhaustive-deps -- overridesKey stands in for overrides (compared by value)
    [preset, overridesKey],
  );

  useLayoutEffect(() => {
    const root = document.documentElement;
    for (const { id } of SYNTACTIC_ROLES) {
      const value = level === 'off' ? OFF_NEUTRAL : colors[id];
      root.style.setProperty(syntacticVarName(id), value);
    }

    root.dataset.shLevel = level;
    setSyntacticLevel(level);
  }, [level, colors, setSyntacticLevel]);

  return level;
}
