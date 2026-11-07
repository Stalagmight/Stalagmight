// Fixture tests for the ESLint gate (scripts/check-eslint.mjs, eslint.config.js).
//
// Two halves:
//   - the ratchet arithmetic, on synthetic counts (a grown count, a new file, a
//     new rule and a loosened count all fail; an exact match passes);
//   - the REAL config, driven with in-memory snippets at paths inside the gate
//     scope, so a config edit that silently drops a rule (or the scope that
//     carries it) fails here. Each rule is pinned with a planted violation it
//     MUST report and a near miss it must NOT.
//
// No test framework (mirrors the sibling scripts/check-*.test.mjs files).
// Run: node scripts/check-eslint.test.mjs   (pnpm lint:eslint:test; CI check job)

import assert from 'node:assert/strict';
import * as path from 'node:path';
import { ESLint } from 'eslint';
import { REPO_ROOT, LINT_TARGETS, ratchet, summarize, total, baselinePayload } from './check-eslint.mjs';

let passed = 0;
const failures = [];
async function test(name, fn) {
  try {
    await fn();
    passed++;
  } catch (e) {
    failures.push(`${name}: ${e.message}`);
  }
}

// --- ratchet arithmetic ----------------------------------------------------------

const R = 'react-hooks/exhaustive-deps';

await test('an exact match passes', () => {
  const { grown, loosened } = ratchet({ [R]: { 'src/a.tsx': 2 } }, { [R]: { 'src/a.tsx': 2 } });
  assert.deepEqual([grown, loosened], [[], []]);
});

await test('a grown count fails', () => {
  const { grown } = ratchet({ [R]: { 'src/a.tsx': 3 } }, { [R]: { 'src/a.tsx': 2 } });
  assert.deepEqual(grown, [{ rule: R, file: 'src/a.tsx', have: 3, allowed: 2 }]);
});

await test('a warning in an unrecorded file fails', () => {
  const { grown } = ratchet({ [R]: { 'src/new.tsx': 1 } }, { [R]: { 'src/a.tsx': 1 } });
  assert.equal(grown.length, 1);
  assert.equal(grown[0].file, 'src/new.tsx');
  assert.equal(grown[0].allowed, 0);
});

await test('a warning from an unrecorded rule fails', () => {
  const { grown } = ratchet({ 'some/other-rule': { 'src/a.tsx': 1 } }, { [R]: { 'src/a.tsx': 1 } });
  assert.equal(grown.length, 1);
  assert.equal(grown[0].rule, 'some/other-rule');
});

await test('moving a warning between files is caught (per-file, not a global total)', () => {
  const { grown, loosened } = ratchet({ [R]: { 'src/a.tsx': 2 } }, { [R]: { 'src/a.tsx': 1, 'src/b.tsx': 1 } });
  assert.equal(grown.length, 1);
  assert.equal(loosened.length, 1);
});

await test('a count below its baseline is reported as loosened (the ratchet must be tightened)', () => {
  const { grown, loosened } = ratchet({ [R]: { 'src/a.tsx': 1 } }, { [R]: { 'src/a.tsx': 2, 'src/gone.tsx': 1 } });
  assert.deepEqual(grown, []);
  assert.deepEqual(loosened.map((l) => [l.file, l.have, l.allowed]), [['src/a.tsx', 1, 2], ['src/gone.tsx', 0, 1]]);
});

await test('summarize splits errors (and fatal parse errors) from per-file warning counts', () => {
  const results = [
    { filePath: 'A', messages: [{ ruleId: R, severity: 1, line: 1, column: 1, message: 'w' }, { ruleId: R, severity: 1, line: 2, column: 1, message: 'w' }] },
    { filePath: 'B', messages: [{ ruleId: null, severity: 2, fatal: true, line: 3, column: 1, message: 'Parsing error' }] },
  ];
  const { errors, warnings } = summarize(results, (f) => `src/${f}.tsx`);
  assert.deepEqual(warnings, { [R]: { 'src/A.tsx': 2 } });
  assert.equal(errors.length, 1);
  assert.equal(errors[0].file, 'src/B.tsx');
  assert.equal(total(warnings), 2);
});

