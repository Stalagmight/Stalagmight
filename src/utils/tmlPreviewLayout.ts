/**
 * Lay a parsed Ariane file out in ONE frame so the import dialog can draw it
 * before anything is imported (#425).
 *
 * `SectionPreview` already knows how to draw several sections at given
 * offsets - that is what the mNemo wizard's segment view uses - but it speaks
 * `DmpSection`. This module is the adapter plus the layout: it applies the same
 * shot filter and the same placement rules the Rust converter applies
 * (`convert_tml_with_options` / `determine_placement` in
 * `src-tauri/crates/parsers/src/parsers/tml/`), so what the user sees is what
 * the import will build.
 *
 * It is a PREVIEW, deliberately reimplemented rather than fetched:
 *   - it re-runs on every checkbox click, which no IPC round-trip should;
 *   - it draws PLAN geometry only (x/y), so it ignores closures and unit
 *     conversion (the parser has already converted feet to metres).
 * What it must agree with the backend on is TOPOLOGY - which section hangs off
 * which, and which sections lose their anchor when a section is deselected -
 * because that is the question the dialog is asking the user; and, since the
 * offsets are station positions, the GEOMETRY of each section, which is the
 * backend's walk (`walkStations`, #576 D15) over the depths the importer
 * resolves.
 *
 * North reference: an Ariane file declares ONE for every section
 * (`tml_azimuth_reference`): true north, or magnetic with an IGRF declination
 * when the file is georeferenced and dated, else magnetic with none. The IGRF
 * model lives only in the backend, so the layout walks the readings as
 * measured. Every section shares the one reference, so that is a rotation of
 * the whole drawing about its origin - the placement of each section relative
 * to the others, which is what the dialog asks about, is exact.
 */
import type { DmpSection, DmpShot, TmlFile, TmlSection, TmlShot } from '../types/commands';
import { importableShots, survivesImport } from './tmlImportStats';
import { walkStations, type WalkShot } from './stationWalk';

/** One section as the preview needs it: geometry plus where it sits. */
export interface TmlPreviewSection {
  /** Index into `TmlFile.sections` - the same numbering the dialog's rows,
   *  the selection set and the backend's `selected_sections` all use. */
  index: number;
  name: string;
  color: string;
  /** The section's own shots, adapted for `SectionPreview`. */
  section: DmpSection;
  /** Meters east/north of the layout's origin. */
  offsetX: number;
  offsetY: number;
  /**
   * True when this section will be placed at the ORIGIN rather than connected
   * to the rest of the survey - i.e. exactly when the backend takes
   * `determine_placement`'s final fallback and emits its warning.
   *
   * Three things are therefore NOT re-anchored, and each was wrong at some
   * point: the root (it belongs at the origin), a section whose own first shot
   * carries usable coordinates (the backend prefers that over the fallback),
   * and a section whose parent resolved normally. A section with NO parent id
   * at all IS re-anchored - the file orphan case, which the backend has always
   * warned about and this used to stay silent on.
   */
  reanchored: boolean;
}

export interface TmlPreviewLayout {
  sections: TmlPreviewSection[];
  /** How many entries carry `reanchored`. */
  reanchoredCount: number;
}

export interface TmlPreviewOptions {
  includeExcluded: boolean;
  /** Section indices to lay out. `null` means every section. */
  selected: ReadonlySet<number> | null;
}

/**
 * A `DmpShot` carrying an Ariane shot's geometry as the importer builds it.
 * Ariane records one azimuth per shot, so both legs get it (as
 * `normalize_tml_shot` does), and `depthIn` is the RESOLVED start depth, so the
 * preview draws the plan length the offsets were walked with.
 */
function toPreviewShot(shot: TmlShot, depthIn: number): DmpShot {
  return {
    shot_type: 'Standard',
    heading_in: shot.azimuth,
    heading_out: shot.azimuth,
    length: shot.length,
    depth_in: depthIn,
    depth_out: shot.depth,
    pitch_in: 0,
    pitch_out: 0,
    left: shot.left,
    right: shot.right,
    up: shot.up,
    down: shot.down,
    temperature: 0,
    hour: 0,
    minute: 0,
    second: 0,
    marker_index: 0,
  };
}

function toPreviewSection(section: TmlSection, shots: TmlShot[], walk: WalkShot[]): DmpSection {
  return {
    version: 0,
    name: section.name,
    direction: 'in',
    date: section.date ?? '',
    shots: shots.map((shot, i) => toPreviewShot(shot, walk[i]!.depth_in)),
    content_hash: '',
  };
}

