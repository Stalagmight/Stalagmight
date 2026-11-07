// String-aware JS/TS comment stripping for the line-based source gates
// (#576 E7 / E2 / E14): check-e2e-parallel-safe.mjs, the read-only gating
// tripwire and check-e2e-no-sleep.mjs.
//
// Removes `//` and `/* */` comments from a file's lines while KEEPING string
// and template contents (a rule must still see `'localhost:5173'` or
// `'closureAdjusted'`). `/* */` and template literals carry across lines;
// '...' and "..." end at the line end. A quoted `/*` or `//` is not a comment -
// the bug class both older per-line strippers had (one blanked the rest of a
// file after 'fixtures/*.dmp', the other missed block comments entirely).
//
// A regex literal is kept verbatim too, so a `/*` or a quote inside one (e.g.
// `/^\/*/`) opens neither a comment nor a string (#576 RV3-FE-12: a regex like
// that used to blank every line up to the next `*/`, hiding code from three
// gates). A `/` starts a regex literal only where an expression may begin: at
// the line start, after an operator or opening bracket, or after a keyword such
// as `return`; anywhere else it is division. Not a parser: a misread `/` can
// only shift where a same-line string or regex ends, and every such span is
// copied through, so a gate still sees the code. `<` is not a prefix: in TSX
// `</div>` is a closing tag, not a regex.
const REGEX_PREFIX_CHARS = new Set(['(', ',', '=', ':', '[', '!', '&', '|', '?', '{', ';', '+', '-', '*', '%', '>', '~', '^']);
const REGEX_PREFIX_WORDS = new Set(['return', 'typeof', 'case', 'do', 'else', 'in', 'of', 'void', 'yield', 'await', 'delete', 'throw', 'new']);

// Does a `/` right after `before` (the text emitted so far on this line,
// comments already removed) start a regex literal?
function regexMayStart(before) {
  const t = before.trimEnd();
  if (t === '') return true;
  const last = t[t.length - 1];
  if (REGEX_PREFIX_CHARS.has(last)) return true;
  const word = /([A-Za-z_$][\w$]*)$/.exec(t);
  return word !== null && REGEX_PREFIX_WORDS.has(word[1]) && t[t.length - word[1].length - 1] !== '.';
}

// End index (exclusive, flags included) of a regex literal whose opening `/`
// is at line[i], or -1 when no closing `/` follows on the line (then it was a
// division after all).
function regexEnd(line, i) {
  let inClass = false;
  for (let j = i + 1; j < line.length; j++) {
    const c = line[j];
    if (c === '\\') { j++; continue; }
    if (inClass) { if (c === ']') inClass = false; continue; }
    if (c === '[') { inClass = true; continue; }
    if (c === '/') {
      let k = j + 1;
      while (k < line.length && /[a-z]/i.test(line[k])) k++;
      return k;
    }
  }
  return -1;
}

export function stripComments(lines) {
  const out = [];
  let inBlock = false;
  let inTemplate = false;
  for (const line of lines) {
    let code = '';
    let quote = null;
    for (let i = 0; i < line.length; i++) {
      const c = line[i];
      const n = line[i + 1];
      if (inBlock) {
        if (c === '*' && n === '/') { inBlock = false; i++; code += ' '; }
        continue;
      }
      if (inTemplate || quote) {
        code += c;
        if (c === '\\') { code += n ?? ''; i++; continue; }
        if (inTemplate && c === '`') inTemplate = false;
        else if (quote && c === quote) quote = null;
        continue;
      }
      if (c === '/' && n === '/') break;
      if (c === '/' && n === '*') { inBlock = true; i++; code += ' '; continue; }
      if (c === '/' && regexMayStart(code)) {
        const end = regexEnd(line, i);
        if (end > 0) { code += line.slice(i, end); i = end - 1; continue; }
      }
      if (c === "'" || c === '"') quote = c;
      else if (c === '`') inTemplate = true;
      code += c;
    }
    out.push(code);
  }
  return out;
}

// Index just past the `)` that closes the `(` at code[open], in
// comment-stripped text (stripComments output joined with '\n'). Quotes,
// templates and regex literals are skipped by the same rules as above, so a
// `)` inside `'...'` or `/\)/` does not close the call. Returns code.length
// when the call never closes: an unterminated call reads as running to the end
// of the file, the conservative answer for a scope check.
export function matchParen(code, open) {
  return matchPair(code, open, '(', ')');
}

// {@link matchParen} for the `{` at code[open]: index just past the `}` that
// closes it (a function body), by the same rules (#576 RV10b-G1).
export function matchBrace(code, open) {
  return matchPair(code, open, '{', '}');
}

function matchPair(code, open, openCh, closeCh) {
  let depth = 0;
  let quote = null;
  let lineStart = code.lastIndexOf('\n', open) + 1;
  for (let i = open; i < code.length; i++) {
    const c = code[i];
    if (c === '\n') lineStart = i + 1;
    if (quote) {
      if (c === '\\') { i++; continue; }
      if (c === quote) quote = null;
      else if (c === '\n' && quote !== '`') quote = null;
      continue;
    }
    if (c === "'" || c === '"' || c === '`') { quote = c; continue; }
    if (c === '/' && regexMayStart(code.slice(lineStart, i))) {
      const lineEnd = code.indexOf('\n', i);
      const end = regexEnd(code.slice(0, lineEnd < 0 ? code.length : lineEnd), i);
      if (end > 0) { i = end - 1; continue; }
    }
    if (c === openCh) depth++;
    else if (c === closeCh && --depth === 0) return i + 1;
  }
  return code.length;
}
