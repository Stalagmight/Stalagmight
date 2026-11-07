/**
 * SessionCore holds everything about a harness session that does NOT depend on
 * the platform's WebDriver stack: the state machine, the command mutex, the
 * recorder + ownership marker, the isolated profile bookkeeping, orphan reaping
 * and crash-marker discovery. Platform classes (AppSession on Linux) implement
 * only the launch/teardown half.
 *
 * The mutex and state transitions are behavior-critical (a heartbeat must never
 * interleave with a tool call, and a rejected call must not break the chain),
 * which is why they live here once instead of being duplicated per platform.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { fileURLToPath } from 'url';
import type { Browser } from 'webdriverio';
import { isolatedEnv } from '../../lib/env';
import type {
  AgentSession,
  SessionState,
  StartOptions,
  StartResult,
} from '../sessionTypes';
import { gitHeadFor } from './build';
import { isPidAlive, reapPidsFromFile } from './procs';
import { Recorder, ARTIFACTS_ROOT, OWNER_FILE, allocateSessionDir } from './recorder';

/** This harness tree, derived from the module's own URL (see recorder.ts). */
const harnessRepoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');

/**
 * Short HEAD of THIS harness tree, for the session record.
 *
 * Anchored to the module's own location, never the process cwd: the MCP server
 * is launched with no cwd pinning, so an inherited cwd could be any repo on the
 * machine (or none), and a session recorded under a stranger's HEAD is a repro
 * nobody can reconstruct. Same defect class as issue #488, one layer up.
 */
export function gitHead(): string {
  return gitHeadFor(harnessRepoRoot);
}

export function safeMtime(file: string): number {
  try {
    return Math.floor(fs.statSync(file).mtimeMs);
  } catch {
    return 0;
  }
}

/**
 * Warn when the running Node major differs from the repo pin in `.nvmrc`.
 *
 * Advisory only: the harness is known to work on more than one major, so this must
 * not block. It exists because the harness is the one tier a human runs by hand,
 * where nothing selects the runtime for you - CI gets it from `setup-node`.
 * Deliberately derived, never a literal, so the next toolchain bump cannot leave a
 * stale number here.
 */
function warnIfOffPinnedNode(): void {
  try {
    // fileURLToPath, not __dirname: this tree is ESM ("type": "module"), where
    // __dirname is undefined at runtime even though @types/node declares it - so
    // the __dirname form typechecks clean and then silently never warns.
    const here = path.dirname(fileURLToPath(import.meta.url));
    const nvmrc = path.resolve(here, '..', '..', '..', '.nvmrc');
    const want = Number.parseInt(fs.readFileSync(nvmrc, 'utf8').trim().replace(/^v/, ''), 10);
    const running = Number.parseInt(process.versions.node.split('.')[0] ?? '', 10);
    if (Number.isInteger(want) && Number.isInteger(running) && want !== running) {
      process.stderr.write(
        `[agent] WARNING: running Node ${process.versions.node}, but .nvmrc pins ${want}. ` +
          `The harness is not routinely exercised off-pin; if app_start fails oddly, ` +
          `try \`nvm use\` first.\n`,
      );
    }
  } catch {
    // Never let a diagnostic break a session start.
  }
}

export abstract class SessionCore implements AgentSession {
  state: SessionState = 'idle';
  protected browserInstance: Browser | null = null;
  protected profileRoot: string | null = null;
  protected profileDataHome: string | null = null;
  protected recorderInstance: Recorder | null = null;
  protected sessionId = '';
  protected startedAtMs = 0;
  private queue: Promise<unknown> = Promise.resolve();

  /** Comm names this platform is allowed to SIGKILL when reaping a pidfile. */
  protected abstract reapCommNames(): string[];

  /** Platform launch: everything from build freshness to a ready browser. */
  protected abstract startInner(opts: StartOptions): Promise<StartResult>;

  abstract stop(reason?: string, opts?: { keepProfile?: boolean }): Promise<void>;

  abstract driverAlive(): boolean;

  get browser(): Browser {
    if (!this.browserInstance) throw new Error('no live session (call app_start)');
    return this.browserInstance;
  }

  get recorder(): Recorder {
    if (!this.recorderInstance) throw new Error('no recorder (session not started)');
    return this.recorderInstance;
  }

  get sessionDir(): string {
    return this.recorderInstance?.dir ?? '';
  }

  get hasRecorder(): boolean {
    return this.recorderInstance !== null;
  }

  /** The app's isolated XDG_DATA_HOME (crash markers/logs live under here). */
  get profileDataDir(): string | null {
    return this.profileDataHome;
  }

  /** Serialize a WebDriver interaction; sets state busy->ready around it. */
  async exclusive<T>(fn: () => Promise<T>): Promise<T> {
    const run = this.queue.then(async () => {
      if (this.state !== 'ready' && this.state !== 'busy') {
        throw new Error(`session not ready (state=${this.state})`);
      }
      this.state = 'busy';
      try {
        return await fn();
      } finally {
        if (this.state === 'busy') this.state = 'ready';
      }
    });
    // Keep the queue chained even if this call rejects.
    this.queue = run.catch(() => undefined);
    return run;
  }

