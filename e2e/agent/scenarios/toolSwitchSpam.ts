/**
 * tool-switch-spam: rapidly cycle the active tool through every ToolType and
 * verify each switch actually takes (uiStore.activeTool reflects it). Catches
 * tool state desync and handler errors on repeated switching. Restores 'select'
 * at the end.
 */
import { shareableTestId } from '../../../scripts/lib/shareable-selector.mjs';
import type { Scenario } from './framework';
import { ensureProjectLoaded } from './common';

const TOOLS = [
  'select',
  'pan',
  'zoom',
  'merge',
  'measure',
  'move_station',
  'move_segment',
  'move_shot',
  'comment',
  'move_comment',
] as const;

export const toolSwitchSpam: Scenario = {
  name: 'tool-switch-spam',
  description: 'Cycle every ToolType repeatedly; assert activeTool tracks each switch.',
  defaultIterations: 100,
  async run(ctx, { iterations }) {
    await ensureProjectLoaded(ctx);

    // Pan and Zoom are hidden unless `map.controls.show_navigation_tools` is on
    // (#409), and MapCanvas force-resets a hidden navigation tool back to
    // 'select' on the next render. Without flipping the setting first, every
    // pan/zoom iteration below would race that effect. `setConfig` updates the
    // in-memory config only - nothing is written to the user's config file.
    await ctx.eval(`
      const project = window.__E2E_TEST__.projectStore.getState();
      const config = project.config;
      if (config) {
        project.setConfig({
          ...config,
          map: {
            ...config.map,
            controls: { ...config.map.controls, show_navigation_tools: true },
          },
        });
      }
      return true;
    `);

    for (let i = 0; i < iterations; i++) {
      const tool = TOOLS[i % TOOLS.length];
      const active = await ctx.eval<string>(
        `
        const ui = window.__E2E_TEST__.uiStore.getState();
        ui.setActiveTool(args[0]);
        return window.__E2E_TEST__.uiStore.getState().activeTool;
      `,
        [tool],
      );
      ctx.metric('switches');
      // The page's answer keeps only the words of the app's source, as a test
      // id does (#576 W58 Q11): every real ToolType prints as itself.
      if (active !== tool) throw new Error(`setActiveTool(${tool}) left activeTool=${shareableTestId(active)}`);
    }

    await ctx.callAction('ui', 'setActiveTool', ['select']);
    ctx.log(`${iterations} tool switches, all tracked`);
  },
};
