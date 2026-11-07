/**
 * AppSession is the LINUX session: build freshness, Xvfb, tauri-driver, the
 * remote() attach and a process-group teardown. Everything platform-neutral
 * (state machine, command mutex, recorder, profile, orphan reaping) lives in
 * SessionCore; the attach + readiness steps live in lib/sessionShared. One
 * session per process. All WebDriver traffic is serialized through the command
 * mutex so a heartbeat/health probe never interleaves with a tool call.
 *
 * Instantiate through createSession(), not directly: it picks the platform.
 */
// FIRST import on purpose: patches webdriver's undici before webdriverio (linked
// transitively below) links it. See the module comment for the Node >= 26 failure
// it fixes.
import './lib/undici-dispatcher-pin';
import { spawn, type ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { agentBuildMode, binaryPathFor } from '../lib/env';
import { checkDisplayAvailable, headlessAppEnv, startXvfb, type XvfbHandle } from '../lib/display';
import { builtFromFor, checkBuildFreshness, buildApp, type BuiltFrom } from './lib/build';
import { DEFAULT_XVFB_DISPLAYS, parseDisplayPool } from './lib/displayPool';
import { findFreePort, isPidAlive, recordPid, reapPidsFromFile, waitForPortListening } from './lib/procs';
import { SessionCore, gitHead, safeMtime } from './lib/sessionCore';
import { attachBrowser, verifyReadiness, E2EHooksMissingError } from './lib/sessionShared';
import { foldLocalPaths } from './lib/shareableFailure';
import type { StartOptions, StartResult } from './sessionTypes';

export type {
  SessionState,
  BuildMode,
  StartOptions,
  StartResult,
  AgentSession,
} from './sessionTypes';

const BASE_PORT = Number.parseInt(process.env.STALAGMIGHT_AGENT_BASE_PORT ?? '4544', 10);
/**
 * The Xvfb pool for this session, and whether the caller pinned it.
 *
 * The pool itself and its validation live in `lib/displayPool.ts`; both are
 * read HERE at call time, never cached at import - the same rule
 * `agentBuildMode()` follows in `e2e/lib/env.ts`, and for the same two reasons:
 * a runner may set the variable after this module's graph has loaded, and a bad
 * value must surface as a failed `start()` rather than as an uncaught throw
 * that takes down whatever imported us (the MCP server imports this file).
 */
function xvfbDisplays(): string[] {
  return parseDisplayPool(process.env.STALAGMIGHT_AGENT_DISPLAYS);
}

/** True when the caller pinned the pool, i.e. there is deliberately no fallback. */
function displayPoolPinned(): boolean {
  return process.env.STALAGMIGHT_AGENT_DISPLAYS !== undefined;
}

const REAP_COMM_NAMES = ['stalagmight', 'tauri-driver', 'WebKitWebDriver'];

export class AppSession extends SessionCore {
  private driver: ChildProcess | null = null;
  private xvfb: XvfbHandle | null = null;
  private display = '';
  private driverPort = 0;
  private nativePort = 0;
  private driverLog: fs.WriteStream | null = null;

  protected reapCommNames(): string[] {
    return REAP_COMM_NAMES;
  }

  protected async startInner(opts: StartOptions): Promise<StartResult> {
    const build = opts.build ?? 'auto';
    const freshProfile = opts.freshProfile ?? true;

    // Reap orphans from previous sessions (scoped to recorded pidfiles).
    this.reapPreviousSessions();

    // Parse the display pool FIRST, before anything has moved `state` or spent
    // minutes in cargo: a malformed STALAGMIGHT_AGENT_DISPLAYS is a caller
    // error, and throwing it from the middle of the launch would leave the
    // session stuck in `building` with a rebuild already paid for.
    const pool = opts.headed ? [] : xvfbDisplays();

    // 1. Build freshness. The profile is resolved ONCE here and threaded
    // through, so a mid-start env change cannot make the freshness check and
    // the launch disagree about which binary they mean.
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

    // 2. Display. headed:true shows the window on the real desktop; the
    // default is a throwaway Xvfb EVEN IF a desktop display exists, so agent
    // runs never pop a window or steal focus while someone is working.
    if (opts.headed) {
      if (!checkDisplayAvailable()) {
        this.state = 'idle';
        throw new Error('headed:true requested but no display (DISPLAY/WAYLAND_DISPLAY) is available');
      }
      this.display = process.env.DISPLAY ?? ':0';
    } else {
      // 1920x1080 screen so window-size testing has headroom up to full HD
      // (the app window starts smaller; resizing is capped by the screen).
      // `pool` was resolved at the top of this method - inside start() so a
      // malformed value fails this session rather than throwing while some
      // importer's module graph is still loading, and BEFORE the build so it
      // does not surface after a ten-minute compile.
      this.xvfb = await startXvfb(pool, '1920x1080x24');
      if (this.xvfb) {
        this.display = this.xvfb.display;
      } else if (displayPoolPinned()) {
        // A pinned pool means a parallel runner handed this session ONE display
        // and is relying on it, so there is nothing to walk on to. Falling
        // through to the real display here would put the app window on the
        // user's desktop and drive it with pointer and keyboard input for the
        // whole run - the precise thing the pool exists to prevent.
        this.state = 'idle';
        throw new Error(
          `Xvfb could not start on ${pool.join(', ')} and STALAGMIGHT_AGENT_DISPLAYS pins the ` +
            `pool, so there is no other display to try. Usually a leaked Xvfb (or its stale lock) still ` +
            `holds the slot: check 'ls /tmp/.X*-lock' and 'pgrep -a Xvfb', or unset ` +
            `STALAGMIGHT_AGENT_DISPLAYS to use the default ${DEFAULT_XVFB_DISPLAYS.length}-display pool.`,
        );
      } else if (checkDisplayAvailable()) {
        process.stderr.write(
          '[agent] WARNING: Xvfb unavailable; falling back to the real display (the app window WILL be visible).\n',
        );
        this.display = process.env.DISPLAY ?? ':0';
      } else {
        this.state = 'idle';
        throw new Error('no display available and Xvfb could not start');
      }
    }

    // 3. Session dir + isolated profile.
    const { profileRoot, profileEnv } = this.prepareSession(freshProfile);

    // 4. Ports.
    this.driverPort = await findFreePort(BASE_PORT);
    this.nativePort = await findFreePort(this.driverPort + 1000);

    // 5. tauri-driver, with the isolated env + test hooks (app inherits it).
    this.driverLog = fs.createWriteStream(path.join(this.recorder.dir, 'tauri-driver.log'), {
      flags: 'a',
    });
    const driverLog = this.driverLog;
    const sessionEnv: NodeJS.ProcessEnv = {
      ...process.env,
      ...profileEnv,
      STALAGMIGHT_TEST_HOOKS: '1',
      RUST_BACKTRACE: '1',
    };
    // On the virtual display: GTK pinned to X11, and no AT-SPI bridge, which
    // parallel runs under their own `dbus-run-session` race for (see
    // `headlessAppEnv`). A headed session keeps the real desktop's environment.
    const driverEnv = this.xvfb
      ? headlessAppEnv(sessionEnv, this.display)
      : { ...sessionEnv, DISPLAY: this.display };
    this.driver = spawn(
      'tauri-driver',
      ['--port', String(this.driverPort), '--native-port', String(this.nativePort)],
      {
        // detached makes tauri-driver a process-group leader so stop() can kill
        // the whole tree (tauri-driver + WebKitWebDriver + the app) with one
        // scoped process.kill(-pid); killing tauri-driver alone orphans the
        // WebKitWebDriver daemon it spawns.
        detached: true,
        stdio: ['ignore', 'pipe', 'pipe'],
        env: driverEnv,
      },
    );
    if (this.driver.pid) recordPid(this.pidFile(), this.driver.pid);
    this.driver.stdout?.on('data', (d: Buffer) => driverLog.write(d));
    this.driver.stderr?.on('data', (d: Buffer) => driverLog.write(d));
    const driverChild = this.driver;
    this.driver.on('exit', (code) => {
      // Only the CURRENT driver dying unexpectedly is a crash. A late exit
      // event from a driver this session already stopped or replaced (e.g. a
      // SIGKILL-path stop reaps after stop() returned and reset state, or
      // app_restart already spawned a successor) must not flip a healthy
      // session to 'crashed' with a bogus crash-detected log line.
      if (this.driver !== driverChild) return;
      if (this.state !== 'stopping') {
        this.state = 'crashed';
        this.recorderInstance?.event('crash-detected', { reason: 'driver-exit', exitCode: code });
      }
    });

    // Wait for tauri-driver to actually accept connections before attaching
    // (a fixed sleep is both slower and flakier on a loaded box).
    const bound = await waitForPortListening(this.driverPort, 15000, () => !this.driverAlive());
    if (!bound) {
      await this.stop('driver-bind-timeout');
      throw new Error(
        `tauri-driver did not start listening on port ${this.driverPort} within 15s ` +
          `(see tauri-driver.log in the session dir)`,
      );
    }

    // 6. Attach. The capability set the retired WDIO config proved out (no browserName;
    // tauri-driver reads the non-standard tauri:options).
    try {
      this.browserInstance = await attachBrowser(this.driverPort, {
        platformName: 'linux',
        'wdio:enforceWebDriverClassic': true,
        'tauri:options': { application: binaryPathFor(buildMode) },
      });
    } catch (err) {
      await this.stop('attach-failed');
      throw err;
    }

    // 7. Ready + hook verification.
    try {
      await verifyReadiness(this.browserInstance);
    } catch (err) {
      // Only the hooks failure gets its own stop reason; anything else falls
      // through to start()'s generic 'start-failed' teardown.
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
      display: this.display,
      headed: Boolean(opts.headed),
      profileDir: profileRoot,
      freshProfile,
    });

    return {
      state: this.state,
      appPidKnown: false,
      driverPort: this.driverPort,
      nativePort: this.nativePort,
      display: this.display,
      profileDir: profileRoot,
      buildAction,
      builtFrom,
      ...(rebuildReasons ? { rebuildReasons } : {}),
      buildMode,
      sessionDir: this.recorder.dir,
    };
  }

  driverAlive(): boolean {
    return Boolean(this.driver && this.driver.pid && isPidAlive(this.driver.pid));
  }

  /**
   * Stop the session. `keepProfile` preserves the isolated profile dir so a
   * following start({freshProfile: false}) sees the same persisted state
   * (app_restart's persisted-state repro mode depends on this).
   */
  async stop(reason = 'app_stop', opts: { keepProfile?: boolean } = {}): Promise<void> {
    this.state = 'stopping';
    if (this.browserInstance) {
      try {
        // Bounded: an unresponsive driver would otherwise block stop() for the
        // full webdriverio connectionRetryTimeout (30s) before the group kill
        // below ever runs.
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
      const groupPid = this.driver.pid;
      // Kill the whole process group (tauri-driver + WebKitWebDriver + app).
      try {
        process.kill(-groupPid, 'SIGTERM');
      } catch {
        // Group already gone.
      }
      await new Promise((r) => setTimeout(r, 1500));
      try {
        process.kill(-groupPid, 'SIGKILL');
      } catch {
        // Group already gone.
      }
      this.driver = null;
    }
    // Close the tauri-driver log stream; without this the fd leaks across
    // every app_restart cycle for the life of the MCP server process.
    this.driverLog?.end();
    this.driverLog = null;
    // Backstop: scoped reap of anything this session recorded, by verified comm.
    if (this.recorderInstance) {
      reapPidsFromFile(this.pidFile(), REAP_COMM_NAMES);
    }
    if (this.xvfb) {
      const xvfbPid = this.xvfb.proc.pid;
      try {
        this.xvfb.proc.kill();
      } catch {
        // Already gone.
      }
      // Escalate: nothing reaps a wedged Xvfb across runs, and a leaked one
      // holds its display slot from the small display pool forever.
      if (xvfbPid) {
        await new Promise((r) => setTimeout(r, 300));
        if (isPidAlive(xvfbPid)) {
          try {
            process.kill(xvfbPid, 'SIGKILL');
          } catch {
            // Already gone.
          }
        }
      }
      this.xvfb = null;
    }
    this.disposeProfile(opts.keepProfile);
    this.finishStop(reason);
  }
}