/** Ariane's "this shot starts where the previous one ended" `DepthIn`
 *  (`schema::DEPTH_IN_SAME_AS_PREVIOUS`): a spelling, matched exactly. */
const DEPTH_IN_SAME_AS_PREVIOUS = -1;

/**
 * The kept shots as the importer hands them to the walk
 * (`convert_shots_to_shot_data`): one azimuth on both legs, and the "same as
 * previous" start depth resolved (`resolve_tml_depth_in`) - to the previous
 * KEPT shot's depth, or for the first kept shot to the end of the dropped lead
 * shots (closure rows are no leg), else the section's start depth.
 *
 * Taken literally the sentinel is a depth change of (depth + 1) m, which folds
 * a shot to nothing once the passage is deeper than the tape is long.
 */
function walkShots(section: TmlSection, kept: TmlShot[], includeExcluded: boolean): WalkShot[] {
  let droppedLeadEnd: number | undefined;
  for (const shot of section.shots) {
    if (survivesImport(shot, includeExcluded)) break;
    if (shot.shot_type !== 'Closure') droppedLeadEnd = shot.depth;
  }
  return kept.map((shot, i) => ({
    azimuth_in: shot.azimuth,
    azimuth_out: shot.azimuth,
    length: shot.length,
    depth_in:
      shot.depth_in === DEPTH_IN_SAME_AS_PREVIOUS
        ? ((i > 0 ? kept[i - 1]!.depth : droppedLeadEnd) ?? section.start_depth)
        : shot.depth_in,
    depth_out: shot.depth,
  }));
}

/**
 * Whether a shot's coordinates would survive `GpsCoordinates::new`.
 *
 * Mirrors `GpsCoordinates::validate` (crates/models/src/station.rs) plus the
 * importer's own null-island guard: Ariane writes `0/0` for "no anchor". The
 * finiteness check is not decoration - every comparison against NaN is false,
 * so a `Math.abs(lat) > 0` test alone reports NaN as a usable anchor, which is
 * the same hole the Rust side had.
 */
export function isOfferableCoordinate(latitude: number, longitude: number): boolean {
  if (!Number.isFinite(latitude) || !Number.isFinite(longitude)) return false;
  if (Math.abs(latitude) > 90 || Math.abs(longitude) > 180) return false;
  // `Number.EPSILON` IS `f64::EPSILON` (both 2^-52), so this is the backend's
  // `<= f64::EPSILON` guard spelled the same way rather than approximated. A
  // plain `> 0` is not the same predicate: it calls (1e-300, 0) an anchor, and
  // the dialog would then preselect "use the file's position" and send an
  // explicit zero datum, georeferencing the survey next to Null Island where
  // the backend on its own would have left it at the origin.
  return Math.abs(latitude) > Number.EPSILON || Math.abs(longitude) > Number.EPSILON;
}

function hasUsableGps(shot: TmlShot | undefined): boolean {
  return shot !== undefined && isOfferableCoordinate(shot.latitude, shot.longitude);
}

/** A laid-out entry while the offsets are still being resolved. */
interface WorkingEntry {
  index: number;
  section: TmlSection;
  shots: TmlShot[];
  walk: WalkShot[];
  stations: Array<{ x: number; y: number }>;
  parent: { section: number; station: number } | undefined;
  offsetX: number;
  offsetY: number;
  reanchored: boolean;
}

/**
 * Where each selected section sits relative to the first one.
 *
 * The placement rules, in the backend's order (`determine_placement`):
 *   1. the ROOT - the first section the import will CREATE, i.e. the first one
 *      that is selected AND still has shots after the exclusion filter - sits
 *      at the origin;
 *   2. any other section merges its station 0 onto the station its
 *      `start_ariane_id` names, but only if that station belongs to a SELECTED
 *      section, since a deselected one is never created;
 *   3. failing that, a section with usable coordinates on its own raw first
 *      shot - imported or not - is anchored on those (drawn at the origin here
 *      - the preview has no georeferencing - but NOT reported as re-anchored,
 *      because the import will not put it at the origin);
 *   4. anything else falls back to the origin and IS reported.
 *
 * Rule 1 is the definition the backend now uses too, and it must stay that way:
 * deriving it from the selection alone ("lowest selected index") disagrees
 * whenever the exclusion filter empties that section, and the placement the
 * dialog collected then belongs to a section that is never built.
 *
 * Rule 2 can point forwards, so offsets are resolved by walking a
 * parent -> children adjacency breadth-first from the parentless entries: one
 * visit per entry. The fixed-point relaxation this replaces re-ran on every
 * checkbox click and needed one full pass per link of a chain.
 */
