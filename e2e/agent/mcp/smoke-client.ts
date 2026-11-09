/**
 * MCP-layer smoke test: spawns e2e/agent/mcp/server.ts as a real MCP stdio
 * server, lists its tools, and (unless AGENT_MCP_SMOKE_LIFECYCLE=0) drives a
 * full app lifecycle through the protocol: app_start -> screenshot -> store_state
 * -> ipc -> logs -> eval -> app_stop. This proves the transport framing is clean
 * (any stray stdout from the server would break JSON-RPC and fail here) and that
 * every tool round-trips. Run on the repo-pinned Node (`nvm use`):
 *
 *   pnpm agent:mcp:smoke
 *
 * It spawns the server with the SAME node that runs this client (process.execPath)
 * plus tsx, so the server always matches whichever node ran the client.
 */
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import * as path from 'path';
import { fileURLToPath } from 'url';

const serverPath = path.resolve(path.dirname(fileURLToPath(import.meta.url)), 'server.ts');
const withLifecycle = process.env.AGENT_MCP_SMOKE_LIFECYCLE !== '0';

interface ToolText {
  content: Array<{ type: string; text?: string; data?: string }>;
  isError?: boolean;
}

function parseText(res: ToolText): unknown {
  const textPart = res.content.find((c) => c.type === 'text');
  if (!textPart?.text) return null;
  try {
    return JSON.parse(textPart.text);
  } catch {
    return textPart.text;
  }
}