  protected pidFile(): string {
    return path.join(this.recorder.dir, 'pids');
  }

  /** Recursively find crash-*.json markers under the isolated data dir. */
  findCrashMarkers(): string[] {
    const root = this.profileDataHome;
    if (!root) return [];
    const out: string[] = [];
    const walk = (dir: string) => {
      let entries: fs.Dirent[];
      try {
        entries = fs.readdirSync(dir, { withFileTypes: true });
      } catch {
        return;
      }
      for (const e of entries) {
        const full = path.join(dir, e.name);
        if (e.isDirectory()) walk(full);
        else if (e.name.startsWith('crash-') && e.name.endsWith('.json')) out.push(full);
      }
    };
    walk(root);
    return out;
  }

  async start(opts: StartOptions = {}): Promise<StartResult> {
    if (this.state !== 'idle' && this.state !== 'crashed') {
      throw new Error(`cannot start from state ${this.state}`);
    }
    // Warn when the running Node is not the pinned one. This used to hardcode 22,
    // justified by webdriverio handing an `Agent` from its bundled undici to
    // Node's built-in fetch so the undici majors had to agree - which is not what
    // webdriver 9.23.0 does (it imports `fetch` from undici alongside `Agent`,
    // build/node.js:1614, and calls that fetch at :1986). A stub-driver session
    // succeeds identically on Node 22 and 24, so the constant was wrong.
    //
    // The CHECK is still worth having: "you are off-pin" is useful whichever layer
    // breaks, and the verified envelope is only {22, 24} while node@26 already
    // exists. So it reads .nvmrc rather than asserting a major, which also means it
    // cannot go stale at the next bump. See docs/agent-harness.md.
    warnIfOffPinnedNode();
    try {
      return await this.startInner(opts);
    } catch (err) {
      // Any throw after something was spawned (build, Xvfb, tauri-driver,
      // waitForAppReady, hook/onboarding checks) must not leak the process
      // group, the Xvfb child, or the temp profile. Failure paths that already
      // stopped leave state 'idle', which skips the duplicate stop.
      if (this.state !== 'idle') await this.stop('start-failed');
      throw err;
    }
  }

  /**
   * Session dir + ownership marker + isolated profile. Returns the profile root
   * and the env the platform passes to the driver it spawns. A profile kept
   * alive by stop(keepProfile) is reused when freshProfile=false; a fresh start
   * discards any kept one first.
   */
  protected prepareSession(freshProfile: boolean): {
    profileRoot: string;
    profileEnv: Record<string, string>;
  } {
    // A directory of our own, created exclusively and already carrying the
    // ownership marker: reapPreviousSessions in ANOTHER harness process skips
    // session dirs whose owning process is still alive, so starting a second
    // agent session can never SIGKILL a live sibling's app/driver mid-run - and
    // two processes starting in the same millisecond can no longer be handed
    // the same directory (#576 HFI-14, see `sessionIdFor`).
    this.sessionId = allocateSessionDir().id;
    this.recorderInstance = new Recorder(this.sessionId);
    if (freshProfile && this.profileRoot) {
      try {
        fs.rmSync(this.profileRoot, { recursive: true, force: true });
      } catch {
        // Best-effort.
      }
      this.profileRoot = null;
    }
    const profileRoot =
      this.profileRoot ?? fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-agent-'));
    this.profileRoot = profileRoot;
    const profileEnv = isolatedEnv(profileRoot);
    this.profileDataHome = profileEnv.XDG_DATA_HOME ?? profileEnv.HOME ?? profileRoot;
    return { profileRoot, profileEnv };
  }

  /** Teardown half of the profile bookkeeping above. */
  protected disposeProfile(keepProfile = false): void {
    if (this.profileRoot && !keepProfile) {
      try {
        fs.rmSync(this.profileRoot, { recursive: true, force: true });
      } catch {
        // Best-effort.
      }
      this.profileRoot = null;
      this.profileDataHome = null;
    }
  }

  protected finishStop(reason: string): void {
    this.recorderInstance?.sessionEnd(reason);
    this.state = 'idle';
  }

  protected reapPreviousSessions(): void {
    let dirs: string[];
    try {
      dirs = fs.readdirSync(ARTIFACTS_ROOT);
    } catch {
      return;
    }
    for (const name of dirs) {
      const dir = path.join(ARTIFACTS_ROOT, name);
      // Liveness guard: skip sessions whose owning harness process is still
      // alive - that is a concurrently-running sibling, and reaping its
      // pidfile would SIGKILL a LIVE session's app/driver mid-run. Our own
      // pid means an earlier session of this process (we only start from
      // idle/crashed), whose orphans are safe to reap.
      try {
        const owner = Number.parseInt(fs.readFileSync(path.join(dir, OWNER_FILE), 'utf8').trim(), 10);
        if (Number.isInteger(owner) && owner > 0 && owner !== process.pid && isPidAlive(owner)) {
          continue;
        }
      } catch {
        // No owner marker (pre-marker session dir): assume dead, reap below.
      }
      reapPidsFromFile(path.join(dir, 'pids'), this.reapCommNames());
    }
  }

  uptimeMs(): number {
    return this.startedAtMs ? Date.now() - this.startedAtMs : 0;
  }
}