export function computeTmlPreviewLayout(
  tml: TmlFile,
  { includeExcluded, selected }: TmlPreviewOptions
): TmlPreviewLayout {
  const isSelected = (i: number) => selected === null || selected.has(i);

  // Ariane shot id -> (section index, station index) over the SELECTED sections
  // only, so a deselected parent resolves to nothing. Shot `i` ends at station
  // `i + 1`.
  //
  // Keyed by the plain-number Ariane id the JSON wire delivers (#576 C6: the
  // generated type used to say bigint, which a lookup could not trust).
  const idToLoc = new Map<number, { section: number; station: number }>();
  const kept: TmlShot[][] = tml.sections.map((section, i) => {
    const shots = importableShots(section, includeExcluded);
    if (isSelected(i)) {
      shots.forEach((shot, si) =>
        idToLoc.set(shot.id, { section: i, station: si + 1 })
      );
    }
    return shots;
  });

  // The entries the import will create, in file order. `entries[0]` is the
  // root by construction - no spread over the selection, which throws
  // RangeError on a set big enough to have one entry per section.
  const entries: WorkingEntry[] = [];
  for (const [index, section] of tml.sections.entries()) {
    if (!isSelected(index) || kept[index]!.length === 0) continue;
    const shots = kept[index]!;
    const walk = walkShots(section, shots, includeExcluded);
    const isRoot = entries.length === 0;
    // The parent the backend would use: the raw first shot's `from_id`, then
    // the first surviving shot's, for a section whose lead shots were excluded.
    const candidates = [section.start_ariane_id, shots[0]?.from_id]
      .filter((id) => id !== undefined)
      // Ariane spells "names nothing" as a negative id (`-1`), on both the
      // section's declared start and a shot's `from_id`.
      .filter((id) => id >= 0);
    const parent = isRoot
      ? undefined
      : candidates
          .map((id) => idToLoc.get(id))
          .find((loc) => loc !== undefined && loc.section !== index);
    entries.push({
      index,
      section,
      shots,
      walk,
      // Station positions in the section's OWN frame, station 0 at (0, 0).
      stations: walkStations(walk),
      parent,
      offsetX: 0,
      offsetY: 0,
      // The RAW first shot, as `gps_placement` reads it: Ariane's GPS rides on
      // the shots leaving the START, and when that shot is excluded the
      // backend still places the section by it, carrying the datum past the
      // excluded leg (#576 D39(j)). Testing the first IMPORTED shot badged
      // such a section re-anchored while the import placed it by GPS.
      reanchored: !isRoot && parent === undefined && !hasUsableGps(section.shots[0]),
    });
  }

  // parent index -> children, then one breadth-first sweep. An entry whose
  // parent is not itself laid out never enters the queue, so a cycle simply
  // leaves its members at the origin instead of looping.
  const positionOf = new Map<number, WorkingEntry>();
  for (const entry of entries) positionOf.set(entry.index, entry);
  const children = new Map<number, WorkingEntry[]>();
  const queue: WorkingEntry[] = [];
  for (const entry of entries) {
    const parentIndex = entry.parent?.section;
    if (parentIndex === undefined || !positionOf.has(parentIndex)) {
      queue.push(entry);
      continue;
    }
    const siblings = children.get(parentIndex);
    if (siblings) siblings.push(entry);
    else children.set(parentIndex, [entry]);
  }
  for (let head = 0; head < queue.length; head++) {
    const parent = queue[head]!;
    for (const child of children.get(parent.index) ?? []) {
      // `child.parent` is defined for everything in `children`.
      const anchor = parent.stations[child.parent!.station];
      if (anchor) {
        child.offsetX = parent.offsetX + anchor.x;
        child.offsetY = parent.offsetY + anchor.y;
      }
      // A station index past the parent's end is not reachable from a file the
      // backend parsed; leave the child at the origin rather than emit NaN, and
      // keep walking so ITS children still resolve.
      queue.push(child);
    }
  }

  return {
    sections: entries.map((entry) => ({
      index: entry.index,
      name: entry.section.name,
      color: entry.section.color,
      section: toPreviewSection(entry.section, entry.shots, entry.walk),
      offsetX: entry.offsetX,
      offsetY: entry.offsetY,
      reanchored: entry.reanchored,
    })),
    reanchoredCount: entries.reduce((n, entry) => n + (entry.reanchored ? 1 : 0), 0),
  };
}
