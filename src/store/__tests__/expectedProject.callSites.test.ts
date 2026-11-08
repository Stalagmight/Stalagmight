/**
 * [#576 W87a-2] Every direct IPC call in `src/` that sends a command taking
 * `expectedProject` passes it.
 *
 * The argument is optional in the generated types, so the compiler cannot see one
 * left out. The backend then reads the call as made with no project open, refuses
 * it with PROJECT_CHANGED while one is (`core/state/expected_project.rs`), and the
 * frontend stays quiet on that code: a call site that forgets the argument drops
 * the user's request with nothing on screen. `projectStore.expectedProject.test.ts`
 * drives the store's own paths; this test reads every other sender's source.
 *
 * The command set comes from the generated manifest, so a command that gains the
 * argument is checked at every call site without touching this file. A call names
 * its command as a string literal and its arguments as an object literal; one that
 * does not cannot be read here, so it must be one of {@link DYNAMIC_SENDERS}.
 */
import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

import manifest from '../../types/ipc/ipcCommands.json';

const SRC = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

/** Every command whose IPC takes `expectedProject`, by the generated manifest. */
const TAKES_EXPECTED_PROJECT: ReadonlySet<string> = new Set(
  manifest.commands.filter((c) => c.args.some((a) => a.key === 'expectedProject')).map((c) => c.name),
);

/** The IPC entry points: the app's wrapper, and Tauri's own `invoke` that a few raw senders use. */
const CALLEES: ReadonlySet<string> = new Set(['invokeIpc', 'invoke']);

/**
 * The files that send a command named by a variable, each with what it sends. A new
 * file doing so fails until it is listed here with a reason, and an entry whose file
 * no longer does fails too.
 */
const DYNAMIC_SENDERS: Record<string, string> = {
  'utils/ipc.ts': 'the invokeIpc wrapper itself, forwarding its caller\'s command',
  'store/projectStore.ts':
    'the store\'s edit, history and import paths, which add expectedProject themselves (projectStore.expectedProject.test.ts)',
  'components/Dialogs/useGridViewPersistence.ts': 'the grid-view column preferences, config commands that name no project',
};

interface CallSite {
  /** `<file relative to src>:<line>`. */
  at: string;
  /** The command literal, or null when the command is not a literal. */
  command: string | null;
  /** Why the call fails the rule, or null when it passes (or names no such command). */
  problem: string | null;
}

/** The callee's name under any parentheses and casts: `(invokeIpc as Forwarded)(...)` is `invokeIpc`. */
function calleeName(expression: ts.Expression): string | null {
  let e = expression;
  while (ts.isParenthesizedExpression(e) || ts.isAsExpression(e) || ts.isTypeAssertionExpression(e)) e = e.expression;
  return ts.isIdentifier(e) ? e.text : null;
}

/** Why `args` does not pass a usable `expectedProject`, or null when it does. */
function expectedProjectProblem(args: ts.Expression | undefined): string | null {
  if (!args) return 'sends no arguments';
  if (!ts.isObjectLiteralExpression(args)) return 'its arguments are not an object literal, so the check cannot read them';
  const property = args.properties.find(
    (p) => (ts.isPropertyAssignment(p) || ts.isShorthandPropertyAssignment(p)) && p.name.getText() === 'expectedProject',
  );
  if (!property) return 'omits expectedProject';
  if (ts.isPropertyAssignment(property)) {
    const value = property.initializer;
    if (value.kind === ts.SyntaxKind.NullKeyword || (ts.isIdentifier(value) && value.text === 'undefined')) {
      return 'passes expectedProject as a literal null or undefined';
    }
  }
  return null;
}

/** Every IPC call in one source text. */
function scanSource(file: string, text: string): CallSite[] {
  const kind = file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS;
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, kind);
  const found: CallSite[] = [];
  const visit = (node: ts.Node): void => {
    if (ts.isCallExpression(node)) {
      const name = calleeName(node.expression);
      if (name !== null && CALLEES.has(name)) {
        const at = `${file}:${source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1}`;
        const first = node.arguments[0];
        if (first && ts.isStringLiteralLike(first)) {
          const problem = TAKES_EXPECTED_PROJECT.has(first.text) ? expectedProjectProblem(node.arguments[1]) : null;
          found.push({ at, command: first.text, problem });
        } else {
          found.push({ at, command: null, problem: null });
        }
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return found;
}

/** Every production `.ts`/`.tsx` under `src/`: no tests, no generated IPC types, no test harness. */
function productionFiles(dir: string, out: string[] = []): string[] {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    const rel = path.relative(SRC, full).split(path.sep).join('/');
    if (entry.isDirectory()) {
      if (entry.name === '__tests__' || rel === 'types' || rel === 'test') continue;
      productionFiles(full, out);
    } else if (/\.tsx?$/.test(entry.name) && !/\.(test|spec)\.tsx?$/.test(entry.name) && !entry.name.endsWith('.d.ts')) {
      out.push(rel);
    }
  }
  return out;
}

const SITES = productionFiles(SRC)
  .sort()
  .flatMap((rel) => scanSource(rel, fs.readFileSync(path.join(SRC, rel), 'utf8')));

describe('every direct IPC call names the project it is made for (#576 W87a-2)', () => {
  it('reads the manifest and finds the senders', () => {
    expect(TAKES_EXPECTED_PROJECT.size).toBeGreaterThan(50);
    const checked = SITES.filter((s) => s.command !== null && TAKES_EXPECTED_PROJECT.has(s.command));
    expect(checked.length).toBeGreaterThan(20);
  });

  it('passes expectedProject at every call of a command that takes it', () => {
    const failures = SITES.filter((s) => s.problem !== null).map((s) => `${s.at} ${s.command}: ${s.problem}`);
    expect(failures, `${failures.length} call(s) leave out expectedProject`).toEqual([]);
  });

  it('names its command as a literal everywhere but the listed dynamic senders', () => {
    const dynamicFiles = [...new Set(SITES.filter((s) => s.command === null).map((s) => s.at.replace(/:\d+$/, '')))].sort();
    expect(dynamicFiles).toEqual(Object.keys(DYNAMIC_SENDERS).sort());
  });

  describe('the rule itself', () => {
    const problems = (text: string) => scanSource('probe.ts', text).map((s) => s.problem);

    it('flags an omitted, unreadable, or empty expectedProject', () => {
      expect(problems("invokeIpc('save_project', { path, isAutosave: false });")).toEqual(['omits expectedProject']);
      expect(problems("invokeIpc('speleodb_push', args);")).toEqual([
        'its arguments are not an object literal, so the check cannot read them',
      ]);
      expect(problems("invokeIpc('speleodb_done');")).toEqual(['sends no arguments']);
      expect(problems("invokeIpc('undo', { expectedProject: null });")).toEqual([
        'passes expectedProject as a literal null or undefined',
      ]);
      expect(problems("(invokeIpc as F)('undo', { ...args });")).toEqual(['omits expectedProject']);
      expect(problems("invoke('speleodb_push', { message });")).toEqual(['omits expectedProject']);
    });

    it('accepts a passed expectedProject, and ignores commands that take none', () => {
      expect(problems("invokeIpc('speleodb_push', { message, expectedProject });")).toEqual([null]);
      expect(problems("invokeIpc('undo', { ...args, expectedProject: expectedOpenProject() });")).toEqual([null]);
      expect(problems("invokeIpc('list_recent_projects', undefined);")).toEqual([null]);
    });
  });
});
