// Hard-coded UI text gate for the dialogs (#576 D20). Zero tolerance.
//
// Only English ships, so English typed straight into a component looks fine -
// until a second locale lands and those strings stay English. The dialogs carried
// ~440 of them (the shared Error/Confirm/unsaved-changes primitives every dialog
// renders among them, and 13 validation errors) while the rest of the app went
// through i18n (src/i18n/locales/en.json, `t('...')`). This gate keeps the dialog
// layer at zero: every user-visible string it finds must come from `t()`, or be
// an allowlisted literal that is not language (a file-format or brand name).
//
// Scope: every production .ts/.tsx under src/components/Dialogs/ (subfolders
// included), plus each src/components/common/ module a dialog file imports - the
// primitives dialogs are built from (Button, Select, Toggle, ...). The common set
// is derived from the imports on every run, so a dialog that starts using a new
// primitive brings it into scope. Tests are excluded (scripts/lib/src-files.mjs).
// Plus the common modules in PINNED_COMMON: shared UI with text of its own that no
// dialog imports, so the derivation never reaches it (#576 NF-W6-2: the survey
// context menu, the panel error fallback and the entity link). A pinned file that
// no longer exists fails the gate, as a stale allowlist row does, so a rename
// cannot quietly drop it from scope.
//
// What counts as user-visible text (TypeScript compiler API, syntax only):
//   - JSX text:                                  <p>Delete shot</p>
//   - a string in a JSX child expression:        {busy ? 'Saving...' : label}
//   - a user-facing JSX attribute:               title="Edit Shot" aria-label={`Close ${x}`}
//     (USER_FACING_ATTRS below)
//   - a user-facing property in an object literal: openDialog('error', { title: 'Failed' })
//     (USER_FACING_PROPS below)
//   - an error/message setter argument:          setError('Name is required')
//     (a call to set...Error / ...Message / ...Warning / ...Notice)
//   - a toast's message:                         showToast('Shot deleted', 'key', 'info')
//     (MESSAGE_ARG_CALLS below: that argument only, never the dedup key or kind)
//   - the argument a message-named parameter of a function declared in the same
//     file takes:                                fail('segment', 'Import failed')
//     (MESSAGE_PARAM below; `useCallback((kind, message) => ...)` counts), unless
//     every use of that parameter is an argument of console.* - a debug trace is
//     not UI (#576 RV6a-3)
//   - every string a binding named as text holds: const FALLBACK_TEXT = { a: 'Failed' }
//     (`..._TEXT`, `...Message`, `...Labels` - isTextName below; i18n keys and code
//     tokens excepted: a lowercase identifier-like string with no space, such as
//     'timeout', 'section-' or 'closureAdjusted', is a value the code switches on,
//     not prose (#576 RV6a-3). Such a token is still reported where it is RENDERED
//     in the file - `{reason}`, `title={label}` - by the sink rules above.)
// Strings are followed through `a ? 'x' : 'y'`, `a && 'x'`, `a ?? 'x'`, `a || 'x'`,
// `'a' + b`, parentheses, `as` casts and `String(...)`, and through the file's own
// bindings: a `const`/`let` read where text is rendered (`{HEADING}`, a template
// held in `msg`), and a map or list declared in the file looked up there
// (`{NAMES[kind]}`, `NAMES.a`). Never into any other call, so `t('key')` and its
// key are never flagged. Bindings resolve by name within the file, so a name
// declared twice resolves to both (the gate errs towards reporting). A variable
// whose value comes from anywhere else stays invisible. A string counts only if it has a letter once template
// placeholders (`${...}`) and HTML entities (`&rarr;`) are removed, so `{' · '}`,
// `{`${n}/${m}`}` and `&times;` pass.
//
// Allowlist (scripts/i18n-literals-allowlist.json): exact {file, text, reason}
// rows for literals that are not language. A row covers every occurrence of that
// text in that file. A row that matches nothing fails (stale), so the list cannot
// quietly outlive its reason.
//
// Usage:  node scripts/check-i18n-literals.mjs   (pnpm lint:i18n-literals)
// Fixtures: scripts/check-i18n-literals.test.mjs (pnpm lint:i18n-literals:test)

