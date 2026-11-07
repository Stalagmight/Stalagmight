import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { listSrcFiles, symlinkProblem } from './lib/src-files.mjs';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

export const INTERACTIVE_TAGS = new Set([
  'a',
  'button',
  'input',
  'select',
  'textarea',
  'summary',
  'Button',
  'IconButton',
  'MenuRow',
  'Select',
  'Toggle',
  'DatePicker',
  'PositiveNumberField',
  'ShortcutRecorder',
  'PanelOpenerTab',
]);

const INTERACTIVE_ROLES = new Set([
  'button',
  'checkbox',
  'combobox',
  'link',
  'menuitem',
  'option',
  'radio',
  'switch',
  'tab',
]);

const INTERACTION_HANDLERS = new Set([
  'onClick',
  'onContextMenu',
  'onDoubleClick',
]);

function hasTestId(node) {
  return node.attributes.properties.some(
    (attr) => ts.isJsxAttribute(attr) && ['data-testid', 'testId'].includes(attr.name.getText()),
  );
}

function literalAttribute(node, names) {
  const attr = node.attributes.properties.find(
    (candidate) => ts.isJsxAttribute(candidate) && names.includes(candidate.name.getText()),
  );
  if (!attr || !ts.isJsxAttribute(attr) || !attr.initializer) return null;
  if (ts.isStringLiteral(attr.initializer)) return attr.initializer.text;
  if (
    ts.isJsxExpression(attr.initializer) &&
    attr.initializer.expression &&
    ts.isStringLiteral(attr.initializer.expression)
  ) {
    return attr.initializer.expression.text;
  }
  return null;
}

function literalTestId(node) {
  return literalAttribute(node, ['data-testid', 'testId']);
}

function hasAttribute(node, names) {
  return node.attributes.properties.some(
    (attr) => ts.isJsxAttribute(attr) && names.includes(attr.name.getText()),
  );
}

export function isInteractive(node, tag) {
  if (INTERACTIVE_TAGS.has(tag)) return true;
  // Deliberate exemption (#576 E2-D1): a capitalized tag that is NOT in
  // INTERACTIVE_TAGS is a custom component, and its call sites are not checked
  // even with a click handler or an interactive role (`<SectionRow onClick>`,
  // `<Card role="button">`). The gate checks each component's own definition
  // instead - the JSX that finally reaches the DOM - and every such call site
  // audited in #576 renders a tagged inner control (ToolButton -> tool-${tool},
  // SectionRow -> section-row-${id}, ...). Gating call sites would demand a
  // redundant testId on each, or an allowlist row per wrapper. What this cannot
  // see - a wrapper spreading props onto an untagged element - is left to review
  // (docs/TESTING.md, UI component testability). A wrapper that IS a plain
  // control belongs in INTERACTIVE_TAGS. Pinned by check-test-ids.test.mjs.
  if (tag[0] !== tag[0]?.toLowerCase()) return false;
  const role = literalAttribute(node, ['role']);
  if (role && INTERACTIVE_ROLES.has(role)) return true;
  return hasAttribute(node, [...INTERACTION_HANDLERS]);
}

function repeatedCallback(node) {
  for (let current = node.parent; current; current = current.parent) {
    if (
      (ts.isArrowFunction(current) || ts.isFunctionExpression(current)) &&
      ts.isCallExpression(current.parent) &&
      current.parent.arguments.includes(current)
    ) {
      const call = current.parent;
      if (
        ts.isPropertyAccessExpression(call.expression) &&
        (['map', 'flatMap'].includes(call.expression.name.text) ||
          (call.expression.expression.getText() === 'Array' && call.expression.name.text === 'from'))
      ) {
        return current;
      }
    }
    if (ts.isSourceFile(current)) return false;
  }
  return null;
}

function containingJsxParent(node) {
  let current = ts.isJsxOpeningElement(node) ? node.parent.parent : node.parent;
  for (; current; current = current.parent) {
    if (ts.isJsxElement(current) || ts.isJsxFragment(current)) return current;
    if (ts.isSourceFile(current)) return null;
  }
  return null;
}

function conditionalBranches(node) {
  const branches = new Map();
  for (let current = node.parent; current; current = current.parent) {
    if (ts.isConditionalExpression(current)) {
      if (node.pos >= current.whenTrue.pos && node.end <= current.whenTrue.end) branches.set(current, 'true');
      else if (node.pos >= current.whenFalse.pos && node.end <= current.whenFalse.end) branches.set(current, 'false');
    }
    if (ts.isSourceFile(current)) break;
  }
  return branches;
}

function mutuallyExclusive(left, right) {
  const leftBranches = conditionalBranches(left);
  const rightBranches = conditionalBranches(right);
  for (const [conditional, branch] of leftBranches) {
    const otherBranch = rightBranches.get(conditional);
    if (otherBranch && otherBranch !== branch) return true;
  }
  return false;
}

export function analyzeSource(fileName, sourceText) {
  const source = ts.createSourceFile(fileName, sourceText, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const findings = [];
  const literalOccurrences = new Map();

  function visit(node) {
    if (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) {
      const tag = node.tagName.getText(source);
      const line = source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1;
      if (isInteractive(node, tag) && !hasTestId(node)) {
        findings.push({ fileName, line, message: `<${tag}> has no data-testid/testId` });
      }
      const literal = literalTestId(node);
      const callback = repeatedCallback(node);
      if (callback && literal) {
        findings.push({
          fileName,
          line,
          message: `test ID${literal ? ` "${literal}"` : ''} is rendered repeatedly; include item identity`,
        });
      }
      if (literal) {
        const container = containingJsxParent(node);
        const occurrences = literalOccurrences.get(literal) ?? [];
        const conflict = occurrences.some(
          (occurrence) => occurrence.container === container && !mutuallyExclusive(occurrence.node, node),
        );
        if (container && conflict) {
          findings.push({ fileName, line, message: `literal test ID "${literal}" is duplicated in one JSX container` });
        }
        if (container) occurrences.push({ container, node });
        literalOccurrences.set(literal, occurrences);
      }
    }
    ts.forEachChild(node, visit);
  }

  visit(source);
  return findings;
}

// Production .tsx under <repoRoot>/src, via the shared lister (#576 E16): the
// same test/generated exclusions as the other src gates, and a symlink is
// reported as a finding instead of being followed (it used to be read through).
export function scan(repoRoot = REPO_ROOT) {
  const { files, symlinks } = listSrcFiles(repoRoot, { exts: ['.tsx'] });
  const linkFindings = symlinks.map((link) => ({
    fileName: path.join(repoRoot, link.rel),
    line: 0,
    message: symlinkProblem(link),
  }));
  return [
    ...linkFindings,
    ...files.flatMap((rel) => {
      const fileName = path.join(repoRoot, rel);
      return analyzeSource(fileName, fs.readFileSync(fileName, 'utf8'));
    }),
  ];
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const findings = scan();
  if (findings.length > 0) {
    for (const finding of findings) {
      console.error(`${path.relative(REPO_ROOT, finding.fileName)}:${finding.line}: ${finding.message}`);
    }
    console.error(`Test-ID gate failed with ${findings.length} finding(s).`);
    process.exitCode = 1;
  } else {
    console.log('Test-ID gate passed.');
  }
}
