// Canonical read-only gating tripwire (#275).
//
// The correct "is the survey map read-only right now" predicate is
// `useEditingBlocked()` / `isEditingBlockedNow()` (src/hooks/useEditingBlocked.ts):
// `isMapReadOnly(state)` (geometryMode === 'closureAdjusted' || appMode !== 'survey')
// composed with `selectSpeleoReadOnly` for a locked remote SpeleoDB project. Two
// recurring bug classes keep re-appearing at individual call sites:
//
//   (A) a RAW `geometryMode === 'closureAdjusted'` comparison used as a mutation
//       gate. This is wrong whenever appMode !== 'survey': Plan/Export force
//       closureAdjusted, but that auto-reverts to 'measured' on a project with no
//       eligible closures - so a geometryMode-only check silently reports
//       "editable" in a workspace that must be read-only (#243 / the #225 bug
//       class). `DataTable.tsx`, `SectionsPanel.tsx`, `WarningResolution.tsx`,
//       `DetailPanel.tsx`, and others all had exactly this bug; fixed in #275.
//
//   (B) a call site that reaches for the raw `isMapReadOnly` / `selectSpeleoReadOnly`
//       symbols directly instead of going through the shared `useEditingBlocked()` /
//       `isEditingBlockedNow()` helper. The prior generation of call sites wrote the
//       composition inline as `useUIStore(isMapReadOnly) || useSpeleoStore(selectSpeleoReadOnly)`,
//       which is a Rules-of-Hooks violation: `||` short-circuits, so the second
//       zustand hook is skipped whenever the first term is already `true`, and React
//       throws "Rendered fewer hooks than expected" if that term flips back to
//       `false` on a later render. Routing every call site through the shared helper
//       (which calls both hooks unconditionally) makes the bug structurally
//       impossible to reintroduce.
//
// Note: Toolbar/keyboard Undo-Redo history controls are DELIBERATELY gated on
// `appMode` alone, not the full editing-blocked predicate - the backend toast
// already handles a locked/read-only project rejecting an undo/redo write (see the
// #243 comment at useKeyboardShortcuts.ts:571-574). That's out of this lint's scope
// by design; it is not a raw closureAdjusted comparison and doesn't reference
// isMapReadOnly/selectSpeleoReadOnly, so it never trips either rule below.
//
// This script is a curated allowlist, not a mechanical LOC ratchet (#157's
// pattern) - the legitimate raw-comparison sites (display/coordinate-selection
// logic, the isMapReadOnly definition itself, the geometry-mode radio control)
// are enumerated by hand below, each with why it's fine. Anything beyond that
// baseline count, in a file already on the list or a brand new file, fails the
// gate. Rule (B) similarly allowlists direct isMapReadOnly/selectSpeleoReadOnly
// references that are reviewed and legitimate (definitions, the helper itself,
// and a couple of pre-existing RoH-safe call sites out of the #275 blocker scope).
//
// Usage:  node scripts/check-readonly-gates.mjs   (or: pnpm lint:readonly-gates)