import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import ts from 'typescript';
import { walkFiles, isTestPath, symlinkProblem } from './lib/src-files.mjs';

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const DIALOGS_DIR = 'src/components/Dialogs';
export const COMMON_DIR = 'src/components/common';
export const ALLOWLIST_FILE = 'scripts/i18n-literals-allowlist.json';
// Always in scope, whether or not a dialog imports them (see the header).
export const PINNED_COMMON = [
  `${COMMON_DIR}/ContextMenu.tsx`,
  `${COMMON_DIR}/DetailLink.tsx`,
  `${COMMON_DIR}/ErrorBoundary.tsx`,
];

export const USER_FACING_ATTRS = new Set([
  'title',
  'label',
  'placeholder',
  'alt',
  'aria-label',
  'aria-description',
  'aria-roledescription',
  'aria-valuetext',
  'aria-placeholder',
  'tooltip',
  'description',
  'message',
  'confirmLabel',
  'cancelLabel',
  'helpText',
  'hint',
  'subtitle',
  'heading',
  'emptyText',
  'emptyMessage',
]);
export const USER_FACING_PROPS = new Set([
  'title',
  'label',
  'message',
  'description',
  'placeholder',
  'confirmLabel',
  'cancelLabel',
  'tooltip',
  'hint',
  'subtitle',
  'detail',
]);
const MESSAGE_SETTER = /^set\w*(Error|Errors|Message|Warning|Notice)$/;

const relPosix = (root, abs) => path.relative(root, abs).split(path.sep).join('/');

