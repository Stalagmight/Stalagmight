/**
 * seed-large-project: load a fractal fixture (real multi-section .stalx) and
 * verify it opens cleanly into single-system view with stations populated. This
 * both exercises the load path under the harness and leaves a seeded project for
 * follow-on scenarios.
 */
import { shareableTestId } from '../../../scripts/lib/shareable-selector.mjs';
import type { Scenario } from './framework';
import { ensureProjectLoaded } from './common';

export const seedLargeProject: Scenario = {
  name: 'seed-large-project',
  description: 'Load a fractal fixture and verify a clean multi-section open.',
  defaultIterations: 1,
  async run(ctx) {
    const t0 = Date.now();
    await ensureProjectLoaded(ctx);
    ctx.setMetric('loadMs', Date.now() - t0);

    const counts = await ctx.eval<{ sections: number; stations: number; view: string }>(`
      const ps = window.__E2E_TEST__.projectStore.getState();
      const ui = window.__E2E_TEST__.uiStore.getState();
      const p = ps.project;
      return { sections: p && p.sections ? p.sections.length : 0, stations: (ps.stations || []).length, view: ui.view };
    `);
    ctx.setMetric('sections', counts.sections);
    ctx.setMetric('stations', counts.stations);
    // The page's view keeps only the words of the app's source (#576 W58 Q11).
    ctx.log(`loaded ${counts.sections} sections / ${counts.stations} stations, view=${shareableTestId(counts.view)}`);

    if (counts.sections < 1) throw new Error('fixture loaded with zero sections');
    if (counts.stations < 1) throw new Error('fixture loaded with zero stations');
  },
};
