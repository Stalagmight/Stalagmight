/**
 * Pure draft-row helpers for the data-table shots grid.
 *
 * The grid always shows one empty "new shot" row at the end (the end draft);
 * "Insert shot above/below" adds more drafts anchored before/after a real
 * shot. Drafts live entirely in DataTable state until their required fields
 * are present, then commit via create_shot with the anchor as the insertion
 * point. These helpers own the anchor bookkeeping: interleaving drafts with
 * the section's shots into the grid's row list, mapping a grid insertion
 * boundary to an anchor, completeness, and the missing-field marking.
 *
 * Kept free of React/store imports so it is unit-testable in the node vitest env.
 */
import type { Shot, ShotData, ShotUpdates } from '../../types/project';
import { AZIMUTH_AVG_KEY } from './dataTableEdits';

/** Where a draft sits relative to the real shots. `'end'` is the always-present
 *  trailing draft; the id anchors pin an inserted draft to a neighbour shot
 *  (surviving unrelated inserts/deletes - a deleted anchor prunes the draft). */
export type DraftAnchor = { afterShotId: number } | { beforeShotId: number } | 'end';

export interface ShotDraft {
  /** Negative, unique per draft and never reused - doubles as the grid row id
   *  (real shot ids are positive). Only the initial end draft, before the table's
   *  key counter has run, is {@link END_DRAFT_KEY}; every later end draft takes a
   *  fresh counter key like any other draft (#576 RV4-FE-FEA-3). */
  key: number;
  anchor: DraftAnchor;
  data: Partial<ShotData>;
  /**
   * An end-anchored row created only to receive a row of a multi-row paste that
   * ran past the trailing blank. It renders at the end like the canonical blank
   * - which is what keeps the pasted rows in order - but it is NOT that row, so
   * committing or discarding it removes it instead of respawning a blank. Every
   * end draft respawning is why a 3-row paste used to leave 3 empty rows behind.
   */
  ephemeral?: true;
}

/** One grid row: a real shot (with its index into the shots array) or a draft. */
export type RowEntry =
  | { kind: 'shot'; shot: Shot; shotIndex: number }
  | { kind: 'draft'; draft: ShotDraft };

/** Sentinel key of the always-present end-of-table draft. */
export const END_DRAFT_KEY = -1;

/** A fresh, empty end draft. Callers allocate keys from a monotonic counter so
 *  a key is NEVER reused - grid-internal per-cell state (missing-cell marks)
 *  is keyed by row identity and must not leak onto a later draft. */
export const endDraft = (key: number = END_DRAFT_KEY): ShotDraft => ({ key, anchor: 'end', data: {} });

/** An end row for pasted overflow - see {@link ShotDraft.ephemeral}. */
export const overflowDraft = (key: number): ShotDraft => ({
  key,
  anchor: 'end',
  data: {},
  ephemeral: true,
});

/** Whether a committed/discarded draft respawns a fresh trailing blank. Only the
 *  canonical end row does; overflow rows simply go. */
export const respawnsBlank = (d: ShotDraft): boolean =>
  d.anchor === 'end' && d.ephemeral !== true;

/** Finite-number guard. Exported because "this cell holds a measurement" is one
 *  rule, shared by the draft checks here, the dialog's row helpers and the shot
 *  editor's missing-cell marking - three copies of it would eventually drift. */
export const isFiniteNumber = (v: unknown): v is number =>
  typeof v === 'number' && Number.isFinite(v);

/** Local shorthand: this module reads the guard on nearly every line. */
const fin = isFiniteNumber;

/**
 * Whether a written cell is indistinguishable from never having been touched:
 * `kind` defaults to Normal, everything else is empty when null, blank or NaN.
 *
 * Judged by VALUE, never by key presence. A cleared optional WRITES its key
 * (`comment: null`, `pitch_in: null` out of `buildShotUpdates`) rather than
 * deleting it, so an emptied row still carries keys - and counting them read
 * that row as half-filled, which painted required cells red on a visibly empty
 * row and disabled Create with nothing on screen to explain why.
 */
export function isDefaultCell(key: string, value: unknown): boolean {
  if (value === null || value === undefined || value === '') return true;
  if (typeof value === 'number' && !Number.isFinite(value)) return true;
  if (key === 'kind') return value === 'Normal';
  return false;
}

const isBlankData = (data: Partial<ShotData>): boolean =>
  Object.entries(data).every(([key, value]) => isDefaultCell(key, value));

/**
 * A draft holding nothing: every cell it has ever been given is back at its
 * default. THE blankness rule - the trailing invitation row stays quiet, submit
 * drops it, and closing does not offer to discard it.
 */
export const isBlankDraft = (draft: ShotDraft): boolean => isBlankData(draft.data);

/** Build the grid's row list: each draft adjacent to its anchor shot (after-
 *  anchored drafts right below it, before-anchored right above, same-anchor
 *  drafts in array order), `'end'` drafts last. Drafts whose anchor shot no
 *  longer exists are pruned (not emitted). */
