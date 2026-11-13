import { useMemo, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { useColorScaleStore } from '../../../store/colorScaleStore';
import type { ColorScaleKind, LevelsScale } from '../../../utils/palettes';
import { ColorScaleEditor } from './ColorScaleEditor';
import { computeHistogram, type HistogramSource } from './histogram';
import { LegendCogPopover } from './LegendCogPopover';

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
export function ColorScaleCog(props: ColorScaleCogProps) {
  const { t } = useTranslation();
  const setDragging = useColorScaleStore((s) => s.setDragging);
  const setPalettePreview = useColorScaleStore((s) => s.setPalettePreview);
  return (
    <LegendCogPopover
      id={props.kind}
      title={t(`map.colorScale.title.${props.kind}`)}
      width={POPOVER_WIDTH}
      onClose={() => {
        setPalettePreview(null);
        setDragging(false);
      }}
    >
      {(popRef) => <ColorScaleCogBody {...props} popRef={popRef} />}
    </LegendCogPopover>
  );
}

function ColorScaleCogBody({
  kind,
  domain,
  histogramSource,
  precision,
  popRef,
}: ColorScaleCogProps & { popRef: RefObject<HTMLDivElement | null> }) {
  const scale = useColorScaleStore((s) => s.mapScale[kind]);
  const setMapScale = useColorScaleStore((s) => s.setMapScale);
  const setDragging = useColorScaleStore((s) => s.setDragging);
  const histogram = useMemo(
    () => (histogramSource ? computeHistogram(histogramSource, domain.min, domain.max) : null),
    [histogramSource, domain.min, domain.max],
  );
  return (
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
  );
}
