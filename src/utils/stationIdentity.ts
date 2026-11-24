/**
 * Station, shot and line identity as the user sees it (SPEC 4.1/4.2).
 *
 * A station is named by its Name (unique among its line's native slots; a
 * foreign slot or a loop end may repeat a name on its line, RS-FX5, so plain
 * text qualifies it: `12 (Main)`, `12 (loop end on Side)`), a shot by its two
 * station labels (`4 → 5`), a line by its project-unique name. The positional
 * handles the IPC addresses things with - `StationRef{line_id, station_index}`,
 * shot ids, line ids - are session handles that are never shown. Everything
 * user-visible that names a station, shot or line goes through here (or the
 * pure formatters it wraps), so no caller re-derives a label from the handles.
 *
 * Names are read from the active `SurveyData` (its dense `stationNames` and
 * `stationIds`); when a caller has no SurveyData (a unit test that seeds only
 * `stations`), the `Station[]` list is scanned instead.
 */
import i18n from '../i18n';
import type { Station, StationRef } from '../types/project';
import type { ShownSlot } from '../types/ipc/ShownSlot';
import type { ClosureArchive } from '../types/ipc/ClosureArchive';
import type { SurveyData } from '../types/survey-data';
import { displayStationName, formatShotLabel, formatStationLabel } from './formatters';

/** The SurveyData columns identity needs (the shown slots only for a rename
 *  check through a loop end). */
export type IdentityData = Pick<
  SurveyData,
  'stationCount' | 'stationLineIds' | 'stationIndices' | 'stationNames' | 'stationIds' | 'lines' | 'shotIds'
> &
  Partial<Pick<SurveyData, 'stationShown'>>;

/** A line as identity needs it: the store's `project.lines` or `SurveyData.lines`. */
export interface IdentityLine {
  id: number;
  name: string | null;
}

export interface IdentitySource {
  surveyData?: IdentityData | null | undefined;
  /** Fallback when there is no SurveyData. */
  stations?: ReadonlyArray<Station> | null | undefined;
  lines?: ReadonlyArray<IdentityLine & { shots?: ReadonlyArray<{ id: number }> }> | null | undefined;
  /** The project's loop closures, for a loop end's own (hidden) station. */
  closures?: ReadonlyArray<{ id: bigint; archived?: ClosureArchive | undefined }> | null | undefined;
}

interface RefIndex {
  indices: Uint32Array;
  byLine: Map<number, number[]>;
}

/** `(line_id, station_index)` -> global station index, built once per SurveyData. */
const refIndexCache = new WeakMap<Uint32Array, RefIndex>();

function refIndexOf(data: IdentityData): RefIndex {
  const cached = refIndexCache.get(data.stationLineIds);
  if (cached && cached.indices === data.stationIndices) return cached;
  const byLine = new Map<number, number[]>();
  for (let g = 0; g < data.stationCount; g++) {
    const lineId = data.stationLineIds[g]!;
    let slots = byLine.get(lineId);
    if (!slots) {
      slots = [];
      byLine.set(lineId, slots);
    }
    slots[data.stationIndices[g]!] = g;
  }
  const built = { indices: data.stationIndices, byLine };
  refIndexCache.set(data.stationLineIds, built);
  return built;
}

/** The global station index of `ref` in `data`, or null when it has no such station. */
export function stationGlobalIndexOf(data: IdentityData, ref: StationRef): number | null {
  return refIndexOf(data).byLine.get(ref.line_id)?.[ref.station_index] ?? null;
}

function findStationIn(stations: ReadonlyArray<Station> | null | undefined, ref: StationRef): Station | undefined {
  return stations?.find(
    (s) => s.station_ref.line_id === ref.line_id && s.station_ref.station_index === ref.station_index,
  );
}

/**
 * A station's stored name, unsanitized (for an edit field). Every station of a
 * loaded project has one; an unknown station falls back to its index as text,
 * which is also the name the backend seeds for an unnamed station.
 */
export function stationNameOf(src: IdentitySource, ref: StationRef): string {
  const data = src.surveyData;
  if (data && data.stationCount > 0) {
    const g = stationGlobalIndexOf(data, ref);
    const name = g === null ? undefined : data.stationNames.get(g);
    if (name !== undefined && name !== '') return name;
    if (g !== null) return String(ref.station_index);
  }
  const station = findStationIn(src.stations, ref);
  return station?.name || String(ref.station_index);
}

