/**
 * Pure helpers extracted from DmpNewImportDialog for testability.
 *
 * These functions handle connection resolution and overlap-aware redirect
 * logic during the batch import config-building phase.
 */

import { isEmittedShot } from '../components/common/emittedShots';
import type { EditableShot } from '../components/common/ShotTableEditor';
import type { VirtualConnection } from './segmentContext';
import type { CrossSectionSuggestion, DmpSection, ImportSectionConfig } from '../types/commands';
import type { SectionData, ShotData } from '../types/project';
import type { Person } from '../types/ipc.generated';
import { computeOverlapIds } from './overlapHelpers';

// ─── Shared types ────────────────────────────────────────────────────────────

export interface SectionEditState {
  name: string;
  /**
   * The section's surveyors and explorers. Absent means "not set for this
   * section": the wizard's step-1 defaults apply ({@link resolveImportPeople}).
   * Overlap treatment creates edit states of its own without them, and must
   * not wipe the defaults by doing so.
   */
  surveyors?: Person[];
  explorers?: Person[];
  date: string;
  color: string;
  direction: 'in' | 'out';
  quality: number;
  /**
   * Reverse this section end-to-end on import (#24), so the file's last station
   * becomes station 0. A one-shot transform applied by the backend *before* the
   * section is created, so placement and station-comment indices address the
   * final order. Independent of `direction`, which keeps its azimuth/averaging
   * meaning.
   */
  reverse: boolean;
  shots: EditableShot[];
}

/** Merge station info recorded when user confirms a suggestion. */
export interface ConfirmedOverlapInfo {
  /** Fields to match the suggestion in the pool (reference equality breaks after save/restore). */
  importStationIndex: number;
  matchedShots: number;
  existingMatchedShots?: number;
  direction: 'forward' | 'reverse';
  existingStationIndex: number;
  targetType: 'existing' | 'importing';
  existingSectionId: number | null;
  targetImportIndex: number | null;
  /** User-selected merge station (determines which overlap shots are actionable). */
  mergeImportStation: number;
}

// ─── resolveImportPeople / importCreditFields ────────────────────────────────

/** The surveyors and explorers an imported section is credited with. */
export interface ImportPeople {
  surveyors: Person[];
  explorers: Person[];
}

/** The wizard's step-1 defaults, as `DmpSelectionConfig` carries them. */
export interface ImportPeopleDefaults {
  /** Optional on read: a selection config built elsewhere (a test, a spec seed) may lack them. */
  defaultSurveyors?: Person[] | null;
  defaultExplorers?: Person[] | null;
}

/**
 * The people a section leaves the wizard with: the section's own lists where
 * it has them, else the step-1 defaults, per role. An empty own list is a
 * choice (the user removed everyone) and is kept.
 */
export function resolveImportPeople(
  edits: Pick<SectionEditState, 'surveyors' | 'explorers'> | undefined,
  defaults: ImportPeopleDefaults,
): ImportPeople {
  return {
    surveyors: edits?.surveyors ?? defaults.defaultSurveyors ?? [],
    explorers: edits?.explorers ?? defaults.defaultExplorers ?? [],
  };
}

/**
 * The credit half of a backend `ImportSectionConfig`. The dialog's people and
 * date OVERRIDE every imported shot (`surveyors` / `explorers` /
 * `override_date`), and are sent only when set: an empty list would replace
 * what the file said with nobody. The legacy free-text `surveyor` is never
 * sent; the legacy `date` keeps carrying the same day as the per-section
 * template the backend gives uncredited shots, which is what it always meant.
 */
export function importCreditFields(
  people: ImportPeople,
  date: string | null,
): Pick<ImportSectionConfig, 'surveyor' | 'date' | 'surveyors' | 'explorers' | 'override_date'> {
  return {
    surveyor: null,
    date,
    ...(people.surveyors.length > 0 ? { surveyors: people.surveyors } : {}),
    ...(people.explorers.length > 0 ? { explorers: people.explorers } : {}),
    ...(date !== null ? { override_date: date } : {}),
  };
}

// ─── resolveImportSectionDate ────────────────────────────────────────────────

/**
 * The date an imported section leaves the wizard with.
 *
 * `date` is `Option<NaiveDate>` on the Rust side (`models/section.rs`), and
 * chrono parses `""` as TOO_SHORT, whose Display is "premature end of input" -
 * so an empty string is not "no date", it is a malformed argument that makes
 * Tauri reject the WHOLE `import_sections` call before it reaches the command
 * (#478). A cleared date field means null.
 *
 * Both halves are load-bearing and they say different things. The `??` chain
 * keeps "the user cleared it" from falling back to the wizard default or to
 * the file's own date - clearing the field has to mean cleared. The trailing
 * `|| null` then turns that empty string into the absence the backend
 * understands. One function rather than the same expression copied per
 * emitter, because every payload builder in the wizard has to agree: this is
 * the whole invariant, and it is exactly the half that went missing.
 *
 * `fileDate` arrives as the DMP's own timestamp, so the day is split off it.
 */
