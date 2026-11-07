import { useEffect, useMemo, useState, type CSSProperties } from 'react';
import { grayscaleTokenStyle } from '../utils/grayscaleTokens';

/**
 * While `active`, the style that dims an element's colours to gray without a CSS filter:
 * every colour custom property re-declared, desaturated (`../utils/grayscaleTokens.ts`,
 * #576 NF-W12DMP-1). Recomputed when the theme or the syntactic palette changes on the
 * document root (`data-theme`, the root's inline `--sh-*`), so a theme switch mid-placement
 * stays gray. Undefined while inactive.
 */
export function useGrayscaleTokens(active: boolean): CSSProperties | undefined {
  const [rootVersion, setRootVersion] = useState(0);
  useEffect(() => {
    if (!active) return undefined;
    const observer = new MutationObserver(() => setRootVersion((v) => v + 1));
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'data-sh-level', 'style'] });
    return () => observer.disconnect();
  }, [active]);
  return useMemo(() => {
    if (!active) return undefined;
    const root = getComputedStyle(document.documentElement);
    return grayscaleTokenStyle((name) => root.getPropertyValue(name)) as CSSProperties;
    // rootVersion stands for the root's theme and palette, which the observer above watches.
    // eslint-disable-next-line react-hooks/exhaustive-deps -- rootVersion re-reads the root's computed tokens
  }, [active, rootVersion]);
}
