// Private-reference leak guard for the public tree (issue #226).
//
// The repo went public under AGPL-3.0-or-later. The de-internalization sweep
// (PR #229) scrubbed maintainer names, private cloud-build infra, personal home
// paths, and internal planning/coordination docs out of the tracked tree. This
// guard stops them creeping back in - a leak committed to PUBLIC git history is
// expensive to undo, and `docs/plans/` already re-accumulates (a planning doc was
// re-added the day after #229 removed the directory).
//
// It scans every git-TRACKED text file (`git ls-files` - exactly the set that
// gets published), and every tracked PATH string - binaries and vendored files
// included, since a file named after a person is published too (#576 E18(e)) -
// for a small set of high-signal markers:
//   - maintainer names / handles. These also catch a name-bearing personal path
//     (`/home/daniel`, `/Users/ndh`). Suppressed only in ROOT attribution files
//     (README, CONTRIBUTING, AUTHORS, LICENSE, CHANGELOG, SECURITY, …), where
//     public credit is legitimate - NOT in nested code-adjacent READMEs.
//   - the private cloud-build provider + its env-var prefix + the SSH-key alias
//     + the private build host's rsync target.
//   - the private planning/coordination doc + tool filenames.
//   - the maintainer's private cave-repo path segment (`~/src/cave/...`).
//   - the private issue-tracker references.
//
// WHERE IT RUNS - its own workflow (.github/workflows/leak-scan.yml) with NO
// path filter. This is load-bearing: the `check` job and the `smoke` job BOTH
// path-ignore `**/*.md` and `docs/**`, which is exactly where a leak lands (a
// `docs/plans/*.md` change runs NEITHER today). So a gate wired into either of
// those would be a no-op for the riskiest files. Mirrors the placement reasoning
// of scripts/check-e2e-parallel-safe.mjs (it lives in `smoke` because `check`
// ignores e2e-playwright/**).
//
// This is a heuristic tripwire for the common, accidental re-introduction - not a
// secret scanner or a guarantee. DELIBERATE coverage choices / accepted gaps:
//   - It targets maintainer-NAME-bearing paths, not generic build-box paths
//     (`/home/ubuntu`, `/home/runner`): those carry no identity and appear all
//     over CI docs, so a generic `/home/<x>` rule was tried and dropped (it
//     false-fired on `/home/v1.0` in a unit test).
//   - A name adjacent to letters (`Danielle`, `unicode`) does NOT fire, but a
//     name adjacent to `_`/`/`/digits (`daniel_saska`, `/home/daniel`) does. The
//     short `dan` nickname is intentionally NOT a marker (too false-positive
//     prone); `daniel` covers the full form.
//   - The doc/tool markers substring-match on purpose (to catch them inside
//     paths like `~/personaldocs/x`); they are distinctive enough that this is
//     safe.
//   - It matches literal markers, so a novel paraphrase ("the maintainer in
//     Prague") slips; it skips binary files and the vendored crate; and it cannot
//     judge intent. A file it CANNOT read, or that exceeds the size cap, is NOT a
//     silent skip - it FAILS the gate (fail-closed), so a marker can't be smuggled
//     past the scan inside a file the scanner declines to read (#232, Copilot).
// Block on a hit; allow-list a verified-benign match with a written reason.
//
// Usage:  node scripts/check-private-leaks.mjs   (or: pnpm lint:private-leaks)

import * as path from 'path';
import { fileURLToPath } from 'url';
import {
  MAX_SCAN_BYTES, hasBinaryExt, looksBinary, listTracked, makeReader, asEntry, toScanText,
} from './lib/tracked-files.mjs';

