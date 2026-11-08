/**
 * #576 W51: one guard over every print and write site of the harness
 * (e2e/agent, e2e/lib) and the repo's Node tooling (scripts/). Eight rounds of
 * review each found a few more outputs that printed the app's or the user's
 * text, or let a forged action log inject markdown; this checks the class
 * rather than the next instance. Every value a shareable sink prints goes
 * through an approved helper (`rules.ts` `HELPERS`), sits where the runner
 * withholds it, or has an entry in its group's allow-list (`allow*.ts`) saying
 * why it is a code, a count, a length, an integer, a harness word or a folded
 * path. Every file write is listed with its class, and every private one is
 * named in docs/agent-harness.md's "Not for sharing".
 *
 * What it reads off the source is where each value goes. It trusts what it is
 * told: a helper or a file helper by its name, a reason by its words, a wrapper
 * as a sink in its own file (none may be exported, and a sink handed on is
 * listed only where a declared wrapper holds it). That a helper returns the
 * shareable form, and that a value comes from where its reason says, is
 * guarded by behaviour tests, not here: scripts/generators.test.ts for
 * report.md (a step's `code(a.tool)` reaches the file inside `scrub(...)`,
 * which this guard takes on trust), and the helpers' own tests in
 * e2e/agent/lib (shareableFailure, shareableRunnerOutput, mcpSmokeOutput, the
 * *Privacy tests). A value the scan cannot follow (what a function declaration
 * returns, an array handed to a function) is one value with a reason of its own.
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

  it('lists a sink handed on only where a declared wrapper holds it, and exports no wrapper (#576 RV20-12)', () => {
    expect(result.unguarded).toEqual([]);
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
    expect(raw.unchecked).toEqual(['a.ts:1 [stream] <module>: station.name']);
    expect(check({ 'a.ts': 'process.stdout.write(`renamed ${station.name.length}\\n`);\n' }).unchecked).toEqual([]);
    expect(check({ 'a.ts': 'console.error(`fatal: ${shareableErrorText(err)}`);\n' }).unchecked).toEqual([]);
  });

  it('follows a const to what it was made from, and a local arrow function to what it returns', () => {
    const viaConst = check({ 'a.ts': 'const line = `at ${p.x}`;\nP(line);\n' }, { wrappers: { 'a.ts': { P: 'stream' } } });
    expect(viaConst.unchecked).toEqual(['a.ts:1 [stream] <module>: p.x']);
    // Each call stands its own argument in for the parameter (#576 W51: `id: v` hid both).
    const viaArrow = check({ 'a.ts': 'const id = (v) => `#${v}`;\nconsole.log(`${id(p.shotId)} in ${id(p.sectionId)}`);\n' });
    expect(viaArrow.unchecked).toEqual(['a.ts:2 [stream] <module>: p.shotId', 'a.ts:2 [stream] <module>: p.sectionId']);
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
    expect(found.unchecked).toEqual(['r.mjs:2 [return] describeAction: a.params.x', 'r.mjs:2 [return] describeAction: a.params.y']);
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
      `s.ts:4 [error] <module>: leftovers.join(", ")`,
    ]);
  });

  it("prints a script's error whole: nothing projects it", () => {
    const source = 'throw new Error(`pnpm licenses failed: ${parsed.error.message}`);\n';
    expect(check({ 's/a.mjs': source }).unchecked).toEqual([]);
    const scripts = check({ 'scripts/a.mjs': source });
    expect(scripts.unchecked).toEqual(['scripts/a.mjs:1 [error] <module>: parsed.error.message']);
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
    const allow = { 'smoke.ts': { '<module>: JSON.stringify(started)': 'the StartResult, folded: its paths print as <repo> and ~' } };
    const raw = check({ 'smoke.ts': 'log(`started: ${JSON.stringify(started)}`);\n' }, { allow, wrappers: { 'smoke.ts': { log: 'stream' } } });
    expect(raw.unchecked).toEqual(['smoke.ts:1 [stream] <module>: JSON.stringify(started)']);
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

  it("keys a call of a parameter by its function, so the fuzzer's `guarded` print allows no other `what()` (#576)", () => {
    // Keyed by text, `what()` allowed every `what()` in the file, wherever it printed.
    const fuzz = 'e2e/agent/scenarios/importDialogFuzz.ts';
    const sources = {
      [fuzz]:
        'const guarded = (what) => {\n  try {\n    return what();\n  } catch (err) {\n    throw err;\n  }\n};\n' +
        'const pushed = guarded(() => checkPushed(shape, before, after, label));\n' +
        'ctx.metric(`f1Pushed:${pushed}`);\n' +
        'function describeQz(what) {\n  ctx.log(`qz ${what()}`);\n}\n',
    };
    // Against the file's real entries: the f1Pushed print through `guarded` passes, the other `what()` does not.
    const allow = { [fuzz]: merged((group) => group.allow)[fuzz] ?? {} };
    expect(check(sources, { allow }).unchecked).toEqual([`${fuzz}:11 [progress] describeQz: what()`]);
    // Each keyed by the function it is written in.
    expect(check(sources).unchecked).toEqual([`${fuzz}:3 [stream] guarded: what()`, `${fuzz}:11 [progress] describeQz: what()`]);
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
    expect(found.unchecked).toEqual(['s.ts:1 [stream] <module>: err.message']);
  });

  it('checks only the arguments a wrapper prints', () => {
    const source = { 'a.ts': "check('session ready', await sessionReady(), `${n} keys`);\n" };
    expect(check(source, { wrappers: { 'a.ts': { check: 'stream' } } }).unchecked).toEqual([
      'a.ts:1 [stream] <module>: sessionReady()',
      'a.ts:1 [stream] <module>: n',
    ]);
    expect(check(source, { wrappers: { 'a.ts': { check: { kind: 'stream', args: [0, 2] } } } }).unchecked).toEqual([
      'a.ts:1 [stream] <module>: n',
    ]);
  });

  it('checks a line pushed straight into an accumulator a sink writes (#576 RV20-5: report.md)', () => {
    const report = {
      wrappers: { 'r.mjs': { P: 'stream' as const } },
      fileHelpers: { 'r.mjs': { code: 'a string of its shape as itself, else its argShape' } },
      fileSinks: { 'r.mjs': { '<module> -> outPath': { class: 'shareable' as const, what: 'report.md', reason: 'pasted into public issues' } } },
      // The entry report.md had: the join, as if only P pushed.
      allow: { 'r.mjs': { "<module>: md.join('\\n')": 'the lines P(...) pushed, each one a checked stream site', 'P: s': R.FORWARD } },
    };
    const pushed = check(
      {
        'r.mjs':
          "const md = [];\nconst P = (s = '') => md.push(s);\nP(`# ${code(title)}`);\n" +
          "md.push(`- Profile: ${start.profileDir}`);\nfs.writeFileSync(outPath, md.join('\\n'));\n",
      },
      report,
    );
    expect(pushed.unchecked).toEqual(['r.mjs:4 [file] <module>: start.profileDir']);
    // Handed to a function, it can hold anything: its join stays one value that needs a reason of its own.
    const handed = check({ 'r.mjs': "const md = [];\naddLines(md);\nfs.writeFileSync(outPath, md.join('\\n'));\n" }, { ...report, allow: {} });
    expect(handed.unchecked).toEqual(["r.mjs:3 [file] <module>: md.join('\\n')"]);
  });

  it("keys a callback's names by its call, not by the function it is written in (#576 RV20-7)", () => {
    const group = {
      wrappers: { 'r.mjs': { P: 'stream' as const } },
      fileHelpers: { 'r.mjs': { scrub: 'the step line with the session dir and the home directory folded' } },
      allow: {
        'r.mjs': {
          // The entry report.md had for its steps callback, and the key that callback has now.
          '<module>: s': 'a step line, at scrub(s) for each of `steps`',
          '<module> > steps.forEach: s': 'a step line, at scrub(s) for each of `steps`',
          'main > files.forEach: f': R.REPO,
        },
      },
    };
    const found = check(
      {
        'r.mjs':
          'steps.forEach((s) => P(scrub(s)));\n' +
          'actions.map((a) => a.params?.text).forEach((s) => P(`- typed: ${s}`));\n' +
          'function main() { files.forEach((f) => P(f)); }\n' +
          'function other() { files.forEach((f) => P(f)); }\n',
      },
      group,
    );
    expect(found.unchecked).toEqual(['r.mjs:2 [stream] <module> > actions.map().forEach: s', 'r.mjs:4 [stream] other > files.forEach: f']);
  });

  it('keys every value by its function, so an error message allowed in one is not allowed in another (#576 RV20-10)', () => {
    const reason = "Node's `Command failed: git checkout -- src-tauri/gen/schemas` (stdio ignored)";
    // The entry build.ts had, file-wide, and the key that one print has now.
    const allow = { 'b.ts': { 'err.message': reason, 'stamp: err.message': reason } };
    const found = check(
      {
        'b.ts':
          'function stamp() {\n  try { x(); } catch (err) { console.error(`stamp failed ${err.message}`); }\n}\n' +
          'function other() {\n  try { y(); } catch (err) { console.error(`other ${err.message}`); }\n}\n',
      },
      { allow },
    );
    expect(found.unchecked).toEqual(['b.ts:5 [stream] other: err.message']);
  });

  it('finds every console method, a warning, Error() without new, an fd write, any context name, a stream under another name (#576 RV20-11)', () => {
    const app = 'm.ts:1 [stream] <module>: station.name';
    // Each probe: a label, its source, and what the guard reports.
    const probes: [string, string, string[]][] = [
      ['console.group', 'console.group(station.name);\n', [app]],
      ['console.assert', 'console.assert(false, station.name);\n', [app]],
      ['console.timeLog', 'console.timeLog(station.name);\n', [app]],
      ['console.count', 'console.count(station.name);\n', [app]],
      ["console['log']", "console['log'](station.name);\n", [app]],
      ['process.emitWarning', 'process.emitWarning(station.name);\n', [app]],
      ['const { error } = console', 'const { error } = console;\nerror(station.name);\n', ['m.ts:1 <module> -> console']],
      // `c.log` reads as a progress line too; the console handed on is the finding.
      ['const c = console', 'const c = console;\nc.log(station.name);\n', ['m.ts:2 [progress] <module>: station.name', 'm.ts:1 <module> -> console']],
      ["import { stdout } from 'node:process'", "import { stdout } from 'node:process';\nstdout.write(station.name);\n", ['m.ts:1 <module> -> process.stdout']],
      ["import { stderr } from 'process'", "import { stderr } from 'process';\nstderr.write(station.name);\n", ['m.ts:1 <module> -> process.stderr']],
      ['const { stdout: out } = process', 'const { stdout: out } = process;\nout.write(station.name);\n', ['m.ts:1 <module> -> process.stdout']],
      ['throw Error(x)', 'throw Error(station.name);\n', ['m.ts:1 [error] <module>: station.name']],
      ['fs.writeSync(2, x)', 'fs.writeSync(2, station.name);\n', [app]],
      ['fs.writeSync(fd, x)', 'fs.writeSync(fd, station.name);\n', ['m.ts:1 <module> -> fd']],
      ['fs.writevSync(fd, [x])', 'fs.writevSync(fd, [station.name]);\n', ['m.ts:1 <module> -> fd']],
      ['fs.openSync + writeSync', "const fd = fs.openSync(p, 'w');\nfs.writeSync(fd, station.name);\n", ['m.ts:2 <module> -> fd']],
      ['context.log', 'export async function run(context) { context.log(`renamed ${station.name}`); }\n', ['m.ts:1 [progress] run: station.name']],
    ];
    const found = Object.fromEntries(
      probes.map(([label, source]) => {
        const result = check({ 'm.ts': source });
        return [label, [...result.unchecked, ...result.unlisted]];
      }),
    );
    expect(found).toEqual(Object.fromEntries(probes.map(([label, , expected]) => [label, expected])));
    // Not a sink: a type, a patched method (mcp/hygiene.ts), a property of that name, an `isError` check.
    const quiet = check({ 'm.ts': "type C = typeof console;\nconsole[m] = (...a) => w(a);\nconst o = { console: 1 };\nisError(station.name);\n" });
    expect([...quiet.unchecked, ...quiet.unlisted]).toEqual([]);
  });

  it('finds the console under the global object, a required process or console, an async fd write and a FileHandle write (#576 W56-T1-NF1)', () => {
    const app = 'm.ts:1 [stream] <module>: station.name';
    const shareable = { class: 'shareable' as const, what: 'report.md', reason: 'pasted into public issues' };
    const open = "const fh = await fs.promises.open(p, 'w');\n";
    // Each probe: a label, its source, what the guard reports, and the lists it is checked against.
    const probes: [string, string, string[], Partial<SiteGroup>?][] = [
      ['globalThis.console.error', 'globalThis.console.error(station.name);\n', [app]],
      ['global.console.warn', 'global.console.warn(station.name);\n', [app]],
      ['window.console.info', 'window.console.info(station.name);\n', [app]],
      ["globalThis.console['error']", "globalThis.console['error'](station.name);\n", [app]],
      ['globalThis.process.stderr.write', 'globalThis.process.stderr.write(station.name);\n', [app]],
      ['const c = globalThis.console', 'const c = globalThis.console;\nc.error(station.name);\n', ['m.ts:1 <module> -> globalThis.console']],
      ['const { error } = global.console', 'const { error } = global.console;\nerror(station.name);\n', ['m.ts:1 <module> -> global.console']],
      ['const out = globalThis.process.stderr', 'const out = globalThis.process.stderr;\nout.write(station.name);\n', ['m.ts:1 <module> -> globalThis.process.stderr']],
      ["const proc = require('process')", "const proc = require('process');\nproc.stdout.write(station.name);\n", ["m.ts:1 <module> -> require('process')"]],
      ["const { stdout } = require('node:process')", "const { stdout } = require('node:process');\nstdout.write(station.name);\n", ['m.ts:1 <module> -> process.stdout']],
      ["require('process').stderr.write", "require('process').stderr.write(station.name);\n", ['m.ts:1 <module> -> process.stderr']],
      ["const c = require('console')", "const c = require('console');\nc.error(station.name);\n", ["m.ts:1 <module> -> require('console')"]],
      ["const { error: e } = require('console')", "const { error: e } = require('console');\ne(station.name);\n", ['m.ts:1 <module> -> console.error']],
      ['fs.write(2, x, cb)', 'fs.write(2, station.name, () => {});\n', [app]],
      ['fs.write(fd, x, cb)', 'fs.write(fd, station.name, () => {});\n', ['m.ts:1 <module> -> fd']],
      ['fs.writev(fd, [x], cb)', 'fs.writev(fd, [station.name], () => {});\n', ['m.ts:1 <module> -> fd']],
      ["import { write } from 'node:fs'", "import { write } from 'node:fs';\nwrite(fd, station.name, () => {});\n", ['m.ts:2 <module> -> fd']],
      ['promisify(fs.write)', 'const write = promisify(fs.write);\nawait write(fd, station.name);\n', ['m.ts:1 <module> -> fs.write']],
      ['FileHandle.write', `${open}await fh.write(station.name);\n`, ['m.ts:2 <module> -> <handle fh>']],
      ['FileHandle.writev', `${open}await fh.writev([station.name]);\n`, ['m.ts:2 <module> -> <handle fh>']],
      [
        'fsp.open, then FileHandle.write',
        "import * as fsp from 'node:fs/promises';\nconst fh = await fsp.open(p, 'w');\nawait fh.write(station.name);\n",
        ['m.ts:3 <module> -> <handle fh>'],
      ],
      // Listed as the guard read it before: its data argument taken for the file it writes.
      [
        'FileHandle.writeFile',
        `${open}const report = \`\${station.name}\\n\`;\nawait fh.writeFile(report);\n`,
        ['m.ts:3 <module> -> <handle fh>'],
        { fileSinks: { 'm.ts': { '<module> -> report': shareable } } },
      ],
      // Listed under the handle, what it writes is checked.
      ['FileHandle.writeFile, listed', `${open}await fh.writeFile(station.name);\n`, ['m.ts:2 [file] <module>: station.name'], { fileSinks: { 'm.ts': { '<module> -> <handle fh>': shareable } } }],
    ];
    const found = Object.fromEntries(
      probes.map(([label, source, , group]) => {
        const result = check({ 'm.ts': source }, group);
        return [label, [...result.unchecked, ...result.unlisted]];
      }),
    );
    expect(found).toEqual(Object.fromEntries(probes.map(([label, , expected]) => [label, expected])));
    // Not a sink: its type, a patched method, what else process hands out, another stream's write, a handle's read.
    const quiet = check({
      'm.ts':
        "const t = typeof globalThis.console;\nglobalThis.console[m] = (...a) => w(a);\nconst { argv } = require('process');\n" +
        "const home = require('node:process').env.HOME;\nconst proc = require('process').exitCode;\nchild.stdin.write(station.name);\n" +
        `${open.replace('\n', '')}\nawait fh.read(buffer);\n`,
    });
    expect([...quiet.unchecked, ...quiet.unlisted]).toEqual([]);
  });

  it('finds a file write handed on, a dynamic import of process or console, and a FileHandle in a callback (#576 W74-NF1)', () => {
    const app = (line: number, fn = '<module>'): string => `m.ts:${line} [stream] ${fn}: station.name`;
    const shareable = { class: 'shareable' as const, what: 'report.md', reason: 'pasted into public issues' };
    const thenFn = '<module> > fs.promises.open().then';
    // Each probe: a label, its source, what the guard reports, and the lists it is checked against.
    const probes: [string, string, string[], Partial<SiteGroup>?][] = [
      // A file write handed on through a variable: by property, the function handed on; taken by name, its calls are writes.
      ['const w = fs.writeFileSync', 'const w = fs.writeFileSync;\nw(p, station.name);\n', ['m.ts:1 <module> -> fs.writeFileSync']],
      ['const c = fs.copyFileSync', 'const c = fs.copyFileSync;\nc(source, p);\n', ['m.ts:1 <module> -> fs.copyFileSync']],
      [
        'promisify(fsp.writeFile)',
        "import * as fsp from 'node:fs/promises';\nconst w = promisify(fsp.writeFile);\nawait w(p, station.name);\n",
        ['m.ts:2 <module> -> fsp.writeFile'],
      ],
      ['const { writeFileSync: w } = fs', 'const { writeFileSync: w } = fs;\nw(p, station.name);\n', ['m.ts:2 <module> -> p']],
      ["import { writeFileSync as w } from 'node:fs'", "import { writeFileSync as w } from 'node:fs';\nw(p, station.name);\n", ['m.ts:2 <module> -> p']],
      ["const { appendFileSync: a } = require('fs')", "const { appendFileSync: a } = require('fs');\na(p, station.name);\n", ['m.ts:2 <module> -> p']],
      ['const { copyFileSync: c } = fs', 'const { copyFileSync: c } = fs;\nc(source, p);\n', ['m.ts:2 <module> -> p']],
      ['const { promises: { writeFile: w } } = fs', 'const { promises: { writeFile: w } } = fs;\nawait w(p, station.name);\n', ['m.ts:2 <module> -> p']],
      ['const { writeSync: ws } = fs', 'const { writeSync: ws } = fs;\nws(2, station.name);\n', [app(2)]],
      [
        'const { createWriteStream: cws } = fs',
        'const { createWriteStream: cws } = fs;\nconst out = cws(p);\nout.write(station.name);\n',
        ['m.ts:2 <module> -> p', 'm.ts:3 <module> -> <stream out>'],
      ],
      [
        "import { writeFileSync } from 'fs', handed on",
        "import { writeFileSync } from 'fs';\nconst w = writeFileSync;\nw(p, station.name);\n",
        ['m.ts:2 <module> -> fs.writeFileSync'],
      ],
      ["import { write } from 'fs', handed on", "import { write } from 'fs';\nconst w = promisify(write);\nawait w(fd, station.name);\n", ['m.ts:2 <module> -> fs.write']],
      [
        "const { writeFileSync: w } = await import('node:fs')",
        "const { writeFileSync: w } = await import('node:fs');\nw(p, station.name);\n",
        ['m.ts:2 <module> -> p'],
      ],
      // A dynamic import of process or console: what it takes that prints, or the module handed on.
      ["const proc = await import('node:process')", "const proc = await import('node:process');\nproc.stdout.write(station.name);\n", ["m.ts:1 <module> -> import('process')"]],
      ["const { stdout } = await import('process')", "const { stdout } = await import('process');\nstdout.write(station.name);\n", ['m.ts:1 <module> -> process.stdout']],
      ["(await import('process')).stderr.write", "(await import('process')).stderr.write(station.name);\n", ['m.ts:1 <module> -> process.stderr']],
      ["import('node:process').then", "import('node:process').then((p) => p.stdout.write(station.name));\n", ["m.ts:1 <module> -> import('process')"]],
      ["const { default: p } = await import('process')", "const { default: p } = await import('process');\np.stdout.write(station.name);\n", ['m.ts:1 <module> -> process.default']],
      ["const process = await import('process')", "const process = await import('process');\nprocess.default.stdout.write(station.name);\n", ["m.ts:1 <module> -> import('process')"]],
      ["const { log } = await import('node:console')", "const { log } = await import('node:console');\nlog(station.name);\n", ['m.ts:1 <module> -> console.log']],
      ["import { default as p } from 'process'", "import { default as p } from 'process';\np.stdout.write(station.name);\n", ['m.ts:1 <module> -> process.default']],
      // A FileHandle handed to a callback: its writes are the file's, what it writes checked once listed.
      ['open().then((fh) => fh.write(x))', "fs.promises.open(p, 'w').then((fh) => fh.write(station.name));\n", [`m.ts:1 ${thenFn} -> <handle fh>`]],
      // Or written straight on the open, held in no name.
      ['(await open()).write(x)', "(await fs.promises.open(p, 'w')).write(station.name);\n", ["m.ts:1 <module> -> <handle (await fs.promises.open(p, 'w'))>"]],
      [
        'fsp.open().then(function (h) { h.writeFile(x) })',
        "import * as fsp from 'node:fs/promises';\nfsp.open(p, 'w').then(async function (h) {\n  await h.writeFile(station.name);\n});\n",
        ['m.ts:3 <module> > fsp.open().then -> <handle h>'],
      ],
      [
        'open().then((fh) => fh.write(x)), listed',
        "fs.promises.open(p, 'w').then((fh) => fh.write(station.name));\n",
        [`m.ts:1 [file] ${thenFn}: station.name`],
        { fileSinks: { 'm.ts': { [`${thenFn} -> <handle fh>`]: shareable } } },
      ],
    ];
    const found = Object.fromEntries(
      probes.map(([label, source, , group]) => {
        const result = check({ 'm.ts': source }, group);
        return [label, [...result.unchecked, ...result.unlisted]];
      }),
    );
    expect(found).toEqual(Object.fromEntries(probes.map(([label, , expected]) => [label, expected])));
    // Not a sink: an fs read taken by name, a write taken and never used, what else an imported process
    // hands out, a handle's read in a callback.
    const quiet = check({
      'm.ts':
        "const { readFileSync: r } = fs;\nr(p);\nimport { writeFileSync as w } from 'node:fs';\n" +
        "const { argv } = await import('node:process');\nconst home = (await import('process')).env.HOME;\n" +
        "fs.promises.open(p).then((fh) => fh.read(buffer));\n",
    });
    expect([...quiet.unchecked, ...quiet.unlisted]).toEqual([]);
  });

  // A FileHandle, or fs/promises' open, handed to a function of the same file (#576 W78-NF): one level, by name.
  const fspImport = "import * as fsp from 'node:fs/promises';";
  /** One source, `m.ts`, a line each. */
  const lines = (...source: string[]): Record<string, string> => ({ 'm.ts': source.map((line) => `${line}\n`).join('') });
  const writeItThen = lines(fspImport, 'async function writeIt(h) {', '  await h.write(station.name);', '}', "fsp.open(p, 'w').then(writeIt);");
  const shareableReport = { class: 'shareable' as const, what: 'report.md', reason: 'pasted into public issues' };
  // Each probe: a label, its sources, what the guard reports, and the lists it is checked against.
  const handedOn: [string, Record<string, string>, string[], Partial<SiteGroup>?][] = [
    // A `.then` given a function of this file rather than one written inline.
    ['open().then(writeIt), a function declaration', writeItThen, ['m.ts:3 writeIt -> <handle h>']],
    [
      'open().then(writeIt), a const arrow',
      lines(fspImport, 'const writeIt = async (h) => {', '  await h.writev([station.name]);', '};', "fsp.open(p, 'w').then(writeIt);"),
      ['m.ts:3 writeIt -> <handle h>'],
    ],
    [
      'open().then(writeIt), a const function expression writing with writeFile',
      lines('const writeIt = function (h) {', '  return h.writeFile(station.name);', '};', "fs.promises.open(p, 'w').then(writeIt);"),
      ['m.ts:2 writeIt -> <handle h>'],
    ],
    // A handle handed to a function of this file: declared after its call or before it, at any argument.
    [
      'doWrite(fh), declared after the call',
      lines(fspImport, "const fh = await fsp.open(p, 'w');", 'await doWrite(fh);', 'async function doWrite(out) {', '  await out.write(station.name);', '}'),
      ['m.ts:5 doWrite -> <handle out>'],
    ],
    [
      'doWrite(x, fh), a const arrow declared before, called in a function',
      lines(fspImport, 'const doWrite = async (text, out) => out.write(text);', 'async function main() {', "  const fh = await fsp.open(p, 'w');", '  await doWrite(station.name, fh);', '}'),
      ['m.ts:2 doWrite -> <handle out>'],
    ],
    [
      'doWrite(await open())',
      lines(fspImport, 'async function doWrite(out) {', '  await out.write(station.name);', '}', "await doWrite(await fsp.open(p, 'w'));"),
      ['m.ts:3 doWrite -> <handle out>'],
    ],
    [
      'open().then((fh) => doWrite(fh))',
      lines(fspImport, 'async function doWrite(out) {', '  await out.write(station.name);', '}', "fsp.open(p, 'w').then((fh) => doWrite(fh));"),
      ['m.ts:3 doWrite -> <handle out>'],
    ],
    [
      'writeAll(await open(), xs), recursing with its parameter',
      lines(fspImport, 'async function writeAll(h, items) {', '  if (items.length === 0) return;', '  await h.write(items[0]);', '  await writeAll(h, items.slice(1));', '}', "await writeAll(await fsp.open(p, 'w'), station.names);"),
      ['m.ts:4 writeAll -> <handle h>'],
    ],
    // open under another name.
    [
      'const o = fsp.open; (await o(p)).write(x)',
      lines(fspImport, 'const o = fsp.open;', "(await o(p, 'w')).write(station.name);"),
      ["m.ts:3 <module> -> <handle (await o(p, 'w'))>"],
    ],
    [
      'const o = fs.promises.open; const fh = await o(p)',
      lines('const o = fs.promises.open;', "const fh = await o(p, 'w');", 'await fh.write(station.name);'),
      ['m.ts:3 <module> -> <handle fh>'],
    ],
    [
      "import { open } from 'node:fs/promises'; const o = open",
      lines("import { open } from 'node:fs/promises';", 'const o = open;', "const fh = await o(p, 'w');", 'await fh.write(station.name);'),
      ['m.ts:4 <module> -> <handle fh>'],
    ],
    // open handed to a function of this file, which opens with its parameter.
    [
      'writeWith(fsp.open)',
      lines(fspImport, 'async function writeWith(openFile) {', "  const fh = await openFile(p, 'w');", '  await fh.write(station.name);', '}', 'await writeWith(fsp.open);'),
      ['m.ts:4 writeWith -> <handle fh>'],
    ],
    [
      'writeWith(o), o an alias of open',
      lines(fspImport, 'const o = fsp.open;', "const writeWith = async (openFile) => (await openFile(p, 'w')).write(station.name);", 'await writeWith(o);'),
      ["m.ts:3 writeWith -> <handle (await openFile(p, 'w'))>"],
    ],
    // A closure that names the handle is read by that name, file-wide, as every held handle is (before #576 W78-NF too).
    [
      'a closure kept for later that names the handle',
      lines(fspImport, "const fh = await fsp.open(p, 'w');", 'const later = () => fh.write(station.name);', 'await later();'),
      ['m.ts:3 later -> <handle fh>'],
    ],
    // Listed under the handle, what the function writes through it is checked.
    ['open().then(writeIt), listed', writeItThen, ['m.ts:3 [file] writeIt: station.name'], { fileSinks: { 'm.ts': { 'writeIt -> <handle h>': shareableReport } } }],
    // Not a sink: a function of this file handed something that is not a handle, or reading through one.
    [
      'not a sink: doWrite(child.stdin), open().then(readIt)',
      lines(fspImport, 'async function doWrite(out) {', '  await out.write(station.name);', '}', 'await doWrite(child.stdin);', 'async function readIt(h) {', '  await h.read(buffer);', '}', 'fsp.open(p).then(readIt);'),
      [],
    ],
    // UNCOVERED, a documented limit (scan.ts' header, docs/agent-harness.md): pinned so a change that follows one notices.
    [
      'UNCOVERED: a FileHandle handed to a function of another file',
      {
        'w.ts': 'export async function writeIt(h) {\n  await h.write(station.name);\n}\n',
        'm.ts': `${fspImport}\nimport { writeIt } from './w';\nconst fh = await fsp.open(p, 'w');\nawait writeIt(fh);\n`,
      },
      [],
    ],
    [
      'UNCOVERED: a higher-order wrapper handing its FileHandle to the function it was given',
      lines(fspImport, 'async function withHandle(file, use) {', "  const fh = await fsp.open(file, 'w');", '  return use(fh);', '}', 'async function writeIt(h) {', '  await h.write(station.name);', '}', 'await withHandle(p, writeIt);'),
      [],
    ],
    [
      'UNCOVERED: a FileHandle handed to a method',
      lines(fspImport, 'class Out {', '  async put(h) {', '    await h.write(station.name);', '  }', '  async run() {', "    await this.put(await fsp.open(p, 'w'));", '  }', '}'),
      [],
    ],
    ['UNCOVERED: a FileHandle stored in an object', lines("const fh = await fs.promises.open(p, 'w');", 'const box = { fh };', 'await box.fh.write(station.name);'), []],
    [
      'UNCOVERED: a FileHandle handed on a second time (two levels), the callees written first',
      lines(fspImport, 'async function inner(g) {', '  await g.write(station.name);', '}', 'async function outer(h) {', '  await inner(h);', '}', "const fh = await fsp.open(p, 'w');", 'await outer(fh);'),
      [],
    ],
  ];
  it.each(handedOn)('follows a FileHandle or open handed to a function of the same file (#576 W78-NF): %s', (_label, sources, expected, group) => {
    const result = check(sources, group);
    expect([...result.unchecked, ...result.unlisted]).toEqual(expected);
  });

  it("does not take the console's type under a qualified name for the console handed on (#576 W74-NF3)", () => {
    const quiet = check({ 'm.ts': 'type C = typeof globalThis.console;\ntype D = typeof x.console;\ntype L = typeof console.log;\n' });
    expect([...quiet.unchecked, ...quiet.unlisted]).toEqual([]);
  });

  it('holds a listed sink handed on to a declared wrapper, and a wrapper to its own file (#576 RV20-12)', () => {
    // A logger handed on and listed: its calls are sites only if the name it is held in is a declared wrapper.
    const alias = { 'a.ts': 'const a1 = console.log;\na1(station.name);\n' };
    const listed = { passthrough: { 'a.ts': { '<module> -> console.log': 'a logger handed on' } } };
    const unheld = check(alias, listed);
    expect([...unheld.unchecked, ...unheld.unlisted, ...(unheld.unguarded ?? [])]).toEqual([
      'a.ts:1 <module> -> console.log: held in a1, not a declared wrapper',
    ]);
    const held = check(alias, { ...listed, wrappers: { 'a.ts': { a1: 'stream' } } });
    expect([...held.unchecked, ...(held.unguarded ?? [])]).toEqual(['a.ts:2 [stream] <module>: station.name']);
    // The console itself in a wrapper: its methods are the wrapper's calls.
    const inWrapper = check(
      { 'c.ts': 'const c = console;\nc.error(station.name);\n' },
      { passthrough: { 'c.ts': { '<module> -> console': 'the console handed on' } }, wrappers: { 'c.ts': { c: 'stream' } } },
    );
    expect([...inWrapper.unchecked, ...(inWrapper.unguarded ?? [])]).toEqual(['c.ts:2 [stream] <module>: station.name']);
    // A wrapper exported and called from another file: there it is no sink at all.
    const exported = check(
      { 'say.ts': 'export function say(m) { console.log(m); }\n', 'b.ts': "import { say } from './say';\nsay(station.name);\n" },
      { wrappers: { 'say.ts': { say: 'stream' } }, allow: { 'say.ts': { 'say: m': R.FORWARD } } },
    );
    expect([...exported.unchecked, ...exported.unlisted, ...(exported.unguarded ?? [])]).toEqual([
      'say.ts:1 export say: a wrapper is a sink in its own file only',
    ]);
  });
});
