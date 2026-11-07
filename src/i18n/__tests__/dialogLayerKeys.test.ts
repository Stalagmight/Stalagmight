import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import i18n from '../index';
import en from '../locales/en.json';
import ts from 'typescript';

/**
 * Every i18n key the dialog layer names resolves (#576 D20).
 *
 * The dialogs' hard-coded English moved into en.json, and
 * scripts/check-i18n-literals.mjs keeps new literals out. The failure mode that
 * leaves is the reverse one: a typo'd or later-deleted key renders as a blank
 * label in production (`parseMissingKeyHandler` returns '' outside dev) while
 * every test stays green. `importDialogKeys.test.ts` pins this for five import
 * dialogs; this extends the same check to every production file under
 * src/components/Dialogs/ and the src/components/common/ modules (the same scope
 * as the literal gate, minus the import derivation: common/ is scanned whole).
 *
 * What counts as a key (TypeScript AST): the first argument of a `t(...)` /
 * `i18n.t(...)` call, an `i18nKey` JSX attribute, and the value of an object
 * property whose name ends in `Key` (`labelKey: 'dialogs.x.y'`), each followed
 * through both arms of `cond ? 'a.b' : 'a.c'` - when the string starts with a
 * top-level namespace of en.json and has a dot. Template-literal keys
 * (`t(\`dialogs.x.${kind}\`)`) cannot be resolved statically and are skipped.
 */

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const COMPONENTS = path.resolve(__dirname, '../../components');
const ROOTS = ['Dialogs', 'common'].map((d) => path.join(COMPONENTS, d));
const NAMESPACES = Object.keys(en);
const isKey = (text: string) => text.includes('.') && NAMESPACES.includes(text.split('.')[0]!);

/** String literals an expression can evaluate to, through ?: arms and parentheses. */
function literals(e: ts.Expression | undefined): string[] {
  if (!e) return [];
  if (ts.isStringLiteral(e) || ts.isNoSubstitutionTemplateLiteral(e)) return [e.text];
  if (ts.isConditionalExpression(e)) return [...literals(e.whenTrue), ...literals(e.whenFalse)];
  if (ts.isParenthesizedExpression(e) || ts.isAsExpression(e)) return literals(e.expression);
  return [];
}

function keysIn(file: string): string[] {
  const source = ts.createSourceFile(file, fs.readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const keys = new Set<string>();
  const visit = (node: ts.Node) => {
    let found: string[] = [];
    if (ts.isCallExpression(node)) {
      const callee = node.expression;
      const name = ts.isIdentifier(callee) ? callee.text : ts.isPropertyAccessExpression(callee) ? callee.name.text : '';
      if (name === 't') found = literals(node.arguments[0]);
    } else if (ts.isJsxAttribute(node) && node.name.getText(source) === 'i18nKey' && node.initializer) {
      const init = node.initializer;
      found = ts.isStringLiteral(init) ? [init.text] : ts.isJsxExpression(init) ? literals(init.expression) : [];
    } else if (ts.isPropertyAssignment(node) && /Key$/.test(node.name.getText(source))) {
      found = literals(node.initializer);
    }
    for (const k of found) if (isKey(k)) keys.add(k);
    ts.forEachChild(node, visit);
  };
  visit(source);
  return [...keys];
}

function productionFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== '__tests__') out.push(...productionFiles(full));
    } else if (/\.tsx?$/.test(entry.name) && !/\.(test|spec)\.tsx?$/.test(entry.name)) {
      out.push(full);
    }
  }
  return out;
}

/** Plural keys exist only as `key_one` / `key_other`; probe with a count. */
function unresolved(key: string): boolean {
  const value = i18n.t(key, { count: 2 });
  return typeof value !== 'string' || value.length === 0 || value === key || value === `[${key}]`;
}

const files = ROOTS.flatMap(productionFiles);

describe('dialog-layer i18n keys', () => {
  it('scans the dialog layer and finds keys in it', () => {
    expect(files.length).toBeGreaterThan(50);
    const total = files.reduce((n, f) => n + keysIn(f).length, 0);
    // Guards the scanner against rotting into a no-op.
    expect(total).toBeGreaterThan(500);
  });

  it.each(files.map((f) => [path.relative(COMPONENTS, f), f]))('%s: every key resolves', (_rel, file) => {
    expect(keysIn(file).filter(unresolved)).toEqual([]);
  });
});
