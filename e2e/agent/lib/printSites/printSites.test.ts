/**
 * #576 W51: one guard over every print and write site of the harness
 * (e2e/agent, e2e/lib) and the repo's Node tooling (scripts/). Eight rounds of
 * review each found a few more outputs that printed the app's or the user's
 * text, or let a forged action log inject markdown; this closes the class
 * instead of the next instance. Every value a shareable sink prints goes
 * through an approved helper (`rules.ts` `HELPERS`), sits where the runner
 * withholds it, or has an entry in its group's allow-list (`allow*.ts`) saying
 * why it is a code, a count, a length, an integer, a harness word or a folded
 * path. Every file write is listed with its class, and every private one is
 * named in docs/agent-harness.md's "Not for sharing".
 *
 * Synthetic sources only: token names (`qz...`) stand in for app data.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { describe, expect, it } from 'vitest';

import { GROUPS, SCANNED_DIRS } from './groups';
import { R, checkSites, covers, scanOptions, type SiteGroup } from './rules';
import { scanFile, scannedFiles, type PrintSite } from './scan';

const REPO = path.resolve(path.dirname(new URL(import.meta.url).pathname), '../../../..');

const merged = <T>(pick: (group: SiteGroup) => Readonly<Record<string, T>>): Record<string, T> =>
  Object.assign({}, ...GROUPS.map(pick)) as Record<string, T>;

const files = scannedFiles(REPO, SCANNED_DIRS);
const options = scanOptions(GROUPS);
const sites: PrintSite[] = files.flatMap((file) => scanFile(REPO, file, options));
const result = checkSites(
  sites,
  merged((group) => group.allow),
  merged((group) => group.fileSinks),
  merged((group) => group.passthrough),
);

describe('every print and write site carries only shareable values (#576 W51)', () => {
  it('finds the sites: a scan that saw none would pass vacuously', () => {
    expect(files.length).toBeGreaterThan(120);
    const kinds = new Set(sites.map((site) => site.kind));
    for (const kind of ['stream', 'progress', 'error', 'file', 'passthrough', 'return'] as const) {
      expect(kinds, kind).toContain(kind);
    }
    expect(sites.length).toBeGreaterThan(500);
  });

  it('puts every scanned file in exactly one group, and every group entry in its own group', () => {
    expect(files.filter((file) => GROUPS.filter((group) => covers(group, file)).length !== 1)).toEqual([]);
    for (const group of GROUPS) {
      const keyed = [
        group.allow,
        group.fileSinks,
        group.passthrough,
        group.wrappers,
        group.builders,
        group.fileHelpers,
        group.programs,
      ].flatMap((list) => Object.keys(list));
      expect(keyed.filter((file) => !covers(group, file)), group.covers.join(' ')).toEqual([]);
    }
  });

  it('every value a shareable sink prints is an approved helper, withheld, or allowed with a reason', () => {
    expect(result.unchecked).toEqual([]);
  });

  it('every file write and child passthrough is listed with its class', () => {
    expect(result.unlisted).toEqual([]);
  });

  it('keeps no entry nothing uses, and gives every entry a reason', () => {
    expect(result.stale).toEqual([]);
    for (const group of GROUPS) {
      for (const [file, entries] of Object.entries(group.allow)) {
        for (const [key, reason] of Object.entries(entries)) expect(reason.length, `${file} ${key}`).toBeGreaterThan(15);
      }
    }
  });

  it('names every private file write in the docs list of what is not for sharing', () => {
    const docs = fs.readFileSync(path.join(REPO, 'docs/agent-harness.md'), 'utf8');
    const start = docs.indexOf('**Not for sharing**');
    expect(start).toBeGreaterThan(0);
    const notForSharing = docs.slice(start, docs.indexOf('\n\n', start));
    const missing = Object.entries(merged((group) => group.fileSinks)).flatMap(([file, entries]) =>
      Object.values(entries)
        .filter((entry) => entry.class === 'private' && !notForSharing.includes(entry.what))
        .map((entry) => `${file}: ${entry.what}`),
    );
    expect(missing).toEqual([]);
  });

  it('classifies every program under the scanned directories that is not JS or TS', () => {
    const programs = merged((group) => group.programs);
    const found: string[] = [];
    const walk = (dir: string): void => {
      for (const entry of fs.readdirSync(path.join(REPO, dir), { withFileTypes: true })) {
        const rel = `${dir}/${entry.name}`;
        if (entry.isDirectory()) {
          if (!['node_modules', 'target'].includes(entry.name)) walk(rel);
          continue;
        }
        const program = /\.(?:sh|py|ps1|rs)$/.test(entry.name);
        const executable = !path.extname(entry.name) && (fs.statSync(path.join(REPO, rel)).mode & 0o111) !== 0;
        if (program || executable) found.push(rel);
      }
    };
    for (const dir of SCANNED_DIRS) walk(dir);
    expect(found.sort()).toEqual(Object.keys(programs).sort());
  });
});

describe('the scan catches what the reviews found (#576 W51)', () => {
  /** Scan synthetic sources, as if they were `<rel>` in the tree, against an allow-list. */
  function check(sources: Record<string, string>, group: Partial<SiteGroup> = {}): ReturnType<typeof checkSites> {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'w51-sites-'));
    try {
      const full: SiteGroup = {
        covers: [''],
        programs: {},
        allow: {},
        fileSinks: {},
        passthrough: {},
        wrappers: {},
        builders: {},
        fileHelpers: {},
        ...group,
      };
      const found = Object.entries(sources).flatMap(([rel, body]) => {
        fs.mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
        fs.writeFileSync(path.join(root, rel), body);
        return scanFile(root, rel, scanOptions([full]));
      });
      return checkSites(found, full.allow, full.fileSinks, full.passthrough);
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  }

  it('flags an app value on stdout, and passes it as a length or a helper result', () => {
    const raw = check({ 'a.ts': 'process.stdout.write(`renamed ${station.name}\\n`);\n' });
    expect(raw.unchecked).toEqual(['a.ts:1 [stream] station.name']);
    expect(check({ 'a.ts': 'process.stdout.write(`renamed ${station.name.length}\\n`);\n' }).unchecked).toEqual([]);
    expect(check({ 'a.ts': 'console.error(`fatal: ${shareableErrorText(err)}`);\n' }).unchecked).toEqual([]);
  });

  it('follows a const to what it was made from, and a local arrow function to what it returns', () => {
    const viaConst = check({ 'a.ts': 'const line = `at ${p.x}`;\nP(line);\n' }, { wrappers: { 'a.ts': { P: 'stream' } } });
    expect(viaConst.unchecked).toEqual(['a.ts:1 [stream] p.x']);
    // Each call stands its own argument in for the parameter (#576 W51: `id: v` hid both).
    const viaArrow = check({ 'a.ts': 'const id = (v) => `#${v}`;\nconsole.log(`${id(p.shotId)} in ${id(p.sectionId)}`);\n' });
    expect(viaArrow.unchecked).toEqual(['a.ts:2 [stream] p.shotId', 'a.ts:2 [stream] p.sectionId']);
    // A validator the scan cannot see through is a file helper, named with its reason.
    const validated = check(
      { 'a.ts': 'const id = (v) => (Number.isInteger(v) ? String(v) : argShape(v));\nconsole.log(`${id(p.shotId)}`);\n' },
      { fileHelpers: { 'a.ts': { id: 'an integer as itself, else its shape' } } },
    );
    expect(validated.unchecked).toEqual([]);
  });

  it('checks what a builder function returns', () => {
    const found = check(
      { 'r.mjs': 'function describeAction(a) {\n  return `coords=(${a.params.x},${a.params.y})`;\n}\n' },
      { builders: { 'r.mjs': ['describeAction'] } },
    );
    expect(found.unchecked).toEqual(['r.mjs:2 [return] a.params.x', 'r.mjs:2 [return] a.params.y']);
  });

  it("withholds an error's or a progress line's app text after `failed: ` and on its later lines", () => {
    const found = check({
      's.ts':
        'throw new Error(`createSection ${i} failed: ${res.error}`);\n' +
        'ctx.log(`x7: open failed: ${opened.error}`);\n' +
        'throw new Error(`state differs\\n  base=${base}`);\n' +
        'throw new Error(`leftover temp file(s) after save ${i}: ${leftovers.join(", ")}`);\n',
    });
    expect(found.unchecked).toEqual([
      's.ts:1 [error] <module>: i',
      's.ts:4 [error] <module>: i',
      `s.ts:4 [error] leftovers.join(", ")`,
    ]);
  });

  it("prints a script's error whole: nothing projects it", () => {
    const source = 'throw new Error(`pnpm licenses failed: ${parsed.error.message}`);\n';
    expect(check({ 's/a.mjs': source }).unchecked).toEqual([]);
    const scripts = check({ 'scripts/a.mjs': source });
    expect(scripts.unchecked).toEqual(['scripts/a.mjs:1 [error] parsed.error.message']);
  });

  it('allows a path only folded', () => {
    const allow = { 'a.ts': { '<module>: runFile': R.FOLDED } };
    expect(check({ 'a.ts': 'process.stdout.write(`ledger: ${runFile}\\n`);\n' }, { allow }).unchecked).toEqual([
      'a.ts:1 [stream] <module>: runFile',
    ]);
    expect(
      check({ 'a.ts': 'process.stdout.write(`ledger: ${foldLocalPaths(runFile)}\\n`);\n' }, { allow }).unchecked,
    ).toEqual([]);
  });

  it('holds a reason that says "folded" to a value that is (#576 W49-NF2: agent:smoke)', () => {
    const allow = { 'smoke.ts': { 'JSON.stringify(started)': 'the StartResult, folded: its paths print as <repo> and ~' } };
    const raw = check({ 'smoke.ts': 'log(`started: ${JSON.stringify(started)}`);\n' }, { allow, wrappers: { 'smoke.ts': { log: 'stream' } } });
    expect(raw.unchecked).toEqual(['smoke.ts:1 [stream] JSON.stringify(started)']);
    const folded = check(
      { 'smoke.ts': 'log(`started: ${foldLocalPaths(JSON.stringify(started))}`);\n' },
      { allow, wrappers: { 'smoke.ts': { log: 'stream' } } },
    );
    expect(folded.unchecked).toEqual([]);
  });

  it('counts as folded a line the runner projects and a line passLinesFolded hands on', () => {
    const allow = {
      's.ts': { 'trace: written': R.FOLDED },
      'b.ts': { 'write: text': R.FOLDED, 'main: line': R.FOLDED },
    };
    const found = check(
      {
        's.ts': 'function trace(written) { ctx.log(`trace written to ${written}`); }\n',
        'b.ts':
          'const write = (text) => { process.stderr.write(text); };\n' +
          'passLinesFolded(child.stderr, write);\n' +
          'function main(line) { process.stderr.write(line); }\n',
      },
      { allow },
    );
    expect(found.unchecked).toEqual(['b.ts:3 [stream] main: line']);
  });

  it('keys a bare identifier by its function, so a name allowed in one is not allowed in another', () => {
    const allow = { 'a.ts': { 'count: n': R.COUNT } };
    const found = check(
      { 'a.ts': 'function count(n) { console.log(`${n} cells`); }\nfunction rename(n) { console.log(`${n} renamed`); }\n' },
      { allow },
    );
    expect(found.unchecked).toEqual(['a.ts:2 [stream] rename: n']);
  });

  it('lists every file write and passthrough, and reports an entry nothing uses', () => {
    const found = check(
      { 'a.ts': "fs.writeFileSync(path.join(dir, 'report.md'), md);\nspawn('x', [], { stdio: 'inherit' });\n" },
      { fileSinks: { 'a.ts': { '<module> -> old': { class: 'private', what: 'x', reason: 'y' } } } },
    );
    expect(found.unlisted).toEqual([
      "a.ts:1 <module> -> path.join(dir, 'report.md')",
      "a.ts:2 <module> -> 'inherit'",
    ]);
    expect(found.stale).toEqual(['a.ts file <module> -> old']);
  });

  it("lists a child whose stderr is ours by default, a copy, and a sink handed on instead of called", () => {
    const found = check({
      'a.mjs':
        "execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' });\n" +
        "execSync('git status', { stdio: 'ignore' });\n" +
        'fs.copyFileSync(source, path.join(dir, name));\n' +
        'export function main(log = console.log) { log(`${station.name}`); }\n' +
        'const out = process.stderr;\n',
    });
    expect(found.unlisted).toEqual([
      'a.mjs:1 <module> -> <stderr by default>',
      'a.mjs:3 <module> -> path.join(dir, name)',
      'a.mjs:4 main -> console.log',
      'a.mjs:5 <module> -> process.stderr',
    ]);
  });

  it("lists a child's one stream handed over, and checks what a file stream is written", () => {
    const found = check({
      'a.ts':
        "new StdioClientTransport({ command: 'node', stderr: 'inherit' });\n" +
        "this.log = fs.createWriteStream(path.join(dir, 'report.md'));\n" +
        'const log = this.log;\n' +
        'log.write(`${station.name}\\n`);\n',
    });
    expect(found.unlisted).toEqual([
      "a.ts:1 <module> -> 'inherit'",
      "a.ts:2 <module> -> path.join(dir, 'report.md')",
      'a.ts:4 <module> -> <stream log>',
    ]);
  });

  it("checks a metric's key, which the PASS/FAIL line prints, and not its number", () => {
    const found = check({ 's.ts': "ctx.metric(`mapSkip:${err.message}`);\nctx.setMetric('maxDepth', depth);\n" });
    expect(found.unchecked).toEqual(['s.ts:1 [stream] err.message']);
  });

  it('checks only the arguments a wrapper prints', () => {
    const source = { 'a.ts': "check('session ready', await sessionReady(), `${n} keys`);\n" };
    expect(check(source, { wrappers: { 'a.ts': { check: 'stream' } } }).unchecked).toEqual([
      'a.ts:1 [stream] sessionReady()',
      'a.ts:1 [stream] <module>: n',
    ]);
    expect(check(source, { wrappers: { 'a.ts': { check: { kind: 'stream', args: [0, 2] } } } }).unchecked).toEqual([
      'a.ts:1 [stream] <module>: n',
    ]);
  });
});
