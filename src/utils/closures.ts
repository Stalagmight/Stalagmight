import type { ClosureStrategy } from '../types/ipc/ClosureStrategy';
import type { DatumRole } from '../types/ipc/DatumRole';

/**
 * Per-strategy eligibility for the least-squares closure-adjustment solve.
 * Mirrors `Project::closure_is_eligible` in the Rust backend: `Move` closures are
 * already coincident (snap-to-merge) and the deferred proportional/equal
 * strategies are not solved.
 *
 * Declared as a full `Record<ClosureStrategy, boolean>` so that adding a variant
 * to the generated `ClosureStrategy` union is a compile error here until it is
 * classified - preventing silent drift from the backend. Keep in sync with
 * `src-tauri/src/models/project.rs`.
 */
const CLOSURE_STRATEGY_ELIGIBILITY: Record<ClosureStrategy, boolean> = {
  Batch: true,
  LeastSquaresWeighted: true,
  Move: false,
  ProportionalByLength: false,
  EqualDistribution: false,
};

/** Strategies that participate in the solve (derived from the classification map). */
export const CLOSURE_ELIGIBLE_STRATEGIES: readonly ClosureStrategy[] = (
  Object.keys(CLOSURE_STRATEGY_ELIGIBILITY) as ClosureStrategy[]
).filter((s) => CLOSURE_STRATEGY_ELIGIBILITY[s]);

/**
 * Whether the project has at least one closure the closure-adjusted view would
 * actually move. When false, the adjusted geometry is identical to measured, so
 * the Display-menu toggle is disabled and the adjusted view auto-reverts to
 * measured rather than presenting an identical, confusingly read-only view.
 */
export function hasEligibleClosures(
  loopClosures: ReadonlyArray<{ strategy: ClosureStrategy }>
): boolean {
  return loopClosures.some((lc) => CLOSURE_STRATEGY_ELIGIBILITY[lc.strategy]);
}

/**
 * Whether the Closures Computed View can differ from the Raw Data View: an
 * eligible closure, or a GPS fix the solve pulls the survey toward (#741). A
 * GPS fix never moves raw geometry, so it is a reason to adjust on its own.
 * Every gate on "is the computed view worth offering / computing" reads this.
 */
export function hasAdjustmentInputs(project: {
  loop_closures: ReadonlyArray<{ strategy: ClosureStrategy }>;
  zero_datums: ReadonlyArray<{ role: DatumRole }>;
}): boolean {
  return hasEligibleClosures(project.loop_closures) || project.zero_datums.some((zd) => zd.role === 'GpsFix');
}
