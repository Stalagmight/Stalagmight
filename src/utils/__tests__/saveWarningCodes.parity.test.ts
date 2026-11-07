/**
 * Every code the backend stamps on `SaveOutcome.warnings` has words in the
 * frontend (#576 W30-4).
 *
 * The codes are the `*_CODE` consts in save_sidecars.rs, read from source - the
 * repo's source-scan idiom (coordinates.test.ts reads FRONTEND_PARITY from geo.rs
 * the same way) - and `surfaceSaveWarnings` turns each into a toast through
 * `SAVE_WARNING_TEXT`. A code with no entry there still reaches the user, but
 * only as the backend's raw English behind "Saved, with a warning:", so a new
 * code added in Rust alone shows untranslated text. The scan fails loudly when it
 * finds no codes, so renaming the consts cannot turn this into a vacuous pass;
 * and the save path may not stamp a bare string literal, which this scan could
 * not see.
 */
import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import i18n from '../../i18n';
import { SAVE_WARNING_TEXT } from '../saveFeedback';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../');
const STATE_DIR = 'src-tauri/crates/core/src/core/state';
const CODES_FILE = `${STATE_DIR}/save_sidecars.rs`;
/** The files that put warnings on a save's `CommandResult`. */
const SAVE_PATH_FILES = [CODES_FILE, `${STATE_DIR}/project_io.rs`];

const read = (rel: string): string => fs.readFileSync(path.join(REPO_ROOT, rel), 'utf8');

/** The values of the `*_CODE` string consts in save_sidecars.rs, or a thrown error. */
function backendSaveWarningCodes(): string[] {
  const anchor = /^(?:pub(?:\([a-z]+\))? )?const [A-Z][A-Z0-9_]*_CODE: &str = "([^"]*)";$/gm;
  const codes = [...read(CODES_FILE).matchAll(anchor)].map((m) => m[1] as string);
  if (codes.length === 0) {
    throw new Error(
      `${CODES_FILE}: no \`const *_CODE: &str\` save-warning codes found - ` +
        'renamed or moved? Update this parity test with them.',
    );
  }
  return codes;
}

describe('save-warning codes: backend consts vs SAVE_WARNING_TEXT (#576 W30-4)', () => {
  const codes = backendSaveWarningCodes();

  it('finds the save-warning codes in save_sidecars.rs', () => {
    expect(codes.length).toBeGreaterThan(0);
    expect(new Set(codes).size, 'a code is declared twice').toBe(codes.length);
  });

  it.each(codes)('%s has an entry in SAVE_WARNING_TEXT', (code) => {
    expect(
      Object.keys(SAVE_WARNING_TEXT),
      `${code} (from ${CODES_FILE}) has no words in SAVE_WARNING_TEXT (src/utils/saveFeedback.ts)`,
    ).toContain(code);
  });

  it.each(Object.entries(SAVE_WARNING_TEXT))('%s -> %s is real English text', (_code, key) => {
    const english: unknown = i18n.getResource('en', 'translation', key);
    expect(typeof english, `${key} is missing from en.json`).toBe('string');
    expect((english as string).trim()).not.toBe('');
    expect(i18n.t(key)).toBe(english);
  });

  it('SAVE_WARNING_TEXT names no code the backend does not declare', () => {
    for (const code of Object.keys(SAVE_WARNING_TEXT)) {
      expect(codes, `${code} is not a *_CODE const in ${CODES_FILE}`).toContain(code);
    }
  });

  it.each(SAVE_PATH_FILES)('%s stamps no save warning with a bare string literal', (rel) => {
    const literals = [...read(rel).matchAll(/\bcode: "([^"]*)"/g)].map((m) => m[1]);
    expect(literals, `${rel}: make each a *_CODE const in ${CODES_FILE}`).toEqual([]);
  });
});
