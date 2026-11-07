// Fixture tests for the dialog-layer i18n literal gate (scripts/check-i18n-literals.mjs).
//
// A gate that never fires proves nothing, so each shape of hard-coded UI text
// is pinned with a planted literal it MUST report, and each near miss (a t()
// key, a className, a punctuation-only string, a placeholder-only template)
// with one it must NOT. The last tests run the whole scan over a throwaway tree
// on disk, so the scope derivation (dialogs + the common modules they import +
// the pinned common modules), the allowlist (covering and stale rows) and the
// exit code are covered too.
//
// Pure Node, no test framework (mirrors the sibling scripts/check-*.test.mjs).
// Run: node scripts/check-i18n-literals.test.mjs  (pnpm lint:i18n-literals:test; CI check job)

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import {
  analyzeSource,
  applyAllowlist,
  hasLetters,
  scopeFiles,
  main,
  ALLOWLIST_FILE,
  PINNED_COMMON,
} from './check-i18n-literals.mjs';

let passed = 0;
const failures = [];
function test(name, fn) {
  try {
    fn();
    passed++;
  } catch (e) {
    failures.push(`${name}: ${e.message}`);
  }
}

const texts = (code) => analyzeSource('src/components/Dialogs/Fixture.tsx', code).map((f) => f.text);
const kinds = (code) => analyzeSource('src/components/Dialogs/Fixture.tsx', code).map((f) => f.kind);

// --- must report ----------------------------------------------------------------

test('JSX text is reported, whitespace-normalised', () => {
  assert.deepEqual(texts('const v = <p>\n  Delete this\n  shot?\n</p>;'), ['Delete this shot?']);
});

test('strings in a JSX child expression are followed through ?:, &&, ??, || and +', () => {
  const code = `const v = <div>
    {busy ? 'Saving...' : 'Save'}
    {err && 'Failed'}
    {msg ?? 'An error occurred.'}
    {name || 'Unnamed'}
    {'Total: ' + n}
    {\`Shot \${n} of \${m}\`}
  </div>;`;
  assert.deepEqual(texts(code), ['Saving...', 'Save', 'Failed', 'An error occurred.', 'Unnamed', 'Total:', 'Shot ${n} of ${m}']);
});

test('user-facing attributes are reported, literal or expression', () => {
  const code = `const v = <>
    <DialogWrapper title="Edit Shot" />
    <input placeholder={'Optional'} aria-label={\`Close \${name}\`} />
    <img alt="Map" />
    <Button title={busy ? 'Working' : 'Go'} />
  </>;`;
  assert.deepEqual(texts(code), ['Edit Shot', 'Optional', 'Close ${name}', 'Map', 'Working', 'Go']);
  assert.ok(kinds(code).every((k) => k.startsWith('attr ')));
});

test('user-facing properties in object literals are reported (dialog payloads, option lists)', () => {
  const code = `openDialog('error', { title: 'Delete failed', message: \`Could not delete \${x}\` });
    const opts = [{ value: 'a', label: 'By section' }];`;
  assert.deepEqual(texts(code), ['Delete failed', 'Could not delete ${x}', 'By section']);
});

test('error / message setters are reported (the validation-error shape)', () => {
  const code = `setError('Length must be greater than 0');
    setDeleteError(ok ? 'Failed to delete shot' : 'Failed');
    setStatusMessage('Saved');`;
  assert.deepEqual(texts(code), ['Length must be greater than 0', 'Failed to delete shot', 'Failed', 'Saved']);
});

// [#576 RV5-LS-1] Text handed to a local callback through a message-named parameter,
// and a map of messages, were invisible: the DMP wizard's failure text shipped in
// English past the zero gate.
test('a literal passed to a local callback through a message-named parameter is reported, the other arguments are not', () => {
  const code = `const fail = useCallback((kind: Step, message: string) => setFailure({ kind, message }), []);
    function report(reason: string) { setError(reason); }
    const warn = (code: string, text: string) => code + text;
    fail('segment', 'Segment import failed: unresolved relative placement target');
    report('Could not read the file');
    warn('w-1', \`Removed \${n} shots\`);
    fail('segment', t('dialogs.x.failed'));`;
  assert.deepEqual(texts(code), [
    'Segment import failed: unresolved relative placement target',
    'Could not read the file',
    'Removed ${n} shots',
  ]);
});

