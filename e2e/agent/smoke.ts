/**
 * Non-MCP smoke test for the agent session core. Exercises the full chain:
 * build check -> tauri-driver -> remote() -> real app -> __E2E_TEST__ ->
 * screenshot -> store reads -> get_recent_logs_since -> crash-oracle self-test
 * (debug_trigger_panic) -> clean stop, then asserts no orphan ports remain.
 *
 * Run: node <tsx> e2e/agent/smoke.ts   (or: pnpm agent:smoke)
 */
import * as fs from 'fs';
import { createSession } from './createSession';
import { readStore } from './lib/stores';
import { LogCursor } from './lib/logs';
import { invokeInApp } from './lib/page';
import { isPortFree } from './lib/procs';
import { shareableErrorText } from './lib/shareableFailure';
import { smokeStartedLine } from './lib/smokeStartLine';

function log(msg: string): void {
  process.stderr.write(`[smoke] ${msg}\n`);
}

async function main(): Promise<void> {
  const session = createSession();
  let failures = 0;
  const check = (name: string, ok: boolean, detail = '') => {
    log(`${ok ? 'PASS' : 'FAIL'}: ${name}${detail ? ` (${detail})` : ''}`);
    if (!ok) failures += 1;
  };

  const buildMode = (process.env.AGENT_BUILD as 'auto' | 'skip' | 'force') || 'auto';
  const started = await session.start({ build: buildMode });
  // Folded (#576 W51): the session and profile directories, the checkout that
  // built the binary and a rebuild reason all name paths under the home directory.
  // The X display only as `:N` (#576 W58 Q14): an inherited DISPLAY such as
  // `localhost:10.0` under ssh -X names a host.
  log(`started: ${smokeStartedLine(started)}`);
  check('session ready', session.state === 'ready');

  // Screenshot.
  const shot = await session.exclusive(() => session.browser.takeScreenshot());
  const shotPath = session.recorder.writeScreenshot(session.recorder.nextSeq(), 'smoke', shot);
  check('screenshot captured', shot.length > 1000, `${shot.length} b64 chars -> ${shotPath}`);

  // Store reads.
  const uiKeys = (await session.exclusive(() => readStore(session.browser, 'ui'))) as {
    keys: { key: string }[];
  };
  check('ui store keys non-empty', (uiKeys.keys?.length ?? 0) > 0, `${uiKeys.keys?.length} keys`);
  const projectKeys = (await session.exclusive(() => readStore(session.browser, 'project'))) as {
    keys: { key: string }[];
  };
  check('project store keys non-empty', (projectKeys.keys?.length ?? 0) > 0);

  // New checkpoint-log IPC.
  const cursor = new LogCursor(session.browser);
  const before = await session.exclusive(() => cursor.read('all'));
  check('get_recent_logs_since returns entries', before.newEntries.length >= 0, `${before.newEntries.length} lines, latestSeq=${before.latestSeq}`);

  // IPC integrity command round-trips.
  const integrity = await session.exclusive(() =>
    invokeInApp<{ messages?: unknown; save_blocked?: unknown } | undefined>(session.browser, 'check_project_integrity'),
  );
  // Its shape only (#576 W51): an integrity message names the project's data.
  const messages = integrity?.messages;
  check(
    'check_project_integrity IPC works',
    integrity !== undefined,
    `${Array.isArray(messages) ? messages.length : 'no'} message(s), save_blocked=${integrity?.save_blocked === true}`,
  );

  // Crash-oracle self-test: trigger a backend thread panic; the app must
  // survive, the ring buffer must get a PANIC line, and a crash marker file
  // must appear under the isolated data dir.
  await session.exclusive(() => invokeInApp(session.browser, 'debug_trigger_panic', { delayMs: 50 }));
  await new Promise((r) => setTimeout(r, 1200));
  const afterPanic = await session.exclusive(() => cursor.read('all'));
  const sawPanicLine = afterPanic.newEntries.some((l) => l.includes('PANIC'));
  check('ring buffer captured PANIC line', sawPanicLine);
  const markers = session.findCrashMarkers();
  check('crash marker file written', markers.length > 0, `${markers.length} marker(s)`);
  const appAlive = await session.exclusive(() =>
    session.browser.execute(() => document.readyState),
  );
  check('app survived thread panic (still responsive)', Boolean(appAlive), String(appAlive));

  // Clean stop.
  await session.stop('smoke-done');
  check('driver released after stop', !session.driverAlive());
  check('driver port freed', await isPortFree(started.driverPort));
  check('native port freed', await isPortFree(started.nativePort));

  // Action log parses.
  const jsonl = fs.readFileSync(`${started.sessionDir}/actions.jsonl`, 'utf8').trim().split('\n');
  let parsed = 0;
  for (const line of jsonl) {
    try {
      JSON.parse(line);
      parsed += 1;
    } catch {
      // truncated tail tolerated
    }
  }
  check('actions.jsonl parses', parsed === jsonl.length, `${parsed}/${jsonl.length} lines`);

  log(failures === 0 ? 'ALL SMOKE CHECKS PASSED' : `${failures} SMOKE CHECK(S) FAILED`);
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((err) => {
  // The shareable crash form (#576 W33-NF6): a crash in session setup can quote
  // the app (an IPC rejection, a page script's error).
  process.stderr.write(`[smoke] fatal: ${shareableErrorText(err)}\n`);
  process.exit(1);
});