// Re-exported for the fixture test (the helper now lives in lib/).
export { looksBinary };

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// Files at the REPO ROOT where naming a maintainer is legitimate public
// attribution, so the `maintainer-name` rule is suppressed there (every OTHER
// rule still applies). Root-only on purpose: a nested code-adjacent README
// (src/.../README.md) is NOT an attribution file and stays fully scanned.
const ATTRIBUTION_BASENAMES = new Set([
  'readme.md', 'contributing.md', 'security.md', 'code_of_conduct.md',
  'authors', 'authors.md', 'contributors', 'contributors.md',
  'license', 'license.md', 'license-mit', 'license-apache', 'notice',
  'changelog.md', 'codeowners',
  // Generated verbatim third-party license texts: upstream copyright lines
  // legitimately contain first names that collide with the maintainer-name
  // markers. Every other rule still scans the file.
  'third-party-licenses.md',
]);

// One rule per marker. `re` MUST carry the `g` flag (scanned with exec-in-loop).
// `exemptAttribution` suppresses the rule in the root attribution files above.
export const RULES = [
  {
    id: 'maintainer-name',
    // Long names use letter-lookaround (not `\b`), so `_`/digit-adjacent forms
    // (`daniel_saska`, `nico2`) and path forms (`/home/daniel`) fire, while a
    // letter-adjacent lookalike (`Danielle`, `unicode`→`nico`) does NOT. `ndhailey`
    // (the longer alt) claims that token before the short `ndh` branch can. `ndh`
    // (the maintainer's home-dir name) is path-anchored - `/Users/ndh` fires, a
    // standalone `ndh` token does not - to avoid false-firing on an incidental
    // 3-char token.
    re: /(?<![A-Za-z])(nico|daniel|saska|ndhailey)(?![A-Za-z])|(?<=[/\\])ndh(?![A-Za-z0-9])/gi,
    exemptAttribution: true,
    why: 'maintainer name/handle in a non-attribution file - internal-coordination leak. '
      + 'Use role-neutral phrasing or an issue link; public credit belongs in AUTHORS/README/CHANGELOG.',
  },
  {
    id: 'private-cloud-infra',
    // The private cloud-build provider, its env-var prefix (any trailing char,
    // incl. digit-led names), the SSH-key alias, and the build host's rsync
    // target (`cog:~/…` / `cog:/…`).
    re: /\bscaleway\b|\bSCW_|\banzu\b|\bcog:[~/]/gi,
    why: 'private cloud-build infra reference (provider / SCW_ env var / SSH-key alias / build-host rsync target) - '
      + 'these live in the maintainer-only private repo, not the public tree.',
  },
  {
    id: 'private-doc-or-tool',
    // Filenames of the private coordination/planning docs and personal tool dirs.
    // Substring-matched on purpose (catches them inside paths); each is distinctive.
    re: /personaldocs|workingnotes|stalagmight-inflight|stalagmight-project-notes|coord_board|ns-CLAUDE|stag-private|nico_sm_prioritized|\bnuc-i5\b/gi,
    why: 'reference to a private/personal coordination doc or tool path - never published.',
  },
  {
    id: 'private-cave-path',
    // The maintainer's local cave-repo tree (`~/src/cave/...`, `/home/x/src/cave/...`,
    // `...\src\cave\...`). Path-anchored by construction: the `src`/`cave` separator IS
    // the path context, so prose about a cave and paths like `src/cavern.ts` or
    // `src/caves/` do NOT fire. No home-prefix is required - a home-joined relative
    // form (`join("src/cave/data/...")`) leaks the same tree.
    re: /(?<![A-Za-z0-9_])src[/\\]cave(?![A-Za-z0-9_])/gi,
    why: 'personal cave-repo path (`~/src/cave/...`) - a maintainer-local checkout that no '
      + 'contributor has. Describe the dataset/checkout instead of naming the home path.',
  },
  {
    id: 'private-tracker',
    // The private issue tracker (avoid bare "plane" - geometry uses the word).
    re: /\bplane\.so\b|\bplane[ -]id\b/gi,
    why: 'private issue-tracker reference (Plane) - track work in GitHub issues for the public repo.',
  },
];

