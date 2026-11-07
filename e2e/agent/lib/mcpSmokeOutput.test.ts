/**
 * `agent:mcp:smoke` prints its check lines in the shareable form (#576 W41-NF1).
 *
 * Every check line used to append its detail as raw JSON, PASS or FAIL: the
 * crash-marker finding with its panic message and marker path, the seed's
 * result (fixture, project, the oracle's findings), store values, a station's
 * map position, a tool's error. Those are the app's text and data.
 *
 * Drives the real smoke client (e2e/agent/mcp/smoke-client.ts) against a stub
 * MCP client whose tool results carry a marker string and a marker coordinate
 * wherever the app's text or data would be, and checks neither reaches the
 * smoke's output, while the flags and counts it computed still print.
 */
import * as path from 'path';
import type { PassThrough } from 'stream';
import { afterEach, describe, expect, it, vi } from 'vitest';

/** Stands in for the app's text: a station, project or fixture name, a panic message. */
const APP = 'QzAppText';
/** Stands in for survey data: a station's longitude / latitude. */
const COORD = 12.3456789;
const REPO = path.resolve(path.dirname(new URL(import.meta.url).pathname), '../../..');

type ToolResult = { content: Array<{ type: string; text?: string; data?: string }>; isError?: boolean };
type Responder = (name: string, args: Record<string, unknown>) => ToolResult;

const stub = vi.hoisted(() => ({
  tools: [] as string[],
  respond: (() => {
    throw new Error('no responder');
  }) as (name: string, args: Record<string, unknown>) => unknown,
  /** The transport the smoke made: its options, and the server's stderr. */
  transport: undefined as { options: { stderr?: unknown }; stderr: PassThrough } | undefined,
}));

vi.mock('@modelcontextprotocol/sdk/client/index.js', () => ({
  Client: class {
    connect(): Promise<void> {
      return Promise.resolve();
    }
    listTools(): Promise<{ tools: Array<{ name: string }> }> {
      return Promise.resolve({ tools: stub.tools.map((name) => ({ name })) });
    }
    callTool(req: { name: string; arguments: Record<string, unknown> }): Promise<unknown> {
      return Promise.resolve(stub.respond(req.name, req.arguments));
    }
    close(): Promise<void> {
      // The server exits: its stderr ends.
      stub.transport?.stderr.end();
      return Promise.resolve();
    }
  },
}));
vi.mock('@modelcontextprotocol/sdk/client/stdio.js', async () => {
  const { PassThrough } = await import('stream');
  return {
    StdioClientTransport: class {
      /** The server's stderr, as the SDK hands it over when asked to pipe it. */
      readonly stderr = new PassThrough();
      constructor(readonly options: { stderr?: unknown }) {
        stub.transport = this;
      }
    },
  };
});

const ALL_TOOLS = [
  'app_start', 'app_stop', 'app_restart', 'app_status',
  'screenshot', 'store_state', 'logs', 'wait_for',
  'click', 'type_text', 'press_keys', 'hover', 'drag', 'scroll',
  'select_option', 'element_state', 'file_dialog_answer', 'eval', 'ipc',
  'store_action', 'map_view', 'map_click', 'map_feature_click',
  'seed', 'driver_script', 'check_problems', 'run_scenario',
];

const text = (value: unknown, isError = false): ToolResult => ({
  content: [{ type: 'text', text: JSON.stringify(value) }],
  ...(isError ? { isError: true } : {}),
});