export function resolveImportSectionDate(
  editedDate: string | null | undefined,
  defaultDate: string | null | undefined,
  fileDate: string | null | undefined,
): string | null {
  return (editedDate ?? defaultDate ?? fileDate?.split(/[T ]/)[0] ?? null) || null;
}

// ─── buildPlacementSectionData ───────────────────────────────────────────────

/**
 * The section metadata that rides along with a segment handed to map placement.
 *
 * Extracted from `buildSegmentPlacementPayload`, which is a `useCallback`
 * closed over a dozen refs and therefore unreachable from any test - so the
 * wizard's SECOND payload emitter could not be asked what it emits, and its
 * copy of the date rule (#478) was pinned by nothing.
 *
 * Deliberately NOT merged with `buildBatchImport`'s config builder. That one
 * resolves its palette slot from the section's position in the selection with
 * a `>= 0 ? pos : 0` floor and also carries `reverse`; this one indexes the
 * palette raw, so an anchor outside the selection falls through to the default
 * colour. Unifying them would change a colour, which is not what this is for.
 *
 * The section data only drives the placement overlay: the segment is then
 * imported through `buildBatchImport`, which carries the people. So no people
 * ride along here (`surveyor` stays null).
 */
export function buildPlacementSectionData(params: {
  dmpSection: Pick<DmpSection, 'name' | 'date' | 'direction'>;
  edits: SectionEditState | undefined;
  defaultDate: string | null;
  /** Section colours, in selection order. */
  palette: readonly string[];
  /** Slot in `palette` for this section; a negative value means "not selected". */
  paletteIndex: number;
  shots: ShotData[];
}): SectionData {
  const { dmpSection, edits, defaultDate, palette, paletteIndex, shots } = params;
  return {
    name: edits?.name ?? dmpSection.name,
    surveyor: null,
    date: resolveImportSectionDate(edits?.date, defaultDate, dmpSection.date),
    color: (edits?.color || null) ?? palette[paletteIndex % palette.length] ?? '#4ecdc4',
    direction: edits?.direction ?? dmpSection.direction,
    quality: edits?.quality ?? 1.0,
    shots,
  };
}

// ─── adjustStationForDeletedShots ────────────────────────────────────────────

/**
 * When overlap shots are deleted from an import section, station indices
 * shift. This counts EMITTED shots before the original station position to
 * compute the adjusted index in the filtered shot array.
 *
 * "Emitted", not merely non-deleted: the payload builders also drop wholly
 * blank new rows, so counting one here left the adjustment expecting a longer
 * section than was sent, and `import_station` pointed a station too far
 * downstream.
 */
export function adjustStationForDeletedShots(
  shots: EditableShot[],
  originalStation: number,
): number {
  let adjusted = 0;
  for (const s of shots) {
    if (!isEmittedShot(s)) continue;
    if (!s.isNew && s.id >= originalStation) break;
    adjusted++;
  }
  return adjusted;
}

// ─── findOverlapForStation ───────────────────────────────────────────────────

/**
 * Find the overlap whose deleted zone contains `targetStation` for a section.
 *
 * Checks confirmedOverlaps first, then falls back to searching the suggestion
 * pool. Handles both existing-to-import and import-to-import overlaps.
 */
export function findOverlapForStation(
  sectionIndex: number,
  targetStation: number,
  confirmedOverlaps: Map<number, ConfirmedOverlapInfo>,
  fullPool: CrossSectionSuggestion[],
): ConfirmedOverlapInfo | null {
  // 1. Check confirmed overlap - the merge station determines the deleted zone boundary
  const confirmed = confirmedOverlaps.get(sectionIndex);
  if (confirmed) {
    const isFront = confirmed.importStationIndex === 0;
    const inDeletedZone = isFront
      ? targetStation < confirmed.mergeImportStation
      : targetStation > confirmed.mergeImportStation;
    if (inDeletedZone) return confirmed;
  }

  // 2. Fall back to suggestion pool - for unconfirmed overlaps ALL overlap shots
  //    are deleted, so the boundary is at the edge of the overlap region.
  for (const s of fullPool) {
    if (s.importSectionIndex !== sectionIndex) continue;
    // If the opposite direction is already confirmed (A→B confirmed, evaluating B→A),
    // ignore this unconfirmed reverse candidate. Redirecting through it can detach a
    // confirmed pair when overlap edits are processed in a different order.
    if (s.targetType === 'importing' && s.targetImportIndex != null) {
      const reverseConfirmed = confirmedOverlaps.get(s.targetImportIndex);
      if (
        reverseConfirmed &&
        reverseConfirmed.targetType === 'importing' &&
        reverseConfirmed.targetImportIndex === sectionIndex
      ) {
        continue;
      }
    }

    const isFront = s.importStationIndex === 0;
    const mergeStation = isFront
      ? s.matchedShots
      : s.importStationIndex - s.matchedShots;

    const inDeletedZone = isFront
      ? targetStation < mergeStation
      : targetStation > mergeStation;

    if (inDeletedZone) {
      return {
        importStationIndex: s.importStationIndex,
        matchedShots: s.matchedShots,
        existingMatchedShots: s.existingMatchedShots,
        direction: s.direction,
        existingStationIndex: s.existingStationIndex,
        targetType: s.targetType as 'existing' | 'importing',
        existingSectionId: s.existingSectionId,
        targetImportIndex: s.targetImportIndex ?? null,
        mergeImportStation: mergeStation,
      };
    }
  }

  return null;
}

