/**
 * #576 W58 Q14: `agent:smoke`'s started line prints the X display only as `:N`
 * (or darwin's `aqua`); any other DISPLAY names a host and prints as a
 * placeholder.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { describe, expect, it } from 'vitest';

import type { StartResult } from '../sessionTypes';
import { shareableDisplay, smokeStartedLine } from './smokeStartLine';

const SMOKE = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'smoke.ts');

const started = (display: string): StartResult => ({
  state: 'ready',
  appPidKnown: false,
  driverPort: 4444,
  nativePort: 4445,
  display,
  profileDir: path.join(os.homedir(), 'profiles', 'p1'),
  buildAction: 'skipped',
  builtFrom: null,
  buildMode: 'debug',
  sessionDir: path.join(os.homedir(), 'sessions', 's1'),
});

describe("agent:smoke's started line (#576 W58 Q14)", () => {
  it('keeps a local display number and the darwin sentinel', () => {
    expect(shareableDisplay(':99')).toBe(':99');
    expect(shareableDisplay(':0')).toBe(':0');
    expect(shareableDisplay('aqua')).toBe('aqua');
  });

  it('prints any other DISPLAY as a placeholder', () => {
    for (const display of ['localhost:10.0', 'cave-lab.example:0', ':0.1', '', 'host/unix:0', ':99\nX']) {
      expect(shareableDisplay(display), JSON.stringify(display)).toBe('<display>');
    }
  });

  it('prints an ssh -X display as <display> and keeps the rest of the StartResult, folded', () => {
    const line = smokeStartedLine(started('cave-lab.example:10.0'));
    expect(line).not.toContain('cave-lab');
    expect(JSON.parse(line)).toMatchObject({
      state: 'ready',
      display: '<display>',
      driverPort: 4444,
      sessionDir: '~/sessions/s1',
      profileDir: '~/profiles/p1',
    });
    expect(JSON.parse(smokeStartedLine(started(':42'))).display).toBe(':42');
  });

  it('is the line agent:smoke prints: the StartResult goes through smokeStartedLine, never raw', () => {
    const source = fs.readFileSync(SMOKE, 'utf8');
    const lines = source.split('\n').filter((line) => line.includes('started: '));
    expect(lines).toEqual(['  log(`started: ${smokeStartedLine(started)}`);']);
    expect(source).not.toMatch(/JSON\.stringify\(started\)/);
  });
});
