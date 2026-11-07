import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { analyzeSource, scan } from './check-test-ids.mjs';

assert.deepEqual(
  analyzeSource('ok.tsx', 'const view = <Button data-testid="save-button">Save</Button>;'),
  [],
);

assert.match(
  analyzeSource('missing.tsx', 'const view = <input value={value} />;')[0]?.message ?? '',
  /has no data-testid/,
);

assert.match(
  analyzeSource(
    'repeated.tsx',
    'const view = rows.map((row) => <button data-testid="remove-row">Remove</button>);',
  )[0]?.message ?? '',
  /include item identity/,
);

assert.match(
  analyzeSource('anchor.tsx', 'const view = <a href="https://example.com">Website</a>;')[0]?.message ?? '',
  /has no data-testid/,
);

assert.match(
  analyzeSource('role.tsx', 'const view = <div role="button" tabIndex={0}>Open</div>;')[0]?.message ?? '',
  /has no data-testid/,
);

assert.match(
  analyzeSource('handler.tsx', 'const view = <div onClick={open}>Open</div>;')[0]?.message ?? '',
  /has no data-testid/,
);

assert.match(
  analyzeSource(
    'repeated-prop.tsx',
    'const view = rows.map((row) => <Button testId="remove-row">Remove</Button>);',
  )[0]?.message ?? '',
  /include item identity/,
);

assert.match(
  analyzeSource(
    'flat-repeated.tsx',
    'const view = rows.flatMap((row) => <button data-testid="remove-row">Remove</button>);',
  )[0]?.message ?? '',
  /include item identity/,
);

assert.match(
  analyzeSource(
    'array-from-repeated.tsx',
    'const view = Array.from({ length: 2 }, (_, index) => <button data-testid="remove-row">Remove</button>);',
  )[0]?.message ?? '',
  /include item identity/,
);

assert.deepEqual(
  analyzeSource(
    'unstable-expression.tsx',
    'const view = rows.map((row) => <button data-testid={sharedTestId}>Remove</button>);',
  ),
  [],
);

assert.match(
  analyzeSource(
    'duplicate-siblings.tsx',
    'const view = <><button data-testid="same">One</button><button data-testid="same">Two</button></>;',
  ).at(-1)?.message ?? '',
  /duplicated in one JSX container/,
);

assert.deepEqual(
  analyzeSource(
    'exclusive-ternary.tsx',
    'const view = <>{flag ? <button data-testid="same">One</button> : <button data-testid="same">Two</button>}</>;',
  ),
  [],
);

assert.match(
  analyzeSource(
    'exclusive-short-circuit.tsx',
    'const view = <>{flag && <button data-testid="same">One</button>}{!flag && <button data-testid="same">Two</button>}</>;',
  ).at(-1)?.message ?? '',
  /duplicated in one JSX container/,
);

assert.match(
  analyzeSource(
    'simultaneous-short-circuit.tsx',
    'const view = <>{canEdit && <button data-testid="same">One</button>}{isAdmin && <button data-testid="same">Two</button>}</>;',
  ).at(-1)?.message ?? '',
  /duplicated in one JSX container/,
);

assert.match(
  analyzeSource(
    'unconditional-and-conditional.tsx',
    'const view = <><button data-testid="same">Always</button>{flag && <button data-testid="same">Sometimes</button>}</>;',
  ).at(-1)?.message ?? '',
  /duplicated in one JSX container/,
);

assert.match(
  analyzeSource(
    'unconditional-and-ternary.tsx',
    'const view = <><button data-testid="same">Always</button>{flag ? <button data-testid="same">Sometimes</button> : null}</>;',
  ).at(-1)?.message ?? '',
  /duplicated in one JSX container/,
);

assert.match(
  analyzeSource(
    'compatible-ternaries.tsx',
    'const view = <>{first ? <button data-testid="same">One</button> : null}{second ? <button data-testid="same">Two</button> : null}</>;',
  ).at(-1)?.message ?? '',
  /duplicated in one JSX container/,
);

assert.match(
  analyzeSource('summary.tsx', 'const view = <summary>Details</summary>;')[0]?.message ?? '',
  /has no data-testid/,
);

assert.deepEqual(
  analyzeSource(
    'unique.tsx',
    'const view = rows.map((row) => <button data-testid={`remove-row-${row.id}`}>Remove</button>);',
  ),
  [],
);

// The custom-component exemption is a stated choice, not an accident (#576
// E2-D1): a capitalized tag outside INTERACTIVE_TAGS is not checked at its call
// site, whatever handler or role it gets; its own definition is checked instead.
assert.deepEqual(analyzeSource('wrapper.tsx', 'const view = <SectionRow onClick={select} />;'), []);
assert.deepEqual(analyzeSource('wrapper-role.tsx', 'const view = <Card role="button" onClick={select}>x</Card>;'), []);
assert.deepEqual(analyzeSource('wrapper-menu.tsx', 'const view = <ToolButton onContextMenu={menu} />;'), []);
// ...while a listed wrapper and a lowercase element with the same props are gated.
assert.match(
  analyzeSource('listed.tsx', 'const view = <Button onClick={save}>Save</Button>;')[0]?.message ?? '',
  /<Button> has no data-testid/,
);
assert.match(
  analyzeSource('div.tsx', 'const view = <div onClick={select}>x</div>;')[0]?.message ?? '',
  /<div> has no data-testid/,
);
assert.match(
  analyzeSource('role.tsx', 'const view = <span role="button">x</span>;')[0]?.message ?? '',
  /<span> has no data-testid/,
);

// scan() walks <repoRoot>/src through the shared lister (scripts/lib/src-files.mjs).
const repoRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-test-id-root-'));
try {
  const src = path.join(repoRoot, 'src');
  fs.mkdirSync(path.join(src, '__tests__'), { recursive: true });
  fs.mkdirSync(path.join(src, 'test'), { recursive: true });
  fs.writeFileSync(path.join(src, 'App.tsx'), 'export const App = () => <button>Run</button>;');
  // Test code is out of scope, in every one of its three shapes.
  fs.writeFileSync(path.join(src, '__tests__', 'App.test.tsx'), 'export const T = () => <button>x</button>;');
  fs.writeFileSync(path.join(src, 'App.spec.tsx'), 'export const S = () => <button>x</button>;');
  fs.writeFileSync(path.join(src, 'test', 'render.tsx'), 'export const R = () => <button>x</button>;');
  let findings = scan(repoRoot);
  assert.deepEqual(findings.map((f) => path.relative(repoRoot, f.fileName)), ['src/App.tsx']);
  assert.match(findings[0].message, /has no data-testid/);

  // A symlinked .tsx used to be read THROUGH the link; it is now reported and
  // fails the gate, the same verdict check-ts-file-size gives it (#576 E16).
  fs.writeFileSync(path.join(repoRoot, 'outside.tsx'), 'export const O = () => <button>x</button>;');
  fs.symlinkSync('../outside.tsx', path.join(src, 'Linked.tsx'));
  findings = scan(repoRoot);
  assert.ok(findings.some((f) => /Linked\.tsx is a symlink/.test(f.message)), JSON.stringify(findings));
  assert.ok(!findings.some((f) => f.fileName.endsWith('outside.tsx')), 'the link target must not be read');
} finally {
  fs.rmSync(repoRoot, { recursive: true, force: true });
}

console.log('Test-ID gate fixtures passed.');
