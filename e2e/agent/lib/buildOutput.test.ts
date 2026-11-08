/**
 * A session build's output on the runner's stderr (#576 W47-NF3).
 *
 * `agent:scenario`, `agent:journeys` and `agent:smoke` start a session that can
 * build, and `buildApp` ran `pnpm tauri build` with the runner's stderr as the
 * build's own (`stdio: ['ignore', 2, 2]`): cargo's `Compiling <crate> (<path>)`
 * and Tauri's `Built application at: <path>` printed raw beside the runner's
 * shareable lines, naming the checkout under the user's home. `buildApp` now
 * pipes both of the build's streams and passes them on a line at a time with
 * the checkout folded to `<repo>` and the home directory to `~`, as
 * `agent:mcp:smoke` does with the MCP server's stderr (#576 RV19b-P8).
 *
 * `child_process` is stubbed: `execSync` as the real one behaves with fd 2
 * inherited (the build writes straight to this process's stderr), `spawn` as a
 * child whose piped streams carry the same output.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import type { PassThrough } from 'stream';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const REPO = path.resolve(path.dirname(new URL(import.meta.url).pathname), '../../..');
/** A synthetic home directory: the user a leak would name. */
const HOME = '/home/qzdiver';

const build = vi.hoisted(() => ({
  /** What `pnpm tauri build` writes to its stderr, in chunks, then to its stdout. */
  stderr: [] as string[],
  stdout: [] as string[],
  /** Its exit code. */
  exitCode: 0,
  /** How each run of the build command was asked to treat its stdio. */
  stdio: [] as unknown[],
}));

vi.mock('child_process', async (importOriginal) => {
  const real = await importOriginal<typeof import('child_process')>();
  const { EventEmitter: Emitter } = await import('events');
  const { PassThrough: Pass } = await import('stream');
  const isBuild = (command: unknown): boolean => /^(exec )?pnpm tauri build/.test(String(command));
  return {
    ...real,
    // The real execSync with `stdio: ['ignore', 2, 2]`: the build writes to this
    // process's stderr directly, and a non-zero exit throws.
    execSync: vi.fn((command: string, options?: { stdio?: unknown }) => {
      if (!isBuild(command)) return '';
      build.stdio.push(options?.stdio);
      const stdio = Array.isArray(options?.stdio) ? options.stdio : [];
      if (stdio[2] === 2) process.stderr.write(build.stderr.join(''));
      if (stdio[1] === 2) process.stderr.write(build.stdout.join(''));
      if (build.exitCode !== 0) throw new Error(`Command failed: ${command}`);
      return null;
    }),
    // gitHeadFor, for the stamp.
    execFileSync: vi.fn(() => 'abc1234\n'),
    spawn: vi.fn((command: string, options?: { stdio?: unknown }) => {
      const child = new Emitter() as InstanceType<typeof Emitter> & { stdout: PassThrough; stderr: PassThrough };
      child.stdout = new Pass();
      child.stderr = new Pass();
      if (isBuild(command)) build.stdio.push(options?.stdio);
      setImmediate(() => {
        for (const chunk of build.stderr) child.stderr.write(chunk);
        for (const chunk of build.stdout) child.stdout.write(chunk);
        child.stdout.end();
        child.stderr.end();
        setImmediate(() => {
          child.emit('exit', build.exitCode, null);
          child.emit('close', build.exitCode, null);
        });
      });
      return child;
    }),
  };
});

describe("a session build's output reaches the runner's stderr folded (#576 W47-NF3)", () => {
  let targetDir: string;
  let written: string[];

  beforeEach(() => {
    targetDir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-build-output-'));
    // The stamp the build writes lands here, not in the real target dir.
    vi.stubEnv('CARGO_TARGET_DIR', targetDir);
    vi.stubEnv('HOME', HOME);
    vi.resetModules();
    build.stderr = [
      // The home directory split across two chunks: it still folds.
      `   Compiling stalagmight v1.0.0 (${HOME.slice(0, 7)}`,
      `${HOME.slice(7)}/src/x)\n    Finished \`dev\` profile\n`,
    ];
    build.stdout = [`    Built application at: ${REPO}/src-tauri/target/debug/stalagmight\n`];
    build.exitCode = 0;
    build.stdio = [];
    written = [];
    vi.spyOn(process.stderr, 'write').mockImplementation((chunk: string | Uint8Array) => {
      written.push(String(chunk));
      return true;
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllEnvs();
    fs.rmSync(targetDir, { recursive: true, force: true });
  });

  it("folds the checkout and the home directory out of cargo's and Tauri's lines", async () => {
    const { buildApp } = await import('./build');
    await buildApp(undefined, 'debug');
    const out = written.join('');

    expect(out).not.toContain(HOME);
    expect(out).not.toContain(REPO);
    expect(out).toContain('   Compiling stalagmight v1.0.0 (~/src/x)\n');
    expect(out).toContain('    Finished `dev` profile\n');
    expect(out).toContain('    Built application at: <repo>/src-tauri/target/debug/stalagmight\n');
    // Neither stream is inherited; stdout least of all, which is the MCP
    // server's JSON-RPC channel.
    expect(build.stdio).toEqual([['ignore', 'pipe', 'pipe']]);
  });

  it('still fails when the build fails', async () => {
    build.exitCode = 101;
    const { buildApp } = await import('./build');
    // Through a promise either way, so a build that throws synchronously reads the same.
    await expect(Promise.resolve().then(() => buildApp(undefined, 'debug'))).rejects.toThrow(
      /^Command failed: pnpm tauri build/,
    );
    expect(fs.existsSync(path.join(targetDir, 'debug', '.stalagmight-e2e-build-stamp.json'))).toBe(false);
  });
});
