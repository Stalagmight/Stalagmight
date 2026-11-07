/**
 * Shared X display helpers for running the real Tauri binary headless. Used by
 * the agent harness. On Linux with no DISPLAY/WAYLAND, start a
 * throwaway Xvfb server so the webview has somewhere to render.
 */
import { spawn, execSync, type ChildProcess } from 'child_process';

/** True if a usable display is already available (or the platform always has one). */
export function checkDisplayAvailable(): boolean {
  if (process.platform === 'win32') return true;
  if (process.platform === 'darwin') return true;
  if (process.env.DISPLAY) return true;
  if (process.env.WAYLAND_DISPLAY) return true;
  return false;
}

/**
 * The environment a headless app gets on a throwaway Xvfb display: `base` with
 * DISPLAY set to it and the two things a desktop session would otherwise reach
 * into turned off. Returns a new object; `base` is not changed.
 *
 * - GTK is pinned to X11 and WAYLAND_DISPLAY dropped: with WAYLAND_DISPLAY set (a
 *   Wayland desktop), GTK prefers Wayland and would render the "headless" app onto
 *   the real compositor, ignoring DISPLAY entirely.
 * - The accessibility bridge is off (`NO_AT_BRIDGE=1`; #576 NF-W12H-2). GTK and
 *   WebKit ask the session bus for the AT-SPI bus, and `at-spi-bus-launcher`
 *   serves it from `$XDG_RUNTIME_DIR/at-spi/bus` - ONE path per runtime dir,
 *   whatever session bus started it. Several harness runs each under their own
 *   `dbus-run-session` (one runtime dir) each start a launcher, every launcher
 *   binds that same path, and an app whose launcher lost the race gets no answer
 *   there: it logs "Couldn't register with accessibility bus: Did not receive a
 *   reply", never finishes starting, and its harness dies in `waitForAppReady`
 *   with "invalid session id". Measured on a Linux host, six sessions released
 *   at once per round: 11 of 30 died that way over five rounds, 0 of 42 with the
 *   bridge off. Nothing the harness drives goes through AT-SPI.
 */
export function headlessAppEnv(base: NodeJS.ProcessEnv, display: string): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...base, DISPLAY: display, GDK_BACKEND: 'x11', NO_AT_BRIDGE: '1' };
  delete env.WAYLAND_DISPLAY;
  return env;
}

export interface XvfbHandle {
  /** The `:N` display the server bound to. */
  display: string;
  /** The Xvfb child process (kill it when done). */
  proc: ChildProcess;
}

/**
 * Try to start an Xvfb server on one of `displays` at `screen` (e.g.
 * "1280x720x24"). Returns a handle with the chosen display and child process,
 * or null if Xvfb is missing or every candidate failed to start.
 *
 * Does NOT mutate `process.env`: the caller sets DISPLAY on whatever
 * environment it will hand to child processes. This is what lets the agent run
 * a headless app without changing the MCP server's own environment.
 */
export async function startXvfb(displays: string[], screen: string): Promise<XvfbHandle | null> {
  if (process.platform === 'win32') return null;

  try {
    execSync('which Xvfb', { stdio: 'ignore' });
  } catch {
    return null;
  }

  for (const display of displays) {
    const child = spawn('Xvfb', [display, '-screen', '0', screen, '-ac', '-nolisten', 'tcp'], {
      stdio: 'ignore',
    });

    const started = await new Promise<boolean>((resolve) => {
      let settled = false;
      const timer = setTimeout(() => {
        if (!settled) {
          settled = true;
          resolve(child.exitCode === null);
        }
      }, 250);
      child.once('error', () => {
        if (!settled) {
          settled = true;
          clearTimeout(timer);
          resolve(false);
        }
      });
      child.once('exit', () => {
        if (!settled) {
          settled = true;
          clearTimeout(timer);
          resolve(false);
        }
      });
    });

    if (started) {
      return { display, proc: child };
    }
  }

  return null;
}
