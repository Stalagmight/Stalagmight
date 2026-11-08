/**
 * A fuzz trace step's label as `fuzz-shrink` prints it (#576 W58 Q10).
 *
 * A trace is attached to issues and shrunk from wherever someone got it, and
 * `fuzz-shrink` prints the minimal trace's steps with their labels. `readTrace`
 * checks a label only as text, so a label is printed only when it is made of
 * what the scenarios write a label from:
 *
 * - the words of the harness's own sources under `e2e/agent` (tests excluded):
 *   the literal parts of every label template, the generators' tables
 *   (corruptions, strategies, field names, the hostile-text corpus the typed
 *   values are drawn from) and the fuzzers' allowlisted test ids;
 * - the words of the app's production source (`codeWords`, as a test id is
 *   printed): the test ids, column keys and enum values the app defines;
 * - numbers, and the punctuation between them, and a word that is one letter
 *   repeated (the hostile-text corpus's `'x'.repeat(512)`), which says only
 *   its length.
 *
 * A word is a letter followed by letters and digits, in any script, so a name in
 * any alphabet is one word. Fail-closed: a label with a word neither source
 * wrote - a forged trace's, or a future label that quotes the app's text - or
 * with a control character prints as its length (`valueShape`).
 */
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

import { codeWords } from '../../../scripts/lib/shareable-selector.mjs';
import { valueShape } from './shareableFailure';

/** `e2e/agent`, whose sources write every label. */
const AGENT_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

/** A word: a letter, then letters and digits, in any script. */
const WORD = /\p{L}[\p{L}\p{N}]*/gu;

/** A word that is one letter repeated: it carries nothing but its length. */
const ONE_LETTER = /^(\p{L})\1*$/u;

/** The longest label printed: the longest a scenario writes is under half of it. */
const MAX_LABEL = 400;

let cachedHarnessWords: Set<string> | null = null;

/**
 * Every word of the harness's non-test sources under `e2e/agent`. Read once; a
 * tree that cannot be read gives no words, so every label prints as its length.
 */
export function harnessWords(): Set<string> {
  if (cachedHarnessWords) return cachedHarnessWords;
  const words = new Set<string>();
  const visit = (dir: string): void => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        if (entry.name !== 'node_modules') visit(full);
      } else if (/\.(?:ts|mts|mjs)$/.test(entry.name) && !/\.test\.\w+$/.test(entry.name)) {
        for (const match of fs.readFileSync(full, 'utf8').matchAll(WORD)) words.add(match[0]);
      }
    }
  };
  try {
    visit(AGENT_ROOT);
  } catch {
    words.clear();
  }
  cachedHarnessWords = words;
  return words;
}

/** Is `label` made only of the words, numbers and punctuation the scenarios write a label from? */
export function stepLabelShaped(
  label: unknown,
  harness: ReadonlySet<string> = harnessWords(),
  app: ReadonlySet<string> = codeWords(),
): boolean {
  if (typeof label !== 'string' || label === '' || label.length > MAX_LABEL) return false;
  if (/\p{Cc}/u.test(label)) return false;
  for (const match of label.matchAll(WORD)) {
    const word = match[0];
    if (!harness.has(word) && !app.has(word) && !ONE_LETTER.test(word)) return false;
  }
  return true;
}

/** A step's label as `describeSteps` prints it: itself when `stepLabelShaped`, else `valueShape`. */
export function shareableStepLabel(label: unknown): string {
  return stepLabelShaped(label) ? (label as string) : valueShape(label);
}
