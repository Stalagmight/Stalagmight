/**
 * #576 RV20-13: `ipc-payload-probe.ts` reads a call by its rejection's class,
 * prints that class and a length, and keeps the whole text only in its private
 * log, only when the developer opts in. Synthetic messages: `qz...` stands in
 * for app text.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { probeVerdict, shownProbeError } from './ipcPayloadProbe';
import { appendProbeError, PROBE_ERRORS_FILE } from './privateErrors';

afterEach(() => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

const MISSING = 'invalid args `config` for command `import_sections`: missing field `qzname`';
const TRUNCATED = 'invalid args `config` for command `import_sections`: premature end of input';

describe('ipc-payload-probe reads a call by the class of its rejection (#576 RV20-13)', () => {
  it("reads an error that is not a known rejection as neither arrived nor truncated", () => {
    expect(probeVerdict(null)).toBe('arrived');
    expect(probeVerdict(MISSING)).toBe('arrived');
    expect(probeVerdict(TRUNCATED)).toBe('truncated');
    // A page script's own failure and a command Tauri does not know say nothing about the body.
    expect(probeVerdict('invoke unavailable')).toBe('other');
    expect(probeVerdict('Command import_sections not found: qzCaveA')).toBe('other');
  });

  it('prints the class and the length, never the text', () => {
    expect(shownProbeError(null)).toBe('(no error)');
    expect(shownProbeError(MISSING)).toBe(`missing field (${MISSING.length} characters of the app's text withheld)`);
    const other = 'Command import_sections not found: qzCaveA';
    expect(shownProbeError(other)).toBe(`not a known rejection (${other.length} characters of the app's text withheld)`);
  });

  it('writes the whole error to the private log only with STALAGMIGHT_AGENT_PRIVATE_ERRORS=1, and nothing to stdout or stderr', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'rv20-13-probe-'));
    const stdout = vi.spyOn(process.stdout, 'write');
    const stderr = vi.spyOn(process.stderr, 'write');
    try {
      const log = path.join(dir, PROBE_ERRORS_FILE);
      vi.stubEnv('STALAGMIGHT_AGENT_PRIVATE_ERRORS', '');
      appendProbeError(dir, 'baseline', MISSING);
      expect(fs.existsSync(log)).toBe(false);
      vi.stubEnv('STALAGMIGHT_AGENT_PRIVATE_ERRORS', '1');
      appendProbeError(dir, 'baseline', MISSING);
      appendProbeError(dir, 'ceiling', null);
      expect(fs.readFileSync(log, 'utf8')).toBe(`${JSON.stringify({ call: 'baseline', error: MISSING })}\n`);
      expect(stdout).not.toHaveBeenCalled();
      expect(stderr).not.toHaveBeenCalled();
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