import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { listSrcFiles, symlinkProblem } from './lib/src-files.mjs';
import { stripComments } from './lib/js-comments.mjs';

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// Rule (A): file -> EXACT allowed count of a raw `=== 'closureAdjusted'` /
// `!== 'closureAdjusted'` comparison (either operand order, i.e. including the
// Yoda form `'closureAdjusted' === x`), or `case 'closureAdjusted':` label,
// counted per MATCH (two on one line count twice), outside comments. Each
// entry is a reviewed, legitimate
// non-mutation usage (display/coordinate-selection, or the canonical
// definition) - not a mutation gate that should instead compose
// useEditingBlocked() / isEditingBlockedNow().
// Pinned EXACT (not a ceiling): a count that drops below its baseline is just as
// suspicious as one that grows past it - it likely means a gate got deleted
// rather than migrated, so keep the allowlist in sync in both directions.
export const CLOSURE_ADJUSTED_ALLOWLIST = {
  // The geometry-mode radio control itself: this IS the setting, not a gate.
  'src/components/SingleSystemView/DisplayMenu.tsx': 2,
  // "Switch and continue" FIX path (#407 follow-up): decides whether the
  // restored/current view still needs dropping to measured before the
  // confirmed import re-routes - a display-mode write, not a mutation gate.
  // The gate that blocks the import is isEditingBlockedNow() in
  // routeOpenedFile; this code runs only after the user confirmed the switch.
  'src/components/Dialogs/ImportBlockedDialog.tsx': 1,
  // Adjusted-geometry data-fetch trigger, the auto-revert-to-measured logic (the
  // very mechanism the appMode trap above exists because of), and the read-only
  // banner display - none are mutation gates.
  'src/components/Map/MapCanvas.tsx': 4,
  // Guarded by an outer `appMode !== 'survey'` branch a few lines up, so the raw
  // geometryMode check is already scoped to appMode === 'survey' - equivalent to
  // isMapReadOnly there.
  'src/components/common/ContextMenu.tsx': 1,
  // Coordinate-set selection (which station/survey-data array to render), not a
  // mutation gate.
  'src/store/geometrySelectors.ts': 2,
  // The canonical definition of isMapReadOnly, plus ONE settings-value sanitizer
  // (validating a persisted enum on the load path), plus
  // showsClosureAdjustedNotice - which decides whether to DRAW the
  // closure-adjusted badge, not whether an edit may proceed. It is deliberately
  // narrower than isMapReadOnly (Survey only, and only when the SpeleoDB badge
  // is not already explaining the read-only state), so it cannot be expressed as
  // that predicate. None are mutation gates.
  //
  // 4 -> 3: the second sanitizer lived in `loadDisplaySettings`, which had no
  // production caller (its last one went in f4e490d2) and was deleted. The
  // surviving sanitizer is in `applyDisplaySettings`, which IS the load path.
  // Recorded rather than silently rebaselined because this gate fails on a DROP
  // as well as a rise, precisely so a deleted guard cannot pass as tidying - the
  // three remaining comparisons were each re-read to confirm none is a mutation
  // gate before this number was changed.
  'src/store/uiStore.ts': 3,
  // The import-blocked error message picks one of several strings depending on
  // which read-only cause is active (closure-adjusted vs Plan/Export vs a
  // locked remote project); the actual guard is isEditingBlockedNow(), this
  // read is purely for message selection, gated behind that guard already
  // having fired.
  'src/utils/openOrImport.ts': 1,
};

// Rule (B): files allowed to reference the raw `isMapReadOnly` / `selectSpeleoReadOnly`
// symbols directly instead of going through `useEditingBlocked()` / `isEditingBlockedNow()`
// (src/hooks/useEditingBlocked.ts). Every entry needs a written reason - this is
// meant to stay tiny. The store modules that DEFINE the two symbols, and the
// helper module itself, are exempt unconditionally (they have no choice but to
// reference their own exports) and are not listed here.
export const DIRECT_READ_ONLY_ACCESS_DEFINITIONS = new Set([
  'src/store/uiStore.ts',
  'src/store/speleoStore.ts',
  'src/hooks/useEditingBlocked.ts',
]);

export const DIRECT_READ_ONLY_ACCESS_EXEMPT = {
  // Reads selectSpeleoReadOnly separately to choose which read-only cause to
  // explain in the import-blocked dialog message; the actual guard is
  // isEditingBlockedNow() (see CLOSURE_ADJUSTED_ALLOWLIST entry above).
  'src/utils/openOrImport.ts':
    'message-selection only - the guard itself is isEditingBlockedNow()',
  // Already composes isMapReadOnly(...) with a separately-hooked speleoReadOnly
  // without a Rules-of-Hooks violation (both terms come from unconditional hook
  // calls, mirroring the pattern useEditingBlocked() now centralizes); out of the
  // #275 blocker scope (isMapReadOnly is called as a plain function here, not as
  // a hook selector, so migrating it isn't a mechanical swap).
  'src/components/Map/MapCanvas.tsx':
    'pre-existing RoH-safe composition, out of #275 blocker scope',
  // Already composes a raw geometryMode comparison (see CLOSURE_ADJUSTED_ALLOWLIST
  // above) with a separately-hooked speleoReadOnly without a Rules-of-Hooks
  // violation; out of the #275 blocker scope.
  'src/components/common/ContextMenu.tsx':
    'pre-existing RoH-safe composition, out of #275 blocker scope',
};

