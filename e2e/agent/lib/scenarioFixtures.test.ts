import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { describe, expect, it } from 'vitest';

import { DEFAULT_FIXTURE, fixtureSourcePath, materializeFixture } from './fixtures';

describe('agent fixture resolution', () => {
  it('copies the default fixture outside the tracked fixture directory', () => {
    const destinationDir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-fixture-test-'));
    const copied = materializeFixture(DEFAULT_FIXTURE, destinationDir);

    expect(path.basename(copied)).toBe(DEFAULT_FIXTURE);
    expect(fs.existsSync(copied)).toBe(true);
    expect(copied).not.toBe(fixtureSourcePath(DEFAULT_FIXTURE));
    expect(fs.readFileSync(copied)).toEqual(fs.readFileSync(fixtureSourcePath(DEFAULT_FIXTURE)));
  });

  it('confines caller-supplied fixture names to the fixture directory', () => {
    expect(fixtureSourcePath('../../outside.stalx')).toBe(fixtureSourcePath('outside.stalx'));
  });

  it('names a missing fixture on a later line than the one a runner prints (#576 W51)', () => {
    const destinationDir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-fixture-missing-'));
    expect(() => materializeFixture('qzCaveA.stalx', destinationDir)).toThrow(
      new Error('fixture missing from e2e/fixtures/\n  name: qzCaveA.stalx'),
    );
  });

  it('refreshes a reused session fixture from the tracked source', () => {
    const destinationDir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-fixture-refresh-'));
    const copied = materializeFixture(DEFAULT_FIXTURE, destinationDir);
    fs.writeFileSync(copied, 'mutated by the first seed');

    materializeFixture(DEFAULT_FIXTURE, destinationDir);

    expect(fs.readFileSync(copied)).toEqual(fs.readFileSync(fixtureSourcePath(DEFAULT_FIXTURE)));
  });
});