/** A whole passing lifecycle, with the app's text and data in every result that can carry it. */
function passingApp(): Responder {
  let seeded = false;
  return (name, args) => {
    const script = String(args.script ?? '');
    switch (name) {
      case 'app_start':
        return text({ state: 'ready', sessionDir: `/home/${APP}/session` });
      case 'app_status':
        return text({ driverAlive: true });
      case 'screenshot':
        return { content: [{ type: 'image', data: 'A'.repeat(200) }] };
      case 'store_state':
        return args.path === 'view'
          ? text({ value: seeded ? 'single' : 'global', path: `view of ${APP}` })
          : text({ keys: ['view', APP] });
      case 'ipc':
        return args.command === 'get_recent_logs_since' ? text({ value: { latest_seq: 7 } }) : text({ value: null });
      case 'eval':
        if (script === 'return 6 * 7;') return text({ value: 42 });
        if (script.includes('__stalagmight_geometry__')) {
          return text({ value: { sectionId: 1, stationIndex: 0, shotId: 2, center: [COORD, COORD] } });
        }
        if (script.includes('startPlacementMode')) return text({ value: { before: 1, target: [COORD, COORD] } });
        return text({ value: { count: 2, placementActive: false, completed: true, success: true } });
      case 'logs':
        return text({ latestSeq: 3 });
      case 'wait_for':
        return text({ met: true });
      case 'element_state':
        return text({ elements: [] });
      case 'click':
        return text({ clicked: true });
      case 'check_problems':
        return text({
          ok: false,
          worst: 'fatal',
          findings: [
            {
              severity: 'fatal',
              kind: 'crash-marker',
              message: 'backend panic marker: crash-1.json',
              detail: {
                location: 'src/lib.rs:10:5',
                thread: 'main',
                message: `panicked while saving ${APP}`,
                file: `/home/${APP}/crash-1.json`,
              },
            },
          ],
        });
      case 'seed':
        seeded = true;
        return text({
          mode: 'fixture',
          fixture: `${APP}.stalx`,
          result: { success: true, project: APP },
          oracle: { worst: 'warning', findings: [{ severity: 'warning', kind: 'log-warn', message: `${APP} warned` }] },
        });
      case 'map_view':
        return text({ center: [COORD, COORD] });
      case 'map_feature_click':
        return text({ clicked: true, selectionVerified: true, featureType: args.featureType, sectionName: APP });
      case 'map_click': {
        const lngLat = args.lngLat as [number, number];
        if (lngLat[0] === 0) return text({ error: `[0,0] is outside the visible viewport of ${APP}` }, true);
        return text({ clicked: true, interaction: 'placement', interactionVerified: true });
      }
      case 'driver_script':
        return text({ value: 'single' });
      case 'app_stop':
        return text({ stopped: true });
      default:
        throw new Error(`unexpected tool ${name}`);
    }
  };
}

/** Runs the smoke's `main()` to its `process.exit`, returning the exit code and everything it wrote. */
async function runSmoke(tools: string[], respond: Responder): Promise<{ code: number | undefined; out: string }> {
  stub.tools = tools;
  stub.respond = respond;
  vi.stubEnv('AGENT_MCP_SMOKE_LIFECYCLE', '1');
  const written: string[] = [];
  const capture = ((chunk: unknown) => {
    written.push(String(chunk));
    return true;
  }) as typeof process.stdout.write;
  vi.spyOn(process.stdout, 'write').mockImplementation(capture);
  vi.spyOn(process.stderr, 'write').mockImplementation(capture);
  let exited: (code: number | undefined) => void = () => undefined;
  const done = new Promise<number | undefined>((resolve) => {
    exited = resolve;
  });
  vi.spyOn(process, 'exit').mockImplementation(((code?: number) => {
    exited(code);
    return undefined as never;
  }) as typeof process.exit);
  vi.resetModules();
  await import('../mcp/smoke-client');
  const code = await done;
  vi.restoreAllMocks();
  return { code, out: written.join('') };
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
});