// Verified-benign matches: {file (repo-relative POSIX), ruleId, reason}. Empty by
// design - the functional self-hosted-runner tokens kept in ci.yml
// (`i5-nuc-runner`, `NUC_RUNNER_READ_PAT`) are NOT matched by any rule above
// (the `nuc-i5` marker is the reversed personal tool-dir name), and a regression
// test pins that. Add a row here only for a real, justified exception.
export const ALLOW = [];

// MAX_SCAN_BYTES (5 MB) and BINARY_EXT come from scripts/lib/tracked-files.mjs,
// shared with the dash guard so the two cannot drift. An oversized file FAILS
// the gate. Text formats that can carry a leak - `.svg` (title/desc/comments),
// `.snap` (serialized DOM/paths) - are deliberately NOT binary, so they get
// scanned.

// Whole files excluded from the scan.
const EXCLUDE_EXACT = new Set([
  'src/types/ipc.generated.ts', // generated IPC types
  'scripts/check-private-leaks.mjs', // this scanner (defines the markers verbatim)
  'scripts/check-private-leaks.test.mjs', // its fixtures plant the markers on purpose
]);
const EXCLUDE_PREFIXES = [
  'src-tauri/vendor/', // vendored upstream crate - left verbatim (its own author/license)
  'src/types/ipc/', // generated IPC types
];
// A tracked SYMLINK is never excluded by extension: its content is its target
// string, which is text whatever the name says.
export function isExcluded(rel, { symlink = false } = {}) {
  if (EXCLUDE_EXACT.has(rel)) return true;
  if (EXCLUDE_PREFIXES.some((p) => rel.startsWith(p))) return true;
  if (rel.endsWith('Cargo.lock') || rel.endsWith('pnpm-lock.yaml')) return true;
  return !symlink && hasBinaryExt(rel);
}

// Attribution exemption applies only to ROOT-level files (no path separator), so a
// nested code-adjacent README is still fully scanned for maintainer names.
export function isAttributionFile(rel) {
  if (rel.includes('/')) return false;
  return ATTRIBUTION_BASENAMES.has(rel.toLowerCase());
}

// Scan one file's text. Returns [{ruleId, lineNo, why, match}]. Note: the matched
// substring (`match`) is the MARKER itself (e.g. "SCW_", "daniel") - never the
// whole line - so printing it to a public CI log can't publish an adjacent secret.
export function findLeaks(text, rel, { attribution = isAttributionFile(rel) } = {}) {
  const out = [];
  const lines = text.split('\n');
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    for (const rule of RULES) {
      if (rule.exemptAttribution && attribution) continue;
      rule.re.lastIndex = 0;
      let m;
      while ((m = rule.re.exec(line)) !== null) {
        if (m.index === rule.re.lastIndex) rule.re.lastIndex++; // guard zero-width
        out.push({ ruleId: rule.id, lineNo: i + 1, why: rule.why, match: m[0] });
      }
    }
  }
  return out;
}

// Every rule over one tracked PATH string (#576 E18(e)): a file NAMED after a
// maintainer or a private planning doc is published as surely as one that
// mentions it. The root-attribution exemption does not apply to paths, and
// paths are checked before the content exclusions (binaries, vendored,
// generated): an image named after a person passes no content scan at all.
export function findPathLeaks(rel) {
  return findLeaks(rel, rel, { attribution: false }).map((v) => ({ ...v, lineNo: 0 }));
}

function isAllowed(rel, ruleId) {
  return ALLOW.some((a) => a.file === rel && a.ruleId === ruleId);
}

// The gate fails if any marker fired OR any tracked file could not be scanned. A
// skip (oversized/unreadable) is a FAILURE, not a warning - otherwise a marker
// could be smuggled past the scan inside a file the scanner declines to read
// (Copilot review on #232).
export function gateFails(violations, skipped) {
  return violations.length > 0 || skipped.length > 0;
}