// ─── findReverseImportConnection ─────────────────────────────────────────────

/**
 * Find a reverse import-to-import connection for a section that is only a TARGET.
 * When section B is only referenced as the target of A→B, B has no connectionMap entry.
 * This searches allConnections for such a connection where the import side IS already
 * emitted, and returns a synthetic reversed connection so B can attach to A.
 */
export function findReverseImportConnection(
  idx: number,
  allConnections: VirtualConnection[],
  emittedPosition: Map<number, number>,
): VirtualConnection | undefined {
  for (const c of allConnections) {
    if (c.targetType !== 'importing') continue;
    if (c.targetImportIndex !== idx) continue;
    if (!emittedPosition.has(c.importSectionIndex)) continue;
    // Create a synthetic reversed connection: idx attaches to c.importSectionIndex
    return {
      importSectionIndex: idx,
      importStationIndex: c.targetImportStationIndex ?? 0,
      targetType: 'importing',
      targetImportIndex: c.importSectionIndex,
      targetImportStationIndex: c.importStationIndex,
    };
  }
  return undefined;
}

// ─── redirectDeletedTargets ──────────────────────────────────────────────────

/**
 * Redirect connections whose target station falls in a deleted overlap zone.
 *
 * When section B overlaps with A (existing) or C (importing) and B's overlap
 * shots are deleted, any connection targeting a deleted station in B must be
 * redirected to the corresponding station in the section that replaced those
 * shots. Handles both existing and importing overlaps, and follows chains
 * (B→C→D) up to MAX_DEPTH.
 *
 * Mutates connections in place.
 */
export function redirectDeletedTargets(
  connections: VirtualConnection[],
  sectionEdits: Map<number, SectionEditState>,
  confirmedOverlaps: Map<number, ConfirmedOverlapInfo>,
  fullPool: CrossSectionSuggestion[],
): void {
  const MAX_DEPTH = 10;

  for (const conn of connections) {
    if (conn.targetType !== 'importing') continue;
    if (conn.targetImportIndex == null || conn.targetImportStationIndex == null) continue;

    let targetIdx = conn.targetImportIndex;
    let targetStation = conn.targetImportStationIndex;
    let depth = 0;

    while (depth < MAX_DEPTH) {
      const overlap = findOverlapForStation(targetIdx, targetStation, confirmedOverlaps, fullPool);
      if (!overlap) break;

      // For import-to-import overlaps, the pool may contain suggestions for both
      // directions (A→B and B→A).  applyAllOverlapTreatments processes them in order
      // and only the first direction actually deletes the import side's shots; the
      // other direction's import side is already treated (interpolated, not deleted).
      // Verify by checking whether the overlap shots are actually deleted in sectionEdits.
      if (overlap.targetType === 'importing') {
        const edits = sectionEdits.get(targetIdx);
        if (edits) {
          const overlapIds = computeOverlapIds(overlap.importStationIndex, overlap.matchedShots);
          const hasDeletedOverlap = edits.shots.some(
            s => !s.isNew && overlapIds.has(s.id) && s.isDeleted,
          );
          if (!hasDeletedOverlap) {
            break;
          }
        }
      }

      // Compute the offset into the overlap and map to the next section
      const isFront = overlap.importStationIndex === 0;
      const offset = isFront
        ? targetStation
        : overlap.importStationIndex - targetStation;

      const newStation = overlap.direction === 'forward'
        ? overlap.existingStationIndex + offset
        : overlap.existingStationIndex - offset;

      depth++;

      if (overlap.targetType === 'existing' && overlap.existingSectionId != null) {
        // Redirect to existing section - end of chain
        conn.targetType = 'existing';
        conn.existingSectionId = overlap.existingSectionId;
        conn.existingStationIndex = newStation;
        delete conn.targetImportIndex;
        delete conn.targetImportStationIndex;
        break;
      } else if (overlap.targetImportIndex != null) {
        // Redirect to another import section - continue chain
        targetIdx = overlap.targetImportIndex;
        targetStation = newStation;
      } else {
        break;
      }
    }

    // Update connection if redirected to another import section (not existing)
    if (conn.targetType === 'importing' && depth > 0) {
      conn.targetImportIndex = targetIdx;
      conn.targetImportStationIndex = targetStation;
    }
  }
}
