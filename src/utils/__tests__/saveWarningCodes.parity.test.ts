/**
 * Every code the backend stamps on `SaveOutcome.warnings` has words in the
 * frontend (#576 W30-4).
 *
 * The codes are the `*_CODE` consts of the save path (save_sidecars.rs, and
 * project_io.rs beside it - #576 RV19a-16 - and the push's pre-push save in
 * speleodb_push.rs - #576 RV20-9), read from source - the
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
/**
 * The files that put warnings on a save's `CommandResult`, and speleodb_push.rs,
 * whose pre-push save stamps them on the push answer's `save_warnings`
 * (#576 RV20-9; the last describe block below keeps this list complete).
 */
const SAVE_PATH_FILES = [CODES_FILE, `${STATE_DIR}/project_io.rs`, `${STATE_DIR}/speleodb_push.rs`];

const read = (rel: string): string => fs.readFileSync(path.join(REPO_ROOT, rel), 'utf8');

/** A `const *_CODE: &str = "...";` declaration line (any visibility, any indent). */
const CODE_CONST = /^\s*(?:pub(?:\([a-z]+\))? )?const [A-Z][A-Z0-9_]*_CODE: &str = "([^"]*)";\s*$/;

/**
 * The values of the `*_CODE` string consts in the save-path files, or a thrown
 * error. Read from every file in SAVE_PATH_FILES, not only save_sidecars.rs, so
 * a code declared beside the save in project_io.rs is held to the same parity
 * (#576 RV19a-16).
 */
function backendSaveWarningCodes(): string[] {
  const codes = SAVE_PATH_FILES.flatMap((rel) =>
    read(rel)
      .split('\n')
      .map((line) => CODE_CONST.exec(line)?.[1])
      .filter((value): value is string => value !== undefined),
  );
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
    const literals = bareCodeLiterals(read(rel));
    expect(literals, `${rel}: make each a *_CODE const in ${CODES_FILE}`).toEqual([]);
  });
});

/**
 * String literals in Rust `source` that could be a save-warning code stamped
 * bare (#576 RV19a-16): any literal assigned to a field whose name ends in
 * `code` (`code: "X"`, `warning_code: "X"`), and any SCREAMING_SNAKE literal
 * anywhere else - a helper argument such as `lost_warning("X")` - except the
 * value of a `const *_CODE` declaration. Comments are skipped, so prose that
 * quotes a code does not count.
 */
function bareCodeLiterals(source: string): string[] {
  const code = productionSource(source)
    .split('\n')
    .filter((line) => !CODE_CONST.test(line))
    .join('\n');
  return literalsIn(code);
}

/**
 * Rust `source` with its comments and its inline `#[cfg(test)] mod x { ... }`
 * blocks removed, so prose and test data do not count (#576 RV20-9). A comment
 * starts only outside a string literal: `"https://x"` is a string, not the start
 * of a `//` comment. Handles escapes, raw strings (`r#"..."#`) and the `'"'`
 * char literal; a test module is a top-level `#[cfg(test)]` item that is a
 * `mod <name> {`, which rustfmt closes with a `}` in column 0.
 */
function productionSource(source: string): string {
  return stripComments(withoutTestModules(source));
}

