/**
 * #576 W51: the rules every print and write site of the harness and the repo's
 * Node tooling is checked against (`printSites.test.ts`). Two classes:
 *
 * - **shareable**: stdout and stderr, a progress line, an error's message (a
 *   runner prints its first line), `report.md`, `finding.json`, a fuzz trace,
 *   the UAT ledger under `docs/uat/`. What reaches one is a code, a count, a
 *   length, an integer, a word from a fixed set the harness writes, or a path
 *   folded by `foldLocalPaths` - through an approved helper (`HELPERS`), or as a
 *   value someone checked and gave a reason for (`allow*.ts`).
 * - **private**: a file docs/agent-harness.md lists under "Not for sharing"
 *   (`actions.jsonl`, `state/`, screenshots, ...). Each such write is listed
 *   in `FILE_SINKS` with the name the docs give it.
 *
 * A value the runner withholds - after the first `failed: ` of a progress line
 * or an error's first line, or on a later line of either that is not a shape
 * the projection keeps - needs no reason: `shareableFailureText` prints its
 * length or counts its line.
 */
import type { PrintSite, PrintValue, ScanOptions, SinkKind, Wrapper } from './scan';

/** Reasons a value may print. An allow entry gives one of these, or its own words. */
export const R = {
  COUNT: 'a number the harness computed: a count, a length, an index, a duration or a size',
  KNOB: 'a seed, an iteration count or a knob value, parsed to a number or picked from the harness table',
  NAME: "a name from the harness's own tables: a scenario, journey, oracle, tool, check, knob, format or fixture id",
  WORD: 'one of a fixed set of words the harness writes: a state, a verdict, a build mode, a kind',
  ENV_NAME: 'the name of an environment variable or a CLI flag the harness reads, from its own table',
  REPO: 'a repo-relative path, a line number or a rule of the public tree (repo tooling; never the app or its data)',
  REPO_TEXT: 'text of a tracked file of the public tree, or of a lint rule (repo tooling; never the app or its data)',
  DEV_TOOL: 'output of a developer tool run on the public tree (git, cargo, tsc, eslint, pnpm); no app data',
  FOLDED: 'a path the harness made or was given, printed through foldLocalPaths (the checkout as <repo>, the home directory as ~)',
  FORWARD: 'the parameter of a wrapper whose calls are sinks of their own (`WRAPPERS`), checked there',
  SHAPED: 'a value already in the shareable form, made by an approved helper earlier in the same function',
} as const;

export type AllowList = Readonly<Record<string, Readonly<Record<string, string>>>>;

export type FileSinkClass = 'shareable' | 'private';
export interface FileSink {
  class: FileSinkClass;
  /** Private: the name docs/agent-harness.md's "Not for sharing" gives it. Shareable: what it is. */
  what: string;
  reason: string;
}
/** Per file: `<fn> -> <target as written>` to what the write is. */
export type FileSinkList = Readonly<Record<string, Readonly<Record<string, FileSink>>>>;
/** Per file: `<fn> -> <stdio as written>` to why the child's output may pass through. */
export type PassthroughList = Readonly<Record<string, Readonly<Record<string, string>>>>;

/**
 * Helpers whose result is shareable wherever they are called, each with why.
 * Their bodies are the projection and have tests of their own.
 */
export const HELPERS: Readonly<Record<string, string>> = {
  shareableErrorText: 'a crash: its name, its headline in the shareable form, folded frames, other lines counted',
  shareableFailureText: 'a failure: headline form, kept shapes, other lines counted',
  scenarioFailOutput: "a scenario's FAIL block in the shareable form",
  shareableProgressLine: 'a progress line in the headline form',
  shareableTail: "the last lines of a child's output, fail-closed",
  withheldAppText: "the app's text as its length",
  valueShape: 'a value read back from a file as a number, a boolean, null, or its type and size, never its text',
  findingsSummary: 'findings as counts per severity, kind and code',
  failureDetail: "a matrix cell's failure: the child's FAIL block, or its tail fail-closed",
  matrixSummary: "the matrix's summary: counts and durations",
  journeyVerdictLine: "a journey's PASS/FAIL line: counts, a failed check as its length",
  ledgerRecord: "the UAT ledger record: a check's detail as its length, the abort reason in the headline form",
  shareableFinding: "a finding's shareable projection (scripts/lib/shareable-finding.mjs)",
  shareableSelector: 'a selector: its structure and the words of the production source, other text as its length',
  shareableTestId: 'a test id: the words of the production source, other text as its length',
  shareableSignature: 'the signature of the shareable failure text',
  describeSignature: "a signature's fields, each from shareableSignature",
};

