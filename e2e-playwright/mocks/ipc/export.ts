import type { MockCommandFamily, MockReply } from './types';

/**
 * The export commands of the Playwright IPC mock: CSV, JSON, Ariane, the dive profile and
 * the stick-map preview. `export_image` stays in the core, whose `__E2E_MOCKS__` hands its
 * last arguments to the specs.
 *
 * Split out of `../tauri-ipc.ts` unchanged (#576 AFK-D3). Runs IN THE PAGE: `setupTauriMocks`
 * serializes this factory with `Function.prototype.toString` into its init script, so it
 * may use only `ctx` and browser globals - never an import (type-only imports are erased).
 */
export const exportCommands: MockCommandFamily = (ctx) => {
  return (cmd, args) => {
    switch (cmd) {
      case 'export_csv':
        console.log('[Mock IPC] export_csv called with:', args);
        return null satisfies MockReply<'export_csv'>;

      case 'export_json':
        console.log('[Mock IPC] export_json called with:', args);
        return null satisfies MockReply<'export_json'>;

      // The Ariane writer's notes (#576 E3): none. What lands in the file is pinned by
      // the export crate's TML tests; a spec asserts the call and its path.
      case 'export_tml':
        return [] satisfies MockReply<'export_tml'>;

      // Dive-profile export (#299). Records the call so a spec can assert the plan INPUTS
      // reached the backend - the real command recomputes the reach field from them rather
      // than trusting a client-held one, so sending them is the contract under test.
      case 'export_dive_profile': {
        const win = window as unknown as Record<string, unknown>;
        const calls = (win.__MOCK_DIVE_PROFILE_EXPORTS__ as unknown[]) ?? [];
        calls.push({ path: args?.path, format: args?.format, request: args?.request });
        win.__MOCK_DIVE_PROFILE_EXPORTS__ = calls;
        return null satisfies MockReply<'export_dive_profile'>;
      }

      case 'render_dive_profile_text':
        return [
          'Mock Cave - planned dive profile',
          'Planned penetration profile from Stalagmight. NOT a decompression schedule.',
          '',
          '   Dist(m)   Depth(m)   Runtime      Gas(L)  Leg',
          '       0.0        0.0      0:00           0  in',
        ].join('\n') satisfies MockReply<'render_dive_profile_text'>;

      case 'preview_svg_export': {
        // Stick-map preview lives in the same backend command pipeline; the
        // mock returns a tiny but valid SVG so the preview component can
        // render without hitting Rust during e2e tests.
        const opts = args?.options as Record<string, unknown> | undefined;
        const threshold = (opts?.branching_threshold as number | undefined) ?? 0;
        // Encode the threshold in a data attribute so tests can verify
        // the preview re-fetches when the slider changes.
        return `<?xml version="1.0" encoding="UTF-8"?>\n<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" data-threshold="${threshold}"><rect width="100" height="100" fill="#ffffff"/><line x1="10" y1="50" x2="90" y2="50" stroke="#000000" stroke-width="1"/></svg>` satisfies MockReply<'preview_svg_export'>;
      }

      default:
        return ctx.unhandled;
    }
  };
};