function withoutTestModules(source: string): string {
  const lines = source.split('\n');
  const kept: string[] = [];
  for (let i = 0; i < lines.length; i++) {
    if (lines[i] === '#[cfg(test)]') {
      let j = i + 1;
      // Skip the module's other attributes and doc lines (`#[expect(` ... `)]`).
      while (j < lines.length && /^(?:#\[|\s|\)\]|\/\/)/.test(lines[j] as string)) j++;
      if (/^(?:pub(?:\([^)]*\))? )?mod \w+ \{\s*$/.test(lines[j] ?? '')) {
        while (j < lines.length && lines[j] !== '}') j++;
        i = j;
        continue;
      }
    }
    kept.push(lines[i] as string);
  }
  return kept.join('\n');
}

function stripComments(source: string): string {
  let out = '';
  let i = 0;
  while (i < source.length) {
    const ch = source[i];
    const next = source[i + 1];
    if (ch === '/' && next === '/') {
      while (i < source.length && source[i] !== '\n') i++;
    } else if (ch === '/' && next === '*') {
      const end = source.indexOf('*/', i + 2);
      i = end < 0 ? source.length : end + 2;
    } else if (ch === "'" && next === '"' && source[i + 2] === "'") {
      out += source.slice(i, i + 3);
      i += 3;
    } else if (ch === 'r' && /^r#*"/.test(source.slice(i)) && !/\w/.test(source[i - 1] ?? '')) {
      const hashes = (/^r(#*)"/.exec(source.slice(i)) as RegExpExecArray)[1] as string;
      const end = source.indexOf(`"${hashes}`, i + hashes.length + 2);
      const stop = end < 0 ? source.length : end + hashes.length + 1;
      out += source.slice(i, stop);
      i = stop;
    } else if (ch === '"') {
      let j = i + 1;
      while (j < source.length && source[j] !== '"') j += source[j] === '\\' ? 2 : 1;
      out += source.slice(i, j + 1);
      i = j + 1;
    } else {
      out += ch;
      i++;
    }
  }
  return out;
}

function literalsIn(code: string): string[] {
  const fields = [...code.matchAll(/\b\w*code\s*:\s*"([^"]*)"/g)].map((m) => m[1] as string);
  const screaming = [...code.matchAll(/"([A-Z][A-Z0-9_]*)"/g)].map((m) => m[1] as string);
  return [...new Set([...fields, ...screaming])];
}

describe('the bare-literal scan sees every shape a code is stamped in (#576 RV19a-16)', () => {
  it.each([
    ['a `code` field', 'code: "NEW_CODE".to_owned(),', ['NEW_CODE']],
    ['a `*_code` field', 'WarnAndContinue { warning_code: "NEW_CODE" }', ['NEW_CODE']],
    ['a helper argument', 'warnings.push(self.lost_warning("NEW_CODE"));', ['NEW_CODE']],
    ['a later helper argument', 'self.warn_lost(sidecar, "NEW_CODE");', ['NEW_CODE']],
    ['a declared const', 'pub(super) const NEW_CODE: &str = "NEW_CODE";', []],
    ['a const reference', 'warning_code: NEW_CODE,', []],
    ['a comment', '// not the generic "SYSTEM" code', []],
    ['a doc comment', '/// stamps `code: "OLD"` (gone)', []],
    // #576 RV20-9: a `//` inside a string is not a comment, so the code after it counts.
    ['a code after a URL in a string', 'let u = "https://x"; push(lost_warning("NEW_CODE"));', ['NEW_CODE']],
    ['a code after an escaped quote', 'let s = "a \\" // b"; push(lost_warning("NEW_CODE"));', ['NEW_CODE']],
    ['a code after a block comment', '/* "OLD" */ push(lost_warning("NEW_CODE"));', ['NEW_CODE']],
    ['a code in an inline test module', '#[cfg(test)]\nmod tests {\n    const X: &str = "NEW_CODE";\n}', []],
    ['a code after an inline test module', '#[cfg(test)]\nmod tests {\n}\nfn f() { lost_warning("NEW_CODE"); }', ['NEW_CODE']],
  ])('%s', (_shape, source, expected) => {
    expect(bareCodeLiterals(source)).toEqual(expected);
  });
});

/**
 * #576 RV20-9: every core state file that stamps a save-warning code is one the
 * parity scans read. A file calls `lost_warning` / `warn_lost` (save_sidecars.rs'
 * stampers) or names a `save_sidecars::*_CODE` const in its production code;
 * speleodb_push.rs does both for the push's own save (useSpeleoActions shows its
 * `save_warnings` through `surfaceSaveWarnings`), and a file missing here would
 * let a bare literal there reach the user untranslated.
 */
describe('the save-path list names every file that stamps a save warning (#576 RV20-9)', () => {
  const STAMPS = /\b(?:lost_warning|warn_lost)\s*\(|\bsave_sidecars::[A-Z][A-Z0-9_]*_CODE\b/;
  const stateFiles = fs
    .readdirSync(path.join(REPO_ROOT, STATE_DIR), { recursive: true, encoding: 'utf8' })
    .filter((rel) => rel.endsWith('.rs') && !rel.endsWith('_tests.rs') && !rel.split(path.sep).includes('tests'))
    .map((rel) => `${STATE_DIR}/${rel.split(path.sep).join('/')}`);

  it('finds the stamping files: a scan that saw none would pass vacuously', () => {
    expect(stateFiles.filter((rel) => STAMPS.test(productionSource(read(rel)))).length).toBeGreaterThan(1);
  });

  it.each(stateFiles)('%s is in SAVE_PATH_FILES if it stamps a save warning', (rel) => {
    if (STAMPS.test(productionSource(read(rel)))) {
      expect(SAVE_PATH_FILES, `${rel} stamps a save warning: add it to SAVE_PATH_FILES`).toContain(rel);
    }
  });
});
