// Value-import cycle gate for src/ (#576 D27-32(b)). Zero tolerance.
//
// Two modules that import each other's VALUES evaluate in an order decided by
// whichever one some other module happened to import first. The one evaluated
// second sees the first's exports in their temporal dead zone, so a single
// top-level read of a cross-import (a `useXStore.getState()` at module scope,
// a `const x = otherModule.thing`) throws a ReferenceError at app start - and
// only for one of the two import orders, so it can pass every test and still
// ship. projectStore <-> speleoStore was such a pair (plus three modules pulled
// into the same loop); the code dodged the hazard only because every cross use
// sat inside a function body.
//
// What counts as an edge (parsed with the TypeScript compiler API, syntax only):
//   - `import x from`, `import * as`, `import { a }`, `import './side-effect'`;
//   - `export { a } from`, `export * from`;
//   - NOT `import type` / `export type`, nor an import or re-export whose every
//     specifier is an inline `type` (TS elides both, so they cannot order
//     evaluation);
//   - NOT a dynamic `import()`: it runs after both modules have evaluated.
// Only relative specifiers are followed (src/ has no path aliases); a package
// cannot import back into src/.
//
// Scope: every .ts/.tsx under src/ (tests included, `.d.ts` excluded),
// found by a filesystem walk like the sibling gates (scripts/lib/src-files.mjs).
//
// How to break a cycle: move the shared value to a leaf module, pass the value
// in as a parameter, or late-bind it (src/store/speleoStoreHandle.ts is the pattern
// projectStore uses to reach speleoStore). Turning the import into
// `import type` is the fix when only types cross.
//
// Usage:  node scripts/check-import-cycles.mjs   (pnpm lint:import-cycles)
// Fixtures: scripts/check-import-cycles.test.mjs (pnpm lint:import-cycles:test)

import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import ts from 'typescript';
import { walkFiles, symlinkProblem } from './lib/src-files.mjs';

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const SCOPE = 'src';

const isTypeOnlyClause = (clause) => {
  if (!clause) return false; // `import './x'`: a side-effect import is a value edge
  if (clause.isTypeOnly) return true;
  if (clause.name) return false; // a default import is a value
  const b = clause.namedBindings;
  if (!b) return false;
  if (ts.isNamespaceImport(b)) return false;
  return b.elements.length > 0 && b.elements.every((e) => e.isTypeOnly);
};

const isTypeOnlyExport = (decl) => {
  if (decl.isTypeOnly) return true;
  const c = decl.exportClause;
  if (!c) return false; // `export * from`
  if (ts.isNamespaceExport(c)) return false;
  return c.elements.length > 0 && c.elements.every((e) => e.isTypeOnly);
};