test('a map or constant named as text (..._TEXT, ...Message, ...Label) is reported where it is declared', () => {
  const code = `const FALLBACK_TEXT: Record<'anchor' | 'early', string> = {
      anchor: 'Failed to import zero datum section',
      early: 'Import failed',
    };
    const emptyMessage = 'Nothing to show';
    const STEP_LABELS = ['Choose file', 'Review'];
    const KIND_CLASS = { anchor: 'bg-red-500' };
    const FALLBACK_KEY = 'dialogs.x.fallback';`;
  assert.deepEqual(texts(code), ['Failed to import zero datum section', 'Import failed', 'Nothing to show', 'Choose file', 'Review']);
});

// [#576 RV5-LS-2] Ordinary code shapes that rendered a literal past the gate.
test('a constant or template held in a variable and rendered in JSX is reported', () => {
  const code = `const HEADING = 'Delete every section';
    function Probe({ n }: { n: number }) {
      const msg = \`Removed \${n} shots\`;
      let note = 'Read only';
      return <div><h2>{HEADING}</h2><p>{msg}</p><p title={note}>{busy ? note : HEADING}</p></div>;
    }`;
  assert.deepEqual(texts(code), ['Delete every section', 'Removed ${n} shots', 'Read only']);
});

test('a literal reached through a map lookup in JSX, or wrapped in String(), is reported', () => {
  const code = `const NAMES = { a: 'First choice', b: 'Second choice' };
    const v = <div>{NAMES[kind]}{NAMES.a}{String('Remove all')}</div>;`;
  assert.deepEqual(texts(code), ['First choice', 'Second choice', 'Remove all']);
});

test('the message of a showToast() is reported, its dedup key and kind are not', () => {
  const code = `showToast('Shot deleted from the survey', 'shot-deleted', 'success');
    useUIStore.getState().showToast(\`Saved \${name}\`, 'saved', 'info');
    showToast(t('toasts.saved'), 'saved', 'info');`;
  assert.deepEqual(texts(code), ['Shot deleted from the survey', 'Saved ${name}']);
});

test('a binding that never reaches a sink, or reaches only a non-text one, is not reported', () => {
  const code = `const cls = 'p-4 text-sm';
    const key = 'dialog-key';
    const label = t('dialogs.x.label');
    const v = <div className={cls} data-testid={key}>{label}</div>;
    fail(kind, label);`;
  assert.deepEqual(texts(code), []);
});

// [#576 RV6a-3] The binding and local-callback rules reached ordinary non-UI code.
// Each shape below is a code token or a debug trace, never shown to the user.
test('a binding named copy/reason/label that holds code tokens is not reported', () => {
  const code = `const copy = { ...o, mode: 'edit' };
    const reason = t ? 'timeout' : 'user';
    const label = 'section-' + id;
    const cause = layer.getDisabledReason() ?? 'unknown';
    const KIND_LABEL = { a: 'closureAdjusted', b: 'shot_kind' };`;
  assert.deepEqual(texts(code), []);
});

test('a message-named parameter that only reaches console.* is not reported', () => {
  const code = `const warn = (message: string) => console.warn(message);
    function trace(text: string) { console.debug('[dmp]', text); console.error(\`at \${text}\`); }
    warn('dev only trace');
    trace('Segment import step started');`;
  assert.deepEqual(texts(code), []);
});

test('the same shapes still report when the text reaches the user', () => {
  const code = `const reason = t ? 'timeout' : 'user';
    const STEP_LABEL = 'Review';
    const warn = (message: string) => { console.warn(message); setError(message); };
    warn('Could not read the file');
    const v = <p title={reason}>{STEP_LABEL}</p>;`;
  assert.deepEqual(texts(code), ['Review', 'Could not read the file', 'timeout', 'user']);
});

