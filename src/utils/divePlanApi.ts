import { invokeIpc } from './ipc';
import type {
  DivePlan,
  DivePlanMeta,
  DivePlanSnapshot,
  DiveProfileRequest,
} from '../types/ipc.generated';
import type { StationRef } from '../types/project';

/**
 * `DiveProfileRequest` is the plan-input tuple `compute_gas_reach_field` takes, and it is GENERATED
 * from the Rust struct rather than hand-declared here - a rename on either side would otherwise
 * compile clean on both and fail only at runtime, as a serde error.
 *
 * The backend recomputes the reach field from these rather than accepting the one the UI is
 * holding: an exported profile is a document the diver may act on later, so it has to be what the
 * engine says now. Every plan edit nulls the cached field, so a client-supplied one could easily be
 * a route the diver has already changed.
 */
export type { DiveProfileRequest };

/** Wire values the backend's `ProfileFormat::from_wire` accepts. */
export type DiveProfileFormat = 'subsurface-xml' | 'text';

/** Write the planned dive profile to `path`. Rejects when the plan has no route. */
export async function exportDiveProfile(
  path: string,
  format: DiveProfileFormat,
  request: DiveProfileRequest
): Promise<void> {
  await invokeIpc(
    'export_dive_profile',
    { path, format, request },
    { purpose: 'export the planned dive profile', fields: { format } }
  );
}

/** Render the profile as text without writing it, for the clipboard. */
export function renderDiveProfileText(request: DiveProfileRequest): Promise<string> {
  return invokeIpc(
    'render_dive_profile_text',
    { request },
    { purpose: 'copy the planned dive profile' }
  );
}

/**
 * Typed wrappers for the dive-plan (`.plnx`) IPC commands (#244). The backend
 * reads/writes a SQLite sibling next to the open project; these calls return `[]`
 * / a friendly error when there is no saved project. See `divePlan.ts` for the
 * snapshot shape and `git show de8eef91:docs/plans/2026-07-06_dive-plan-persistence.md` (doc removed from master in #229).
 */

/** List the saved plans for the open project (newest first; `[]` if none). */
export function listDivePlans(): Promise<DivePlanMeta[]> {
  return invokeIpc('list_dive_plans', {}, { purpose: 'list dive plans' });
}

/** Load one plan; the backend nulls any station refs that no longer resolve and
 *  reports how many via `stale_ref_count`. */
export function loadDivePlan(id: number): Promise<DivePlan> {
  return invokeIpc(
    'load_dive_plan',
    { id },
    { purpose: 'load dive plan', fields: { id } }
  );
}

/** Save a plan: `id = null` inserts (Save as), a number overwrites (Save). */
export function saveDivePlan(
  id: number | null,
  name: string,
  snapshot: DivePlanSnapshot
): Promise<DivePlanMeta> {
  return invokeIpc(
    'save_dive_plan',
    { id, name, snapshot },
    { purpose: 'save dive plan', fields: { id: id ?? 'new' } }
  );
}

/** Delete a plan by id. */
export async function deleteDivePlan(id: number): Promise<void> {
  await invokeIpc(
    'delete_dive_plan',
    { id },
    { purpose: 'delete dive plan', fields: { id } }
  );
}

/**
 * A backend station anchor (#310), deliberately OPAQUE on this side.
 *
 * The backend names a station by the shots on either side of it, so an anchor survives a
 * renumbering that a positional `StationRef` does not (insert, delete, split, reverse). TypeScript
 * only ever carries one from `plan_anchors_for_refs` back to `plan_refs_for_anchors`; typing its
 * innards here would create a second, hand-maintained copy of a Rust enum that changes for
 * backend-only reasons, and the compiler would be checking a shape nothing on this side reads.
 */
export type PlanAnchor = unknown;

/** The anchors a `load_dive_plan` reply carries alongside the snapshot, one per persisted pin. */
export interface LoadedPlanAnchors {
  origin: PlanAnchor | null;
  endpoint: PlanAnchor | null;
  waypoints: (PlanAnchor | null)[];
  marked_points: (PlanAnchor | null)[];
  stage_drop_overrides: (PlanAnchor | null)[];
}

/**
 * Read the anchors off a loaded plan, or `null` when the reply carries none.
 *
 * Read defensively rather than off the generated `DivePlan` type: the e2e IPC mock and any
 * pre-#310 backend answer without them, and the alternative - trusting the field and handing
 * `undefined` arrays to the store - turns a missing feature into a crash on the load path.
 */
export function planAnchorsOf(plan: DivePlan): LoadedPlanAnchors | null {
  const raw: unknown = plan.anchors;
  if (raw === null || typeof raw !== 'object') return null;
  const a = raw as Partial<LoadedPlanAnchors>;
  if (
    !Array.isArray(a.waypoints) ||
    !Array.isArray(a.marked_points) ||
    !Array.isArray(a.stage_drop_overrides)
  ) {
    return null;
  }
  return {
    origin: a.origin ?? null,
    endpoint: a.endpoint ?? null,
    waypoints: a.waypoints,
    marked_points: a.marked_points,
    stage_drop_overrides: a.stage_drop_overrides,
  };
}

/** Anchor live plan pins, index-aligned with `refs`; `null` = that ref names no real station. */
export function planAnchorsForRefs(refs: StationRef[]): Promise<(PlanAnchor | null)[]> {
  return invokeIpc(
    'plan_anchors_for_refs',
    { refs },
    { purpose: 'anchor the live dive-plan pins', fields: { count: refs.length } }
  );
}

/** Re-resolve anchors to today's numbering; `null` = the anchor no longer names a station. */
export function planRefsForAnchors(anchors: PlanAnchor[]): Promise<(StationRef | null)[]> {
  return invokeIpc(
    'plan_refs_for_anchors',
    { anchors },
    { purpose: 're-resolve the dive-plan pins after a survey edit', fields: { count: anchors.length } }
  );
}