/** The names of every station of line `lineId`, by station index (for a rename check). */
export function lineStationNamesOf(src: IdentitySource, lineId: number): string[] {
  const data = src.surveyData;
  if (data && data.stationCount > 0) {
    const slots = refIndexOf(data).byLine.get(lineId);
    if (slots) return Array.from(slots, (g, i) => (g === undefined ? String(i) : data.stationNames.get(g) || String(i)));
  }
  const names: string[] = [];
  for (const s of src.stations ?? []) {
    if (s.station_ref.line_id === lineId) names[s.station_ref.station_index] = s.name || String(s.station_ref.station_index);
  }
  return names;
}

/** Station ID -> the global indices of every slot that carries it, built once per SurveyData. */
const idIndexCache = new WeakMap<Uint32Array, Map<number, number[]>>();

function idIndexOf(data: IdentityData): Map<number, number[]> {
  const cached = idIndexCache.get(data.stationIds);
  if (cached) return cached;
  const byId = new Map<number, number[]>();
  for (let g = 0; g < data.stationCount; g++) {
    const id = data.stationIds[g];
    if (id === undefined) continue;
    const slots = byId.get(id);
    if (slots) slots.push(g);
    else byId.set(id, [g]);
  }
  idIndexCache.set(data.stationIds, byId);
  return byId;
}

/**
 * Every slot of the station `ref` is on: the slots that share its node id.
 * On the station graph a junction is ONE station that two or more lines pass
 * (plan 9.2), so its slots share one id, and every line shows its kept
 * station (RS 4.1); a station on one line has the one slot. `ref` alone when
 * the station is unknown.
 */
export function stationSlotsOf(src: IdentitySource, ref: StationRef): StationRef[] {
  const data = src.surveyData;
  if (data && data.stationCount > 0) {
    const g = stationGlobalIndexOf(data, ref);
    const id = g === null ? undefined : data.stationIds[g];
    const slots = id === undefined ? undefined : idIndexOf(data).get(id);
    if (slots) {
      return slots.map((s) => ({ line_id: data.stationLineIds[s]!, station_index: data.stationIndices[s]! }));
    }
    if (g !== null) return [ref];
  }
  const station = findStationIn(src.stations, ref);
  if (!station) return [ref];
  return (src.stations ?? []).filter((s) => s.id === station.id).map((s) => s.station_ref);
}

/** The Station ID the slot at global index `g` shows (RS 6): its shown
 *  identity's, the node's own unless a loop closure archives the node. */
function shownIdAt(data: IdentityData, g: number): number | undefined {
  return data.stationShown?.get(g)?.id ?? data.stationIds[g];
}

/** The Station ID the station shows, from the `Station[]` list. */
function shownIdOfStation(station: Station): number {
  return station.shown?.id ?? station.id;
}

/** A native slot: it shows its node's own identity on one of the node's
 *  native lines (no `shown`), so it counts for name uniqueness (N8, RS-FX5). */
function isNativeAt(data: IdentityData, g: number): boolean {
  return data.stationShown?.get(g) === undefined;
}

/**
 * The first line on which a NATIVE slot of another station already shows
 * `name`, among the lines a rename of the station at `ref` is checked on, or
 * null when the name is free. A rename through any slot renames the identity
 * the slot shows (RS 5.4: a junction shows its kept station on every line, a
 * loop end its closure's kept end). N8 holds among native slots only
 * (RS-FX5): a foreign slot (the dot) and a loop end (the loop mark) count for
 * no uniqueness. So the rename is checked on the lines that identity is
 * native on (where a slot shows it with no dot or loop mark), against the
 * native slots of other identities there, as the backend's
 * `conflicts_with_name` does; a line that shows it only through a foreign
 * slot or a loop end is not checked. On one line this is the old per-line
 * check. A foreign slot or a loop end asks whose station it is first
 * (`preview_station_identity`, `ShownIdentityPrompt`).
 */
