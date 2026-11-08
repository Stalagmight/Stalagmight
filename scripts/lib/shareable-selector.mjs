/**
 * A step's test id or selector as a shareable report prints it (#576 RV19b-P1).
 *
 * report.md is pasted into public issues, which keep codes, counts and code
 * locations (docs/agent-harness.md). A test id or a selector names an element
 * by what the app's code defines - a test id's words, a tag, a class, an
 * attribute - but it can also name it by the user's data: a test id built from
 * a value (`system-card-<encoded file path>`), the element's text
 * (`span=<section name>`, `//td[text()='<station>']`), or an attribute that
 * holds text (`[aria-label="<section name>"]`). So each keeps its structure and
 * the words the app's source uses, and any other text is printed as its length:
 *
 * - A test id, and every other value a selector compares or names (an `#id`,
 *   an `id=` value, an attribute's value), splits at `-` and `_` into words. A
 *   word is kept when it is a number or a word of the app's source under `src/`
 *   (`codeWords`); the span from the first other word to the last one is
 *   printed as its length (`publish-to-speleodb-dialog-collab-<17 characters>`).
 * - Text a selector matches - WebdriverIO's `=text`, `*=text`, `tag=text`,
 *   `tag*=text`, `aria/label` and `~label`, an xpath string that is not an
 *   attribute's value, and the value of an attribute that holds text
 *   (`TEXT_ATTRIBUTES`) - is printed as its length whatever its words.
 * - The rest - tags, classes, combinators, xpath axes and functions - is
 *   structure: its punctuation stays, and its words are kept as a test id's are.
 *
 * Fail-closed: a word nobody wrote in `src/` is counted, so a name the app shows
 * is counted wherever a selector or a test id puts it.
 */
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

import { isTestPath, walkFiles } from './src-files.mjs';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');

const cachedWords = new Map();

/**
 * Every word (`[A-Za-z][A-Za-z0-9]*`) of the app's production source under
 * `<repoRoot>/src` (tests excluded, generated IPC types included: their enum
 * values reach test ids). Read once per root; a tree that cannot be read gives
 * no words, so every word is counted.
 */
export function codeWords(repoRoot = REPO_ROOT) {
  const known = cachedWords.get(repoRoot);
  if (known) return known;
  const words = new Set();
  try {
    const { files } = walkFiles(repoRoot, 'src', { exts: ['.ts', '.tsx'], skip: isTestPath });
    for (const rel of files) {
      for (const match of fs.readFileSync(path.join(repoRoot, rel), 'utf8').matchAll(/[A-Za-z][A-Za-z0-9]*/g)) {
        words.add(match[0]);
      }
    }
  } catch {
    // No source to read: no word is known, so every word is counted.
  }
  cachedWords.set(repoRoot, words);
  return words;
}

/** Attributes whose value is text a user can read (or a URL): always counted. */
const TEXT_ATTRIBUTES = new Set([
  'alt', 'aria-description', 'aria-label', 'aria-placeholder', 'aria-roledescription', 'aria-valuetext',
  'content', 'download', 'href', 'label', 'placeholder', 'src', 'srcdoc', 'title', 'value',
]);

const count = (text) => ({ n: String(text).length });
const kept = (text) => ({ t: text });

/**
 * A lowercase source word of three letters or more with a counter glued on
 * (`stage3` from `gas-readout-stage${i + 1}-reset`, #576 RV20-14). A short or
 * capitalised stem stays counted: `A1` or `st4` reads like a station name.
 */
const COUNTED_WORD = /^([a-z]{3,})\d+$/;

/**
 * A value's parts: the words that are numbers, `words`, or a `COUNTED_WORD` of
 * one of `words` stay, and the span from the first other word to the last is its
 * length. An empty word (two separators in a row) is only the separators.
 */