/**
 * Helpers that fold paths out of their argument. What they return is only as
 * shareable as what they were given, so the values of the argument are checked,
 * and `R.FOLDED` holds for them only.
 */
export const FOLDS: Readonly<Record<string, string>> = {
  foldLocalPaths: 'the checkout folded to <repo> and the home directory to ~',
  redactCorpusText: "a private corpus entry's path and names folded to its corpus id",
};

/** Helpers that hand the function they are given only folded text, each with why. */
export const FOLDING_CALLBACKS: Readonly<Record<string, string>> = {
  passLinesFolded: 'hands its callback each line of a child process output after foldLocalPaths (lib/foldedLines.ts)',
};

/**
 * A reason that says a value is folded holds only for a value that is: one
 * inside a `FOLDS` call, in a line the runner projects (it folds every line), a
 * parameter a `FOLDING_CALLBACKS` helper calls, or one whose own text calls a
 * fold (`xs.map((x) => foldLocalPaths(x))`).
 */
export function claimsFold(reason: string): boolean {
  return reason === R.FOLDED || /\bfolded\b/i.test(reason);
}
function isFolded(value: PrintValue): boolean {
  return value.folded || Object.keys(FOLDS).some((fold) => value.text.includes(`${fold}(`));
}

/** One group's lists, each keyed by repo-relative file (`allow*.ts`). */
export interface SiteGroup {
  /**
   * The files the group covers: `<dir>/` covers the directory and everything
   * under it, `<dir>/*` only the files directly in it.
   */
  covers: readonly string[];
  /** Per program that is not JS/TS (a shell, Python, PowerShell or Rust source): its class and why. */
  programs: Readonly<Record<string, { class: FileSinkClass | 'tooling'; reason: string }>>;
  /** Per file: value key (`valueKey`) to why it may print. */
  allow: AllowList;
  /** Per file: write site (`siteKey`) to what it writes. */
  fileSinks: FileSinkList;
  /** Per file: passthrough site (`siteKey`) to why the child's output may pass. */
  passthrough: PassthroughList;
  /** Per file: the functions whose calls are sinks of their own, and of which kind. */
  wrappers: Readonly<Record<string, Readonly<Record<string, Wrapper>>>>;
  /** Per file: the functions whose `return` values are shared text. */
  builders: Readonly<Record<string, readonly string[]>>;
  /** Per file: helpers whose result is shareable in that file only, each with why. */
  fileHelpers: Readonly<Record<string, Readonly<Record<string, string>>>>;
}

/**
 * Where an error's message prints whole: no runner projects it. A script under
 * `scripts/` has no `shareableErrorText`; Node prints what it dies of in full.
 */
export const UNPROJECTED: readonly string[] = ['scripts/'];

/** Does `group` cover `file`? */
export function covers(group: SiteGroup, file: string): boolean {
  return group.covers.some((entry) =>
    entry.endsWith('/*')
      ? file.startsWith(entry.slice(0, -1)) && !file.slice(entry.length - 1).includes('/')
      : file.startsWith(entry),
  );
}

export function scanOptions(groups: readonly SiteGroup[]): ScanOptions {
  const merge = <T>(pick: (group: SiteGroup) => Readonly<Record<string, T>>): Map<string, T> =>
    new Map(groups.flatMap((group) => Object.entries(pick(group))));
  return {
    wrappers: merge((group) => group.wrappers),
    builders: merge((group) => group.builders),
    helpers: new Set(Object.keys(HELPERS)),
    fileHelpers: new Map([...merge((group) => group.fileHelpers)].map(([file, helpers]) => [file, Object.keys(helpers)])),
    folds: new Set(Object.keys(FOLDS)),
    foldingCallbacks: new Set(Object.keys(FOLDING_CALLBACKS)),
    unprojected: UNPROJECTED,
  };
}

/** The kinds whose values reach a shareable sink. */
const VALUE_KINDS: ReadonlySet<SinkKind> = new Set(['stream', 'progress', 'error', 'return']);

/** How an allow-list keys a value: a bare identifier by its function, anything else as written. */
export function valueKey(value: PrintValue): string {
  return value.bare ? `${value.fn}: ${value.text}` : value.text;
}

