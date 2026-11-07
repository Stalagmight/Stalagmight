/**
 * Session directories: how one is claimed, and which ones retention may delete.
 *
 * Every case runs in a throwaway root, never `.agent-artifacts/`, so a test can
 * neither prune nor claim a real session's directory.
 */
import { spawn, spawnSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { defaultTracePath } from './fuzzTrace';
import {
  OWNER_FILE,
  RUN_START_ENV,
  Recorder,
  allocateSessionDir,
  pruneOldSessions,
  runStartMs,
  sessionIdFor,
} from './recorder';

const here = path.dirname(fileURLToPath(import.meta.url));
let root = '';

beforeEach(() => {
  root = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-recorder-test-'));
});

afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

const ownerOf = (dir: string): string => fs.readFileSync(path.join(dir, OWNER_FILE), 'utf8');

describe('session ids (#576 HFI-14)', () => {
  const instant = new Date(2026, 10, 4, 0, 2, 31, 417);

  it('leads with the wall-clock second, so a sorted listing stays chronological', () => {
    const earlier = sessionIdFor(new Date(2026, 10, 4, 0, 2, 30, 999), 99999, 'ffffff');
    const later = sessionIdFor(instant, 1, '000000');
    expect(later.startsWith('20261104-000231-417-')).toBe(true);
    expect([later, earlier].sort()).toEqual([earlier, later]);
  });

  it('gives two processes different ids in the same millisecond, even on the same random tail', () => {
    expect(sessionIdFor(instant, 404520, 'abcdef')).not.toBe(sessionIdFor(instant, 404616, 'abcdef'));
  });

  it('names the root it gave up on with the home directory folded to ~ (#576 W51)', () => {
    const home = process.env.HOME;
    process.env.HOME = root;
    try {
      const sessions = path.join(root, 'qzUser', 'sessions');
      const deps = { now: () => instant, pid: 4242, tail: () => 'aaaaaa' };
      allocateSessionDir(sessions, deps);
      expect(() => allocateSessionDir(sessions, deps)).toThrow(
        new Error('could not allocate a unique session directory under ~/qzUser/sessions in 16 attempts'),
      );
    } finally {
      if (home === undefined) delete process.env.HOME;
      else process.env.HOME = home;
    }
  });

  it('hands two creates in the SAME millisecond two directories, each with its own owner', () => {
    // The worst case the old id could not survive: one clock reading for both
    // creates. Here even the pid and the first random draw are the same, so the
    // second create's first candidate IS the first create's directory - only the
    // exclusive mkdir can tell, and it has to make the second one draw again.
    const draws = ['aaaaaa', 'aaaaaa', 'bbbbbb'];
    const deps = { now: () => instant, pid: 4242, tail: () => draws.shift() ?? 'cccccc' };
    const first = allocateSessionDir(root, deps);
    const second = allocateSessionDir(root, deps);
    expect(second.id).not.toBe(first.id);
    expect(second.dir).not.toBe(first.dir);
    expect(fs.statSync(first.dir).isDirectory()).toBe(true);
    expect(fs.statSync(second.dir).isDirectory()).toBe(true);
    // One pid per marker: the collision wrote '404520404616' into one file.
    expect(ownerOf(first.dir)).toBe('4242');
    expect(ownerOf(second.dir)).toBe('4242');
    expect(draws).toEqual([]);
  });

  it('never shares a directory between two processes creating at once', async () => {
    // Two REAL processes, released by one shared deadline, each claiming a
    // session directory under the same root with the production defaults.
    const tsx = path.resolve(here, '..', '..', '..', 'node_modules', '.bin', 'tsx');
    const recorderModule = path.resolve(here, 'recorder.ts');
    const releaseAt = Date.now() + 1500;
    const script =
      `(async () => {` +
      `const { allocateSessionDir } = await import(${JSON.stringify(recorderModule)});` +
      `while (Date.now() < ${releaseAt}) {}` +
      `const at = Date.now();` +
      `const claimed = allocateSessionDir(${JSON.stringify(root)});` +
      `process.stdout.write(JSON.stringify({ at, pid: process.pid, ...claimed }));` +
      `})();`;
    const run = (): Promise<{ at: number; pid: number; id: string; dir: string }> =>
      new Promise((resolve, reject) => {
        const child = spawn(tsx, ['--eval', script], { stdio: ['ignore', 'pipe', 'pipe'] });
        let out = '';
        let err = '';
        child.stdout.on('data', (chunk: Buffer) => (out += chunk.toString()));
        child.stderr.on('data', (chunk: Buffer) => (err += chunk.toString()));
        child.on('error', reject);
        child.on('close', (code) => {
          if (code !== 0) reject(new Error(`child exited ${code}: ${err}`));
          else resolve(JSON.parse(out) as { at: number; pid: number; id: string; dir: string });
        });
      });
    const [a, b] = await Promise.all([run(), run()]);
    expect(a.id).not.toBe(b.id);
    expect(fs.readdirSync(root).sort()).toEqual([a.id, b.id].sort());
    expect(ownerOf(a.dir)).toBe(String(a.pid));
    expect(ownerOf(b.dir)).toBe(String(b.pid));
  }, 30_000);
});

describe('retention (#576 HFI-6)', () => {
  /** A session allocated a day ago, so every session created after it is newer. */
  const oldSession = (pid: number): string =>
    allocateSessionDir(root, { now: () => new Date(Date.now() - 24 * 3600 * 1000), pid }).dir;

  /** What every harness start does: claim a directory, then construct the recorder (which prunes). */
  const startNewerSessions = (count: number): void => {
    for (let i = 0; i < count; i += 1) new Recorder(allocateSessionDir(root).id, root);
  };

  /** A pid that WAS a process and is not any more. */
  const deadPid = (): number => {
    const done = spawnSync('true');
    expect(done.pid).toBeGreaterThan(0);
    return done.pid as number;
  };

  it('keeps a live sibling session through twenty newer sessions', () => {
    // A real, live process that is not this one: the shape of a sibling harness
    // run that is still going while 20 short cells start and finish.
    const sibling = spawn('sleep', ['60'], { stdio: 'ignore' });
    try {
      expect(sibling.pid).toBeGreaterThan(0);
      const live = oldSession(sibling.pid as number);
      fs.writeFileSync(path.join(live, 'actions.jsonl'), '{"type":"session-start"}\n');
      startNewerSessions(20);
      expect(fs.existsSync(path.join(live, 'actions.jsonl'))).toBe(true);
      expect(fs.readdirSync(root)).toHaveLength(21);
    } finally {
      sibling.kill('SIGKILL');
    }
  });

  it('keeps a finished session whose trace was written during this run', () => {
    const finished = oldSession(deadPid());
    const trace = defaultTracePath(finished, 'probeScenario', 7);
    fs.mkdirSync(path.dirname(trace), { recursive: true });
    fs.writeFileSync(trace, '{}\n');
    startNewerSessions(20);
    expect(fs.existsSync(trace)).toBe(true);
  });

  it('still prunes what nothing needs: a dead owner, an older trace, or an earlier session of this process', () => {
    const dead = oldSession(deadPid());
    const staleTrace = defaultTracePath(dead, 'probeScenario', 1);
    fs.mkdirSync(path.dirname(staleTrace), { recursive: true });
    fs.writeFileSync(staleTrace, '{}\n');
    const before = (runStartMs() - 3600 * 1000) / 1000;
    fs.utimesSync(staleTrace, before, before);
    const ours = oldSession(process.pid);
    startNewerSessions(20);
    expect(fs.existsSync(dead)).toBe(false);
    expect(fs.existsSync(ours)).toBe(false);
    expect(fs.readdirSync(root)).toHaveLength(20);
  });

  it('reports what it deleted and leaves protected directories out of the count', () => {
    const live = oldSession(31337);
    const gone = oldSession(31338);
    for (let i = 0; i < 2; i += 1) allocateSessionDir(root);
    const deleted = pruneOldSessions(root, 2, {
      runStartMs: Date.now(),
      pid: process.pid,
      isAlive: (pid) => pid === 31337,
    });
    expect(deleted).toEqual([path.basename(gone)]);
    expect(fs.existsSync(live)).toBe(true);
  });

  it('takes an inherited run start only when it is earlier and well-formed', () => {
    const own = 1_000_000;
    expect(runStartMs({}, own)).toBe(own);
    expect(runStartMs({ [RUN_START_ENV]: '400000' }, own)).toBe(400_000);
    expect(runStartMs({ [RUN_START_ENV]: '2000000' }, own)).toBe(own);
    for (const junk of ['', ' ', 'soon', '-5', '0', 'Infinity']) {
      expect(runStartMs({ [RUN_START_ENV]: junk }, own)).toBe(own);
    }
  });
});