export function interleaveRows(shots: Shot[], drafts: ShotDraft[]): RowEntry[] {
  const before = new Map<number, ShotDraft[]>();
  const after = new Map<number, ShotDraft[]>();
  const end: ShotDraft[] = [];
  const ids = new Set(shots.map((s) => s.id));
  const push = (map: Map<number, ShotDraft[]>, id: number, d: ShotDraft) => {
    const arr = map.get(id);
    if (arr) arr.push(d);
    else map.set(id, [d]);
  };
  for (const d of drafts) {
    if (d.anchor === 'end') end.push(d);
    else if ('afterShotId' in d.anchor) {
      if (ids.has(d.anchor.afterShotId)) push(after, d.anchor.afterShotId, d);
    } else if (ids.has(d.anchor.beforeShotId)) {
      push(before, d.anchor.beforeShotId, d);
    }
  }
  const entries: RowEntry[] = [];
  shots.forEach((shot, shotIndex) => {
    for (const draft of before.get(shot.id) ?? []) entries.push({ kind: 'draft', draft });
    entries.push({ kind: 'shot', shot, shotIndex });
    for (const draft of after.get(shot.id) ?? []) entries.push({ kind: 'draft', draft });
  });
  for (const draft of end) entries.push({ kind: 'draft', draft });
  return entries;
}

/** Drop drafts whose anchor shot was deleted (`'end'` drafts always survive).
 *  Returns the input array when nothing changed, so it is setState-friendly. */
export function pruneDrafts(drafts: ShotDraft[], shots: Shot[]): ShotDraft[] {
  const ids = new Set(shots.map((s) => s.id));
  const kept = drafts.filter(
    (d) =>
      d.anchor === 'end' ||
      ids.has('afterShotId' in d.anchor ? d.anchor.afterShotId : d.anchor.beforeShotId),
  );
  return kept.length === drafts.length ? drafts : kept;
}

/** Insert `count` blank drafts at a grid insertion boundary (an index into
 *  {@link interleaveRows}' entries, as the grid's onInsertRows reports it).
 *  The new drafts anchor to the boundary's neighbour: after the shot above,
 *  before the first shot at the very top, and between same-anchor drafts they
 *  keep the clicked position. Returns the new drafts array, or `null` when
 *  there is nothing to anchor to (no shots yet - the end draft IS the insert
 *  affordance there). */
export function insertDraftsAt(
  shots: Shot[],
  drafts: ShotDraft[],
  beforeIndex: number,
  count: number,
  nextKey: () => number,
): ShotDraft[] | null {
  const firstShot = shots[0];
  const lastShot = shots[shots.length - 1];
  if (firstShot === undefined || lastShot === undefined || count < 1) return null;
  const entries = interleaveRows(shots, drafts);
  const boundary = Math.max(0, Math.min(beforeIndex, entries.length));
  const prev = entries[boundary - 1];
  const next = entries[boundary];
  let anchor: DraftAnchor;
  if (prev === undefined) {
    // Top boundary: join a draft already sitting above the first shot, else
    // anchor before the first shot itself.
    anchor =
      next !== undefined && next.kind === 'draft' && next.draft.anchor !== 'end'
        ? next.draft.anchor
        : { beforeShotId: firstShot.id };
  } else if (prev.kind === 'shot') {
    anchor = { afterShotId: prev.shot.id };
  } else if (prev.draft.anchor === 'end') {
    // Below the end draft - there is no further row, so after the last shot.
    anchor = { afterShotId: lastShot.id };
  } else {
    anchor = prev.draft.anchor;
  }
  const created: RowEntry[] = Array.from({ length: count }, () => ({
    kind: 'draft' as const,
    draft: { key: nextKey(), anchor, data: {} },
  }));
  // Splice into the display order and read the drafts back out: same-anchor
  // groups keep array order through interleaveRows, so the new drafts land
  // exactly at the clicked boundary.
  const spliced = [...entries.slice(0, boundary), ...created, ...entries.slice(boundary)];
  return spliced.flatMap((e) => (e.kind === 'draft' ? [e.draft] : []));
}

/** Shot-shaped grid row for a draft: `id` is the (negative) draft key; unset
 *  required numbers render as NaN (the grid formats non-finite as blank). */
export function materializeDraft(draft: ShotDraft): Shot {
  const d = draft.data;
  return {
    id: draft.key,
    azimuth_in: (d.azimuth_in ?? NaN) as number,
    azimuth_out: (d.azimuth_out ?? NaN) as number,
    length: (d.length ?? NaN) as number,
    depth_in: (d.depth_in ?? NaN) as number,
    depth_out: (d.depth_out ?? NaN) as number,
    pitch_in: d.pitch_in ?? null,
    pitch_out: d.pitch_out ?? null,
    left: d.left ?? null,
    right: d.right ?? null,
    up: d.up ?? null,
    down: d.down ?? null,
    comment: d.comment ?? null,
    kind: d.kind ?? 'Normal',
    marker_index: null,
    temperature: null,
  };
}