export function lineWithStationNamed(src: IdentitySource, ref: StationRef, name: string): number | null {
  const data = src.surveyData;
  if (data && data.stationCount > 0) {
    const g = stationGlobalIndexOf(data, ref);
    const target = g === null ? undefined : shownIdAt(data, g);
    if (target !== undefined) {
      const lines: number[] = [];
      for (const s of idIndexOf(data).get(target) ?? []) {
        const line = data.stationLineIds[s]!;
        if (isNativeAt(data, s) && !lines.includes(line)) lines.push(line);
      }
      const byLine = refIndexOf(data).byLine;
      for (const line of lines) {
        const slots = byLine.get(line) ?? [];
        for (let i = 0; i < slots.length; i++) {
          const s = slots[i];
          if (s === undefined || !isNativeAt(data, s) || data.stationIds[s] === target) continue;
          if ((data.stationNames.get(s) || String(i)) === name) return line;
        }
      }
      return null;
    }
  }
  const stations = src.stations ?? [];
  const station = findStationIn(stations, ref);
  // An unknown station: its own line, against the other native slots there.
  const target = station ? shownIdOfStation(station) : null;
  const lines = station
    ? [...new Set(stations.filter((s) => s.id === target && !s.shown).map((s) => s.station_ref.line_id))]
    : [ref.line_id];
  for (const line of lines) {
    for (const s of stations) {
      if (s.station_ref.line_id !== line || s.shown) continue;
      if (station ? s.id === target : s.station_ref.station_index === ref.station_index) continue;
      if ((s.name || String(s.station_ref.station_index)) === name) return line;
    }
  }
  return null;
}

/**
 * The datum the station at `ref` shows (its datum dialog edits): the one the
 * wire lists on this slot, else the first on any other slot of the same
 * station. A datum is listed on its `via` slot, but it flags EVERY slot of its
 * station (plan W1, W2), so a junction's other slot finds it here.
 */
export function datumOfStation<D extends { station_ref: StationRef }>(
  src: IdentitySource,
  datums: readonly D[] | null | undefined,
  ref: StationRef,
): D | undefined {
  const same = (a: StationRef, b: StationRef) => a.line_id === b.line_id && a.station_index === b.station_index;
  const own = datums?.find((d) => same(d.station_ref, ref));
  if (own || !datums) return own;
  const slots = stationSlotsOf(src, ref);
  return datums.find((d) => slots.some((s) => same(s, d.station_ref)));
}

/**
 * `refs` with one slot per station, in order (the first slot of each kept). A
 * junction is one station (plan 9.1, 9.3), so a move of co-located slots sends
 * ONE `move_station` per Station ID rather than one per slot, which would move
 * it once per line through it. A slot whose station is unknown is kept.
 */
export function onePerStation(src: IdentitySource, refs: readonly StationRef[]): StationRef[] {
  const seen = new Set<number>();
  const out: StationRef[] = [];
  for (const ref of refs) {
    const id = stationIdOf(src, ref);
    if (id !== null) {
      if (seen.has(id)) continue;
      seen.add(id);
    }
    out.push(ref);
  }
  return out;
}

/** The project-unique Station ID of `ref`'s node, or null when the station
 *  is unknown: what groupings, moves and selections key on. A display of the
 *  Station ID reads {@link shownStationIdOf}. */
export function stationIdOf(src: IdentitySource, ref: StationRef): number | null {
  const data = src.surveyData;
  if (data && data.stationCount > 0) {
    const g = stationGlobalIndexOf(data, ref);
    if (g !== null) return data.stationIds[g] ?? null;
  }
  return findStationIn(src.stations, ref)?.id ?? null;
}

/**
 * How the slot at `ref` shows its node (RS 6): the Station ID it shows, the
 * home line whose colour its dot takes, whether it is foreign and whether a
 * loop closure archives its node. Undefined for a slot that shows its node's
 * own identity on one of its native lines (no dot, no loop mark), and for an
 * unknown station.
 */
export function shownSlotOf(src: IdentitySource, ref: StationRef): ShownSlot | undefined {
  const data = src.surveyData;
  if (data && data.stationCount > 0) {
    const g = stationGlobalIndexOf(data, ref);
    if (g !== null) return data.stationShown?.get(g);
  }
  return findStationIn(src.stations, ref)?.shown;
}

