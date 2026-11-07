/**
 * new-project-from-dmp: the J1 first-run golden flow (#391/#394 class - the one
 * two expert field testers could not complete). With NO project open, a `.dmp`
 * routed through the app's own file router must carry the user through the
 * new-project-for-import screen and the wizard to a live, SAVED project with a
 * receipt - the deferred-initial-save contract (#92/#104: the first import
 * resolves the reserved save) and the importContext teardown (#477 class)
 * checked at the end.
 *
 * Unlike the other journeys this one drives the real DIALOG UI, not store
 * actions - the flow under test IS the dialog chain. It reuses the import
 * fuzzer's bridge (screen digest + real clicks on testids) but walks a
 * DETERMINISTIC primary path: each screen has exactly one scripted action, and
 * any screen outside the script aborts the journey with the screen name.
 */
import * as fs from 'fs';
import type { Journey, JourneyContext } from './framework';
import { disableAutoSave } from '../scenarios/common';
import {
  injectScreenDigest,
  observeScreen,
  runUiStep,
  routeFile,
  dismissFirstRunModal,
  type Move,
} from '../scenarios/importDialogBridge';
import { oracleInjectionSource } from '../lib/fuzzOracleCore';
import { mapClick } from '../lib/controls';
import { parseReceiptSummary } from '../lib/importFlowOracles';

const DMP_FIXTURE = 'test_v2.dmp';
/** Generous cap: the whole golden path is well under this many screen ACTIONS. */
const MAX_STEPS = 40;
/**
 * Separate budget for transient observations (wiz-init/placing/blank/etc.).
 * Transients must NOT consume the action budget: a slow placing phase on a
 * large fixture or a loaded nightly box would otherwise exhaust MAX_STEPS by
 * polling and fail as a load flake. Each poll is a settled observe + 150ms.
 */
const MAX_TRANSIENT_POLLS = 200;

interface ParsedDmpShot {
  shot_type: string;
  length: number;
}

interface ParsedDmp {
  sections: { shots: ParsedDmpShot[] }[];
}

/**
 * Mirror of the selection dialog's DEFAULT selection and shot conversion
 * (`src/utils/shotConversion.ts` `isImportableDmpShot`, and
 * `DmpSelectionDialog.tsx`'s initial `selectedIndices`): a shot is importable
 * when it is `Standard` and its length is not the 1.0 excluded-shot sentinel
 * (`DEFAULT_EXCLUDED_DMP_SHOT_LENGTH = 1`); a section is selected by default
 * when it has MORE THAN ONE importable shot. The golden path accepts the
 * defaults, so expectations are derived through this rule. (Duplicate-hash
 * auto-deselection cannot fire here: the project is brand new.)
 */
function defaultSelectionCounts(parsed: ParsedDmp): { sections: number; shots: number } {
  const importable = (s: ParsedDmpShot): boolean =>
    s.shot_type === 'Standard' && Math.abs(s.length - 1) > 1e-9;
  let sections = 0;
  let shots = 0;
  for (const section of parsed.sections) {
    const kept = section.shots.filter(importable).length;
    if (kept <= 1) continue;
    sections += 1;
    shots += kept;
  }
  return { sections, shots };
}

const click = (testid: string): Move => ({ kind: 'ui', testid, label: `click ${testid}` });

/**
 * The scripted primary action per screen. Transient wizard states return null
 * (observe again); a screen missing from this table ends the journey.
 */
function primaryAction(screen: string, live: Record<string, 0 | 1 | 2>): Move | null | 'done' {
  switch (screen) {
    case 'idle':
      return 'done';
    case 'first-run':
      return click('telemetry-opt-in-no');
    case 'new-project':
      // The save-path input stays EMPTY on purpose: a blank path is what
      // selects the deferred initial save (NewProjectForImportDialog passes
      // `savePath` only when non-empty, and createProject defers iff no
      // savePath) - filling it would switch this journey off the #92/#104
      // contract it exists to exercise. The resulting default-location file
      // is deleted in the journey's teardown.
      return click('create-and-continue-button');
    case 'dmp-select':
      // The golden path ACCEPTS the default selection (see
      // defaultSelectionCounts, which mirrors it) - never click select-all.
      // GATE on actionability: continue is disabled until two async hash
      // checks settle, and the bridge's settle deliberately does NOT cover
      // that window (importDialogBridge.ts, the race-continue note) - an
      // ungated click there dies as 'not-actionable'.
      return live['continue-button'] === 2 ? click('continue-button') : null;
    case 'zero-datum-select':
      return click('zero-datum-selection-skip');
    case 'wiz-suggest':
      return live['confirm-button'] === 2 ? click('confirm-button') : null;
    case 'wiz-init':
    case 'wiz-placing':
    case 'wiz-blank':
    // `wiz-done` has no footer buttons in phase 'done'; it auto-advances via
    // the completion effect. A transient here; if it sticks, the poll budget
    // names it.
    case 'wiz-done':
      return null; // transient - observe again
    case 'orphan-placement':
      return click('orphan-close');
    case 'receipt':
      return click('import-receipt-continue');
    default:
      return 'done'; // caller checks: non-idle here is an abort
  }
}