/** The key of a file or passthrough site. */
export function siteKey(site: PrintSite): string {
  return `${site.fn} -> ${site.target ?? ''}`;
}

export interface CheckResult {
  /** `<file>:<line> [<kind>] <key>`: a value nothing allows. */
  unchecked: string[];
  /** `<file> <key>`: an allow entry no value uses. */
  stale: string[];
  /** `<file>:<line> <key>`: a file write or passthrough nothing lists. */
  unlisted: string[];
  /** How each site is made safe, for the audit table. */
  audit: AuditRow[];
}

export interface AuditRow {
  site: string;
  kind: SinkKind;
  sink: string;
  class: FileSinkClass | 'page';
  how: string;
}

/** Check `sites` against the allow-lists. */
export function checkSites(
  sites: readonly PrintSite[],
  allowed: AllowList,
  fileSinks: FileSinkList,
  passthrough: PassthroughList,
): CheckResult {
  const used = new Set<string>();
  const unchecked: string[] = [];
  const unlisted: string[] = [];
  const audit: AuditRow[] = [];
  for (const site of sites) {
    const where = `${site.file}:${site.line}`;
    if (site.kind === 'page') {
      audit.push({ site: where, kind: site.kind, sink: site.sink, class: 'page', how: "runs in the app's webview: the app's console, read by the log oracle" });
      continue;
    }
    if (site.kind === 'passthrough') {
      const key = siteKey(site);
      const reason = passthrough[site.file]?.[key];
      if (reason === undefined) unlisted.push(`${where} ${key}`);
      else used.add(`${site.file}\u0000passthrough\u0000${key}`);
      audit.push({ site: where, kind: site.kind, sink: site.sink, class: 'shareable', how: reason ?? 'UNLISTED' });
      continue;
    }
    let cls: FileSinkClass = 'shareable';
    const hows: string[] = [];
    if (site.kind === 'file') {
      const key = siteKey(site);
      const entry = fileSinks[site.file]?.[key];
      if (entry === undefined) {
        unlisted.push(`${where} ${key}`);
        audit.push({ site: where, kind: site.kind, sink: site.sink, class: 'shareable', how: 'UNLISTED' });
        continue;
      }
      used.add(`${site.file}\u0000file\u0000${key}`);
      cls = entry.class;
      hows.push(`${entry.what}: ${entry.reason}`);
      if (cls === 'private') {
        audit.push({ site: where, kind: site.kind, sink: site.sink, class: cls, how: hows.join('; ') });
        continue;
      }
    }
    if (VALUE_KINDS.has(site.kind) || site.kind === 'file') {
      const withheld = site.values.filter((value) => value.withheld).length;
      if (withheld > 0) hows.push(`${withheld} value(s) withheld by the projection`);
      for (const value of site.values) {
        if (value.withheld) continue;
        const key = valueKey(value);
        const reason = allowed[site.file]?.[key];
        // A path is shareable folded, never raw: a reason that says it is folded holds only if it is.
        if (reason === undefined || (claimsFold(reason) && !isFolded(value))) {
          unchecked.push(`${site.file}:${value.line} [${site.kind}] ${key}`);
          hows.push(`UNCHECKED ${key}`);
          continue;
        }
        used.add(`${site.file}\u0000value\u0000${key}`);
        hows.push(`${key}: ${reason}`);
      }
      if (site.values.length === 0) hows.push('literal text only, or approved helpers');
    }
    audit.push({ site: where, kind: site.kind, sink: site.sink, class: cls, how: hows.join('; ') });
  }
  const stale: string[] = [];
  for (const [file, entries] of Object.entries(allowed)) {
    for (const key of Object.keys(entries)) if (!used.has(`${file}\u0000value\u0000${key}`)) stale.push(`${file} ${key}`);
  }
  for (const [file, entries] of Object.entries(fileSinks)) {
    for (const key of Object.keys(entries)) if (!used.has(`${file}\u0000file\u0000${key}`)) stale.push(`${file} file ${key}`);
  }
  for (const [file, entries] of Object.entries(passthrough)) {
    for (const key of Object.keys(entries)) if (!used.has(`${file}\u0000passthrough\u0000${key}`)) stale.push(`${file} passthrough ${key}`);
  }
  return { unchecked, stale, unlisted, audit };
}