export function valueParts(value, words = codeWords()) {
  const text = String(value);
  const pieces = text.split(/([-_])/);
  const isKnown = (word) =>
    word === '' ||
    /^\d+$/.test(word) ||
    (/^[A-Za-z][A-Za-z0-9]*$/.test(word) && words.has(word)) ||
    words.has(COUNTED_WORD.exec(word)?.[1] ?? '');
  let first = -1;
  let last = -1;
  for (let i = 0; i < pieces.length; i += 2) {
    if (isKnown(pieces[i])) continue;
    if (first < 0) first = i;
    last = i;
  }
  if (first < 0) return [kept(text)];
  return [
    kept(pieces.slice(0, first).join('')),
    count(pieces.slice(first, last + 1).join('')),
    kept(pieces.slice(last + 1).join('')),
  ];
}

const TEXT_STRATEGIES = ['=', '*=', 'aria/', '~', 'android=', 'ios='];
// WebdriverIO's own detection (webdriverio `findStrategy`): `<strategy>:<value>`,
// and the "element with certain text" form `tag.class[attr="v"]*=text`.
const DIRECT = /^(id|css selector|xpath|link text|partial link text|name|tag name|class name|-android uiautomator|-android datamatcher|-android viewmatcher|-android viewtag|-ios uiautomation|-ios predicate string|-ios class chain|accessibility id):([\s\S]+)$/;
const ELEMENT_WITH_TEXT = /^([a-z0-9|-]*)(?:(\.|#)(-?[_a-zA-Z]+[_a-zA-Z0-9-]*))?(?:\[(-?[_a-zA-Z]+[_a-zA-Z0-9-]*)(?:=("|')([a-zA-Z0-9\-_. ]+)("|'))?\])?(\.)?(\*)?=([\s\S]+)$/;
const XPATH_START = ['/', '(', '../', './', '*/'];
const IMAGE = /\.(jpe?g|gif|png|bmp|svg)$/i;

// Punctuation of CSS and XPath: kept as it is. Every other run of characters is
// a word run, judged by `valueParts`.
const STRUCTURE = /[\s[\](){}>+~,*=^$|!@:.#/'"<\\]/;
const QUOTED = /^("(?:[^"\\]|\\[\s\S])*"|'(?:[^'\\]|\\[\s\S])*')/;
const CSS_ATTRIBUTE = /^\[(\s*)(-?[_a-zA-Z][\w-]*)(\s*)([~|^$*]?=)(\s*)("(?:[^"\\]|\\[\s\S])*"|'(?:[^'\\]|\\[\s\S])*'|[^\s\]]+)(\s*[iIsS]?\s*)\]/;
const XPATH_ATTRIBUTE = /^@([\w.:-]+)(\s*(?:!=|=|,)\s*)("[^"]*"|'[^']*')/;

function attributeValueParts(name, value, words) {
  const quote = /^["']/.test(value) ? value[0] : '';
  const inner = quote ? value.slice(1, -1) : value;
  const parts = TEXT_ATTRIBUTES.has(name.toLowerCase()) ? [count(inner)] : valueParts(inner, words);
  return [kept(quote), ...parts, kept(quote)];
}

/**
 * A CSS (or, with `xpath`, an XPath) selector's parts: its structure, and each
 * value or text judged as the module says.
 */
function structureParts(selector, words, xpath = false) {
  const parts = [];
  let rest = selector;
  while (rest.length > 0) {
    let m = xpath ? null : CSS_ATTRIBUTE.exec(rest);
    if (m) {
      const [whole, sp1, name, sp2, op, sp3, value, flags] = m;
      parts.push(kept(`[${sp1}`), ...valueParts(name, words), kept(`${sp2}${op}${sp3}`));
      parts.push(...attributeValueParts(name, value, words), kept(`${flags}]`));
      rest = rest.slice(whole.length);
      continue;
    }
    m = xpath ? XPATH_ATTRIBUTE.exec(rest) : null;
    if (m) {
      const [whole, name, op, value] = m;
      parts.push(kept('@'), ...valueParts(name, words), kept(op), ...attributeValueParts(name, value, words));
      rest = rest.slice(whole.length);
      continue;
    }
    m = QUOTED.exec(rest);
    if (m) {
      // A string that is not an attribute's value is text the element shows.
      parts.push(kept(m[0][0]), count(m[0].slice(1, -1)), kept(m[0][0]));
      rest = rest.slice(m[0].length);
      continue;
    }
    if (STRUCTURE.test(rest[0])) {
      parts.push(kept(rest[0]));
      rest = rest.slice(1);
      continue;
    }
    let end = 1;
    while (end < rest.length && !STRUCTURE.test(rest[end])) end += 1;
    parts.push(...valueParts(rest.slice(0, end), words));
    rest = rest.slice(end);
  }
  return parts;
}

/** A selector's parts, by the strategy WebdriverIO reads it with. */
export function selectorParts(selector, words = codeWords()) {
  const text = String(selector);
  const direct = DIRECT.exec(text);
  if (direct) {
    const [, strategy, value] = direct;
    const head = kept(`${strategy}:`);
    if (strategy === 'css selector' || strategy === 'xpath') {
      return [head, ...structureParts(value, words, strategy === 'xpath')];
    }
    if (strategy === 'id' || strategy === 'tag name' || strategy === 'class name' || strategy === 'name') {
      return [head, ...valueParts(value, words)];
    }
    return [head, count(value)];
  }
  if (IMAGE.test(text)) return [count(text)];
  if (XPATH_START.some((start) => text.startsWith(start)) || text === '.' || text === '..') {
    return structureParts(text, words, true);
  }
  const textStrategy = TEXT_STRATEGIES.find((prefix) => text.startsWith(prefix));
  if (textStrategy) return [kept(textStrategy), count(text.slice(textStrategy.length))];
  if (text.startsWith('id=')) return [kept('id='), ...valueParts(text.slice(3), words)];
  if (text.startsWith('>>>')) return [kept('>>>'), ...structureParts(text.slice(3), words)];
  const withText = ELEMENT_WITH_TEXT.exec(text);
  if (withText) {
    const [, tag, prefix, name, attr, open, attrValue, close, insensitive, partial, query] = withText;
    const parts = [...valueParts(tag, words)];
    if (prefix) parts.push(kept(prefix), ...valueParts(name, words));
    if (attr) {
      parts.push(kept('['), ...valueParts(attr, words));
      if (open) parts.push(kept('='), ...attributeValueParts(attr, `${open}${attrValue}${close}`, words));
      parts.push(kept(']'));
    }
    parts.push(kept(`${insensitive ?? ''}${partial ?? ''}=`), count(query));
    return parts;
  }
  return structureParts(text, words);
}

/** A test id's parts: see `valueParts`. */
export function testIdParts(testId, words = codeWords()) {
  return valueParts(String(testId), words);
}

const MARKDOWN = { '`': "'", '<': '&lt;', '>': '&gt;', '|': '\\|' };

/**
 * `parts` as one markdown-inline string: kept text flattened to one line with
 * markdown neutralised (backticks, `<`, `>` and `|`), a count as `<N characters>`, cut
 * at `max` characters of output with `...`.
 */
export function renderParts(parts, max = 80) {
  let out = '';
  let used = 0;
  for (const part of parts) {
    if ('n' in part) {
      const say = `<${part.n} character${part.n === 1 ? '' : 's'}>`;
      if (used + say.length > max) return `${out}...`;
      out += say;
      used += say.length;
      continue;
    }
    const flat = part.t.replace(/[\r\n]+/g, ' ');
    const room = max - used;
    const shown = flat.length > room ? flat.slice(0, room) : flat;
    out += shown.replace(/[`<>|]/g, (c) => MARKDOWN[c]);
    used += shown.length;
    if (flat.length > room) return `${out}...`;
  }
  return out;
}

/** A test id as report.md prints it. */
export function shareableTestId(testId, words = codeWords()) {
  return renderParts(testIdParts(testId, words));
}

/** A selector as report.md prints it. */
export function shareableSelector(selector, words = codeWords()) {
  return renderParts(selectorParts(selector, words));
}
