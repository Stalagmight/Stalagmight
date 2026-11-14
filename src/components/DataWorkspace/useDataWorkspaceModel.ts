/**
 * Build the Data workspace's model (rows, columns with their logic, the ctx
 * every cell reads) from the project / UI stores. Rebuilt only when its inputs
 * change: the rows on a data or header-toggle change (O(rows)), the ctx on any
 * of its lookups. Coordinates come from the active geometry (measured or
 * closure-adjusted); the shot arrays are shared by both, so edits stay on the
 * measured data.
 */
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { selectActiveSurveyData } from '../../store/geometrySelectors';
import { useEditingBlocked } from '../../hooks/useEditingBlocked';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { getNumberFormatLocale } from '../../i18n/format';
import type { ProjectWarning } from '../../types/commands';
import type { SurveyData } from '../../types/survey-data';
import { countsTowardLength } from '../../utils/shotKind';
import { shotKindFromFlags } from '../../types/survey-data';
import { useDataWorkspaceStore } from './store';
import type { DataModel } from './DataModelContext';
import type { DataColumnDef, DataColumnKey, DataModelCtx, DataRow } from './types';
import { buildColumnDefs } from './model/columns';
import { buildDataRows } from './model/rows';
import { buildSectionLabels, cleanSectionName } from './model/sectionLabels';

/** Column logic is static: build it once. */
const COLUMN_DEFS: DataColumnDef[] = buildColumnDefs();
const COLUMN_BY_KEY = new Map<DataColumnKey, DataColumnDef>(COLUMN_DEFS.map((c) => [c.key, c]));

export interface SectionStats {
  stations: number;
  shots: number;
  /** Surveyed length in metres (Reference shots excluded, as the stats do). */
  length: number;
}

/** Per-section station / shot counts and surveyed length, from the SoA. */
export function sectionStatsOf(data: SurveyData): Map<number, SectionStats> {
  const stations = new Map<number, number>();
  for (let g = 0; g < data.stationCount; g++) {
    const sid = data.stationSectionIds[g]!;
    stations.set(sid, (stations.get(sid) ?? 0) + 1);
  }
  const out = new Map<number, SectionStats>();
  for (const sec of data.sections) {
    let length = 0;
    for (let i = sec.shotOffset; i < sec.shotOffset + sec.shotCount; i++) {
      if (countsTowardLength(shotKindFromFlags(data.shotFlags[i] ?? 0))) length += data.shotLength[i]!;
    }
    out.set(sec.id, { stations: stations.get(sec.id) ?? 0, shots: sec.shotCount, length });
  }
  return out;
}

export interface DataWorkspaceModel extends DataModel {
  sectionStats: Map<number, SectionStats>;
}

export function useDataWorkspaceModel(): DataWorkspaceModel | null {
  const { t, i18n } = useTranslation();
  const geometryMode = useUIStore((s) => s.geometryMode);
  const project = useProjectStore((s) => s.project);
  const data = useProjectStore((s) => selectActiveSurveyData(s, geometryMode));
  const warnings = useProjectStore((s) => s.warnings);
  const frozenShots = useProjectStore((s) => s.frozenShots);
  const showSectionHeaders = useDataWorkspaceStore((s) => s.showSectionHeaders);
  const editingBlocked = useEditingBlocked();
  const decimals = useDecimalPrecision();
  const language = i18n.language;

  const sectionInfo = useMemo(() => {
    if (!data) return null;
    const defaultName = (id: number) => t('sections.defaultName', { id });
    return {
      sectionsById: new Map(data.sections.map((s) => [s.id, s])),
      sectionLabels: buildSectionLabels(data.sections, defaultName),
      sectionNames: new Map(data.sections.map((s) => [s.id, cleanSectionName(s.name) || defaultName(s.id)])),
      sectionStats: sectionStatsOf(data),
    };
  }, [data, t]);

  const rows = useMemo<DataRow[]>(() => (data ? buildDataRows(data, { showSectionHeaders }) : []), [data, showSectionHeaders]);
  const rowByKey = useMemo(() => new Map(rows.map((r) => [r.key, r])), [rows]);

  const warningsByShot = useMemo(() => {
    const m = new Map<string, ProjectWarning[]>();
    for (const w of warnings) {
      const k = `${w.sectionId}:${w.shotId}`;
      const list = m.get(k);
      if (list) list.push(w);
      else m.set(k, [w]);
    }
    return m;
  }, [warnings]);

  const frozen = useMemo(() => new Set(frozenShots.map((f) => `${f.section_id}:${f.shot_index}`)), [frozenShots]);

  return useMemo(() => {
    if (!project || !data || !sectionInfo) return null;
    const ctx: DataModelCtx = {
      data,
      project,
      sectionsById: sectionInfo.sectionsById,
      sectionLabels: sectionInfo.sectionLabels,
      sectionNames: sectionInfo.sectionNames,
      warningsByShot,
      frozenShots: frozen,
      precision: decimals.length,
      decimals,
      // The effective number locale (never the 'system' sentinel), so the grid
      // parses with the separators it displays. `language` keeps it fresh.
      locale: language ? getNumberFormatLocale() : 'en-US',
      editingBlocked,
      t,
    };
    return { rows, rowByKey, columns: COLUMN_DEFS, columnByKey: COLUMN_BY_KEY, ctx, sectionStats: sectionInfo.sectionStats };
  }, [project, data, sectionInfo, rows, rowByKey, warningsByShot, frozen, decimals, language, editingBlocked, t]);
}
