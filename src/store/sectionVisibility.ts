/**
 * Which lines the map leaves out while Sections are hidden (#327).
 *
 * A hidden Section's lines vanish from the map - geometry, stations, labels,
 * comments, closures, hover and picking - but stay in the project, the
 * statistics and the Sections panel (dimmed there). One exception: a line the
 * user SELECTS (in the panel, the data table, a Details link) is drawn while it
 * is selected, so a selection is never something the map silently refuses to
 * show and "Zoom to" always frames something. The Section stays hidden; moving
 * the selection elsewhere hides the line again.
 */
import type { Line } from '../types/project';
import type { SurveyData } from '../types/survey-data';
import { selectActiveSurveyData, type GeometrySlice } from './geometrySelectors';
import { filterSurveyDataLinesCached } from '../utils/surveyDataLineFilter';
import type { GeometryMode, Selection } from './uiStore';

export interface MapHiddenLines {
  ids: ReadonlySet<number>;
  /** Changes exactly when `ids`' contents do ('' when nothing is hidden). */
  key: string;
}

const NONE: MapHiddenLines = { ids: new Set(), key: '' };

/** The line a selection entry is on, if it is on one. */
function lineOfSelection(s: Selection | null | undefined): number | undefined {
  if (!s) return undefined;
  return s.type === 'station' ? s.stationRef?.line_id : s.lineId;
}

/** The lines the current selection keeps on the map. */
export function revealedLineIds(selection: Selection | null, multiSelection: readonly Selection[]): Set<number> {
  const out = new Set<number>();
  for (const s of [selection, ...multiSelection]) {
    const id = lineOfSelection(s);
    if (id !== undefined) out.add(id);
  }
  return out;
}

/** The lines of hidden Sections, minus the revealed ones. */
export function computeMapHiddenLines(
  lines: readonly Pick<Line, 'id' | 'cave_section_id'>[],
  hiddenSections: readonly number[],
  revealed: ReadonlySet<number>,
): MapHiddenLines {
  if (hiddenSections.length === 0) return NONE;
  const hiddenSet = new Set(hiddenSections);
  const ids: number[] = [];
  for (const line of lines) {
    const section = line.cave_section_id;
    if (section !== undefined && section !== null && hiddenSet.has(section) && !revealed.has(line.id)) {
      ids.push(line.id);
    }
  }
  if (ids.length === 0) return NONE;
  ids.sort((a, b) => a - b);
  return { ids: new Set(ids), key: ids.join(',') };
}

interface MemoInputs {
  lines: readonly Line[] | undefined;
  hiddenSections: readonly number[];
  selection: Selection | null;
  multiSelection: readonly Selection[];
}
let memo: { inputs: MemoInputs; result: MapHiddenLines } | null = null;

/** {@link computeMapHiddenLines} over store state, memoized on the inputs'
 *  identities and returning the previous result when the contents match, so
 *  callers can compare `key`s (or the object) cheaply on every render. */
export function mapHiddenLines(
  lines: readonly Line[] | undefined,
  ui: { hiddenSections: readonly number[]; selection: Selection | null; multiSelection: readonly Selection[] },
): MapHiddenLines {
  const inputs: MemoInputs = {
    lines,
    hiddenSections: ui.hiddenSections,
    selection: ui.selection,
    multiSelection: ui.multiSelection,
  };
  if (
    memo &&
    memo.inputs.lines === inputs.lines &&
    memo.inputs.hiddenSections === inputs.hiddenSections &&
    memo.inputs.selection === inputs.selection &&
    memo.inputs.multiSelection === inputs.multiSelection
  ) {
    return memo.result;
  }
  const next = computeMapHiddenLines(
    lines ?? [],
    ui.hiddenSections,
    ui.hiddenSections.length === 0 ? new Set() : revealedLineIds(ui.selection, ui.multiSelection),
  );
  const result = memo && memo.result.key === next.key ? memo.result : next;
  memo = { inputs, result };
  return result;
}

/**
 * The survey data the MAP draws: the active geometry (measured or adjusted,
 * `selectActiveSurveyData`) without the hidden lines. Everything else that
 * reads survey data - statistics, tables, data exports - keeps the whole
 * project; the image export leaves the hidden Sections out by itself
 * (`withHiddenLines`).
 */
export function selectMapSurveyData(
  project: GeometrySlice & { project: { lines: Line[] } | null },
  ui: {
    geometryMode: GeometryMode;
    hiddenSections: readonly number[];
    selection: Selection | null;
    multiSelection: readonly Selection[];
  },
): SurveyData | null {
  const data = selectActiveSurveyData(project, ui.geometryMode);
  if (!data) return data;
  const hidden = mapHiddenLines(project.project?.lines, ui);
  return filterSurveyDataLinesCached(data, hidden.ids, hidden.key);
}