describe('agent:mcp:smoke check lines (#576 W41-NF1)', () => {
  it('a passing run prints no app text or position, and keeps its flags and counts', async () => {
    const { code, out } = await runSmoke(ALL_TOOLS, passingApp());

    expect(code).toBe(0);
    expect(out).toContain('MCP SMOKE PASSED');
    expect(out).not.toContain(APP);
    expect(out).not.toContain(String(COORD));
    // The counts and values the smoke computed itself.
    expect(out).toMatch(new RegExp(`PASS: \\S+ has all ${ALL_TOOLS.length} tools ${ALL_TOOLS.length}\\n`));
    expect(out).toContain('PASS: store_state ui has keys 2\n');
    // A value a tool returned: its length (the full result is in actions.jsonl).
    expect(out).toContain(`PASS: store_state ui.view reads a value "(${JSON.stringify({ value: 'global', path: `view of ${APP}` }).length} characters of the app's text withheld)"\n`);
    expect(out).toContain(`PASS: eval returns computed value "(2 characters of the app's text withheld)"\n`);
    // The crash-marker finding as a shareable artifact carries it: severity, kind, code location.
    expect(out).toMatch(
      /PASS: oracle detects backend crash marker \(fatal\) \{"severity":"fatal","kind":"crash-marker",.*"location":"src\/lib\.rs:10:5"/,
    );
    // The seed's result: its length only.
    expect(out).toMatch(/PASS: seed opens an isolated copy of the default fixture "\(\d+ characters of the app's text withheld\)"\n/);
  });

  it('a failing run prints no app text either, and still names a missing tool', async () => {
    const respond: Responder = (name) =>
      name === 'app_start'
        ? text({ error: `app build failed: ${APP} not found\n  in /home/${APP}/src-tauri` }, true)
        : text({});
    const { code, out } = await runSmoke(ALL_TOOLS.filter((tool) => tool !== 'run_scenario'), respond);

    expect(code).toBe(1);
    expect(out).toContain('MCP SMOKE FAILED');
    expect(out).not.toContain('PASS: app_start');
    // app_start's error as a crash headline: the prefix stays, the app's text is its length.
    expect(out).toContain(
      `app_start -> ready "app build failed: (${`${APP} not found`.length} characters of the app's text withheld)\\n  (1 line(s) withheld:`,
    );
    expect(out).not.toContain(APP);
    // The throw it ends in keeps the crash form (#576 W39-NF3): name, headline, frames.
    expect(out).toMatch(/: unexpected exception "Error: app_start failed: \(\d+ characters of the app's text withheld\)\\n {4}at /);
    // A missing tool's name is one of the smoke's own expected names.
    expect(out).toContain('"missing":["run_scenario"]');
  });
});

describe("agent:mcp:smoke passes the server's stderr through folded (#576 RV19b-P8)", () => {
  /** A synthetic home directory: the user a leak would name. */
  const HOME = `/home/${APP}`;

  it("folds the checkout and the home directory out of an app_start build's output", async () => {
    vi.stubEnv('HOME', HOME);
    const app = passingApp();
    const { code, out } = await runSmoke(ALL_TOOLS, (name, args) => {
      const server = stub.transport?.stderr;
      // What `pnpm tauri build` writes to the server's stderr: the home
      // directory split across two chunks, then Tauri's last line.
      if (name === 'app_start') {
        server?.write(`   Compiling stalagmight v1.0.0 (${HOME.slice(0, 7)}`);
        server?.write(`${HOME.slice(7)}/src/x)\n    Built application at: ${REPO}/src-tauri/target/debug/stalagmight\n`);
      }
      // A last line with no newline, written as the server stops.
      if (name === 'app_stop') server?.write(`[agent-mcp] stopped in ${HOME}/x`);
      return app(name, args);
    });

    // Piped, not inherited: inherited, the server writes to the terminal past the fold.
    expect(stub.transport?.options.stderr).toBe('pipe');
    expect(code).toBe(0);
    expect(out).toContain('   Compiling stalagmight v1.0.0 (~/src/x)\n');
    expect(out).toContain('    Built application at: <repo>/src-tauri/target/debug/stalagmight\n');
    // The trailing partial line is flushed when the stream ends, before the verdict.
    expect(out).toContain('[agent-mcp] stopped in ~/x');
    expect(out.indexOf('[agent-mcp] stopped in ~/x')).toBeLessThan(out.indexOf('MCP SMOKE PASSED'));
    expect(out).not.toContain(APP);
    expect(out).not.toContain(REPO);
  });

  it('passes each line on once it is whole, without waiting for the stream to end', async () => {
    vi.stubEnv('HOME', HOME);
    const { PassThrough } = await import('stream');
    const { passLinesFolded } = await import('./foldedLines');
    const input = new PassThrough();
    const written: string[] = [];
    const done = passLinesFolded(input, (text) => written.push(text));
    const tick = (): Promise<void> => new Promise((resolve) => setImmediate(resolve));

    // The home directory split across two chunks: folded once its line is whole.
    input.write(`a ${HOME}/1\nb ${HOME.slice(0, 7)}`);
    await tick();
    expect(written).toEqual(['a ~/1\n']);
    input.write(`${HOME.slice(7)}/2\nc ${HOME}`);
    await tick();
    expect(written).toEqual(['a ~/1\n', 'b ~/2\n']);
    input.end();
    await done;
    expect(written).toEqual(['a ~/1\n', 'b ~/2\n', 'c ~']);
  });
});
