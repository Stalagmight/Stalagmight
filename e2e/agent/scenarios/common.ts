/**
 * Shared scenario helpers. Fixture-path resolution, project seeding, a cheap
 * deterministic state signature, and a valid minimal LineData factory (shapes
 * verified against src/types/ipc/{LineData,ShotData}.ts).
 */
import type { ScenarioContext } from './framework';
export { DEFAULT_FIXTURE } from '../lib/fixtures';
import { DEFAULT_FIXTURE, stripUndoHistory, type StripUndoHistoryResult } from '../lib/fixtures';

export async function hasProject(ctx: ScenarioContext): Promise<boolean> {
  return ctx.eval<boolean>('return !!window.__E2E_TEST__.projectStore.getState().project;');
}

/** Open a fixture project if none is loaded (loadProject also switches to single view). */
export async function ensureProjectLoaded(ctx: ScenarioContext, fixture = DEFAULT_FIXTURE): Promise<void> {
  if (await hasProject(ctx)) return;
  const p = ctx.fixturePath(fixture);
  const res = (await ctx.callAction('project', 'loadProject', [p])) as {
    ok: boolean;
    error?: string;
    result?: { success?: boolean; error?: string };
  };
  if (!res.ok || res.result?.success === false) {
    throw new Error(`loadProject failed: ${res.result?.error ?? res.error ?? 'unknown'}`);
  }
  if (!(await hasProject(ctx))) throw new Error('project still null after loadProject');
}

/**
 * Open a fixture with NO inherited undo history, closing whatever is already
 * open first.
 *
 * `ensureProjectLoaded` is materialize+load in one step and is deliberately
 * lenient (it keeps any project that happens to be open). A scenario that models
 * undo DEPTH cannot use it: it needs a known baseline, and it needs the
 * fixture's persisted undo stack gone. So the three steps are split here -
 * materialize the temp copy, strip its history, then load - with the strip
 * landing on the COPY, never on the tracked fixture.
 *
 * CLOSE FIRST, and not as a matter of taste. `ctx.fixturePath` always resolves
 * to the same path (`<sessionDir>/fixtures/<name>.stalx`), so a caller that
 * already has that very file open - `seed(mode:'fixture')` followed by
 * `run_scenario`, which does not close anything - would have the copy TRUNCATED
 * and then SQLite-written underneath a live read-write handle holding a 256 MB
 * mmap. That is the SIGBUS hazard `project_io.rs` documents at its own
 * `close_project`, and it surfaces as the app crashing or reporting "database
 * disk image is malformed" - a fabricated app bug that is really the harness
 * corrupting the database it had open.
 */
export async function loadFixtureWithoutUndoHistory(
  ctx: ScenarioContext,
  fixture = DEFAULT_FIXTURE,
): Promise<{ path: string; stripped: StripUndoHistoryResult }> {
  // Same close the scenario smoke runner does between scenarios. Unlike there it
  // is NOT best-effort: the next two statements rewrite the file the backend
  // would still have open, so a project that refuses to close has to stop the
  // scenario rather than be written over.
  if (await hasProject(ctx)) {
    await ctx.callAction('project', 'closeProject', []).catch(() => undefined);
    if (await hasProject(ctx)) {
      throw new Error(
        `a project is still open after closeProject; refusing to rewrite ${fixture} underneath ` +
          `the backend's live SQLite handle (see loadFixtureWithoutUndoHistory)`,
      );
    }
  }

  const materialized = ctx.fixturePath(fixture);
  const stripped = stripUndoHistory(materialized);

  const res = (await ctx.callAction('project', 'loadProject', [materialized])) as {
    ok: boolean;
    error?: string;
    result?: { success?: boolean; error?: string };
  };
  if (!res.ok || res.result?.success === false) {
    throw new Error(`loadProject(${fixture}) failed: ${res.result?.error ?? res.error ?? 'unknown'}`);
  }
  if (!(await hasProject(ctx))) throw new Error(`project still null after loading ${fixture}`);
  return { path: materialized, stripped };
}

/**
 * Turn auto-save off for this session (frontend-only: `setConfig` does not
 * persist, so the user's/profile's config file is untouched).
 *
 * Any scenario that compares project state across time needs this. `save_project`
 * MUTATES the live project with no undo entry - `ensure_shot_uuids`,
 * `ensure_unique_shot_uuids`, `capture_missing_gauge_anchors`, orphan
 * `line_offsets` retain and non-finite sanitize (`core/state/project_io.rs`) -
 * so a 30 s auto-save timer firing mid-run silently changes state that no command
 * accounts for. Returns true when auto-save is definitely off (including the case
 * where config has not loaded yet, which `useAutoSave` already treats as off).
 */
export async function disableAutoSave(ctx: ScenarioContext): Promise<boolean> {
  return ctx.eval<boolean>(`
    const store = window.__E2E_TEST__.projectStore;
    const config = store.getState().config;
    if (!config) return true;
    if (config.general && config.general.auto_save === false) return true;
    store.getState().setConfig({ ...config, general: { ...config.general, auto_save: false } });
    const now = store.getState().config;
    return !!now && !!now.general && now.general.auto_save === false;
  `);
}

/** A valid minimal line (two standard shots), unique per index. */
export function makeLine(i: number): Record<string, unknown> {
  const shot = (az: number, len: number, dIn: number, dOut: number) => ({
    azimuth_in: az,
    azimuth_out: az,
    length: len,
    depth_in: dIn,
    depth_out: dOut,
    pitch_in: null,
    pitch_out: null,
    left: 1,
    right: 1,
    up: 1,
    down: 1,
    comment: null,
    kind: 'Normal',
    marker_index: null,
    temperature: null,
  });
  return {
    name: `harness-churn-${i}`,
    surveyor: 'harness',
    date: '2026-01-01',
    color: '#3388FF',
    direction: 'out',
    quality: 3,
    shots: [shot(0, 5, 0, 1), shot(90, 4, 1, 2)],
  };
}

/** Cheap deterministic signature of mutable project state (for undo/redo equality). */
export async function stateSignature(ctx: ScenarioContext): Promise<string> {
  return ctx.eval<string>(`
    const p = window.__E2E_TEST__.projectStore.getState().project;
    if (!p) return 'null';
    const lines = (p.lines || []).map(s => (s.name || '') + '#' + (s.shots ? s.shots.length : 0));
    return JSON.stringify({ lines, merges: (p.station_merges || []).length });
  `);
}

/** getUndoStack().length (async IPC round-trip). */
export async function undoDepth(ctx: ScenarioContext): Promise<number> {
  const res = (await ctx.callAction('project', 'getUndoStack', [])) as { ok: boolean; result?: unknown[] };
  return Array.isArray(res.result) ? res.result.length : 0;
}