// Global: every match on a line counts (#576 E2-D2 - counting LINES let a second
// raw gate ride along on an already-allowlisted line). A `case 'closureAdjusted':`
// label is the same comparison spelled as a switch, so it counts too.
// A template literal (`closureAdjusted` in backticks) is the same comparison
// (#576 RV3-FE-12: it used to pass uncounted).
const CLOSURE_ADJUSTED_RE =
  /(===|!==)\s*(['"`])closureAdjusted\2|(['"`])closureAdjusted\3\s*(===|!==)|\bcase\s+(['"`])closureAdjusted\5\s*:/g;
const RAW_IS_MAP_READ_ONLY_RE = /\bisMapReadOnly\b/;
const RAW_SELECT_SPELEO_READ_ONLY_RE = /\bselectSpeleoReadOnly\b/;

// Comments are stripped before matching, so a comment that merely MENTIONS the
// pattern (common in this file's own docs, or explanatory code comments)
// doesn't count as a real comparison. The shared scanner
// (scripts/lib/js-comments.mjs) is string-aware - a `//` inside a string
// literal (e.g. a URL like "http://example.com") is NOT the start of a comment,
// so a real comparison later on the line is not lost - and, unlike the old
// line-only stripper here, it also removes `/* ... */` block comments, carrying
// them across lines (#576 E2-D2: a block-comment mention used to count).
// `stripLineComment` keeps its one-line contract for the fixture test.
export function stripLineComment(line) {
  return stripComments([line])[0];
}

// Count of raw `=== 'closureAdjusted'` / `!== 'closureAdjusted'` comparisons
// (either operand order) and `case 'closureAdjusted':` labels in the file's
// code - every match, not every matching line.
export function countClosureAdjustedComparisons(text) {
  let count = 0;
  for (const line of stripComments(text.split('\n'))) {
    count += (line.match(CLOSURE_ADJUSTED_RE) ?? []).length;
  }
  return count;
}

// Rule (B) boolean check: does any CODE line reference the raw symbol? Comments
// are stripped first (same treatment as Rule (A)) so a doc comment that merely
// mentions isMapReadOnly/selectSpeleoReadOnly can't trip the gate.
function anyCodeLineMatches(text, re) {
  return stripComments(text.split('\n')).some((line) => re.test(line));
}

export function usesIsMapReadOnly(text) {
  return anyCodeLineMatches(text, RAW_IS_MAP_READ_ONLY_RE);
}

export function usesSpeleoReadOnly(text) {
  return anyCodeLineMatches(text, RAW_SELECT_SPELEO_READ_ONLY_RE);
}

function main() {
  // The shared src/ lister (#576 E16): a filesystem walk (an unstaged file
  // cannot dodge the gate), the same test/generated exclusions as the other
  // src gates, and a symlink is reported instead of being read through.
  const { files, symlinks } = listSrcFiles(REPO_ROOT);

  const violationsA = [];
  const violationsB = [];

  for (const rel of files) {
    const full = path.join(REPO_ROOT, rel);
    let text;
    try {
      text = fs.readFileSync(full, 'utf8');
    } catch (e) {
      console.error(`check-readonly-gates: could not read ${rel}: ${e.code || e.message}`);
      return 1;
    }

    const count = countClosureAdjustedComparisons(text);
    const allowed = CLOSURE_ADJUSTED_ALLOWLIST[rel] ?? 0;
    if (count !== allowed) {
      violationsA.push(
        `${rel}: ${count} raw \`=== 'closureAdjusted'\` comparison(s) (allowlisted: ${allowed}, exact match required). ` +
          `If this is a genuine mutation gate, use useEditingBlocked() (in a component) or isEditingBlockedNow() (outside React) ` +
          `instead - see docs comment at the top of scripts/check-readonly-gates.mjs. If it is a ` +
          `reviewed display/coordinate-selection usage, update the count in CLOSURE_ADJUSTED_ALLOWLIST there ` +
          `(a count BELOW baseline likely means a gate was deleted rather than migrated - check before lowering it).`,
      );
    }

    const isDirectAccessFile = usesIsMapReadOnly(text) || usesSpeleoReadOnly(text);
    if (isDirectAccessFile && !DIRECT_READ_ONLY_ACCESS_DEFINITIONS.has(rel) && !(rel in DIRECT_READ_ONLY_ACCESS_EXEMPT)) {
      violationsB.push(
        `${rel}: references isMapReadOnly or selectSpeleoReadOnly directly instead of the shared ` +
          `useEditingBlocked() / isEditingBlockedNow() helper (src/hooks/useEditingBlocked.ts). ` +
          `Composing the two store hooks inline is a Rules-of-Hooks violation (\`||\` short-circuits the ` +
          `second hook call). Use \`useEditingBlocked()\` in a component or \`isEditingBlockedNow()\` ` +
          `outside React, or add the file to DIRECT_READ_ONLY_ACCESS_EXEMPT with a written reason if it ` +
          `genuinely needs the raw symbol.`,
      );
    }
  }

  const violations = [...symlinks.map(symlinkProblem), ...violationsA, ...violationsB];
  if (violations.length === 0) {
    console.log(`✓ readonly-gates: ${files.length} production TS/TSX files under src/ checked, no new raw closureAdjusted gates or direct isMapReadOnly/selectSpeleoReadOnly access.`);
    return 0;
  }

  console.error(`\n✗ readonly-gates check failed - ${violations.length} issue(s):\n`);
  for (const v of violations) console.error(`  ${v}`);
  console.error('');
  return 1;
}

const isMainModule =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMainModule) process.exit(main());