// --- must NOT report --------------------------------------------------------------

test('t() calls and their keys are never reported, in any position', () => {
  const code = `const v = <div title={t('dialogs.x.title')}>
    {t('dialogs.x.body', { n })}
    {busy ? t('common.actions.saving') : t('common.actions.save')}
    <Trans i18nKey="dialogs.x.rich" components={{ b: <b /> }} />
  </div>;
  setError(t('dialogs.x.error'));
  openDialog('error', { title: t('dialogs.x.failed') });`;
  assert.deepEqual(texts(code), []);
});

test('non-user-facing attributes and strings are not reported', () => {
  const code = `const v = <div className="p-4 text-sm" data-testid="edit-shot" role="dialog" variant="danger" type="button" />;
    const key = 'some-key';
    trackDialogComplete('error');
    throw new Error('internal');
    setSelection({ type: 'station' });`;
  assert.deepEqual(texts(code), []);
});

test('text without a letter is not reported (punctuation, numbers, entities, placeholder-only templates)', () => {
  const code = `const v = <span>
    {' '}{' · '}&times;{'/'}{\`\${a}/\${b}\`}{\`\${n}%\`} 42 &rarr; &#x26A0;
  </span>;`;
  assert.deepEqual(texts(code), []);
  assert.equal(hasLetters('&rarr;'), false);
  assert.equal(hasLetters('${count} m'), true);
});

// --- allowlist --------------------------------------------------------------------

test('an allowlist row covers every occurrence of its text in its file only; unused rows are stale', () => {
  const findings = [
    { file: 'a.tsx', line: 1, kind: 'jsx-text', text: 'PNG' },
    { file: 'a.tsx', line: 9, kind: 'jsx-text', text: 'PNG' },
    { file: 'b.tsx', line: 2, kind: 'jsx-text', text: 'PNG' },
  ];
  const rows = [
    { file: 'a.tsx', text: 'PNG', reason: 'format name' },
    { file: 'a.tsx', text: 'SVG', reason: 'format name' },
  ];
  const { violations, stale } = applyAllowlist(findings, rows);
  assert.deepEqual(violations.map((v) => v.file), ['b.tsx']);
  assert.deepEqual(stale.map((s) => s.text), ['SVG']);
});

// --- the whole scan on a throwaway tree ---------------------------------------------

// Every tree holds the PINNED_COMMON files (clean) unless a test overrides one;
// `null` leaves it out.
function tree(files) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'i18n-literals-'));
  const pinned = Object.fromEntries(PINNED_COMMON.map((rel) => [rel, 'export {};\n']));
  for (const [rel, text] of Object.entries({ ...pinned, ...files })) {
    if (text === null) continue;
    fs.mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
    fs.writeFileSync(path.join(root, rel), text);
  }
  return root;
}

const quiet = (fn) => {
  const { log, error } = console;
  console.log = () => {};
  console.error = () => {};
  try {
    return fn();
  } finally {
    console.log = log;
    console.error = error;
  }
};

const ALLOW = (entries) => JSON.stringify({ entries });

test('scope: every dialog file plus the common modules a dialog imports and the pinned ones, tests excluded', () => {
  const root = tree({
    'src/components/Dialogs/A.tsx': "import { Button } from '../common/Button';\nexport const A = () => <Button />;\n",
    'src/components/Dialogs/sub/B.tsx': "import X from '../../common/Grid';\nexport const B = X;\n",
    'src/components/Dialogs/__tests__/A.test.tsx': 'const v = <p>Test text</p>;\n',
    'src/components/Dialogs/A.test.tsx': 'const v = <p>Test text</p>;\n',
    'src/components/common/Button.tsx': 'export const Button = () => null;\n',
    'src/components/common/Grid/index.tsx': 'export default 1;\n',
    'src/components/common/Unused.tsx': 'const v = <p>Not a dialog primitive</p>;\n',
    [ALLOWLIST_FILE]: ALLOW([]),
  });
  assert.deepEqual(scopeFiles(root).files, [
    'src/components/Dialogs/A.tsx',
    'src/components/Dialogs/sub/B.tsx',
    'src/components/common/Button.tsx',
    'src/components/common/ContextMenu.tsx',
    'src/components/common/DetailLink.tsx',
    'src/components/common/ErrorBoundary.tsx',
    'src/components/common/Grid/index.tsx',
  ]);
  assert.deepEqual(scopeFiles(root).missingPinned, []);
});