await test('the baseline payload is sorted and carries its total', () => {
  const p = baselinePayload({ [R]: { 'src/b.tsx': 1, 'src/a.tsx': 2 } });
  assert.equal(p.total, 3);
  assert.deepEqual(Object.keys(p.warnings[R]), ['src/a.tsx', 'src/b.tsx']);
});

// --- the real config ------------------------------------------------------------

const eslint = new ESLint({ cwd: REPO_ROOT });
async function lint(rel, code) {
  const [result] = await eslint.lintText(code, { filePath: path.join(REPO_ROOT, rel) });
  return result.messages.map((m) => ({ ruleId: m.ruleId, severity: m.severity, line: m.line, message: m.message }));
}
const ruleHits = (msgs, rule) => msgs.filter((m) => m.ruleId === rule);
const SRC = 'src/components/__eslint_fixture__/Fixture.tsx';

await test('the gate scope is src and both test-automation trees', () => {
  assert.deepEqual([...LINT_TARGETS].sort(), ['e2e', 'e2e-playwright', 'src']);
});

await test('rules-of-hooks: a hook behind || is an error', async () => {
  const msgs = await lint(SRC, `
    import { useState } from 'react';
    function useA() { return useState(0)[0]; }
    function useB() { return useState(1)[0]; }
    export function Probe() {
      const v = useA() || useB();
      return <span>{v}</span>;
    }
  `);
  const hits = ruleHits(msgs, 'react-hooks/rules-of-hooks');
  assert.equal(hits.length, 1, JSON.stringify(msgs));
  assert.equal(hits[0].severity, 2);
  assert.match(hits[0].message, /useB/);
});

await test('rules-of-hooks: a hook after an early return is an error', async () => {
  const msgs = await lint(SRC, `
    import { useState } from 'react';
    export function Probe({ on }: { on: boolean }) {
      if (!on) return null;
      const [v] = useState(0);
      return <span>{v}</span>;
    }
  `);
  assert.equal(ruleHits(msgs, 'react-hooks/rules-of-hooks').length, 1, JSON.stringify(msgs));
});

await test('rules-of-hooks: hooks at the top level are clean', async () => {
  const msgs = await lint(SRC, `
    import { useState } from 'react';
    export function Probe({ on }: { on: boolean }) {
      const [v] = useState(0);
      if (!on) return null;
      return <span>{v}</span>;
    }
  `);
  assert.deepEqual(msgs, []);
});

await test('exhaustive-deps: a missing dependency is a warning, a complete list is clean', async () => {
  const missing = await lint(SRC, `
    import { useEffect } from 'react';
    export function Probe({ id }: { id: number }) {
      useEffect(() => { console.log(id); }, []);
      return null;
    }
  `);
  const hits = ruleHits(missing, 'react-hooks/exhaustive-deps');
  assert.equal(hits.length, 1, JSON.stringify(missing));
  assert.equal(hits[0].severity, 1);
  const complete = await lint(SRC, `
    import { useEffect } from 'react';
    export function Probe({ id }: { id: number }) {
      useEffect(() => { console.log(id); }, [id]);
      return null;
    }
  `);
  assert.deepEqual(complete, []);
});

await test('an unused eslint-disable directive is an error', async () => {
  const msgs = await lint(SRC, `
    import { useEffect } from 'react';
    export function Probe({ id }: { id: number }) {
      // eslint-disable-next-line react-hooks/exhaustive-deps -- nothing to suppress here
      useEffect(() => { console.log(id); }, [id]);
      return null;
    }
  `);
  assert.equal(msgs.length, 1, JSON.stringify(msgs));
  assert.equal(msgs[0].severity, 2);
  assert.match(msgs[0].message, /Unused eslint-disable directive/);
});

