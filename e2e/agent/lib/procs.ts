/**
 * Process + port helpers for the agent harness. Port probing and SCOPED
 * process reaping: only PIDs recorded in a session pidfile whose comm name
 * matches an expected one are killed. Never a broad `pkill stalagmight` (that
 * lesson came from the retired scripts/wdio.sh).
 *
 * comm/pgid come from /proc on Linux and from `ps` on macOS, which has no
 * /proc - without the darwin arm the comm check never matches and every reap
 * silently no-ops, leaking the app and its driver.
 */
import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as net from 'net';
import * as path from 'path';

const isDarwin = process.platform === 'darwin';

/** One `ps -o <field>= -p <pid>` line, or '' when the pid is gone (ps exits 1). */
function psField(pid: number, field: string): string {
  try {
    return execFileSync('ps', ['-o', `${field}=`, '-p', String(pid)], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim();
  } catch {
    return '';
  }
}

/** True if `port` on 127.0.0.1 can be bound right now. */
export function isPortFree(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const server = net.createServer();
    server.once('error', () => resolve(false));
    server.once('listening', () => server.close(() => resolve(true)));
    server.listen(port, '127.0.0.1');
  });
}

/** Return the first free port in [base, base + span), or throw. */
export async function findFreePort(base: number, span = 10): Promise<number> {
  for (let port = base; port < base + span; port++) {
    if (await isPortFree(port)) return port;
  }
  throw new Error(`No free port found in [${base}, ${base + span})`);
}

/**
 * Wait until something is accepting connections on 127.0.0.1:`port`. Polls with
 * a connect() probe (never binds, so it cannot race the server for the port).
 * `giveUp` lets the caller abort early (e.g. the server process already died).
 */
export async function waitForPortListening(
  port: number,
  timeoutMs = 15000,
  giveUp?: () => boolean,
): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (giveUp?.()) return false;
    const listening = await new Promise<boolean>((resolve) => {
      const sock = net.connect({ port, host: '127.0.0.1' });
      sock.once('connect', () => {
        sock.destroy();
        resolve(true);
      });
      sock.once('error', () => resolve(false));
    });
    if (listening) return true;
    await new Promise((r) => setTimeout(r, 150));
  }
  return false;
}

/** True if `pid` is alive (owned by us or not). */
export function isPidAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    // ESRCH => dead; EPERM => alive but not ours (still "alive").
    return (err as NodeJS.ErrnoException).code === 'EPERM';
  }
}

/** The executable name of `pid`, or '' if unavailable. */
export function pidComm(pid: number): string {
  if (isDarwin) {
    // macOS `ps -o comm=` prints the path the process was exec'd with (e.g.
    // /sbin/launchd), so it needs a basename to compare like Linux comm does.
    const out = psField(pid, 'comm');
    return out ? path.basename(out) : '';
  }
  try {
    return fs.readFileSync(`/proc/${pid}/comm`, 'utf8').trim();
  } catch {
    return '';
  }
}

/** The process-group id of `pid`, or 0 if unavailable. */
export function pidPgid(pid: number): number {
  if (isDarwin) {
    const pgid = Number.parseInt(psField(pid, 'pgid'), 10);
    return Number.isInteger(pgid) && pgid > 0 ? pgid : 0;
  }
  try {
    const stat = fs.readFileSync(`/proc/${pid}/stat`, 'utf8');
    // The comm field is parenthesised and may itself contain spaces/parens;
    // fields after the LAST ')' are: state ppid pgrp ...
    const after = stat.slice(stat.lastIndexOf(')') + 2);
    const pgrp = Number.parseInt(after.split(' ')[2] ?? '', 10);
    return Number.isInteger(pgrp) && pgrp > 0 ? pgrp : 0;
  } catch {
    return 0;
  }
}

/**
 * Kill each PID listed in `pidFile` whose comm name contains one of
 * `commSubstrings`. Returns the PIDs actually signalled. Best-effort and
 * scoped: a PID whose comm does not match (e.g. reused by an unrelated process)
 * is left alone.
 */
export function reapPidsFromFile(pidFile: string, commSubstrings: string[]): number[] {
  if (!fs.existsSync(pidFile)) return [];
  let pids: number[];
  try {
    pids = fs
      .readFileSync(pidFile, 'utf8')
      .split('\n')
      .map((line) => Number.parseInt(line.trim(), 10))
      .filter((pid) => Number.isInteger(pid) && pid > 0);
  } catch {
    return [];
  }
  const killed: number[] = [];
  for (const pid of pids) {
    const comm = pidComm(pid);
    if (comm && commSubstrings.some((s) => comm.includes(s))) {
      try {
        // A recorded pid that leads its own process group (tauri-driver is
        // spawned detached) gets a GROUP kill, so its unrecorded children
        // (WebKitWebDriver, the app) die with it - killing the leader alone
        // permanently orphans them when the harness process died without
        // running stop(). Non-leaders get a plain kill: kill(-pid) for a
        // non-leader would target whatever unrelated group has pgid == pid.
        process.kill(pidPgid(pid) === pid ? -pid : pid, 'SIGKILL');
        killed.push(pid);
      } catch {
        // Already gone.
      }
    }
  }
  return killed;
}

/** Append a PID to a pidfile (one per line), best-effort. */
export function recordPid(pidFile: string, pid: number): void {
  try {
    fs.appendFileSync(pidFile, `${pid}\n`);
  } catch {
    // Best-effort tracking.
  }
}
