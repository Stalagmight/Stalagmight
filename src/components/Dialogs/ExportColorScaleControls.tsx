import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ColorScaleEditor } from '../Map/colorScale/ColorScaleEditor';
import { HISTOGRAM_BINS } from '../Map/colorScale/histogram';
import { useColorScaleStore } from '../../store/colorScaleStore';
import { useProjectStore } from '../../store/projectStore';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { invokeIpc } from '../../utils/ipc';
import type { ColorScaleKind } from '../../utils/palettes';
import type { ValueHistogram } from '../../types/ipc.generated';

/**
 * The export's palette + Levels controls for "By depth" / "By penetration
 * distance". The palette is the same persisted choice the map uses; the range
 * is the export's own (session-only), over the export's own data: the
 * histogram comes from the backend so it covers exactly what the file colours
 * (the adjusted solve for depth, distance from the entrance).
 */
export function ExportColorScaleControls({ kind }: { kind: ColorScaleKind }) {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  const project = useProjectStore((s) => s.project);
  const includeRefLegs = useProjectStore((s) => s.config?.gas.include_reference_legs);
  const scale = useColorScaleStore((s) => s.exportScale[kind]);
  const setExportScale = useColorScaleStore((s) => s.setExportScale);
  const [histogram, setHistogram] = useState<ValueHistogram | null>(null);

  useEffect(() => {
    if (!project) return undefined;
    let cancelled = false;
    invokeIpc<ValueHistogram>(
      'export_value_histogram',
      { kind, bins: HISTOGRAM_BINS },
      { purpose: 'histogram the values the image export colours' },
    )
      .then((h) => {
        if (!cancelled) setHistogram(h);
      })
      .catch(() => {
        if (!cancelled) setHistogram(null);
      });
    return () => {
      cancelled = true;
    };
  }, [kind, project, includeRefLegs]);

  if (!histogram) {
    return (
      <p className="text-xs text-text-secondary" data-testid="export-color-scale-loading">
        {t(`map.colorScale.title.${kind}`)}
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-1 rounded border border-border p-2" data-testid={`export-color-scale-${kind}`}>
      <span className="text-xs font-medium text-text-primary">{t(`map.colorScale.title.${kind}`)}</span>
      <ColorScaleEditor
        kind={kind}
        domain={{ min: histogram.min, max: histogram.max }}
        histogram={histogram.counts}
        scale={scale}
        onScaleChange={(next) => setExportScale(kind, next)}
        precision={kind === 'depth' ? precision.depth : precision.coordinates}
        testIdPrefix="export-color-scale"
      />
    </div>
  );
}
