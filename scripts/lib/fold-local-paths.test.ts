/**
 * #576 W58: scripts/lib/fold-local-paths.mjs is the one foldLocalPaths. The
 * harness re-exports it (e2e/agent/lib/shareableFailure.ts) and the repo's Node
 * tools under scripts/ import it, so a path folds the same way whichever prints
 * it. Synthetic roots only (`qz...`).
 */
import * as path from 'path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { foldLocalPaths as harnessFold } from '../../e2e/agent/lib/shareableFailure';
import { foldLocalPaths } from './fold-local-paths.mjs';

const REPO = path.resolve(__dirname, '..', '..');
const ENV = ['HOME', 'CARGO_TARGET_DIR', 'TMPDIR'] as const;

describe('foldLocalPaths (scripts/lib/fold-local-paths.mjs)', () => {
  let saved: Partial<Record<(typeof ENV)[number], string>>;
  beforeEach(() => {
    saved = Object.fromEntries(ENV.map((key) => [key, process.env[key]]));
    process.env.HOME = '/qzhome/qzbob';
    process.env.CARGO_TARGET_DIR = '/qzbuild/target';
    process.env.TMPDIR = '/qzscratch/t';
  });
  afterEach(() => {
    for (const key of ENV) {
      const value = saved[key];
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  });

  it('is the function the harness folds with: one implementation', () => {
    expect(harnessFold).toBe(foldLocalPaths);
  });

  it('finds the checkout from scripts/lib/', () => {
    expect(foldLocalPaths(`wrote ${REPO}/.agent-artifacts/sessions/s1/report.md`)).toBe(
      'wrote <repo>/.agent-artifacts/sessions/s1/report.md',
    );
  });

  it('folds the home dir to ~, CARGO_TARGET_DIR to <target> and the temp dir to <tmp>', () => {
    expect(foldLocalPaths('/qzhome/qzbob/s1 /qzbuild/target/debug/app /qzscratch/t/x.json')).toBe(
      '~/s1 <target>/debug/app <tmp>/x.json',
    );
  });

  it('folds the longest root first, never inside a longer name, and folding twice changes nothing', () => {
    process.env.CARGO_TARGET_DIR = '/qzhome/qzbob/.cache/target';
    const once = foldLocalPaths('/qzhome/qzbob/.cache/target/debug /qzhome/qzbobby/x /qzscratch/tt/y');
    expect(once).toBe('<target>/debug /qzhome/qzbobby/x /qzscratch/tt/y');
    expect(foldLocalPaths(once)).toBe(once);
  });

  it('folds nothing for a root of / or a relative CARGO_TARGET_DIR', () => {
    process.env.HOME = '/';
    process.env.TMPDIR = '/';
    process.env.CARGO_TARGET_DIR = 'target';
    expect(foldLocalPaths('/qzhome/qzbob/a target/debug')).toBe('/qzhome/qzbob/a target/debug');
  });
});
