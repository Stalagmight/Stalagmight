/**
 * #576 W58 Q3: a runner prints an error's first line only when the harness
 * wrote it. A first line webdriverio, Node or the engine wrote - which can
 * quote a selector, a path or the text it failed on - prints as its length.
 *
 * Synthetic names only (`qzCaveA`); the "outside" module is written to a temp
 * directory at run time, as a dependency's code would sit outside the harness.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'url';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { describeRunError } from './fuzzHarness';
import { foldLocalPaths, scenarioFailOutput, shareableErrorText, shareableFailureText, shareableTail } from './shareableFailure';

const NAME = 'qzCaveA';
const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');

/** A real Node error: ENOENT, its message naming the path. */
function nodeError(): Error {
  try {
    fs.readFileSync(path.join(os.tmpdir(), `qz-nowhere-${process.pid}`, `${NAME}.stalx`));
  } catch (err) {
    return err as Error;
  }
  throw new Error('expected ENOENT');
}

/** A real engine error: JSON.parse quotes the text it could not parse. */
function syntaxError(): Error {
  try {
    JSON.parse(`{"${NAME}": }`);
  } catch (err) {
    return err as Error;
  }
  throw new Error('expected a SyntaxError');
}

let outsideDir = '';
let outside: (message: string) => Error = () => new Error('not loaded');

beforeAll(() => {
  outsideDir = fs.mkdtempSync(path.join(os.tmpdir(), 'qz-third-party-'));
  const file = path.join(outsideDir, 'dep.cjs');
  fs.writeFileSync(file, 'exports.fail = (message) => new Error(message);\n');
  // Node's own loader: the module runs from where it sits, outside the checkout.
  outside = (createRequire(import.meta.url)(file) as { fail: (message: string) => Error }).fail;
});
afterAll(() => {
  if (outsideDir) fs.rmSync(outsideDir, { recursive: true, force: true });
});

/** An error whose stack webdriverio's `waitUntil` spliced: its own frames dropped, a `---` line between two stacks. */
function splicedError(): Error {
  const err = new Error(`waitUntil condition failed with the following reason: element ("[data-testid=${NAME}]") still not displayed`);
  err.stack = [
    `Error: ${err.message}`,
    `    at Object.run (${path.join(REPO, 'e2e', 'agent', 'scenarios', 'qz.ts')}:12:5)`,
    '    ---',
    `    at Timer.listOnTimeout (node:internal/timers:588:17)`,
  ].join('\n');
  return err;
}

describe("a third-party error's first line prints as its length (#576 W58 Q3)", () => {
  it("withholds Node's, the engine's, a dependency's, a spliced and a frameless error's first line on a FAIL block", () => {
    const frameless = new Error(`element ("[data-testid=${NAME}]") still not displayed after 5000ms`);
    frameless.stack = `Error: ${frameless.message}`;
    for (const err of [nodeError(), syntaxError(), outside(`no such element: ${NAME}`), splicedError(), frameless]) {
      const out = scenarioFailOutput('undo-redo-fuzz', err);
      expect(out, err.message).not.toContain(NAME);
      expect(out.split('\n')[1], err.message).toMatch(/^a third-party \w+: \(\d+ characters withheld\)$/);
    }
    // The count is of the line as the path folds left it (`<tmp>` for the temp dir).
    expect(scenarioFailOutput('x', nodeError()).split('\n')[1]).toBe(
      `a third-party Error: (${foldLocalPaths(nodeError().message).length} characters withheld)`,
    );
    expect(scenarioFailOutput('x', syntaxError()).split('\n')[1]).toMatch(/^a third-party SyntaxError: /);
  });

  it("keeps a first line the harness built, and WebDriver's javascript error prefix", () => {
    expect(scenarioFailOutput('x', new Error(`E2 INTEGRITY: ${NAME}.stalx opened SAVE-BLOCKED`)).split('\n')[1]).toBe(
      `E2 INTEGRITY: ${NAME}.stalx opened SAVE-BLOCKED`,
    );
    const page = new Error(`javascript error: ${NAME} is not defined`);
    page.stack = `Error: ${page.message}`;
    expect(scenarioFailOutput('x', page).split('\n')[1]).toBe("javascript error: (22 characters of the app's text withheld)");
  });

  it('withholds it in a crash line too, keeping the frames, and projecting twice changes nothing', () => {
    const err = nodeError();
    const crash = shareableErrorText(err);
    expect(crash).not.toContain(NAME);
    expect(crash.split('\n')[0]).toBe(`a third-party Error: (${foldLocalPaths(err.message).length} characters withheld)`);
    expect(crash.split('\n').slice(1).every((line) => /^\s+at |^ {2}\(\d+ line/.test(line))).toBe(true);
    expect(shareableTail([`fatal: ${crash.split('\n')[0]}`])).toBe(`fatal: ${crash.split('\n')[0]}`);
    expect(shareableFailureText(crash)).toBe(shareableFailureText(shareableFailureText(crash)));
  });

  it("keeps the full line in the failure a fuzz run raises, and withholds it in the shareable form", () => {
    const report = describeRunError(nodeError(), 'click qz-button');
    expect(report.stall).toBe(false);
    expect(report.headline).toContain(NAME);
    expect(shareableFailureText(report.headline)).toMatch(/^a third-party Error: \(\d+ characters withheld\)$/);
    expect(describeRunError(new Error('E2 INTEGRITY: x opened SAVE-BLOCKED'), null).headline).toBe(
      'E2 INTEGRITY: x opened SAVE-BLOCKED',
    );
  });
});
