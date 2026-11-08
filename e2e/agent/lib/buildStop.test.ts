/**
 * A build that is still running when the MCP server shuts down is stopped, not
 * orphaned (#576 RV20-16).
 *
 * `runBuildCommand` spawns `pnpm tauri build` with piped streams; the server's
 * shutdown (SIGINT, SIGTERM, stdin closed) ends in `process.exit(0)`, which
 * leaves a child running. `stopRunningBuilds` signals every build still
 * running, so the shutdown can call it first. A real child here: a stub could
 * not show the process is gone.
 */
import { describe, expect, it } from 'vitest';

import { runBuildCommand, stopRunningBuilds } from './build';

const alive = (pid: number): boolean => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

describe('a running build is stopped on shutdown (#576 RV20-16)', () => {
  it('signals a build still running, which then fails as a build does', async () => {
    // A stand-in for the build: a shell that runs a node that never exits.
    const build = runBuildCommand(`"${process.execPath}" -e "setInterval(() => {}, 1000)"`, {
      cwd: process.cwd(),
      env: process.env,
    });
    const settled = build.then(
      () => 'resolved',
      (err: unknown) => (err instanceof Error ? err.message : String(err)),
    );
    await new Promise((r) => setTimeout(r, 300));
    const pids = stopRunningBuilds();
    expect(pids, 'the running build was not signalled').toHaveLength(1);
    const outcome = await Promise.race([settled, new Promise((r) => setTimeout(() => r('still running after 5 s'), 5000))]);
    expect(outcome).toMatch(/^Command failed: /);
    expect(pids.filter(alive), 'a build process outlived the stop').toEqual([]);
    expect(stopRunningBuilds(), 'a finished build is still listed').toEqual([]);
  });

  it('lists no build once one has finished', async () => {
    await runBuildCommand(`"${process.execPath}" -e "0"`, { cwd: process.cwd(), env: process.env });
    expect(stopRunningBuilds()).toEqual([]);
  });
});