// The letters left once placeholders and HTML entities are gone.
export function hasLetters(text) {
  const stripped = text
    .replace(/\$\{[^}]*\}/g, '')
    .replace(/&(?:[a-zA-Z]+|#\d+|#x[0-9a-fA-F]+);/g, '');
  return /[A-Za-z]/.test(stripped);
}

// [#576 RV5-LS-2] A call whose argument at a known position is the user-visible
// message, though the callee is not declared in the file: the toast API
// (`showToast(message, key, kind, options)` in src/store/uiStore.ts). Only that
// position: the dedup key and the kind are identifiers, not text.
export const MESSAGE_ARG_CALLS = new Map([['showToast', [0]]]);

// [#576 RV5-LS-1] A parameter of a function declared in the file whose argument is
// text: `fail(kind, message)`, `report(reason)`. The other parameters are not.
export const MESSAGE_PARAM = /^(?:message|msg|text|reason|label|title|description|detail|notice|caption)$|(?:Message|Text|Reason|Label|Title|Notice|Caption)$/;

// [#576 RV5-LS-1] A binding named as text - `FALLBACK_TEXT`, `emptyMessage`,
// `STEP_LABELS` - holds user-visible strings, wherever it is read: every string in
// its initializer (the values of an object map, the items of an array) counts.
const TEXT_NAME_WORDS = new Set([
  'text', 'texts', 'message', 'messages', 'label', 'labels', 'title', 'titles',
  'notice', 'notices', 'reason', 'reasons', 'caption', 'captions', 'heading', 'headings',
  'tooltip', 'tooltips', 'placeholder', 'placeholders', 'hint', 'hints',
]);
// [#576 RV6a-3] `copy` is not in the set: `const copy = { ...o, mode: 'edit' }` is a
// copy of a value far more often than UI copy.

// [#576 RV6a-3] A code token: lowercase-led, identifier characters joined by
// `-` `_` `.` `:` `/`, no space ('timeout', 'section-', 'closureAdjusted',
// 'shot_kind'). The binding rule skips these; a rendered one is still reported.
const CODE_TOKEN = /^[a-z][a-zA-Z0-9]*(?:[-_.:/][a-zA-Z0-9]*)*$/;
export function isTextName(name) {
  const words = name
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .split(/[\s_]+/)
    .filter(Boolean);
  const last = words[words.length - 1];
  return last !== undefined && TEXT_NAME_WORDS.has(last.toLowerCase());
}

// The local bindings of one file, by name: `const|let NAME = <initializer>` and
// the parameters of the functions declared there. Syntax only, so a name declared
// twice in different scopes resolves to both - the gate errs towards reporting.
function collectBindings(source) {
  const values = new Map();
  const functions = new Map();
  const push = (map, name, value) => map.set(name, [...(map.get(name) ?? []), value]);
  const fnOf = (init) => {
    if (!init) return null;
    if (ts.isArrowFunction(init) || ts.isFunctionExpression(init)) return init;
    // `useCallback((kind, message) => ..., [])` and friends: the function argument.
    if (ts.isCallExpression(init)) {
      const first = init.arguments[0];
      if (first && (ts.isArrowFunction(first) || ts.isFunctionExpression(first))) return first;
    }
    return null;
  };
  const visit = (node) => {
    if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.initializer) {
      push(values, node.name.text, node.initializer);
      const fn = fnOf(node.initializer);
      if (fn) push(functions, node.name.text, fn);
    } else if (ts.isFunctionDeclaration(node) && node.name) {
      push(functions, node.name.text, node);
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return { values, functions };
}

// The string-literal nodes an expression can evaluate to (see the header).
function stringsIn(e, ctx, seen = new Set()) {
  if (!e || seen.has(e)) return [];
  seen.add(e);
  const inner = (x) => stringsIn(x, ctx, seen);
  if (ts.isStringLiteral(e) || ts.isNoSubstitutionTemplateLiteral(e) || ts.isTemplateExpression(e)) return [e];
  if (ts.isConditionalExpression(e)) return [...inner(e.whenTrue), ...inner(e.whenFalse)];
  if (ts.isBinaryExpression(e)) {
    const k = e.operatorToken.kind;
    if (k === ts.SyntaxKind.AmpersandAmpersandToken) return inner(e.right);
    if (k === ts.SyntaxKind.BarBarToken || k === ts.SyntaxKind.QuestionQuestionToken || k === ts.SyntaxKind.PlusToken) {
      return [...inner(e.left), ...inner(e.right)];
    }
    return [];
  }
  if (ts.isParenthesizedExpression(e) || ts.isAsExpression(e) || ts.isNonNullExpression(e) || ts.isSatisfiesExpression?.(e)) {
    return inner(e.expression);
  }
  // [#576 RV5-LS-2] `String('Remove all')` renders its argument.
  if (ts.isCallExpression(e) && ts.isIdentifier(e.expression) && e.expression.text === 'String') {
    return inner(e.arguments[0]);
  }
  if (!ctx) return [];
  // [#576 RV5-LS-2] A binding read where text is rendered: its initializer, if
  // that is a string expression (`const HEADING = '...'`, a template held in a
  // variable). An object or array held in it is text only when looked up.
  if (ts.isIdentifier(e)) {
    return (ctx.values.get(e.text) ?? []).flatMap((init) =>
      ts.isObjectLiteralExpression(init) || ts.isArrayLiteralExpression(init) ? [] : inner(init),
    );
  }
  // `NAMES[kind]` / `NAMES.a` / `ITEMS[i]` on a map or list declared in the file.
  if ((ts.isElementAccessExpression(e) || ts.isPropertyAccessExpression(e)) && ts.isIdentifier(e.expression)) {
    const key = ts.isPropertyAccessExpression(e) ? e.name.text : null;
    return (ctx.values.get(e.expression.text) ?? []).flatMap((init) => collectionStrings(init, key, inner));
  }
  return [];
}

// The strings a map or list literal can yield: one property when the key is
// named, every value otherwise.
function collectionStrings(init, key, inner) {
  let e = init;
  while (ts.isAsExpression(e) || ts.isParenthesizedExpression(e) || ts.isSatisfiesExpression?.(e)) e = e.expression;
  if (ts.isArrayLiteralExpression(e)) return key === null ? e.elements.flatMap((x) => inner(x)) : [];
  if (!ts.isObjectLiteralExpression(e)) return [];
  return e.properties.flatMap((p) => {
    if (!ts.isPropertyAssignment(p)) return [];
    const name = ts.isIdentifier(p.name) || ts.isStringLiteral(p.name) ? p.name.text : null;
    return key === null || name === key ? inner(p.initializer) : [];
  });
}

// A template's text with its placeholders kept as `${expr}` and its escapes
// cooked, so a `\n` between two placeholders is a newline, not the letter n.
const literalText = (node, source) =>
  ts.isTemplateExpression(node)
    ? node.head.text + node.templateSpans.map((span) => `\${${span.expression.getText(source)}}${span.literal.text}`).join('')
    : node.text;

// [#576 RV5-LS-1] An i18n key (`dialogs.export.phases.generating`): what a map of
// keys holds, looked up through `t()` where it is rendered. Not text itself.
const I18N_KEY = /^[A-Za-z][\w-]*(?:\.[\w-]+)+$/;

const normalize = (text) => text.replace(/\s+/g, ' ').trim();

// The callee's name: `fail(...)`, `store.getState().showToast(...)`.
const calleeName = (call) =>
  ts.isIdentifier(call.expression)
    ? call.expression.text
    : ts.isPropertyAccessExpression(call.expression)
      ? call.expression.name.text
      : null;

// `console.warn(...)` and every other `console.<method>(...)`.
const isConsoleCall = (node) =>
  ts.isCallExpression(node) &&
  ts.isPropertyAccessExpression(node.expression) &&
  ts.isIdentifier(node.expression.expression) &&
  node.expression.expression.text === 'console';

// [#576 RV6a-3] Whether the parameter `name` of `fn` only ever reaches console.*:
// every read of it in the body sits inside the arguments of a console call (or it
// is never read). Such a parameter carries a debug trace, not UI text. Syntax
// only, by name, like the bindings above.
function paramOnlyLogs(fn, name) {
  let reaches = false;
  const visit = (node) => {
    if (reaches) return;
    if (ts.isIdentifier(node) && node.text === name) {
      const parent = node.parent;
      const isName =
        (ts.isPropertyAccessExpression(parent) && parent.name === node) ||
        (ts.isPropertyAssignment(parent) && parent.name === node);
      if (!isName) {
        let inConsole = false;
        for (let up = parent; up && up !== fn; up = up.parent) {
          if (isConsoleCall(up) && up.arguments.some((arg) => arg.pos <= node.pos && node.end <= arg.end)) {
            inConsole = true;
            break;
          }
        }
        if (!inConsole) reaches = true;
      }
    }
    ts.forEachChild(node, visit);
  };
  if (fn.body) visit(fn.body);
  return !reaches;
}

// Findings in one file: [{file, line, kind, text}].
export function analyzeSource(file, sourceText) {
  const kind = file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS;
  const source = ts.createSourceFile(file, sourceText, ts.ScriptTarget.Latest, true, kind);
  const ctx = collectBindings(source);
  const findings = [];
  // One finding per literal: a constant rendered in two places is one string to
  // move.
  const reported = new Set();
  const add = (node, what, text) => {
    if (!hasLetters(text)) return;
    const id = node.getStart(source);
    if (reported.has(id)) return;
    reported.add(id);
    const line = source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1;
    findings.push({ file, line, kind: what, text: normalize(text) });
  };
  const addAll = (expr, what) => {
    for (const s of stringsIn(expr, ctx)) add(s, what, literalText(s, source));
  };
  const visit = (node) => {
    if (ts.isJsxText(node)) {
      add(node, 'jsx-text', node.text);
    } else if (ts.isJsxAttribute(node) && node.initializer && USER_FACING_ATTRS.has(node.name.getText(source))) {
      const what = `attr ${node.name.getText(source)}`;
      if (ts.isStringLiteral(node.initializer)) add(node, what, node.initializer.text);
      else if (ts.isJsxExpression(node.initializer)) addAll(node.initializer.expression, what);
    } else if (ts.isJsxExpression(node) && (ts.isJsxElement(node.parent) || ts.isJsxFragment(node.parent))) {
      addAll(node.expression, 'jsx-expr');
    } else if (ts.isCallExpression(node) && calleeName(node) !== null) {
      const name = calleeName(node);
      if (ts.isIdentifier(node.expression) && MESSAGE_SETTER.test(name)) {
        for (const arg of node.arguments) addAll(arg, `${name}()`);
      } else if (MESSAGE_ARG_CALLS.has(name)) {
        for (const i of MESSAGE_ARG_CALLS.get(name)) addAll(node.arguments[i], `${name}()`);
      } else if (ts.isIdentifier(node.expression) && ctx.functions.has(name)) {
        // [#576 RV5-LS-1] A local function: the arguments its message-named
        // parameters take.
        for (const fn of ctx.functions.get(name)) {
          fn.parameters.forEach((param, i) => {
            if (!ts.isIdentifier(param.name) || !MESSAGE_PARAM.test(param.name.text)) return;
            if (paramOnlyLogs(fn, param.name.text)) return;
            addAll(node.arguments[i], `${name}()`);
          });
        }
      }
    } else if (
      ts.isPropertyAssignment(node) &&
      ts.isObjectLiteralExpression(node.parent) &&
      (ts.isIdentifier(node.name) || ts.isStringLiteral(node.name)) &&
      USER_FACING_PROPS.has(node.name.text)
    ) {
      addAll(node.initializer, `prop ${node.name.text}`);
    } else if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.initializer && isTextName(node.name.text)) {
      // [#576 RV5-LS-1] A binding named as text: its strings, a map's values and a
      // list's items included.
      const what = `binding ${node.name.text}`;
      const strings = [...stringsIn(node.initializer, ctx), ...collectionStrings(node.initializer, null, (x) => stringsIn(x, ctx))];
      for (const s of strings) {
        const text = literalText(s, source);
        if (!I18N_KEY.test(text) && !CODE_TOKEN.test(text)) add(s, what, text);
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return findings;
}

// Relative imports of a file, resolved to repo-relative paths that exist.
function resolvedImports(root, rel, sourceText) {
  const source = ts.createSourceFile(rel, sourceText, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const out = [];
  for (const stmt of source.statements) {
    const spec =
      (ts.isImportDeclaration(stmt) || ts.isExportDeclaration(stmt)) && stmt.moduleSpecifier && ts.isStringLiteral(stmt.moduleSpecifier)
        ? stmt.moduleSpecifier.text
        : null;
    if (!spec || !spec.startsWith('.')) continue;
    const base = path.posix.normalize(path.posix.join(path.posix.dirname(rel), spec));
    for (const cand of [base, `${base}.tsx`, `${base}.ts`, `${base}/index.tsx`, `${base}/index.ts`]) {
      const abs = path.join(root, cand);
      if (fs.existsSync(abs) && fs.statSync(abs).isFile()) {
        out.push(cand);
        break;
      }
    }
  }
  return out;
}

// The files in scope: {files, symlinks, missingPinned}. See the header.
export function scopeFiles(root = REPO_ROOT) {
  const exts = ['.ts', '.tsx'];
  const dialogs = walkFiles(root, DIALOGS_DIR, { exts, skip: isTestPath });
  const common = new Set();
  for (const rel of dialogs.files) {
    for (const dep of resolvedImports(root, rel, fs.readFileSync(path.join(root, rel), 'utf8'))) {
      if (dep.startsWith(`${COMMON_DIR}/`) && !isTestPath(dep) && /\.tsx?$/.test(dep)) common.add(dep);
    }
  }
  const missingPinned = [];
  for (const rel of PINNED_COMMON) {
    const abs = path.join(root, rel);
    if (fs.existsSync(abs) && fs.statSync(abs).isFile()) common.add(rel);
    else missingPinned.push(rel);
  }
  return { files: [...dialogs.files, ...[...common].sort()], symlinks: dialogs.symlinks, missingPinned };
}

export function loadAllowlist(root = REPO_ROOT) {
  const parsed = JSON.parse(fs.readFileSync(path.join(root, ALLOWLIST_FILE), 'utf8'));
  return parsed.entries ?? [];
}

// Split findings into those the allowlist does not cover, and allowlist rows that
// cover nothing.
export function applyAllowlist(findings, entries) {
  const key = (file, text) => `${file}\u0000${text}`;
  const allowed = new Map(entries.map((e) => [key(e.file, normalize(e.text)), e]));
  const used = new Set();
  const violations = [];
  for (const f of findings) {
    const k = key(f.file, f.text);
    if (allowed.has(k)) used.add(k);
    else violations.push(f);
  }
  const stale = entries.filter((e) => !used.has(key(e.file, normalize(e.text))));
  return { violations, stale };
}

export function scan(root = REPO_ROOT) {
  const { files, symlinks, missingPinned } = scopeFiles(root);
  const findings = files.flatMap((rel) => analyzeSource(rel, fs.readFileSync(path.join(root, rel), 'utf8')));
  return { files, symlinks, missingPinned, findings };
}

export function main(root = REPO_ROOT) {
  const { files, symlinks, missingPinned, findings } = scan(root);
  const entries = loadAllowlist(root);
  const bad = entries.filter((e) => typeof e.reason !== 'string' || e.reason.trim() === '');
  const { violations, stale } = applyAllowlist(findings, entries);
  let failed = false;
  for (const link of symlinks) {
    failed = true;
    console.error(`✗ ${symlinkProblem(link)}`);
  }
  if (violations.length > 0) {
    failed = true;
    console.error(`\n✗ i18n literals: ${violations.length} hard-coded UI string(s) in the dialog layer:\n`);
    for (const v of violations) console.error(`  ${v.file}:${v.line}  ${v.kind}  ${JSON.stringify(v.text)}`);
    console.error(
      `\nMove each into src/i18n/locales/en.json and render it with t('...') (keep the English byte-identical).` +
        `\nA literal that is not language (a file-format or brand name) goes in ${ALLOWLIST_FILE} with a reason.`,
    );
  }
  if (stale.length > 0) {
    failed = true;
    console.error(`\n✗ i18n literals: ${stale.length} allowlist row(s) match nothing - remove them:\n`);
    for (const s of stale) console.error(`  ${s.file}  ${JSON.stringify(s.text)}`);
  }
  if (bad.length > 0) {
    failed = true;
    console.error(`\n✗ i18n literals: ${bad.length} allowlist row(s) without a reason.`);
  }
  if (missingPinned.length > 0) {
    failed = true;
    console.error(`\n✗ i18n literals: ${missingPinned.length} PINNED_COMMON file(s) do not exist - update the list:\n`);
    for (const m of missingPinned) console.error(`  ${m}`);
  }
  if (failed) return 1;
  console.log(
    `✓ i18n literals: ${files.length} dialog-layer files, no hard-coded UI text ` +
      `(${findings.length - violations.length} allowlisted literal(s) in ${ALLOWLIST_FILE}).`,
  );
  return 0;
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) process.exit(main());