async function main(): Promise<void> {
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: ['--import', 'tsx', serverPath],
    env: { ...process.env } as Record<string, string>,
    stderr: 'inherit',
  });
  const client = new Client({ name: 'stalagmight-mcp-smoke', version: '0.1.0' });
  await client.connect(transport);

  let failed = false;
  const check = (label: string, ok: boolean, detail?: unknown) => {
    process.stdout.write(`${ok ? 'PASS' : 'FAIL'}: ${label}${detail !== undefined ? ` ${JSON.stringify(detail)}` : ''}\n`);
    if (!ok) failed = true;
  };

  try {
    const { tools } = await client.listTools();
    const names = tools.map((t) => t.name).sort();
    const expected = [
      'app_start', 'app_stop', 'app_restart', 'app_status',
      'screenshot', 'store_state', 'logs', 'wait_for',
      'click', 'type_text', 'press_keys', 'hover', 'drag', 'scroll',
      'select_option', 'element_state', 'file_dialog_answer', 'eval', 'ipc',
      'store_action', 'map_view', 'map_click', 'map_feature_click',
      'seed', 'driver_script', 'check_problems', 'run_scenario',
    ];
    const missing = expected.filter((n) => !names.includes(n));
    check(`tools/list has all ${expected.length} tools`, missing.length === 0, missing.length ? { missing } : names.length);

    if (!withLifecycle) {
      check('lifecycle skipped (AGENT_MCP_SMOKE_LIFECYCLE=0)', true);
      await client.close();
      process.exit(failed ? 1 : 0);
    }

    const call = (name: string, args: Record<string, unknown> = {}) =>
      client.callTool({ name, arguments: args }) as Promise<ToolText>;

    const started = (await call('app_start', { build: 'auto' })) as ToolText;
    const startVal = parseText(started) as { state?: string; error?: string; sessionDir?: string };
    check('app_start -> ready', startVal?.state === 'ready', startVal?.error ?? startVal?.state);
    if (startVal?.state !== 'ready') throw new Error(`app_start failed: ${JSON.stringify(startVal)}`);

    const status = parseText(await call('app_status')) as { driverAlive?: boolean };
    check('app_status driverAlive', status?.driverAlive === true);

    const shot = (await call('screenshot', { name: 'smoke' })) as ToolText;
    const hasImage = shot.content.some((c) => c.type === 'image' && typeof c.data === 'string' && c.data.length > 100);
    check('screenshot returns image content', hasImage);

    const ui = parseText(await call('store_state', { store: 'ui' })) as { keys?: unknown[] };
    check('store_state ui has keys', Array.isArray(ui?.keys) && ui.keys.length > 0, ui?.keys?.length);

    const view = parseText(await call('store_state', { store: 'ui', path: 'view' })) as { value?: unknown };
    check('store_state ui.view reads a value', view !== null && 'value' in (view as object), view);

    const ipcRes = parseText(await call('ipc', { command: 'get_recent_logs_since', args: { afterSeq: 0 } })) as {
      value?: { latest_seq?: number };
    };
    check('ipc get_recent_logs_since', ipcRes?.value?.latest_seq !== undefined, ipcRes?.value?.latest_seq);

    const evalRes = parseText(await call('eval', { script: 'return 6 * 7;' })) as { value?: unknown };
    check('eval returns computed value', evalRes?.value === 42, evalRes?.value);

    const logs = parseText(await call('logs', { level: 'all' })) as { latestSeq?: number };
    check('logs read', typeof logs?.latestSeq === 'number');

    const wait = parseText(await call('wait_for', { condition: 'app-ready', timeoutMs: 5000 })) as { met?: boolean };
    check('wait_for app-ready', wait?.met === true);

    const dismissTelemetryPrompt = async () => {
      const telemetryPrompt = parseText(
        await call('element_state', { testid: 'telemetry-opt-in-dialog' }),
      ) as { value?: { elements?: Array<{ displayed?: boolean }> } };
      if (telemetryPrompt?.value?.elements?.some((element) => element.displayed)) {
        // This is setup, like the scenario harness's first-run dismissal. The
        // macOS driver cannot run WebdriverIO's element actionability helpers.
        const hidden = parseText(await call('eval', {
          script: `
            const button = document.querySelector('[data-testid="telemetry-opt-in-no"]');
            if (!(button instanceof HTMLElement)) throw new Error('telemetry decline button missing');
            button.click();
            const deadline = Date.now() + 5000;
            while (document.querySelector('[data-testid="telemetry-opt-in-dialog"]')) {
              if (Date.now() >= deadline) throw new Error('telemetry prompt did not close');
              await new Promise((resolve) => setTimeout(resolve, 50));
            }
            return true;
          `,
        })) as { value?: boolean };
        check('telemetry prompt dismissed', hidden?.value === true, hidden);
      }
    };
    await dismissTelemetryPrompt();

    // Oracle self-test (harness doctor): a deliberate backend thread panic (app
    // survives) must surface as a fatal crash-marker finding from check_problems.
    await call('ipc', { command: 'debug_trigger_panic', args: { delayMs: 50 } });
    let crashFinding: { severity?: string; kind?: string } | undefined;
    for (let i = 0; i < 20 && !crashFinding; i++) {
      await new Promise((r) => setTimeout(r, 250));
      const sweep = parseText(await call('check_problems')) as {
        findings?: Array<{ severity?: string; kind?: string }>;
      };
      crashFinding = sweep?.findings?.find((f) => f.kind === 'crash-marker');
    }
    check('oracle detects backend crash marker (fatal)', crashFinding?.severity === 'fatal', crashFinding);

    const respondsAfterPanic = parseText(await call('store_state', { store: 'ui', path: 'view' })) as { value?: unknown };
    check('app survives thread panic (still responds)', respondsAfterPanic?.value === 'global');

    // Seed + driver_script (do this last: seed opens a project -> single view).
    const seeded = parseText(await call('seed', { mode: 'fixture' })) as { mode?: string };
    check('seed opens an isolated copy of the default fixture', seeded?.mode === 'fixture', seeded);
    const seededView = parseText(await call('store_state', { store: 'ui', path: 'view' })) as { value?: unknown };
    check('seed switched to single view', seededView?.value === 'single', seededView?.value);
    await dismissTelemetryPrompt();
    await call('wait_for', { condition: 'map-idle', timeoutMs: 10000 });
    const featureIdentity = parseText(
      await call('eval', {
        script:
          'const geometry = window.__stalagmight_geometry__?.(); const station = geometry?.stations?.[0]; const shot = geometry?.shots?.[0]; return station && shot ? { sectionId: station.sectionId, stationIndex: station.stationIndex, shotId: shot.shotId, center: station.lngLat } : null;',
      }),
    ) as { value?: { sectionId?: number; stationIndex?: number; shotId?: number; center?: [number, number] } };
    const feature = featureIdentity?.value;
    if (feature?.center) {
      await call('map_view', { set: { center: feature.center, zoom: 20, bearing: 0 } });
      await call('wait_for', { condition: 'map-idle', timeoutMs: 10000 });
    }
    const semanticClick = feature?.sectionId != null && feature.stationIndex != null
      ? parseText(
          await call('map_feature_click', {
            featureType: 'station',
            sectionId: feature.sectionId,
            stationIndex: feature.stationIndex,
          }),
        ) as { clicked?: boolean; selectionVerified?: boolean }
      : null;
    check(
      'map_feature_click resolves and selects generated geometry',
      semanticClick?.clicked === true && semanticClick.selectionVerified === true,
      semanticClick ?? featureIdentity,
    );
    const semanticShotClick = feature?.sectionId != null && feature.shotId != null
      ? parseText(
          await call('map_feature_click', {
            featureType: 'shot',
            sectionId: feature.sectionId,
            shotId: feature.shotId,
          }),
        ) as { clicked?: boolean; selectionVerified?: boolean }
      : null;
    check(
      'map_feature_click resolves and selects a generated shot',
      semanticShotClick?.clicked === true && semanticShotClick.selectionVerified === true,
      semanticShotClick ?? featureIdentity,
    );
    const placementSetup = parseText(
      await call('eval', {
        script: `
          const e2e = window.__E2E_TEST__;
          const map = window.__stalagmight_map__;
          if (!e2e?.uiStore || !e2e?.projectStore || !map) throw new Error('placement test hooks unavailable');
          const project = e2e.projectStore.getState().project;
          const surface = map.getCanvas();
          const target = map.unproject([surface.clientWidth * 0.65, surface.clientHeight * 0.65]).toArray();
          const sectionData = {
            name: 'MCP Placement Section', surveyor: null, date: null, color: '#4ecdc4', direction: null, quality: 1,
            shots: [{
              azimuth_in: 90, azimuth_out: 90, length: 10, depth_in: 0, depth_out: 0,
              pitch_in: null, pitch_out: null, left: null, right: null, up: null, down: null,
              comment: null, kind: 'Normal', marker_index: null, temperature: null,
            }],
          };
          delete window.__MCP_PLACEMENT_RESULT__;
          e2e.uiStore.getState().startPlacementMode(sectionData, 0, async (result) => {
            const created = await e2e.projectStore.getState().createSection(sectionData);
            window.__MCP_PLACEMENT_RESULT__ = { created, result };
          });
          return { before: project.sections.length, target };
        `,
      }),
    ) as { value?: { before?: number; target?: [number, number] } };
    const placementTarget = placementSetup?.value?.target;
    const placementClick = placementTarget
      ? parseText(await call('map_click', { lngLat: placementTarget })) as {
          clicked?: boolean;
          interaction?: string;
        }
      : null;
    type PlacementProbe = {
      value?: { count?: number; placementActive?: boolean; completed?: boolean; success?: boolean };
    };
    let placementResult: PlacementProbe | null = null;
    for (let i = 0; i < 20; i++) {
      placementResult = parseText(
        await call('eval', {
          script: `
            const e2e = window.__E2E_TEST__;
            const marker = window.__MCP_PLACEMENT_RESULT__;
            return {
              count: e2e.projectStore.getState().project.sections.length,
              placementActive: e2e.uiStore.getState().placementMode !== null,
              completed: Boolean(marker?.result?.position),
              success: marker?.created?.success === true,
            };
          `,
        }),
      ) as PlacementProbe;
      if (placementResult?.value?.success) break;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    check(
      'map_click completes active section placement through the map surface',
      placementClick?.clicked === true &&
        placementClick.interaction === 'placement' &&
        placementResult?.value?.placementActive === false &&
        placementResult?.value?.completed === true &&
        placementResult?.value?.success === true &&
        placementResult?.value?.count === (placementSetup?.value?.before ?? -1) + 1,
      { placementClick, placementSetup, placementResult },
    );
    const offscreenClick = await call('map_click', { lngLat: [0, 0] });
    const offscreenDetail = parseText(offscreenClick);
    check(
      'map_click rejects off-viewport coordinates instead of reporting clicked',
      offscreenClick.isError === true && JSON.stringify(offscreenDetail).includes('outside the visible viewport'),
      offscreenDetail,
    );
    const driven = parseText(
      await call('driver_script', { script: 'const v = await readStore("ui", "view"); return v.value;' }),
    ) as { value?: unknown };
    check('driver_script runs with helpers in scope', driven?.value === 'single', driven?.value);

    const stopped = parseText(await call('app_stop')) as { stopped?: boolean };
    check('app_stop', stopped?.stopped === true);
  } catch (err) {
    check('unexpected exception', false, err instanceof Error ? err.message : String(err));
  } finally {
    await client.close().catch(() => undefined);
  }

  process.stdout.write(failed ? '\nMCP SMOKE FAILED\n' : '\nMCP SMOKE PASSED\n');
  process.exit(failed ? 1 : 0);
}

main().catch((err) => {
  process.stderr.write(`fatal: ${err instanceof Error ? err.stack : String(err)}\n`);
  process.exit(1);
});
