import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { IconButton } from '../../common/Button';
import { SettingsIcon } from '../../common/icons';
import { useAnchoredFixedMenu } from '../../../hooks/useAnchoredFixedMenu';
import { useDismissableLayer } from '../../../hooks/useDismissableLayer';
import { useColorScaleStore } from '../../../store/colorScaleStore';
import type { ColorScaleKind, LevelsScale } from '../../../utils/palettes';
import { ColorScaleEditor } from './ColorScaleEditor';
import { computeHistogram, type HistogramSource } from './histogram';

export interface ColorScaleCogProps {
  kind: ColorScaleKind;
  /** The data's extent for this kind. */
  domain: { min: number; max: number };
  /** The values to histogram; binned only while the popover is open. */
  histogramSource: HistogramSource | null;
  precision: number;
}

const POPOVER_WIDTH = 300;

/**
 * The small cog beside a depth / distance legend, and the colour-scale popover
 * it opens (palette + Levels range for the MAP; the export has its own range).
 * Changes preview live on the map; closing (outside click, Escape) ends any
 * preview.
 */
export function ColorScaleCog({ kind, domain, histogramSource, precision }: ColorScaleCogProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const cogRef = useRef<HTMLDivElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const scale = useColorScaleStore((s) => s.mapScale[kind]);
  const setMapScale = useColorScaleStore((s) => s.setMapScale);
  const setDragging = useColorScaleStore((s) => s.setDragging);
  const setPalettePreview = useColorScaleStore((s) => s.setPalettePreview);

  // The popover renders into document.body (the legend sits in a z-index:1 box
  // the side panels and data table would paint over). The placement hook reads
  // the toolbar / status-bar heights as CSS variables inherited from the app
  // root, which a body-level element no longer inherits, so carry them over.
  const [chromeVars, setChromeVars] = useState<CSSProperties>({});
  const openPopover = () => {
    const el = cogRef.current;
    if (el) {
      const cs = getComputedStyle(el);
      setChromeVars({
        '--toolbar-h': cs.getPropertyValue('--toolbar-h'),
        '--statusbar-h': cs.getPropertyValue('--statusbar-h'),
      } as CSSProperties);
    }
    setOpen(true);
  };

  const close = () => {
    setOpen(false);
    setPalettePreview(null);
    setDragging(false);
  };

  const histogram = useMemo(
    () => (open && histogramSource ? computeHistogram(histogramSource, domain.min, domain.max) : null),
    [open, histogramSource, domain.min, domain.max],
  );

  const style = useAnchoredFixedMenu(open, cogRef, popRef, { align: 'center', gap: 6, width: POPOVER_WIDTH });
  useDismissableLayer({ open, onDismiss: close, contains: [wrapRef, popRef], survivesUnrelatedScroll: true });

  // Escape closes the popover (an inner field or the palette list claims it
  // first by stopping it). The global Escape handler stands down while any
  // [data-dropdown-open] is present, so it does not also clear the selection.
  useEffect(() => {
    if (!open) return undefined;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented) return;
      e.preventDefault();
      close();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
    // close only touches stable store setters and local state.
  }, [open]);

  return (
    <div
      ref={wrapRef}
      onMouseDown={(e) => e.stopPropagation()}
      onPointerDown={(e) => e.stopPropagation()}
      onWheel={(e) => e.stopPropagation()}
    >
      <div ref={cogRef}>
        <IconButton
          icon={<SettingsIcon size={14} />}
          size="sm"
          variant="ghost"
          active={open}
          tooltip={t('map.colorScale.settingsButton')}
          aria-label={t('map.colorScale.settingsButton')}
          aria-expanded={open}
          onClick={() => (open ? close() : openPopover())}
          className="bg-[var(--cc-text-halo-alt)]"
          data-testid={`${kind}-legend-settings-button`}
        />
      </div>
      {open && createPortal(
        <div
          ref={popRef}
          role="dialog"
          aria-label={t(`map.colorScale.title.${kind}`)}
          className="z-50 flex flex-col gap-2 rounded border border-border bg-panel p-2 shadow-lg text-text-primary"
          style={{ ...chromeVars, ...style }}
          data-dropdown-open
          onMouseDown={(e) => e.stopPropagation()}
          onPointerDown={(e) => e.stopPropagation()}
          onWheel={(e) => e.stopPropagation()}
          data-testid={`color-scale-popover-${kind}`}
        >
          <span className="text-xs font-medium">{t(`map.colorScale.title.${kind}`)}</span>
          <ColorScaleEditor
            kind={kind}
            domain={domain}
            histogram={histogram}
            scale={scale}
            onScaleChange={(next: LevelsScale | null, live: boolean) => {
              setMapScale(kind, next);
              setDragging(live);
            }}
            precision={precision}
            testIdPrefix="color-scale"
            pickerPortalInto={popRef}
          />
        </div>,
        document.body,
      )}
    </div>
  );
}