// Scan tracked entries: repo-relative paths, or {rel, symlink} as
// listTracked() returns them. `readFile` is injected (a file's bytes, or
// `{ symlink: '<target>' }` for a link) so the fixture test can drive the
// fail-closed branches that a healthy tree never reaches.
export function scan(files, readFile) {
  const violations = [];
  const skipped = []; // {rel, reason} - oversized/unreadable; each FAILS the gate (fail-closed, #232)
  const symlinks = []; // scanned as their link text, never followed
  let scanned = 0;
  const entries = [...files].map(asEntry).sort((a, b) => (a.rel < b.rel ? -1 : a.rel > b.rel ? 1 : 0));
  for (const { rel, symlink } of entries) {
    for (const v of findPathLeaks(rel)) violations.push({ rel, path: true, ...v });
    if (isExcluded(rel, { symlink })) continue;
    let result;
    try {
      result = readFile(rel);
    } catch (e) {
      // staged-but-absent / submodule pointer / permission / race. Don't drop it
      // silently - a leak-containing file that fails to read must not pass as clean.
      skipped.push({ rel, reason: `unreadable (${e.code || e.message})` });
      continue;
    }
    // A NUL byte in a non-binary extension, or an oversized file, is undecidable:
    // fail closed (#232 / #241) instead of skipping silently.
    const t = toScanText(result, MAX_SCAN_BYTES);
    if (t.skip) {
      skipped.push({ rel, reason: t.skip });
      continue;
    }
    // A symlink is scanned as its TARGET STRING - what git publishes for it -
    // and never followed (NF-LINK). Following it (the old behaviour) read
    // whatever the link pointed at, on a CI runner possibly a file outside the
    // repo chosen by a fork PR, and failed a dangling link as "unreadable"
    // while the link text itself (`-> ../personaldocs/x.md`) was never checked.
    if (t.link) symlinks.push(rel);
    scanned++;
    for (const v of findLeaks(t.text, rel)) {
      if (isAllowed(rel, v.ruleId)) continue;
      violations.push({ rel, link: t.link, ...v });
    }
  }
  return { violations, skipped, scanned, symlinks };
}

export function report(result, log = console.log, err = console.error) {
  const { violations, skipped, scanned, symlinks = [] } = result;
  if (!gateFails(violations, skipped)) {
    const note = symlinks.length ? ` (${symlinks.length} symlink(s) scanned as their link text, not followed)` : '';
    log(`✓ private-leak scan: ${scanned} tracked text files, no internal/personal references.${note}`);
    return 0;
  }

  err('\n✗ private-leak scan failed:\n');
  for (const v of violations) {
    // Print the MARKER and location only - never the surrounding line (a public CI
    // log on a public repo must not echo a secret that sits beside the marker).
    const where = v.path ? ' (path)' : `${v.link ? ' (symlink target)' : ''}:${v.lineNo}`;
    err(`  [${v.ruleId}] ${v.rel}${where}  (matched "${v.match}")`);
    err(`      ${v.why}\n`);
  }
  // Fail closed on a skip: a file the scanner can't read (or that exceeds the size
  // cap) must not pass as clean - else a marker could be smuggled inside it (#232).
  for (const s of skipped) {
    err(`  [unscanned] ${s.rel} - ${s.reason}`);
    err('      could not scan this file; failing closed (shrink it, or exclude it deliberately).\n');
  }
  err(
    'Remove the reference (move it to the maintainer-only private repo) or shrink/exclude\n' +
      'the unscanned file. If a match is genuinely benign, add an ALLOW row with a reason in\n' +
      'scripts/check-private-leaks.mjs. See the file header for the rationale.\n',
  );
  return 1;
}

export function main() {
  let files;
  try {
    files = listTracked(REPO_ROOT);
  } catch (e) {
    // The first line only (`Command failed: ...`): the rest repeats the child's
    // stderr, which already reached the terminal and can name the checkout's
    // absolute path (#576 W51).
    console.error(`check-private-leaks: could not list tracked files (run from a git repo): ${e.message.split('\n')[0]}`);
    return 1;
  }
  return report(scan(files, makeReader(REPO_ROOT)));
}

const isMainModule =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMainModule) process.exit(main());