/** The draft's still-missing required fields, split into visible columns (tint
 *  the cell red) and hidden ones (tint the columns button / menu row instead).
 *  Inserted drafts surface them immediately - the user asked for a row there,
 *  so it is incomplete by construction. The end draft is an invitation, not an
 *  obligation: it stays quiet until it holds a real value ({@link isBlankDraft},
 *  which reads the values rather than counting keys - clearing an optional field
 *  on it leaves a key behind and must not turn the row red). */
export function draftMissing(
  draft: ShotDraft,
  hiddenCols: ReadonlySet<string>,
  azimuthMode: 'avg' | 'inout' | 'off',
): { cells: Set<string>; hiddenProblems: Set<string> } {
  const cells = new Set<string>();
  const hiddenProblems = new Set<string>();
  const d = draft.data;
  if (draft.anchor === 'end' && isBlankDraft(draft)) return { cells, hiddenProblems };
  const need = (colKey: string, menuKey: string) => {
    if (hiddenCols.has(colKey)) hiddenProblems.add(menuKey);
    else cells.add(colKey);
  };
  if (!fin(d.length)) need('length', 'length');
  if (!fin(d.depth_in)) need('depth_in', 'depthIn');
  if (!fin(d.depth_out)) need('depth_out', 'depthOut');
  if (!fin(d.azimuth_in) && !fin(d.azimuth_out)) {
    if (azimuthMode === 'avg') cells.add(AZIMUTH_AVG_KEY);
    else if (azimuthMode === 'inout') {
      cells.add('azimuth_in');
      cells.add('azimuth_out');
    } else {
      hiddenProblems.add('azimuth');
    }
  }
  return { cells, hiddenProblems };
}

/** Whether a draft has everything it needs to commit: length > 0, both depths,
 *  and at least one azimuth (inclination/LRUD/comment stay optional).
 *
 *  Split out of {@link completeDraftData} so a caller that only needs the answer
 *  does not pay for the ShotData it would otherwise allocate and discard - the
 *  new-section dialog asks this for every row on every keystroke to decide
 *  whether Create is available. The rule itself still lives in ONE place; the
 *  builder below delegates to it. */
export function isDraftComplete(draft: ShotDraft): boolean {
  const d = draft.data;
  const azimuth = fin(d.azimuth_in) ? d.azimuth_in : d.azimuth_out;
  return fin(d.length) && d.length > 0 && fin(d.depth_in) && fin(d.depth_out) && fin(azimuth);
}

/** The full ShotData to commit, or `null` while required fields are missing
 *  (see {@link isDraftComplete}; the missing azimuth leg duplicates the other). */
export function completeDraftData(draft: ShotDraft): ShotData | null {
  const d = draft.data;
  if (!isDraftComplete(draft)) return null;
  const azimuth = fin(d.azimuth_in) ? d.azimuth_in : d.azimuth_out;
  // Unreachable: `isDraftComplete` above is the rule and has already established
  // all four. This exists solely so TypeScript narrows them to `number` - a type
  // predicate on `draft` cannot reach through to its individual `data` fields.
  if (!fin(d.length) || !fin(d.depth_in) || !fin(d.depth_out) || !fin(azimuth)) {
    return null;
  }
  return {
    azimuth_in: fin(d.azimuth_in) ? d.azimuth_in : azimuth,
    azimuth_out: fin(d.azimuth_out) ? d.azimuth_out : azimuth,
    length: d.length,
    depth_in: d.depth_in,
    depth_out: d.depth_out,
    pitch_in: d.pitch_in ?? null,
    pitch_out: d.pitch_out ?? null,
    left: d.left ?? null,
    right: d.right ?? null,
    up: d.up ?? null,
    down: d.down ?? null,
    comment: d.comment ?? null,
    kind: d.kind ?? 'Normal',
    marker_index: null,
    temperature: null,
  };
}

/**
 * What changed in `draft` since `sent` - its create_shot payload - was taken from
 * it: the values typed into the row while the create was in flight (#576 D39(f)),
 * as an update for the created shot. Read through `completeDraftData` so a missing
 * azimuth leg compares as the duplicate that was sent. A required field cleared
 * since is left out - a real shot cannot hold a blank one - and so is anything that
 * still reads as it was sent.
 */
export function lateDraftEdits(sent: ShotData, draft: ShotDraft): Partial<ShotUpdates> {
  const now = completeDraftData(draft);
  const out: Record<string, unknown> = {};
  for (const key of Object.keys(sent) as (keyof ShotData)[]) {
    const value = now === null ? draft.data[key] : now[key];
    if (value === undefined || Object.is(value, sent[key])) continue;
    out[key] = value;
  }
  return out as Partial<ShotUpdates>;
}

/** create_shot insertion args for a draft: its anchor, with `'end'` resolved
 *  to "after the current last shot" at commit time (append when none). */
export function commitAnchorArgs(
  draft: ShotDraft,
  shots: Shot[],
): { afterShotId?: number; beforeShotId?: number } {
  if (draft.anchor === 'end') {
    const last = shots[shots.length - 1];
    return last ? { afterShotId: last.id } : {};
  }
  return 'afterShotId' in draft.anchor
    ? { afterShotId: draft.anchor.afterShotId }
    : { beforeShotId: draft.anchor.beforeShotId };
}
