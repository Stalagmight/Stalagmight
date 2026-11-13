/**
 * A row's display name ("Main:3") from its key, for chrome that lives outside
 * the workspace's providers (the map pane header, the Back-to-Data chip).
 * The section labels are cached per SurveyData sections array + language.
 */
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import type { SurveyData } from '../../types/survey-data';
import { buildSectionLabels } from './model/sectionLabels';
import { parseRowKey } from './model/rows';

const labelCache = new WeakMap<SurveyData['sections'], { lang: string; labels: Map<number, string> }>();

export function rowDisplayName(
  data: SurveyData,
  rowKey: string,
  defaultName: (id: number) => string,
  lang: string,
): string | null {
  const parsed = parseRowKey(rowKey);
  if (!parsed) return null;
  let entry = labelCache.get(data.sections);
  if (!entry || entry.lang !== lang) {
    entry = { lang, labels: buildSectionLabels(data.sections, defaultName) };
    labelCache.set(data.sections, entry);
  }
  const label = entry.labels.get(parsed.sectionId);
  if (label === undefined) return null;
  if (parsed.kind === 'station') return `${label}:${parsed.stationIndex}`;
  if (parsed.kind === 'section') return label;
  const sec = data.sections.find((s) => s.id === parsed.sectionId);
  if (!sec) return null;
  for (let i = 0; i < sec.shotCount; i++) {
    if (data.shotIds[sec.shotOffset + i] === parsed.shotId) return `${label}:${i + 1}`;
  }
  return null;
}

export function useRowDisplayName(rowKey: string | null): string | null {
  const { t, i18n } = useTranslation();
  const data = useProjectStore((s) => s.surveyData);
  return useMemo(
    () => (rowKey && data ? rowDisplayName(data, rowKey, (id) => t('sections.defaultName', { id }), i18n.language) : null),
    [rowKey, data, t, i18n.language],
  );
}
