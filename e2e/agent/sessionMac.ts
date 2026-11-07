/**
 * MacSession is the macOS session: tauri-wd (the W3C front-end for the in-app
 * tauri-plugin-webdriver-automation server, compiled in via the `e2e` cargo
 * feature) plus e2e/mac-app-wrapper.mjs, the same stack the legacy WDIO suite
 * uses on this platform. Everything platform-neutral lives in SessionCore; the
 * attach + readiness steps live in lib/sessionShared.
 *
 * Differences from the Linux AppSession that are contracts, not choices
 * (validated by the #421 Phase 0 probes - see
 * docs/plans/2026-08-05_macos-agent-harness.md section 8):
 *
 * - tauri-wd SPAWNS THE APP ITSELF (session -> tauri-wd -> wrapper -> app), so
 *   the isolated profile env is passed to the tauri-wd spawn and inherited down.
 * - `deleteSession` SIGKILLs the wrapper, which cannot forward SIGKILL, so the
 *   app is ORPHANED by a plain teardown. The wrapper records the real app pid
 *   into STALAGMIGHT_TEST_PIDFILE; the pidfile reap is the authoritative kill.
 * - macOS has no headless display: every session is headed on the real desktop.
 *   To coexist with whatever the human is doing, the window is parked on the
 *   right half of the screen immediately after attach (override with
 *   STALAGMIGHT_AGENT_WINDOW_RECT="x,y,w,h").
 * - Input synthesis is JS dispatchEvent in the webview (synthetic, untrusted)
 *   and screenshots are DOM-only; see the design doc's tier model.
 *
 * Instantiate through createSession(), not directly: it picks the platform.
 */