await test('a directive naming a rule that is not loaded is an error', async () => {
  const msgs = await lint(SRC, `
    // eslint-disable-next-line react/no-danger -- planted: the rule is not loaded
    export const x = 1;
  `);
  assert.equal(msgs.length, 1, JSON.stringify(msgs));
  assert.equal(msgs[0].severity, 2);
  assert.match(msgs[0].message, /Definition for rule 'react\/no-danger' was not found/);
});

await test('test files under src are in scope', async () => {
  const msgs = await lint('src/components/__eslint_fixture__/__tests__/Fixture.test.tsx', `
    import { useState } from 'react';
    export function Probe({ on }: { on: boolean }) {
      if (!on) return null;
      const [v] = useState(0);
      return <span>{v}</span>;
    }
  `);
  assert.equal(ruleHits(msgs, 'react-hooks/rules-of-hooks').length, 1, JSON.stringify(msgs));
});

await test('generated IPC types are ignored', async () => {
  assert.equal(await eslint.isPathIgnored(path.join(REPO_ROOT, 'src/types/ipc/Fixture.ts')), true);
  assert.equal(await eslint.isPathIgnored(path.join(REPO_ROOT, 'src/types/ipc.generated.ts')), true);
  assert.equal(await eslint.isPathIgnored(path.join(REPO_ROOT, 'src/types/project.ts')), false);
});

// --- typescript-eslint recommended, and the per-tree severities -------------------

const PW = 'e2e-playwright/__eslint_fixture__.spec.ts';
const AGENT = 'e2e/agent/__eslint_fixture__.ts';

// [#576 NF-W7F-5] The e2e trees carried no `any` left (the baseline held no
// no-explicit-any row), so the rule is an error there too, like in src.
await test('recommended: `any` is an error in src and in both e2e trees', async () => {
  const code = 'export const x: any = 1;\n';
  for (const rel of [SRC, PW, AGENT]) {
    const hits = ruleHits(await lint(rel, code), '@typescript-eslint/no-explicit-any');
    assert.equal(hits.length, 1, rel);
    assert.equal(hits[0].severity, 2, rel);
  }
});

await test('recommended: an unused name is an error in src, a warning in e2e, and `_`-prefixed names are exempt', async () => {
  const unused = 'export function f(a: number, _b: number) { const c = 1; const _d = 2; return 0; }\n';
  const inSrc = ruleHits(await lint(SRC, unused), '@typescript-eslint/no-unused-vars');
  assert.deepEqual(inSrc.map((m) => [m.severity, /'(\w+)'/.exec(m.message)[1]]), [[2, 'a'], [2, 'c']]);
  const inPw = ruleHits(await lint(PW, unused), '@typescript-eslint/no-unused-vars');
  assert.deepEqual(inPw.map((m) => m.severity), [1, 1]);
  const caught = 'export function g() { try { return 1; } catch (_e) { return 0; } }\n';
  assert.deepEqual(await lint(SRC, caught), []);
});

await test('recommended: the other rules stay errors everywhere (prefer-const, no-unused-expressions)', async () => {
  for (const rel of [SRC, PW, AGENT]) {
    const msgs = await lint(rel, 'export function f(a: boolean, g: () => void) { let x = 1; a ? g() : g(); return x; }\n');
    assert.equal(ruleHits(msgs, 'prefer-const')[0]?.severity, 2, rel);
    assert.equal(ruleHits(msgs, '@typescript-eslint/no-unused-expressions')[0]?.severity, 2, rel);
  }
});

await test('the React hook rules apply to src only (a Playwright fixture calls `use`)', async () => {
  const fixture = `
    export const fixtures = {
      store: async ({ page }: { page: unknown }, use: (v: unknown) => Promise<void>) => { await use(page); },
    };
  `;
  assert.deepEqual(await lint(PW, fixture), []);
});

await test('an unused directive is an error in the e2e trees too', async () => {
  const msgs = await lint(AGENT, '// eslint-disable-next-line no-console -- not enabled, so this suppresses nothing\nexport const x = 1;\n');
  assert.equal(msgs.length, 1, JSON.stringify(msgs));
  assert.equal(msgs[0].severity, 2);
});