export const newProjectFromDmp: Journey = {
  name: 'new-project-from-dmp',
  description: 'Route a .dmp with no project open; the dialog chain must end in a saved project.',
  async run(ctx) {
    // The flow under test starts from NOTHING open.
    await ctx.callAction('project', 'closeProject', []).catch(() => undefined);
    await disableAutoSave(ctx);
    const dmp = ctx.fixturePath(DMP_FIXTURE);

    const parsedRes = (await ctx.callAction('project', 'importDmp', [dmp])) as {
      ok: boolean;
      result?: ParsedDmp;
      error?: string;
    };
    if (!parsedRes.ok || !parsedRes.result) {
      throw new Error(`importDmp (parse) failed: ${parsedRes.error ?? 'unknown'}`);
    }
    const { sections: expectedSections, shots: expectedShots } = defaultSelectionCounts(parsedRes.result);
    ctx.expect('fixture parses with default-selected sections', expectedSections > 0,
      `default-selected sections=${expectedSections} shots=${expectedShots}` +
        ` (of ${parsedRes.result.sections.length} parsed)`);

    // The digest's invariant hook reads the fuzz oracle global, so the oracle
    // source goes in first (same order as importDialogFuzz).
    await ctx.eval<boolean>(`${oracleInjectionSource()}\nreturn true;`);
    await injectScreenDigest(ctx);
    await dismissFirstRunModal(ctx);
    // `routeFile` answers a RouteOutcome, not a boolean: it also reports the two
    // store values `importFlowOwnsScreen` read on the same tick, which is what
    // lets the import fuzzer tell a fired guard from a screen that moved under
    // it. This journey only needs the "did the hook run" half.
    const routed = await routeFile(ctx, dmp);
    ctx.expect('file router accepted the .dmp', routed.routed, 'routeOpenedFile hook missing or threw');

    const visited: string[] = [];
    let digest = await observeScreen(ctx);
    let actions = 0;
    let polls = 0;
    while (actions < MAX_STEPS && polls < MAX_TRANSIENT_POLLS) {
      if (digest.screen !== visited[visited.length - 1]) visited.push(digest.screen);
      if (digest.screen === 'idle') break;
      // A WARNING-BEARING receipt is a modal and gets its content checked here.
      // A clean receipt is a TOAST (#456) that expires before a settled digest
      // can see it, so receipt PRESENCE is deliberately not a check - the flow
      // oracles measured that gate as unenforceable (importFlowOracles.ts).
      if (digest.screen === 'receipt' && digest.receipt) {
        const claim = parseReceiptSummary(digest.receipt.summary);
        // Same overlap allowance as the final count checks: a confirmed
        // suggestion legitimately deletes actionable overlap shots, and the
        // receipt reports what actually imported.
        const overlapPath = visited.includes('wiz-suggest');
        const shotsOk = overlapPath
          ? claim.shots !== null && claim.shots > 0 && claim.shots <= expectedShots
          : claim.shots === expectedShots;
        ctx.expect('receipt accounts for every imported section and shot',
          claim.sections === expectedSections && shotsOk,
          `summary="${digest.receipt.summary}" expected ${expectedSections}/` +
            `${overlapPath ? `<=${expectedShots}` : expectedShots}`);
        await ctx.checkpoint('receipt');
      }
      if (digest.screen === 'map-placement') {
        // Place at the live map centre, through MapCanvas - the only route
        // that runs the wizard's completion handler (a direct
        // `confirmPlacement()` abandons the flow; see importDialogGenerators).
        const centre = await ctx.eval<[number, number] | null>(`
          const map = window.__stalagmight_map__;
          if (!map || !map.getCenter) return null;
          const c = map.getCenter();
          return [c.lng, c.lat];
        `);
        if (!centre) throw new Error('map-placement with no live map to click');
        // mapClick THROWS on any failure (off-viewport, missing hooks,
        // unprocessed click) - the abort message carries the reason.
        await ctx.exclusive(() => mapClick(ctx.browser, centre));
        actions += 1;
        digest = await observeScreen(ctx);
        continue;
      }
      const action = primaryAction(digest.screen, digest.live);
      if (action === 'done') {
        throw new Error(`unscripted screen "${digest.screen}" (path: ${visited.join(' -> ')})`);
      }
      if (action !== null) {
        const outcome = await runUiStep(ctx, action);
        if (!outcome.performed) {
          throw new Error(
            `"${action.label}" not performed on "${digest.screen}" (${outcome.reason ?? 'unknown'};` +
              ` path: ${visited.join(' -> ')})`,
          );
        }
        actions += 1;
      } else {
        polls += 1;
        await ctx.sleep(150);
      }
      digest = await observeScreen(ctx);
    }
    ctx.expect('flow returned to idle within the step budget', digest.screen === 'idle',
      `ended on "${digest.screen}" after path: ${visited.join(' -> ')}`);

    // MapCanvas clears placementMode BEFORE the wizard's async completion
    // callback imports/saves and tears down importContext, so the first
    // post-click idle can be a transient. Give completion a bounded settle;
    // the assertions below still run (and still fail) if it never settles,
    // so the #477 residue check keeps its teeth.
    for (let i = 0; i < 20 && (digest.ctx !== null || digest.importInProgress); i++) {
      await ctx.sleep(150);
      digest = await observeScreen(ctx);
    }
    await ctx.checkpoint('after-flow');

    const project = await projectFacts(ctx);
    ctx.expect('a project exists after the flow', project.exists);
    ctx.expect('project carries every default-selected section',
      project.sections === expectedSections,
      `sections=${project.sections} expected=${expectedSections}`);
    // A CONFIRMED overlap suggestion legitimately deletes actionable overlap
    // shots (docs/dmp-import.md, overlap treatment), so the exact-count claim
    // only holds when the wizard never offered one. With a suggestion in the
    // path, exactness would red on correct behavior - assert no-invention
    // instead. KNOWN LIMIT: a suggestion with matchedShots === 0 deletes
    // nothing and exactness should really still hold there, but the screen
    // digest does not expose matchedShots - exposing it is a digest
    // extension, tracked on #466. test_v2.dmp takes the exact branch today;
    // a fixture swap that starts suggesting is visible in the recorded path,
    // not a silent skip.
    const sawSuggestion = visited.includes('wiz-suggest');
    if (sawSuggestion) {
      ctx.expect('no shots invented beyond the default selection (overlap path)',
        project.shots > 0 && project.shots <= expectedShots,
        `shots=${project.shots} expected<=${expectedShots}; path: ${visited.join(' -> ')}`);
    } else {
      ctx.expect('project carries every importable shot of those sections',
        project.shots === expectedShots,
        `shots=${project.shots} expected=${expectedShots}`);
    }
    ctx.expect('deferred initial save resolved to a real file on disk',
      project.filePath !== null && fs.existsSync(project.filePath),
      `file_path=${project.filePath ?? 'null'}`);

    ctx.expect('importContext cleared after completion (the #477 class)',
      digest.ctx === null, `importContext=${String(digest.ctx)}`);
    ctx.expect('no import reads as in-progress at idle', !digest.importInProgress);

    // Teardown: the deferred save necessarily landed in the app's DEFAULT
    // project location (an empty save path is what selects deferral, see the
    // new-project comment), so the journey removes its own artifact - stale
    // copies would otherwise accumulate via save auto-renaming and leak into
    // later runs. Close FIRST: deleting the backing file under a live SQLite
    // handle is the documented SIGBUS hazard.
    if (project.filePath !== null) {
      const closed = (await ctx.callAction('project', 'closeProject', [])) as { ok: boolean };
      if (closed.ok) fs.rmSync(project.filePath, { force: true });
      ctx.expect('journey artifact cleaned up', closed.ok && !fs.existsSync(project.filePath),
        `file_path=${project.filePath} closed=${closed.ok}`);
    }
  },
};

async function projectFacts(ctx: JourneyContext): Promise<{
  exists: boolean;
  sections: number;
  shots: number;
  filePath: string | null;
}> {
  return ctx.eval(`
    const p = window.__E2E_TEST__.projectStore.getState().project;
    if (!p) return { exists: false, sections: 0, shots: 0, filePath: null };
    return {
      exists: true,
      sections: (p.sections || []).length,
      shots: (p.sections || []).reduce((n, s) => n + ((s.shots || []).length), 0),
      filePath: p.file_path ?? null,
    };
  `);
}