/** The Station ID the slot at `ref` shows (RS 6: the Station ID row, the pill
 *  superscript, the Station ID column), or null when the station is unknown. */
export function shownStationIdOf(src: IdentitySource, ref: StationRef): number | null {
  return shownSlotOf(src, ref)?.id ?? stationIdOf(src, ref);
}

/** The line whose colour the dot after a foreign slot's name takes (its shown
 *  station's home line, RS 2), or null for a slot that needs no dot. */
export function homeMarkOf(src: IdentitySource, ref: StationRef): number | null {
  const shown = shownSlotOf(src, ref);
  return shown?.foreign && shown.home_line !== undefined ? shown.home_line : null;
}

/**
 * The line a slot's shown station belongs to, when it is not the slot's own
 * line (RS-OWNERSHIP): a foreign slot's shown station's home line, or a loop
 * end's kept end's home line. A station pill takes this line's colour and
 * names it as its context; a shot pill colours the half holding that end by
 * it. Null for a native slot (and a loop end on its kept end's own line).
 */
export function ownerLineOf(src: IdentitySource, ref: StationRef): number | null {
  const shown = shownSlotOf(src, ref);
  if (!shown || shown.home_line === undefined || shown.home_line === ref.line_id) return null;
  return shown.foreign || shown.loop_end ? shown.home_line : null;
}

/** A line's name for display (sanitized), never its id: "Unnamed line" when it has none. */
export function lineDisplayName(name: string | null | undefined): string {
  const shown = displayStationName(name).trim();
  return shown || i18n.t('detailPanel.section.unnamed');
}

/** The display name of line `lineId` (see {@link lineDisplayName}). */
export function lineNameOf(src: IdentitySource, lineId: number): string {
  const line =
    src.lines?.find((l) => l.id === lineId) ?? src.surveyData?.lines.find((l) => l.id === lineId);
  return lineDisplayName(line?.name);
}

/**
 * A slot's plain-text label: `17`, or `Main Line / 17` with `withLine`. It is
 * the one label every text that names a slot without drawing the dot or the
 * loop mark uses (warnings, toasts, search results, history rows, aria labels,
 * tooltips, dialog sentences, closure texts), so it says whose station a
 * dotted slot shows (RS 6, U-P3-8), with or without the line: two slots of one
 * line may show one name since dotted slots don't collide (RS-FX5), and the
 * qualifier tells them apart.
 *
 * - native: `12`, `Side / 12`;
 * - foreign (Side's slot showing Main's 12): `12 (Main)`, `Side / 12 (Main)`;
 * - loop end (a closure on Side shows Main's 3): `3 (loop end on Side)`, and
 *   with the line, named on the shown station's home line,
 *   `Main / 3 (loop end on Side)`.
 *
 * A pill, which draws the dot and the loop mark itself, takes the bare name
 * ({@link stationPillLabelOf}); an edit field takes {@link stationNameOf}.
 */
export function stationLabelOf(src: IdentitySource, ref: StationRef, withLine = false): string {
  const name = stationNameOf(src, ref);
  const shown = shownSlotOf(src, ref);
  if (shown?.loop_end) {
    const home = withLine ? lineNameOf(src, shown.home_line ?? ref.line_id) : null;
    return i18n.t('identity.loopEndLabel', {
      station: formatStationLabel(name, home),
      line: lineNameOf(src, ref.line_id),
    });
  }
  const label = formatStationLabel(name, withLine ? lineNameOf(src, ref.line_id) : null);
  if (shown?.foreign && shown.home_line !== undefined && shown.home_line !== ref.line_id) {
    return i18n.t('identity.foreignLabel', { station: label, home: lineNameOf(src, shown.home_line) });
  }
  return label;
}

/**
 * A slot's bare name for a pill's main part (`12`), where the pill takes the
 * colour of the line a foreign slot's station belongs to (RS-OWNERSHIP) and
 * draws a loop end's loop mark: the colour and the mark say what
 * {@link stationLabelOf}'s qualifier says in text.
 */
export function stationPillLabelOf(src: IdentitySource, ref: StationRef): string {
  return formatStationLabel(stationNameOf(src, ref));
}