test('pinned: the context menu, error boundary and entity link are in scope with no dialog importing them', () => {
  assert.deepEqual(PINNED_COMMON, [
    'src/components/common/ContextMenu.tsx',
    'src/components/common/DetailLink.tsx',
    'src/components/common/ErrorBoundary.tsx',
  ]);
  const base = {
    'src/components/Dialogs/A.tsx': 'export const A = () => null;\n',
    [ALLOWLIST_FILE]: ALLOW([]),
  };
  assert.equal(quiet(() => main(tree(base))), 0);
  for (const [rel, code] of [
    ['src/components/common/ContextMenu.tsx', "const items = [{ label: 'Edit Station', action }];\n"],
    ['src/components/common/DetailLink.tsx', 'const v = <button title={`${type} not found`} />;\n'],
    ['src/components/common/ErrorBoundary.tsx', 'const v = <button>Try Again</button>;\n'],
  ]) {
    assert.equal(quiet(() => main(tree({ ...base, [rel]: code }))), 1, rel);
  }
});

test('pinned: a pinned file that no longer exists fails the gate (a rename must update the list)', () => {
  const root = tree({
    'src/components/Dialogs/A.tsx': 'export const A = () => null;\n',
    'src/components/common/DetailLink.tsx': null,
    [ALLOWLIST_FILE]: ALLOW([]),
  });
  assert.deepEqual(scopeFiles(root).missingPinned, ['src/components/common/DetailLink.tsx']);
  assert.equal(quiet(() => main(root)), 1);
});

test('main: a literal in a dialog or in an imported primitive fails; t() and allowlisted text pass', () => {
  const clean = {
    'src/components/Dialogs/A.tsx':
      "import { Button } from '../common/Button';\nexport const A = () => <Button title={t('x.title')}>PNG</Button>;\n",
    'src/components/common/Button.tsx': 'export const Button = () => <button>{children}</button>;\n',
    [ALLOWLIST_FILE]: ALLOW([{ file: 'src/components/Dialogs/A.tsx', text: 'PNG', reason: 'format name' }]),
  };
  assert.equal(quiet(() => main(tree(clean))), 0);
  assert.equal(quiet(() => main(tree({ ...clean, 'src/components/Dialogs/C.tsx': 'const v = <p>Delete</p>;\n' }))), 1);
  const primitive = { ...clean, 'src/components/common/Button.tsx': 'export const Button = () => <button aria-label="Close" />;\n' };
  assert.equal(quiet(() => main(tree(primitive))), 1);
});

test('main: a stale allowlist row fails, and so does a row without a reason', () => {
  const base = { 'src/components/Dialogs/A.tsx': 'export const A = () => null;\n' };
  const stale = { ...base, [ALLOWLIST_FILE]: ALLOW([{ file: 'src/components/Dialogs/A.tsx', text: 'PNG', reason: 'format name' }]) };
  assert.equal(quiet(() => main(tree(stale))), 1);
  const reasonless = {
    'src/components/Dialogs/A.tsx': 'const v = <b>PNG</b>;\n',
    [ALLOWLIST_FILE]: ALLOW([{ file: 'src/components/Dialogs/A.tsx', text: 'PNG', reason: '' }]),
  };
  assert.equal(quiet(() => main(tree(reasonless))), 1);
});

if (failures.length > 0) {
  console.error(`\n✗ check-i18n-literals fixtures: ${failures.length} failed, ${passed} passed\n`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log(`✓ check-i18n-literals fixtures: ${passed} passed`);