// The value-import specifiers of one module, with their 1-based line numbers.
export function valueImports(fileName, text) {
  const sf = ts.createSourceFile(fileName, text, ts.ScriptTarget.Latest, false, fileName.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const out = [];
  const line = (node) => sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1;
  for (const stmt of sf.statements) {
    if (ts.isImportDeclaration(stmt)) {
      if (!isTypeOnlyClause(stmt.importClause)) out.push({ spec: stmt.moduleSpecifier.text, line: line(stmt) });
    } else if (ts.isExportDeclaration(stmt) && stmt.moduleSpecifier) {
      if (!isTypeOnlyExport(stmt)) out.push({ spec: stmt.moduleSpecifier.text, line: line(stmt) });
    } else if (ts.isImportEqualsDeclaration(stmt) && !stmt.isTypeOnly && ts.isExternalModuleReference(stmt.moduleReference)) {
      out.push({ spec: stmt.moduleReference.expression.text, line: line(stmt) });
    }
  }
  return out;
}

const RESOLVE_SUFFIXES = ['', '.ts', '.tsx', '/index.ts', '/index.tsx'];

// Resolve a relative specifier against the set of in-scope files (POSIX
// repo-relative paths). Returns null for a package or an out-of-scope target.
export function resolveSpecifier(fromRel, spec, files) {
  if (!spec.startsWith('.')) return null;
  const base = path.posix.normalize(path.posix.join(path.posix.dirname(fromRel), spec));
  for (const suffix of RESOLVE_SUFFIXES) {
    const cand = base + suffix;
    if (files.has(cand)) return cand;
  }
  return null;
}

// {rel: [{to, line}]} for every in-scope file.
export function buildGraph(fileTexts) {
  const files = new Set(Object.keys(fileTexts));
  const graph = {};
  for (const [rel, text] of Object.entries(fileTexts)) {
    const edges = [];
    for (const imp of valueImports(rel, text)) {
      const to = resolveSpecifier(rel, imp.spec, files);
      if (to) edges.push({ to, line: imp.line });
    }
    graph[rel] = edges;
  }
  return graph;
}

// Strongly connected components with a cycle in them: size > 1, or a module
// that imports itself. Iterative Tarjan (the module graph is deep enough that
// recursion is a needless risk). Each component's modules are sorted.
export function findCycles(graph) {
  let index = 0;
  const idx = new Map();
  const low = new Map();
  const onStack = new Set();
  const stack = [];
  const out = [];
  for (const root of Object.keys(graph).sort()) {
    if (idx.has(root)) continue;
    const work = [[root, 0]];
    idx.set(root, index);
    low.set(root, index);
    index++;
    stack.push(root);
    onStack.add(root);
    while (work.length) {
      const frame = work[work.length - 1];
      const [v, i] = frame;
      const edges = graph[v] ?? [];
      if (i < edges.length) {
        frame[1]++;
        const w = edges[i].to;
        if (!idx.has(w)) {
          idx.set(w, index);
          low.set(w, index);
          index++;
          stack.push(w);
          onStack.add(w);
          work.push([w, 0]);
        } else if (onStack.has(w)) {
          low.set(v, Math.min(low.get(v), idx.get(w)));
        }
        continue;
      }
      work.pop();
      if (work.length) {
        const parent = work[work.length - 1][0];
        low.set(parent, Math.min(low.get(parent), low.get(v)));
      }
      if (low.get(v) === idx.get(v)) {
        const comp = [];
        let w;
        do {
          w = stack.pop();
          onStack.delete(w);
          comp.push(w);
        } while (w !== v);
        const selfLoop = comp.length === 1 && (graph[v] ?? []).some((e) => e.to === v);
        if (comp.length > 1 || selfLoop) out.push(comp.sort());
      }
    }
  }
  return out.sort((a, b) => b.length - a.length || (a[0] < b[0] ? -1 : 1));
}

// One concrete loop through a component, as [{from, to, line}], starting at its
// first module: the shortest path back to the start, by BFS inside the component.
export function exampleCycle(graph, comp) {
  const inComp = new Set(comp);
  const start = comp[0];
  const prev = new Map();
  const queue = [start];
  const seen = new Set([start]);
  while (queue.length) {
    const v = queue.shift();
    for (const e of graph[v] ?? []) {
      if (!inComp.has(e.to)) continue;
      if (e.to === start) {
        const pathEdges = [{ from: v, to: start, line: e.line }];
        let cur = v;
        while (cur !== start) {
          const p = prev.get(cur);
          pathEdges.unshift(p);
          cur = p.from;
        }
        return pathEdges;
      }
      if (!seen.has(e.to)) {
        seen.add(e.to);
        prev.set(e.to, { from: v, to: e.to, line: e.line });
        queue.push(e.to);
      }
    }
  }
  return [];
}

export function scan(root = REPO_ROOT) {
  const { files, symlinks } = walkFiles(root, SCOPE, { exts: ['.ts', '.tsx'], skip: (rel) => rel.endsWith('.d.ts') });
  const texts = {};
  for (const rel of files) texts[rel] = fs.readFileSync(path.join(root, rel), 'utf8');
  const graph = buildGraph(texts);
  return { files: files.length, symlinks, graph, cycles: findCycles(graph) };
}

export function main(root = REPO_ROOT) {
  const { files, symlinks, graph, cycles } = scan(root);
  const problems = symlinks.map(symlinkProblem);
  if (problems.length === 0 && cycles.length === 0) {
    console.log(`✓ import cycles: ${files} modules under ${SCOPE}/, no value-import cycle.`);
    return 0;
  }
  for (const p of problems) console.error(`  ${p}`);
  if (cycles.length > 0) {
    console.error(`\n✗ import cycles: ${cycles.length} value-import cycle(s) under ${SCOPE}/:\n`);
    for (const comp of cycles) {
      console.error(`  ${comp.length} module(s): ${comp.join(', ')}`);
      for (const e of exampleCycle(graph, comp)) console.error(`    ${e.from}:${e.line} imports ${e.to}`);
      console.error('');
    }
    console.error(
      'Break each loop: move the shared value to a leaf module, pass it in as a parameter,\n' +
        'or late-bind it (see src/store/speleoStoreHandle.ts); use `import type` when only types cross.\n',
    );
  }
  return 1;
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) process.exit(main());