/**
 * A Plan mode pin's label (RS 6): `Main Line / 17`, as {@link stationLabelOf}
 * with the line, except a loop end, which is named by its own station first,
 * then the station it is shown as: `Loop / L8 (loop end, shown as Main / 8)`.
 * "Main / 8 (loop end)" read as if Main / 8 were the loop end (RS-FX6,
 * P3-10). Without the closure's archive (an older backend), `Loop end on
 * Loop, shown as Main / 8`.
 */
export function planStationLabelOf(src: IdentitySource, ref: StationRef): string {
  const shown = shownSlotOf(src, ref);
  if (!shown?.loop_end) return stationLabelOf(src, ref, true);
  const station = formatStationLabel(stationNameOf(src, ref), lineNameOf(src, shown.home_line ?? ref.line_id));
  const own =
    shown.loop_closure === undefined
      ? undefined
      : src.closures?.find((c) => c.id === shown.loop_closure)?.archived?.station;
  if (!own) return i18n.t('identity.loopEndPinLabelLine', { line: lineNameOf(src, ref.line_id), station });
  return i18n.t('identity.loopEndPinLabel', {
    own: formatStationLabel(own.name, lineNameOf(src, own.home_line ?? ref.line_id)),
    station,
  });
}

/**
 * `4 → 5` (or `Main Line: 4 → 5` with `withLine`) for the shot at 0-based
 * position `shotIndex` of line `lineId`: it runs from station `shotIndex` to
 * station `shotIndex + 1`. Each end reads its plain-text label without the
 * line ({@link stationLabelOf}): `12 (Main) → 13`, `Branch: 12 (Main) → 13`,
 * `Side: 3 (loop end on Side) → 4`. The backend names a shot in its texts
 * the same way (`Shot Branch: 12 (Main) → 13 has zero length`), which
 * {@link warningMessageBody} relies on.
 */
export function shotLabelAt(src: IdentitySource, lineId: number, shotIndex: number, withLine = false): string {
  const from = stationLabelOf(src, { line_id: lineId, station_index: shotIndex });
  const to = stationLabelOf(src, { line_id: lineId, station_index: shotIndex + 1 });
  return formatShotLabel(from, to, withLine ? lineNameOf(src, lineId) : null);
}

/**
 * A shot pill's main part, `12 → 13`: each end's bare name, where the pill
 * draws each foreign end's dot and loop mark and colours an end of another
 * line's half by that line ({@link stationPillLabelOf}).
 */
export function shotPillLabelAt(src: IdentitySource, lineId: number, shotIndex: number): string {
  return formatShotLabel(
    stationPillLabelOf(src, { line_id: lineId, station_index: shotIndex }),
    stationPillLabelOf(src, { line_id: lineId, station_index: shotIndex + 1 }),
  );
}

/** The 0-based position of shot `shotId` in line `lineId`, or null when it is not there. */
export function shotIndexOf(src: IdentitySource, lineId: number, shotId: number): number | null {
  const data = src.surveyData;
  const meta = data?.lines.find((l) => l.id === lineId);
  if (data && meta) {
    for (let k = 0; k < meta.shotCount; k++) {
      if (data.shotIds[meta.shotOffset + k] === shotId) return k;
    }
    return null;
  }
  const shots = src.lines?.find((l) => l.id === lineId)?.shots;
  const k = shots?.findIndex((s) => s.id === shotId) ?? -1;
  return k < 0 ? null : k;
}

/** {@link shotLabelAt} for a shot addressed by its id; null when the shot is unknown. */
export function shotLabelOf(src: IdentitySource, lineId: number, shotId: number, withLine = false): string | null {
  const k = shotIndexOf(src, lineId, shotId);
  return k === null ? null : shotLabelAt(src, lineId, k, withLine);
}

