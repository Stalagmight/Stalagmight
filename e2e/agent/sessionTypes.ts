/**
 * Platform-neutral session contract. `AgentSession` is exactly the surface the
 * MCP server, the oracles, the scenario framework and the smokes consume, so a
 * platform implementation (Linux tauri-driver today, macOS tauri-wd next) can be
 * swapped in behind createSession() without touching a caller. It structurally
 * satisfies the existing `OracleTarget` and `ScenarioTarget` seams.
 */
import type { Browser } from 'webdriverio';
import type { Recorder } from './lib/recorder';

export type SessionState =
  | 'idle'
  | 'building'
  | 'launching'
  | 'ready'
  | 'busy'
  | 'crashed'
  | 'stopping';

export type BuildMode = 'auto' | 'skip' | 'force';

export interface StartOptions {
  build?: BuildMode;
  headed?: boolean | undefined;
  freshProfile?: boolean | undefined;
}

export interface StartResult {
  state: SessionState;
  appPidKnown: boolean;
  driverPort: number;
  /** 0 on darwin: tauri-wd has no native port. */
  nativePort: number;
  /** Xvfb/X display on Linux; the sentinel 'aqua' on darwin. */
  display: string;
  profileDir: string;
  buildAction: 'skipped' | 'built' | 'fresh';
  /**
   * Which source tree built the binary being driven, per its E2E build stamp.
   * The cargo target dir is shared across worktrees, so "which tree is this" is
   * not answerable from the session's own cwd - see issue #488.
   *
   * Null means NO STAMP VOUCHES FOR THE BINARY ON DISK, which is more than
   * "somebody else built it": no stamp at all, a v1/v2/corrupt one, a stamp our
   * own build failed to write, or a stamp whose recorded binary mtime no longer
   * matches the file (`./scripts/dev.sh` relinked it in place). Null is "cannot
   * say", never "not this tree".
   */
  builtFrom: { repoRoot: string; gitHead: string } | null;
  /**
   * Why the binary was rebuilt, when staleness (not `build: "force"`) triggered
   * it. Absent when nothing was rebuilt or the caller forced the build.
   */
  rebuildReasons?: string[];
  /** Cargo profile this session is driving ('release' only when opted in). */
  buildMode: 'debug' | 'release';
  sessionDir: string;
  /**
   * Set (true) on darwin whenever the caller did not explicitly ask for
   * headed:true - macOS has no headless display, so the default headless
   * request is coerced to a visible window. Absent on Linux.
   */
  headedForced?: boolean;
}

export interface AgentSession {
  /** Mutable: driver_script's wedge handler poisons a session by writing 'crashed'. */
  state: SessionState;
  readonly browser: Browser;
  readonly recorder: Recorder;
  readonly hasRecorder: boolean;
  readonly sessionDir: string;
  /** The app's isolated data dir (crash markers/logs live under here). */
  readonly profileDataDir: string | null;
  exclusive<T>(fn: () => Promise<T>): Promise<T>;
  /**
   * The launched app's own pid, when the platform can identify it. Tier 2 OS
   * input aims at that window and only that window, so it is a hard requirement
   * there - and optional here because the Linux tauri-driver route never learns
   * the app pid (it only owns the driver's).
   */
  appPid?(): number | null;
  findCrashMarkers(): string[];
  start(opts?: StartOptions): Promise<StartResult>;
  stop(reason?: string, opts?: { keepProfile?: boolean }): Promise<void>;
  uptimeMs(): number;
  driverAlive(): boolean;
}