// FIRST import on purpose: patches webdriver's undici before webdriverio links it.
import './lib/undici-dispatcher-pin';
import { spawn, type ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { agentBuildMode, binaryPathFor, macWrapperPath } from '../lib/env';
import { builtFromFor, checkBuildFreshness, buildApp, type BuiltFrom } from './lib/build';
import { findFreePort, isPidAlive, recordPid, reapPidsFromFile, waitForPortListening } from './lib/procs';
import { SessionCore, gitHead, safeMtime } from './lib/sessionCore';
import { attachBrowser, verifyReadiness, E2EHooksMissingError } from './lib/sessionShared';
import { foldLocalPaths, valueShape, withheldAppText } from './lib/shareableFailure';
import type { StartOptions, StartResult } from './sessionTypes';

const BASE_PORT = Number.parseInt(process.env.STALAGMIGHT_AGENT_BASE_PORT ?? '4544', 10);
// Never 'node': the wrapper is a node process, and reaping by that comm name
// could kill unrelated tooling. Killing the app collapses the wrapper anyway
// (it exits on child-exit).
const REAP_COMM_NAMES = ['stalagmight', 'tauri-wd'];

interface WindowRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** STALAGMIGHT_AGENT_WINDOW_RECT="x,y,w,h" -> rect, or null when unset/bogus. */
function windowRectFromEnv(): WindowRect | null {
  const raw = process.env.STALAGMIGHT_AGENT_WINDOW_RECT;
  if (!raw) return null;
  const parts = raw.split(',').map((p) => Number.parseInt(p.trim(), 10));
  if (parts.length !== 4 || parts.some((n) => !Number.isInteger(n))) {
    process.stderr.write(
      // Its shape (#576 W51): the value is the user's text.
      `[agent] WARNING: ignoring malformed STALAGMIGHT_AGENT_WINDOW_RECT, ${valueShape(raw)} (want "x,y,w,h")\n`,
    );
    return null;
  }
  const [x, y, width, height] = parts as [number, number, number, number];
  return { x, y, width, height };
}

export class MacSession extends SessionCore {
  private driver: ChildProcess | null = null;
  private driverPort = 0;
  private driverLog: fs.WriteStream | null = null;

  protected reapCommNames(): string[] {
    return REAP_COMM_NAMES;
  }

  protected async startInner(opts: StartOptions): Promise<StartResult> {
    const build = opts.build ?? 'auto';
    const freshProfile = opts.freshProfile ?? true;

    this.reapPreviousSessions();

    // 1. Build freshness (the darwin arm adds `--features e2e`; the stamp's
    // e2eFeature bit keeps a plugin-less binary from passing as fresh). The
    // PROFILE is threaded the same way as on Linux, so STALAGMIGHT_AGENT_RELEASE
    // means the same thing on both platforms and the stamp says which it is.
    const buildMode = agentBuildMode();
    let buildAction: StartResult['buildAction'] = 'skipped';
    let rebuildReasons: string[] | undefined;
    // Who built the binary this session will drive. Taken from the freshness
    // result on the `fresh` path - the stamp it already read is the same stamp,
    // and asking again would be the second read that comment forbids. The other
    // two paths have no such stamp in hand: a build has just REWRITTEN it, and
    // `build: "skip"` never computed freshness at all.
    let builtFrom: BuiltFrom = null;
    if (build !== 'skip') {
      const freshness = checkBuildFreshness({ mode: buildMode });
      if (build === 'force' || !freshness.fresh) {
        this.state = 'building';
        if (build !== 'force' && freshness.reasons.length > 0) {
          // Folded (#576 W47-NF3): a reason can name the checkout a stamp came from.
          process.stderr.write(`Rebuilding: ${foldLocalPaths(freshness.reasons.join('; '))}\n`);
          // Reported alongside the result too: the stderr line is invisible to
          // an MCP client, and "why did that take four minutes" (and, since
          // #488, "whose binary was I about to drive") is exactly what a caller
          // needs to see.
          rebuildReasons = freshness.reasons;
        }
        await buildApp((line) => process.stderr.write(`${foldLocalPaths(line)}\n`), buildMode);
        buildAction = 'built';
        builtFrom = builtFromFor(buildMode);
      } else {
        buildAction = 'fresh';
        builtFrom = freshness.builtFrom;
      }
    } else {
      builtFrom = builtFromFor(buildMode);
    }

    this.state = 'launching';

    // 2. Display: macOS has no Xvfb equivalent, so headed is the only mode.
    // Coerce rather than refuse - callers get a session, not a lecture.
    const headedForced = !opts.headed;
    if (headedForced) {
      process.stderr.write(
        '[agent] NOTE: macOS sessions are always headed (no headless display); ' +
          'the app window will appear on the right half of the screen.\n',
      );
    }

    // 3. Session dir + isolated profile.
    const { profileRoot, profileEnv } = this.prepareSession(freshProfile);

    // 4. Port. tauri-wd has no native port; 0 in StartResult means "none".
    this.driverPort = await findFreePort(BASE_PORT);

    // 5. tauri-wd. It spawns wrapper -> app itself, so everything the app must
    // see (isolated HOME/TMPDIR, test hooks, the binary path for the wrapper,
    // the pidfile the wrapper appends the real app pid to) rides this env.
    this.driverLog = fs.createWriteStream(path.join(this.recorder.dir, 'tauri-wd.log'), {
      flags: 'a',
    });
    const driverLog = this.driverLog;
    this.driver = spawn('tauri-wd', ['--port', String(this.driverPort)], {
      stdio: ['ignore', 'pipe', 'pipe'],
      env: {
        ...process.env,
        ...profileEnv,
        STALAGMIGHT_TEST_HOOKS: '1',
        RUST_BACKTRACE: '1',
        STALAGMIGHT_BINARY: binaryPathFor(buildMode),
        STALAGMIGHT_TEST_PIDFILE: this.pidFile(),
      },
    });
    if (this.driver.pid) recordPid(this.pidFile(), this.driver.pid);
    this.driver.stdout?.on('data', (d: Buffer) => driverLog.write(d));
    this.driver.stderr?.on('data', (d: Buffer) => driverLog.write(d));
    const driverChild = this.driver;
    this.driver.on('error', (err) => {
      // ENOENT lands here, not in 'exit': surface the install path immediately
      // instead of a 15s port timeout (#422).
      if (this.driver !== driverChild) return;
      driverLog.write(`spawn error: ${err.message}\n`);
    });
    this.driver.on('exit', (code) => {
      // Only the CURRENT driver dying unexpectedly is a crash (see AppSession).
      if (this.driver !== driverChild) return;
      if (this.state !== 'stopping') {
        this.state = 'crashed';
        this.recorderInstance?.event('crash-detected', { reason: 'driver-exit', exitCode: code });
      }
    });

    const bound = await waitForPortListening(this.driverPort, 15000, () => !this.driverAlive());
    if (!bound) {
      const alive = this.driverAlive();
      await this.stop('driver-bind-timeout');
      throw new Error(
        alive
          ? `tauri-wd did not start listening on port ${this.driverPort} within 15s ` +
            `(see tauri-wd.log in the session dir)`
          : `tauri-wd exited before listening on port ${this.driverPort}. Is it installed? ` +
            '(`cargo install --git https://github.com/FumblingBarbell/tauri-webdriver ' +
            'tauri-webdriver-automation` puts the tauri-wd binary on PATH). ' +
            `Details in tauri-wd.log in the session dir.`,
      );
    }

    // 6. Attach. Same darwin capabilities the retired WDIO config used: the wrapper is the
    // "application" (it delays the port announcement past the window race and
    // records the app pid), the real binary rides STALAGMIGHT_BINARY.
    try {
      this.browserInstance = await attachBrowser(
        this.driverPort,
        {
          platformName: 'mac',
          'wdio:enforceWebDriverClassic': true,
          'tauri:options': { application: macWrapperPath },
        },
        'tauri-wd',
      );
    } catch (err) {
      await this.stop('attach-failed');
      throw err;
    }

    // 7. Park the window before any readiness wait, so the time it spends at
    // the default centered position (over whatever the human is doing) is as
    // short as possible. Best-effort: a failed move is a warning, not a failed
    // start.
    await this.parkWindow();

    // 8. Ready + hook verification.
    try {
      await verifyReadiness(this.browserInstance);
    } catch (err) {
      if (err instanceof E2EHooksMissingError) await this.stop('hooks-missing');
      throw err;
    }

    this.state = 'ready';
    this.startedAtMs = Date.now();
    this.recorder.sessionStart({
      sessionId: this.sessionId,
      startedAt: new Date().toISOString(),
      gitHead: gitHead(),
      binaryPath: binaryPathFor(buildMode),
      binaryMtime: safeMtime(binaryPathFor(buildMode)),
      display: 'aqua',
      headed: true,
      profileDir: profileRoot,
      freshProfile,
    });

    return {
      state: this.state,
      appPidKnown: this.appPid() !== null,
      driverPort: this.driverPort,
      nativePort: 0,
      display: 'aqua',
      profileDir: profileRoot,
      buildAction,
      builtFrom,
      ...(rebuildReasons ? { rebuildReasons } : {}),
      buildMode,
      sessionDir: this.recorder.dir,
      headedForced,
    };
  }

  /**
   * Last stalagmight pid the wrapper appended to the session pidfile - the
   * app's own pid, not the driver's. Public because Tier 2 OS input needs it to
   * aim at the app window (`e2e/agent/os/input.ts`); read from disk on every
   * call so a relaunch inside one session cannot serve a stale pid.
   */
  appPid(): number | null {
    try {
      const lines = fs.readFileSync(this.pidFile(), 'utf8').trim().split('\n');
      for (let i = lines.length - 1; i >= 0; i--) {
        const pid = Number.parseInt((lines[i] ?? '').trim(), 10);
        if (Number.isInteger(pid) && pid > 0 && pid !== this.driver?.pid) return pid;
      }
    } catch {
      // Missing pidfile: the wrapper never got far enough to record the app.
    }
    return null;
  }

  /**
   * Move the window to STALAGMIGHT_AGENT_WINDOW_RECT or the right half of the
   * available screen (verified working: the plugin's /window/set-rect drives
   * backend set_position/set_size, no webview capability involved).
   */
  private async parkWindow(): Promise<void> {
    const browser = this.browserInstance;
    if (!browser) return;
    try {
      let rect = windowRectFromEnv();
      if (!rect) {
        const screen = await browser.execute(() => ({
          w: window.screen.availWidth,
          h: window.screen.availHeight,
          left: (window.screen as { availLeft?: number }).availLeft ?? 0,
          top: (window.screen as { availTop?: number }).availTop ?? 0,
        }));
        rect = {
          x: screen.left + Math.floor(screen.w / 2),
          y: screen.top,
          width: Math.floor(screen.w / 2),
          height: screen.h,
        };
      }
      await browser.setWindowRect(rect.x, rect.y, rect.width, rect.height);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      // Its length (#576 W51): a WebDriver error can quote the page.
      process.stderr.write(`[agent] WARNING: could not position the app window: ${withheldAppText(msg)}\n`);
    }
  }

  driverAlive(): boolean {
    return Boolean(this.driver && this.driver.pid && isPidAlive(this.driver.pid));
  }

  async stop(reason = 'app_stop', opts: { keepProfile?: boolean } = {}): Promise<void> {
    this.state = 'stopping';
    if (this.browserInstance) {
      try {
        // Bounded, same rationale as AppSession. Known side effect (probe R3):
        // tauri-wd SIGKILLs the wrapper here and the APP SURVIVES - the pidfile
        // reap below is what actually kills it.
        await Promise.race([
          this.browserInstance.deleteSession(),
          new Promise((r) => setTimeout(r, 5000)),
        ]);
      } catch {
        // Session may already be dead.
      }
      this.browserInstance = null;
    }
    if (this.driver?.pid) {
      const pid = this.driver.pid;
      try {
        process.kill(pid, 'SIGTERM');
      } catch {
        // Already gone.
      }
      await new Promise((r) => setTimeout(r, 1500));
      if (isPidAlive(pid)) {
        try {
          process.kill(pid, 'SIGKILL');
        } catch {
          // Already gone.
        }
      }
      this.driver = null;
    }
    this.driverLog?.end();
    this.driverLog = null;
    // The authoritative app kill: the wrapper recorded the real app pid here.
    if (this.recorderInstance) {
      reapPidsFromFile(this.pidFile(), REAP_COMM_NAMES);
    }
    this.disposeProfile(opts.keepProfile);
    this.finishStop(reason);
  }
}