/** The identity labels bound to one source; what {@link useIdentityLabels} returns. */
export interface IdentityLabels {
  stationName: (ref: StationRef) => string;
  lineStationNames: (lineId: number) => string[];
  /** Every slot of the station (see {@link stationSlotsOf}). */
  stationSlots: (ref: StationRef) => StationRef[];
  /** The line a rename to `name` would collide on (see {@link lineWithStationNamed}). */
  stationNameTakenOn: (ref: StationRef, name: string) => number | null;
  /** One slot per station (see {@link onePerStation}). */
  onePerStation: (refs: readonly StationRef[]) => StationRef[];
  /** The datum the station shows (see {@link datumOfStation}). */
  datumOf: <D extends { station_ref: StationRef }>(datums: readonly D[] | null | undefined, ref: StationRef) => D | undefined;
  /** The node's Station ID (groupings; see {@link stationIdOf}). */
  stationId: (ref: StationRef) => number | null;
  /** The Station ID the slot shows (displays; see {@link shownStationIdOf}). */
  shownStationId: (ref: StationRef) => number | null;
  /** How the slot shows its node (see {@link shownSlotOf}). */
  shown: (ref: StationRef) => ShownSlot | undefined;
  /** The home-line dot a foreign shot end carries (see {@link homeMarkOf}). */
  homeMark: (ref: StationRef) => number | null;
  /** The line whose colour a station pill takes (see {@link ownerLineOf}). */
  ownerLine: (ref: StationRef) => number | null;
  lineName: (lineId: number) => string;
  /** A slot's plain-text label, qualified for a dotted slot (see {@link stationLabelOf}). */
  station: (ref: StationRef, withLine?: boolean) => string;
  /** A station pill's bare main part (see {@link stationPillLabelOf}). */
  stationPill: (ref: StationRef) => string;
  /** A Plan pin's label (see {@link planStationLabelOf}). */
  planStation: (ref: StationRef) => string;
  /** A shot's plain-text label, each end qualified (see {@link shotLabelAt}). */
  shotAt: (lineId: number, shotIndex: number, withLine?: boolean) => string;
  /** A shot pill's bare main part (see {@link shotPillLabelAt}). */
  shotPillAt: (lineId: number, shotIndex: number) => string;
  /** {@link shotLabelAt} for a shot addressed by its id (see {@link shotLabelOf}). */
  shot: (lineId: number, shotId: number, withLine?: boolean) => string | null;
}

export function identityLabels(src: IdentitySource): IdentityLabels {
  return {
    stationName: (ref) => stationNameOf(src, ref),
    lineStationNames: (lineId) => lineStationNamesOf(src, lineId),
    stationSlots: (ref) => stationSlotsOf(src, ref),
    stationNameTakenOn: (ref, name) => lineWithStationNamed(src, ref, name),
    onePerStation: (refs) => onePerStation(src, refs),
    datumOf: (datums, ref) => datumOfStation(src, datums, ref),
    stationId: (ref) => stationIdOf(src, ref),
    shownStationId: (ref) => shownStationIdOf(src, ref),
    shown: (ref) => shownSlotOf(src, ref),
    homeMark: (ref) => homeMarkOf(src, ref),
    ownerLine: (ref) => ownerLineOf(src, ref),
    lineName: (lineId) => lineNameOf(src, lineId),
    station: (ref, withLine) => stationLabelOf(src, ref, withLine),
    stationPill: (ref) => stationPillLabelOf(src, ref),
    planStation: (ref) => planStationLabelOf(src, ref),
    shotAt: (lineId, shotIndex, withLine) => shotLabelAt(src, lineId, shotIndex, withLine),
    shotPillAt: (lineId, shotIndex) => shotPillLabelAt(src, lineId, shotIndex),
    shot: (lineId, shotId, withLine) => shotLabelOf(src, lineId, shotId, withLine),
  };
}

/**
 * A survey warning's message without the shot it opens with ("has zero
 * length"), for a view that shows the shot itself (the Warnings tab's shot
 * pill). The backend names the shot by its line and stations, each end
 * qualified as {@link shotLabelAt} qualifies it (`Shot Main Line: 4 → 5 has
 * zero length`, `Shot Branch: 12 (Main) → 13 has zero length`), so the
 * prefix is matched against the same label; a message that does not open
 * with it (a datum warning, or one computed before a rename) is returned
 * unchanged.
 */
export function warningMessageBody(
  labels: Pick<IdentityLabels, 'shotAt'>,
  warning: { lineId: number; shotIndex: number; message: string },
): string {
  const prefix = `Shot ${labels.shotAt(warning.lineId, warning.shotIndex, true)} `;
  return warning.message.startsWith(prefix) ? warning.message.slice(prefix.length) : warning.message;
}