// --- stalagmight/no-render-ref-write (#576 D17) ----------------------------------

const REF_RULE = 'stalagmight/no-render-ref-write';
const refHits = async (rel, code) => ruleHits(await lint(rel, code), REF_RULE);

await test('no-render-ref-write: a render-body mirror in a component is an error', async () => {
  const hits = await refHits(SRC, `
    import { useRef } from 'react';
    export function Probe({ value }: { value: number }) {
      const valueRef = useRef(value);
      valueRef.current = value;
      return <span>{valueRef.current}</span>;
    }
  `);
  assert.equal(hits.length, 1);
  assert.equal(hits[0].severity, 2);
  assert.match(hits[0].message, /valueRef\.current.*Probe.*useLatest/);
});

await test('no-render-ref-write: hooks, arrow components, memo/forwardRef wrappers and nested render-time blocks are covered', async () => {
  const hits = await refHits(SRC, `
    import { forwardRef, memo, useMemo, useRef } from 'react';
    export function useThing(v: number) {
      const r = useRef(v);
      r.current = v;
      return r;
    }
    export const Arrow = ({ v }: { v: number }) => {
      const r = useRef(v);
      if (v > 0) { r.current = v; }
      return null;
    };
    export const Memoed = memo(function Inner({ v }: { v: number }) {
      const r = useRef(v);
      r.current = v;
      return null;
    });
    export const Forwarded = forwardRef<HTMLDivElement, { v: number }>(({ v }, ref) => {
      const r = useRef(v);
      r.current = v;
      return <div ref={ref} />;
    });
    export function InMemo({ v }: { v: number }) {
      const r = useRef(0);
      useMemo(() => { r.current = v; }, [v]);
      return null;
    }
  `);
  assert.equal(hits.length, 5, JSON.stringify(hits));
});

await test('no-render-ref-write: writes in effects, handlers and plain functions, and lazy init, are clean', async () => {
  const hits = await refHits(SRC, `
    import { useEffect, useRef } from 'react';
    export function Probe({ value }: { value: number }) {
      const a = useRef(value);
      const b = useRef<number[] | null>(null);
      if (b.current === null) b.current = [];
      const c = useRef<Map<string, number> | null>(null);
      c.current ??= new Map();
      useEffect(() => { a.current = value; });
      const onClick = () => { a.current = 0; };
      return <button onClick={onClick} />;
    }
    export function helper(r: { current: number }) {
      r.current = 1;
    }
  `);
  assert.deepEqual(hits, []);
});

await test('no-render-ref-write: src only (the e2e trees render no React)', async () => {
  const code = 'export function Probe(r: { current: number }) { r.current = 1; }\n';
  assert.equal((await refHits(SRC, code)).length, 1);
  assert.deepEqual(await refHits(PW, code), []);
});

// --- stalagmight/require-store-selector (#576 D2) --------------------------------

const STORE_RULE = 'stalagmight/require-store-selector';
const storeHits = async (rel, code) => ruleHits(await lint(rel, code), STORE_RULE);

