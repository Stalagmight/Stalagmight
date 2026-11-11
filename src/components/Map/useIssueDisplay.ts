/**
 * The Survey Issues inputs the map hands its survey layer: the warnings, the
 * committed colour scale (per-error levels, switched-off errors, combine mode),
 * the palette, and how to format the Errors labels.
 *
 * Returns null while neither the graded colouring nor the labels are on, and a
 * stable object otherwise (it only changes when one of its inputs does). That
 * keeps today's guarantee that a warnings refresh - every edit, resolve and
 * ignore - does not rebuild the map while Survey Issues is off.
 */
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import type { ErrorLabelFormat, IssueRenderInput } from './surveyLayer/issueScore';
import { useIssuePalette, useIssueScale } from './colorScale/useIssueScale';

export function useIssueRenderInput(colorOn: boolean, labelsOn: boolean): IssueRenderInput | null {
  const { t } = useTranslation();
  const warnings = useProjectStore((s) => s.warnings);
  const lengthUnit = useProjectStore((s) => s.config?.units.length ?? 'meters');
  const precision = useDecimalPrecision();
  const scale = useIssueScale();
  const palette = useIssuePalette();

  const labels = useMemo<ErrorLabelFormat | null>(
    () =>
      labelsOn
        ? {
            precision,
            lengthUnit,
            abbrev: {
              az: t('map.errorLabels.abbrev.azimuth'),
              inc: t('map.errorLabels.abbrev.inclination'),
              dep: t('map.errorLabels.abbrev.inclinationDepth'),
              len: t('map.errorLabels.abbrev.length'),
              loop: t('map.errorLabels.abbrev.closure'),
            },
          }
        : null,
    [labelsOn, precision, lengthUnit, t],
  );

  return useMemo<IssueRenderInput | null>(
    () => (colorOn || labels ? { color: colorOn, labels, warnings, scale, palette } : null),
    [colorOn, labels, warnings, scale, palette],
  );
}