await test('require-store-selector: a no-selector store hook call is an error', async () => {
  const hits = await storeHits(SRC, `
    import { useProjectStore } from '../../store/projectStore';
    export function Probe() {
      const { project } = useProjectStore();
      return <span>{project?.name}</span>;
    }
  `);
  assert.equal(hits.length, 1, JSON.stringify(hits));
  assert.equal(hits[0].severity, 2);
  assert.match(hits[0].message, /useProjectStore\(\)` subscribes to the whole store/);
});

await test('require-store-selector: identity selectors (bare, block-bodied, under useShallow) and useStore(store) are errors', async () => {
  const hits = await storeHits(SRC, `
    import { useStore } from 'zustand';
    import { useShallow } from 'zustand/react/shallow';
    import { useUIStore, uiStore } from '../../store/uiStore';
    export function useAll() {
      const a = useUIStore((s) => s);
      const b = useUIStore(function (state) { return state; });
      const c = useUIStore(useShallow((s) => s));
      const d = useStore(uiStore);
      return [a, b, c, d];
    }
  `);
  assert.deepEqual(hits.map((h) => h.line), [6, 7, 8, 9], JSON.stringify(hits));
  assert.ok(hits.every((h) => h.severity === 2));
});

await test('require-store-selector: real selectors, shallow picks and non-hook store calls are clean', async () => {
  const hits = await storeHits(SRC, `
    import { useStore } from 'zustand';
    import { useShallow } from 'zustand/react/shallow';
    import { useUIStore, uiStore } from '../../store/uiStore';
    import { useProjectStore } from '../../store/projectStore';
    export function Probe() {
      const project = useProjectStore((s) => s.project);
      const { a, b } = useUIStore(useShallow((s) => ({ a: s.view, b: s.closeDialog })));
      const view = useStore(uiStore, (s) => s.view);
      const snapshot = useProjectStore.getState();
      useProjectStore.setState({ isDirty: false });
      const stop = useUIStore.subscribe(() => {});
      stop();
      return <span>{project?.name}{a}{String(b)}{view}{String(snapshot.isDirty)}</span>;
    }
  `);
  assert.deepEqual(hits, []);
});

// [#576 RV5-LS-3] Two whole-store subscriptions the identity check missed: a
// selector that builds a fresh object or array (zustand compares with Object.is,
// so the answer differs on every store write), and useShallow over a copy of the
// whole state.
await test('require-store-selector: fresh-object/array selectors without useShallow and a shallow spread of the state are errors', async () => {
  const hits = await storeHits(SRC, `
    import { useStore } from 'zustand';
    import { useShallow } from 'zustand/react/shallow';
    import { useUIStore, uiStore } from '../../store/uiStore';
    import { useProjectStore } from '../../store/projectStore';
    export function useFresh() {
      const a = useUIStore((s) => ({ a: s.view }));
      const b = useUIStore((s) => [s.view, s.closeDialog]);
      const c = useProjectStore((s) => s.project?.sections.map((x) => x.id));
      const d = useUIStore(useShallow((s) => ({ ...s })));
      const e = useStore(uiStore, (s) => ({ v: s.view }));
      const f = useUIStore(function (state) { return { v: state.view }; });
      const g = useProjectStore((s) => Object.keys(s));
      return [a, b, c, d, e, f, g];
    }
  `);
  assert.deepEqual(hits.map((h) => h.line), [7, 8, 9, 10, 11, 12, 13], JSON.stringify(hits));
  assert.ok(hits.every((h) => h.severity === 2));
  assert.match(hits[3].message, /copies the whole store/);
  assert.match(hits[0].message, /new object or array/);
});

await test('require-store-selector: a fresh value under useShallow or with an equality fn, and a spread of a field, are clean', async () => {
  const hits = await storeHits(SRC, `
    import { useStore } from 'zustand';
    import { shallow } from 'zustand/shallow';
    import { useShallow } from 'zustand/react/shallow';
    import { useUIStore, uiStore } from '../../store/uiStore';
    import { useProjectStore } from '../../store/projectStore';
    export function useClean() {
      const a = useUIStore(useShallow((s) => ({ a: s.view })));
      const b = useUIStore(useShallow((s) => [s.view, s.closeDialog]));
      const c = useUIStore((s) => ({ a: s.view }), shallow);
      const d = useStore(uiStore, (s) => [s.view], shallow);
      const e = useUIStore(useShallow((s) => ({ ...s.view })));
      const f = useProjectStore((s) => s.project?.sections);
      const g = useProjectStore(useShallow((s) => s.project?.sections.map((x) => x.id) ?? []));
      const h = useProjectStore((s) => s.project?.sections ?? []);
      return [a, b, c, d, e, f, g, h];
    }
  `);
  assert.deepEqual(hits, []);
});

// [#576 RV6a-8] `.slice` / `.concat` on a STRING answer a primitive, which
// Object.is compares by value: not a fresh value. And a block-bodied selector that
// builds its answer in more than one statement is still a fresh-value selector.
await test('require-store-selector: string slice/concat are clean, a multi-statement fresh selector is an error', async () => {
  const hits = await storeHits(SRC, `
    import { useUIStore } from '../../store/uiStore';
    import { useProjectStore } from '../../store/projectStore';
    export function useProbe() {
      const a = useUIStore((s) => String(s.appMode).slice(0, 3));
      const b = useUIStore((s) => s.appMode.concat('-mode'));
      const c = useUIStore((s) => \`\${s.appMode}\`.slice(1));
      const d = useUIStore((s) => { const m = s.appMode; return { m }; });
      const e = useProjectStore((s) => { if (!s.project) return [s.isDirty]; return [s.project.name]; });
      const f = useProjectStore((s) => { const p = s.project; return p?.name; });
      const g = useProjectStore((s) => { if (!s.project) return []; return s.project.sections; });
      const h = useProjectStore((s) => s.project?.sections.slice(1));
      return [a, b, c, d, e, f, g, h];
    }
  `);
  assert.deepEqual(hits.map((h) => h.line), [8, 9, 12], JSON.stringify(hits));
});

await test('require-store-selector: covers src tests too, not the e2e trees', async () => {
  const code = `
    import { useProjectStore } from '../src/store/projectStore';
    export function useProbe() { return useProjectStore(); }
  `;
  assert.equal((await storeHits('src/components/__tests__/Fixture.test.tsx', code)).length, 1);
  assert.deepEqual(await storeHits(PW, code), []);
});

// --- stalagmight/require-disable-reason (#576 RV5-LS-4) ----------------------------

const REASON_RULE = 'stalagmight/require-disable-reason';
const reasonHits = async (rel, code) => ruleHits(await lint(rel, code), REASON_RULE);

await test('require-disable-reason: an eslint-disable directive with no reason after -- is an error, in src and e2e', async () => {
  const code = `
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    export const f = (x: any) => x;
    export const g = (x: any) => x; // eslint-disable-line @typescript-eslint/no-explicit-any
    /* eslint-disable-next-line @typescript-eslint/no-explicit-any -- */
    export const h = (x: any) => x;
  `;
  const src = await reasonHits(SRC, code);
  assert.deepEqual(src.map((h) => h.line), [2, 4, 5], JSON.stringify(src));
  assert.ok(src.every((h) => h.severity === 2));
  assert.match(src[0].message, /needs a reason/);
  assert.deepEqual((await reasonHits(PW, code)).map((h) => h.line), [2, 4, 5]);
});

// [#576 RV6a-7] ESLint reads a LINE comment as a directive only when it is
// eslint-disable-line / eslint-disable-next-line; `// eslint-disable ...` is prose
// to it. A block comment opening with eslint-disable is a directive.
await test('require-disable-reason: a line comment that opens with the bare word is prose, a block one is a directive', async () => {
  const code = `
    // eslint-disable is not allowed in this file, see docs/linting.md
    export const f = 1;
    /* eslint-disable @typescript-eslint/no-explicit-any */
    export const g = (x: any) => x;
  `;
  const hits = await reasonHits(SRC, code);
  assert.deepEqual(hits.map((h) => h.line), [4], JSON.stringify(hits));
});

await test('require-disable-reason: a directive with a reason, and eslint-enable, are clean', async () => {
  const hits = await reasonHits(SRC, `
    // eslint-disable-next-line @typescript-eslint/no-explicit-any -- the wire value is untyped JSON
    export const f = (x: any) => x;
    /* eslint-disable @typescript-eslint/no-explicit-any -- a block of untyped fixtures */
    export const g = (x: any) => x;
    /* eslint-enable @typescript-eslint/no-explicit-any */
    // A comment that only mentions eslint-disable is not a directive.
  `);
  assert.deepEqual(hits, []);
});

if (failures.length > 0) {
  console.error(`\n✗ check-eslint fixtures: ${failures.length} failed, ${passed} passed\n`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log(`✓ check-eslint fixtures: ${passed} passed`);
