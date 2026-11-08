/**
 * #576 W51: every place the harness and the repo's Node tooling print or write
 * something, and every value each one prints, read off the source.
 * `printSites.test.ts` checks them against `rules.ts` and the group allow-lists:
 * a value reaches a shareable sink only through an approved helper or as a
 * value someone checked and gave a reason for. This file only finds the sites
 * and the values.
 *
 * A site is a sink call:
 * - `stream`: every `console` method (`console['log']` too; `console.assert`'s
 *   data, not its condition), `process.stdout.write` / `process.stderr.write`,
 *   `process.emitWarning`, each under the global object too
 *   (`globalThis.console.error`), `fs.writeSync` / `writevSync` / `write` /
 *   `writev` to descriptor 1 or 2, or a call of a function a file declares as a
 *   wrapper of one (`smoke.ts`' `log`, the repro report's `P`);
 * - `progress`: `<context>.log` under any name (`ctx.log`, `context.log`),
 *   printed through `shareableProgressLine`;
 * - `stream` too: the key of `<context>.metric` / `setMetric`, which the runner's
 *   PASS/FAIL line prints (`JSON.stringify(metrics.metrics)`);
 * - `error`: `new <Name>Error(message)` or `<Name>Error(message)` without `new`,
 *   whose message a runner prints through
 *   `shareableErrorText` / `scenarioFailOutput`, which keep its first line
 *   (under `unprojected` prefixes - `scripts/` - the whole message prints);
 * - `return`: a `return` of a function a file declares as a builder of shared
 *   text (the repro report's `describeAction`);
 * - `file`: `writeFileSync`, `writeFile`, `appendFileSync`, `appendFile`,
 *   `createWriteStream`, `writeSync` / `writevSync` / `fs.write` / `fs.writev`
 *   to another descriptor, a `.write` / `.end` on a stream one opened, a
 *   `.write` / `.writev` / `.writeFile` / `.appendFile` on a FileHandle one
 *   opened (`fs.promises.open`, or `open` under another name: `const o =
 *   fsp.open`), handed to a `.then` callback, or handed to a function of the
 *   same file (below), and the copies
 *   `copyFileSync`, `copyFile`, `cpSync`; each taken from fs by another name too
 *   (`const { writeFileSync: w } = fs`, `import { copyFile as c } from 'fs'`);
 * - `passthrough`: a child process's `stdio` (or `stdout` / `stderr` option)
 *   that hands it this process's stdout or stderr (`'inherit'`, `1`, `2`,
 *   `process.stdout`), an `execSync` / `execFileSync` that sets no `stdio`
 *   (its stderr is ours), a `.pipe(process.stdout)`, and a sink handed on as a
 *   value (`log = console.log`, `const out = process.stderr`, `const c =
 *   console`, `const { error } = console`, `const { stdout } = process`,
 *   `import { stdout } from 'node:process'`, anything imported from `console`,
 *   the same taken with `require` or a dynamic `import()`, `globalThis.console`,
 *   `promisify(fs.write)`, a file write handed on: `const w = fs.writeFileSync`).
 *
 * A value is followed through a `const` to what it was made from, through
 * a call of a local arrow function to what it returns, each parameter standing
 * for its argument, and through the `join` of a `const` array to what was
 * pushed into it (report.md's `md`), while nothing else can add to it.
 * `.length`, arithmetic, comparisons and `Math.*` are numbers and need no reason. A value is marked folded inside a `FOLDS` call, in a line
 * the runner projects (it folds every line), or as the parameter of a function a
 * `FOLDING_CALLBACKS` helper calls.
 *
 * A file stream or a FileHandle is followed by the name it is held in, file-wide
 * (a closure that names it is read by that name). A FileHandle, or fs/promises'
 * `open`, handed to a function of the same file is followed one level further
 * (#576 W78-NF): to the parameter it is given, of the function declaration or
 * `const` arrow or function expression the call names (`doWrite(fh)`,
 * `doWrite(await fsp.open(p))`, `writeWith(fsp.open)`, `fsp.open(p).then(writeIt)`),
 * so that parameter's writes are the handle's. Not followed, so a write that way
 * passes the guard unseen: a function of another file; a method (`this.write(fh)`,
 * `obj.write(fh)`); a function handed to a wrapper that calls it with the handle
 * (`withHandle(p, writeIt)`, whose `use(fh)` calls a parameter); a handle, or a
 * function that takes one, stored in an object or an array and used later
 * (`box.fh.write(..)`, `jobs.forEach((job) => job(fh))`), or a promise of one held
 * in a name (`pending.then(writeIt)`); a destructured or rest parameter, or an
 * argument after a spread; a function bound or wrapped (`.then(writeIt.bind(x))`);
 * and a parameter handed on again to another function, more than one level (one
 * that hands it to itself, recursing, is the same parameter). The names are read
 * in one pass in source order, so a chain whose caller is written before its
 * callees can happen to be followed further; nothing relies on that.
 * `printSites.test.ts` pins some of these as uncovered.
 *
 * Code inside a function handed to the page (`browser.execute(...)` and the
 * like) runs in the app's webview, not here: its `console.*` is the app's
 * console, which the log oracle reads. Those sites are `page`.
 */
import * as fs from 'fs';
import * as path from 'path';
import ts from 'typescript';

export type SinkKind = 'stream' | 'progress' | 'error' | 'return' | 'file' | 'passthrough' | 'page';

export interface PrintSite {
  /** Repo-relative, `/`-separated. */
  file: string;
  line: number;
  /** The function it is written in (`fnName`): the nearest named one or `<module>`, a callback as its call under it. */
  fn: string;
  kind: SinkKind;
  /** The callee as written (`process.stdout.write`), or `return` / `stdio`. */
  sink: string;
  /** `file` sites: the target argument as written. */
  target?: string;
  values: PrintValue[];
  /**
   * A `reference` passthrough (a sink handed on): the names it is held in, and
   * whether every one is a declared wrapper of the file, so that its calls are
   * checked sites (#576 RV20-12).
   */
  holders?: readonly string[];
  held?: boolean;
}

export interface PrintValue {
  /** The expression as written, whitespace collapsed. */
  text: string;
  line: number;
  /** Where it sits makes the runner withhold it: after the first `failed: ` of the first line, or on a later line. */
  withheld: boolean;
  /** The value is a call of this function (the callee's last name), if any. */
  callee?: string;
  /** The function the value is written in (a const or a local arrow function can be another than the site's): its allow key's first half. */
  fn: string;
  /** Inside a call of a folding helper (`foldLocalPaths`): a path in it prints folded. */
  folded: boolean;
}

export type WrapperKind = 'stream' | 'progress' | 'error';
/** A wrapper's kind; or its kind and the argument positions it prints (all of them by default). */
export type Wrapper = WrapperKind | { readonly kind: WrapperKind; readonly args: readonly number[] };

export interface ScanOptions {
  /** Per file: the functions whose calls are sinks of their own, and of which kind. */
  wrappers: ReadonlyMap<string, Readonly<Record<string, Wrapper>>>;
  /** Per file: the functions whose `return` values are shared text. */
  builders: ReadonlyMap<string, readonly string[]>;
  /** Callee names whose result is safe to print, wherever they are called. */
  helpers: ReadonlySet<string>;
  /** Per file: callee names whose result is safe in that file only. */
  fileHelpers: ReadonlyMap<string, readonly string[]>;
  /** Callee names that fold paths out of their argument, whose values are then checked as folded. */
  folds: ReadonlySet<string>;
  /**
   * Callee names that hand their callback only folded text (`passLinesFolded`):
   * the parameters of a function given to one print folded.
   */
  foldingCallbacks: ReadonlySet<string>;
  /**
   * Prefixes of files whose errors print whole: no runner projects them (a
   * script under `scripts/` dies with Node's own uncaught-error print).
   */
  unprojected: readonly string[];
}

/**
 * Every console method prints (`group`, `assert`, `count`, `timeLog` too), and so does a process warning,
 * under the global object too (`globalThis.console.error`, #576 W56-T1-NF1).
 */
const STREAM = /^(?:(?:globalThis|global|window)\.)?(?:console\.\w+|process\.(?:stdout|stderr)\.write|process\.emitWarning)$/;
/** The console under the global object (`const c = globalThis.console`): the console itself. */
const GLOBAL_CONSOLE = /^(?:globalThis|global|window)\.console$/;
/** This process's stdout or stderr, under the global object too. */
const STD_STREAM = /^(?:(?:globalThis|global|window)\.)?process\.(?:stdout|stderr)$/;
const FILE_WRITE = new Set(['writeFileSync', 'writeFile', 'appendFileSync', 'appendFile', 'createWriteStream', 'writeSync', 'writevSync']);
/** `fs.writeSync` / `writevSync` to one of these descriptors is stdout or stderr. */
const STD_FD = /^(?:1|2|process\.(?:stdout|stderr)\.fd)$/;
const FD_WRITE = new Set(['writeSync', 'writevSync']);
/** `fs.write` / `fs.writev`, the same writes with a callback: named only on the fs module, as `write` alone is any stream's. */
const FS_ASYNC_WRITE = new Set(['write', 'writev']);
/** What a FileHandle (`fs.promises.open`) writes with: the data is its first argument. */
const HANDLE_WRITE = new Set(['write', 'writev', 'writeFile', 'appendFile']);
/** A scenario's progress line, under whatever name its context has (`ctx.log`, `context.log`, `base.log`). */
const PROGRESS = /^(?!console\.|Math\.)[\w$]+(?:\.[\w$]+)*\.log$/;
/** A scenario's metric, under whatever name its context has: its key prints in the runner's PASS/FAIL line. */
const METRIC = /^[\w$]+(?:\.[\w$]+)*\.(?:metric|setMetric)$/;
/** `Error(message)` without `new` makes the same error: a constructor's name, capitalised. */
const ERROR_CALL = /(?:^|\.)(?:[A-Z]\w*)?Error$/;
/** Modules that hand out this process's streams or console under another name. */
const STD_MODULE = /^(?:node:)?(?:process|console)$/;
/**
 * What a destructure or a named import of `process` takes that prints: `default` too, which a module
 * namespace of it holds the whole process in (`const { default: p } = await import('process')`).
 */
const PROCESS_SINKS = new Set(['stdout', 'stderr', 'emitWarning', 'default']);
/** Copies: the target is the second argument, the content another file. */
const FILE_COPY = new Set(['copyFileSync', 'copyFile', 'cpSync']);
/** What fs and fs/promises hand out that writes or copies a file: a site where it is called, a sink handed on anywhere else. */
const FS_FILE_WRITE = new Set([...FILE_WRITE, ...FILE_COPY]);
/** Child processes whose stderr is this process's unless `stdio` says otherwise. */
const EXEC_INHERITS_STDERR = new Set(['execSync', 'execFileSync']);
/** A call whose function argument runs in the page. */
const PAGE_CALL = /(?:^|\.)(?:execute|executeAsync|executeScript|evaluate|evaluateHandle|addInitScript)$/;
/** Calls that only compute a number or a boolean. */
const NUMERIC_CALL =
  /^(?:Math\.\w+|Number(?:\.\w+)?|parseInt|parseFloat|Date\.now|performance\.now|process\.hrtime(?:\.bigint)?|Array\.isArray|Boolean|isFinite|isNaN)$/;
/** Methods that keep their receiver's text (or a number) and add only their own. */
const KEEPS_RECEIVER = new Set(['padStart', 'padEnd', 'trim', 'trimEnd', 'trimStart', 'toUpperCase', 'toLowerCase', 'toString']);
const NUMERIC_METHOD = new Set(['toFixed', 'toPrecision', 'getTime', 'indexOf', 'lastIndexOf', 'includes', 'startsWith', 'endsWith', 'test', 'has', 'some', 'every']);

/** The scanned files: the non-test JS/TS of `dirs`, repo-relative. */
export function scannedFiles(repo: string, dirs: readonly string[]): string[] {
  const out: string[] = [];
  const walk = (dir: string): void => {
    for (const entry of fs.readdirSync(path.join(repo, dir), { withFileTypes: true })) {
      const rel = `${dir}/${entry.name}`;
      if (entry.isDirectory()) {
        if (!['node_modules', 'target', 'replays'].includes(entry.name)) walk(rel);
      } else if (/\.(?:ts|mts|mjs|js|cjs)$/.test(entry.name) && !/\.(?:test|spec)\.|\.d\.m?ts$/.test(entry.name)) {
        out.push(rel);
      }
    }
  };
  for (const dir of dirs) walk(dir);
  return out.sort();
}

export function scanFile(repo: string, file: string, opts: ScanOptions): PrintSite[] {
  const abs = path.join(repo, file);
  const kind = /\.(?:ts|mts)$/.test(file) ? ts.ScriptKind.TS : ts.ScriptKind.JS;
  const source = ts.createSourceFile(abs, fs.readFileSync(abs, 'utf8'), ts.ScriptTarget.Latest, true, kind);
  const text = (node: ts.Node): string => node.getText(source).replace(/\s+/g, ' ');
  const lineOf = (node: ts.Node): number => source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1;
  const wrappers = opts.wrappers.get(file) ?? {};
  const builders = opts.builders.get(file) ?? [];
  const fileHelpers = new Set(opts.fileHelpers.get(file) ?? []);
  const errorsProjected = !opts.unprojected.some((prefix) => file.startsWith(prefix));
  const sites: PrintSite[] = [];

  /** A callee without its arguments (`actions.filter().map`): stable across edits of what it is called with. */
  const calleeName = (callee: ts.Expression): string => {
    if (ts.isIdentifier(callee) || callee.kind === ts.SyntaxKind.ThisKeyword) return callee.getText(source);
    if (ts.isPropertyAccessExpression(callee)) return `${calleeName(callee.expression)}.${callee.name.text}`;
    if (ts.isCallExpression(callee)) return `${calleeName(callee.expression)}()`;
    if (ts.isElementAccessExpression(callee)) return `${calleeName(callee.expression)}[]`;
    if (ts.isParenthesizedExpression(callee) || ts.isAwaitExpression(callee) || ts.isNonNullExpression(callee) || ts.isAsExpression(callee)) {
      return calleeName(callee.expression);
    }
    return '<expression>';
  };
  /**
   * The function a node is written in: the nearest named one (a declaration, a
   * method, a function held in a variable or a property), or `<module>`. A
   * function handed to a call as an argument is that call, under the function it
   * is written in (`<module> > steps.forEach`, `main > files.map > rows.forEach`):
   * a callback's names are its own, not its enclosing function's (#576 RV20-7).
   */
  const fnName = (node: ts.Node): string => {
    const calls: string[] = [];
    const named = (name: string): string => [name, ...calls].join(' > ');
    for (let at: ts.Node | undefined = node.parent; at; at = at.parent) {
      if ((ts.isFunctionDeclaration(at) || ts.isMethodDeclaration(at)) && at.name) return named(at.name.getText(source));
      if ((ts.isArrowFunction(at) || ts.isFunctionExpression(at)) && at.parent) {
        const holder: ts.Node = at.parent;
        if ((ts.isVariableDeclaration(holder) || ts.isPropertyAssignment(holder)) && holder.name) {
          return named(holder.name.getText(source));
        }
        if ((ts.isCallExpression(holder) || ts.isNewExpression(holder)) && holder.arguments?.includes(at)) {
          calls.unshift(`${ts.isNewExpression(holder) ? 'new ' : ''}${calleeName(holder.expression)}`);
        }
      }
    }
    return named('<module>');
  };
  const inPage = (node: ts.Node): boolean => {
    for (let at: ts.Node | undefined = node.parent; at; at = at.parent) {
      if ((ts.isArrowFunction(at) || ts.isFunctionExpression(at)) && at.parent && ts.isCallExpression(at.parent)) {
        if (PAGE_CALL.test(text(at.parent.expression)) && at.parent.arguments.includes(at as ts.Expression)) return true;
      }
    }
    return false;
  };
  /** The `const` this identifier names, declared in a scope that encloses `use`, if exactly one does. */
  const constInit = (id: ts.Identifier): ts.Expression | undefined => {
    const name = id.text;
    for (let scope: ts.Node | undefined = id.parent; scope; scope = scope.parent) {
      if (!(ts.isBlock(scope) || ts.isSourceFile(scope) || ts.isCaseClause(scope) || ts.isModuleBlock(scope))) continue;
      const found: ts.Expression[] = [];
      const statements = ts.isCaseClause(scope) ? scope.statements : scope.statements;
      for (const statement of statements) {
        if (!ts.isVariableStatement(statement)) continue;
        if (!(statement.declarationList.flags & ts.NodeFlags.Const)) continue;
        for (const decl of statement.declarationList.declarations) {
          if (ts.isIdentifier(decl.name) && decl.name.text === name && decl.initializer) found.push(decl.initializer);
        }
      }
      if (found.length === 1) return found[0];
      if (found.length > 1) return undefined;
    }
    return undefined;
  };

  const lastName = (callee: ts.Expression): string =>
    ts.isPropertyAccessExpression(callee) ? callee.name.text : ts.isIdentifier(callee) ? callee.text : text(callee);
  /** The names of functions handed to a folding callback (`passLinesFolded(out, write)`). */
  const foldedCallbackNames = new Set<string>();
  source.forEachChild(function handed(node: ts.Node): void {
    if (ts.isCallExpression(node) && opts.foldingCallbacks.has(lastName(node.expression))) {
      for (const arg of node.arguments) if (ts.isIdentifier(arg)) foldedCallbackNames.add(arg.text);
    }
    node.forEachChild(handed);
  });
  /** Is `id` a parameter of a function a folding callback calls, so what it holds is folded? */
  const foldedParameter = (id: ts.Identifier): boolean => {
    for (let at: ts.Node | undefined = id.parent; at; at = at.parent) {
      if (!ts.isFunctionLike(at)) continue;
      if (!at.parameters.some((param) => ts.isIdentifier(param.name) && param.name.text === id.text)) continue;
      const holder = at.parent;
      if (holder && ts.isCallExpression(holder) && opts.foldingCallbacks.has(lastName(holder.expression))) {
        return holder.arguments.some((arg) => arg === at);
      }
      return !!holder && ts.isVariableDeclaration(holder) && ts.isIdentifier(holder.name) && foldedCallbackNames.has(holder.name.text);
    }
    return false;
  };

  /** The local arrow functions being inlined, each parameter to the argument it stands for. */
  const bindings = new Map<ts.Node, Map<string, ts.Expression>>();
  /** The argument a parameter of an inlined arrow function stands for, if `id` is one. */
  const boundArgument = (id: ts.Identifier): ts.Expression | undefined => {
    for (let at: ts.Node | undefined = id.parent; at; at = at.parent) {
      if (!ts.isFunctionLike(at)) continue;
      const declares = at.parameters.some((param) => ts.isIdentifier(param.name) && param.name.text === id.text);
      if (declares) return bindings.get(at)?.get(id.text);
    }
    return undefined;
  };

  /**
   * An accumulator (`const md = []; md.push(..); md.join('\n')`): its elements,
   * then each argument a `push` on it adds, in source order, so a line pushed
   * past the helper that should print it (`P`) is a value of the join's site.
   * Only while every other use of it only reads it: one handed to a function,
   * assigned, or reordered (`unshift`, `sort`, `x[i] = ..`) can hold anything,
   * so its join stays one value that needs a reason of its own.
   */
  const pushedTo = new Map<ts.Node, readonly ts.Expression[] | null>();
  const READS = new Set(['join', 'length', 'slice', 'map', 'filter', 'forEach', 'some', 'every', 'includes', 'indexOf', 'find', 'at']);
  const accumulated = (receiver: ts.Expression): readonly ts.Expression[] | undefined => {
    const init = ts.isIdentifier(receiver) ? constInit(receiver) : undefined;
    if (!init || !ts.isArrayLiteralExpression(init)) return undefined;
    if (!pushedTo.has(init)) {
      const found: ts.Expression[] = [];
      let closed = true;
      // Its uses are in the block that declares it.
      let scope: ts.Node = init;
      while (!(ts.isBlock(scope) || ts.isSourceFile(scope) || ts.isCaseClause(scope) || ts.isModuleBlock(scope))) scope = scope.parent;
      scope.forEachChild(function uses(node: ts.Node): void {
        if (ts.isIdentifier(node) && node.text === (receiver as ts.Identifier).text && constInit(node) === init) {
          const parent = node.parent;
          const member = ts.isPropertyAccessExpression(parent) && parent.expression === node ? parent.name.text : undefined;
          if (member === 'push' && ts.isCallExpression(parent.parent) && parent.parent.expression === parent) {
            found.push(...parent.parent.arguments);
          } else if (
            (ts.isVariableDeclaration(parent) || ts.isPropertyAccessExpression(parent) || ts.isPropertyAssignment(parent)) &&
            parent.name === node
          ) {
            // Its own declaration, or a property of the same name.
          } else if (!(member !== undefined && READS.has(member)) && !ts.isSpreadElement(parent) && !(ts.isForOfStatement(parent) && parent.expression === node)) {
            closed = false;
          }
        }
        node.forEachChild(uses);
      });
      pushedTo.set(init, closed ? found : null);
    }
    const pushed = pushedTo.get(init);
    return pushed ? [...init.elements, ...pushed] : undefined;
  };

  /** One printed part: literal text, or a value. */
  type Part = { lit: string } | { expr: ts.Expression };
  const flatten = (node: ts.Expression): Part[] => {
    if (ts.isParenthesizedExpression(node)) return flatten(node.expression);
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) return [{ lit: node.text }];
    if (ts.isTemplateExpression(node)) {
      const parts: Part[] = [{ lit: node.head.text }];
      for (const span of node.templateSpans) parts.push({ expr: span.expression }, { lit: span.literal.text });
      return parts;
    }
    if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.PlusToken) {
      return [...flatten(node.left), ...flatten(node.right)];
    }
    // `[a, b].join(sep)`, or an accumulator's: the elements, the separator between them.
    const joined =
      ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression) && node.expression.name.text === 'join'
        ? ts.isArrayLiteralExpression(node.expression.expression)
          ? node.expression.expression.elements
          : accumulated(node.expression.expression)
        : undefined;
    if (joined && ts.isCallExpression(node)) {
      const sepArg = node.arguments[0];
      const sep = sepArg && ts.isStringLiteralLike(sepArg) ? sepArg.text : ',';
      const parts: Part[] = [];
      joined.forEach((el, i) => {
        if (i > 0) parts.push({ lit: sep });
        if (!ts.isSpreadElement(el) && !ts.isOmittedExpression(el)) parts.push(...flatten(el));
        else parts.push({ expr: el as ts.Expression });
      });
      return parts;
    }
    return [{ expr: node }];
  };

  const valuesOf = (node: ts.Expression, withheld: boolean, out: PrintValue[], seen: Set<ts.Node>, folded = false): void => {
    if (seen.has(node)) return;
    seen.add(node);
    const add = (n: ts.Expression, callee?: string): void => {
      out.push({ text: text(n), line: lineOf(n), withheld, ...(callee ? { callee } : {}), folded, fn: fnName(n) });
    };
    if (ts.isParenthesizedExpression(node) || ts.isAsExpression(node) || ts.isNonNullExpression(node)) {
      return valuesOf(node.expression, withheld, out, seen, folded);
    }
    if (ts.isAwaitExpression(node)) return valuesOf(node.expression, withheld, out, seen, folded);
    if (ts.isTypeOfExpression(node) || ts.isVoidExpression(node)) return;
    if (
      ts.isStringLiteralLike(node) ||
      ts.isNumericLiteral(node) ||
      ts.isBigIntLiteral(node) ||
      node.kind === ts.SyntaxKind.TrueKeyword ||
      node.kind === ts.SyntaxKind.FalseKeyword ||
      node.kind === ts.SyntaxKind.NullKeyword ||
      (ts.isIdentifier(node) && node.text === 'undefined')
    ) {
      return;
    }
    if (ts.isPrefixUnaryExpression(node) || ts.isPostfixUnaryExpression(node)) return;
    if (ts.isTemplateExpression(node) || (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.PlusToken)) {
      for (const part of flatten(node)) if ('expr' in part) valuesOf(part.expr, withheld, out, seen, folded);
      return;
    }
    if (ts.isBinaryExpression(node)) {
      const op = node.operatorToken.kind;
      if (op === ts.SyntaxKind.QuestionQuestionToken || op === ts.SyntaxKind.BarBarToken) {
        valuesOf(node.left, withheld, out, seen, folded);
        valuesOf(node.right, withheld, out, seen, folded);
        return;
      }
      if (op === ts.SyntaxKind.AmpersandAmpersandToken) return valuesOf(node.right, withheld, out, seen, folded);
      if (op === ts.SyntaxKind.CommaToken) return valuesOf(node.right, withheld, out, seen, folded);
      // Arithmetic, comparison, `in`, `instanceof`: a number or a boolean.
      return;
    }
    if (ts.isConditionalExpression(node)) {
      valuesOf(node.whenTrue, withheld, out, seen, folded);
      valuesOf(node.whenFalse, withheld, out, seen, folded);
      return;
    }
    if (ts.isPropertyAccessExpression(node) && ['length', 'size', 'byteLength'].includes(node.name.text)) return;
    if (ts.isIdentifier(node)) {
      const bound = boundArgument(node);
      if (bound) return valuesOf(bound, withheld, out, seen, folded);
      const init = constInit(node);
      if (init && !ts.isArrowFunction(init) && !ts.isFunctionExpression(init)) return valuesOf(init, withheld, out, seen, folded);
      if (!folded && foldedParameter(node)) {
        out.push({ text: text(node), line: lineOf(node), withheld, folded: true, fn: fnName(node) });
        return;
      }
      return add(node);
    }
    if (ts.isArrayLiteralExpression(node)) {
      for (const el of node.elements) {
        if (ts.isSpreadElement(el)) add(el.expression);
        else if (!ts.isOmittedExpression(el)) valuesOf(el, withheld, out, seen, folded);
      }
      return;
    }
    if (ts.isCallExpression(node)) {
      const calleeText = text(node.expression);
      const last = ts.isPropertyAccessExpression(node.expression)
        ? node.expression.name.text
        : ts.isIdentifier(node.expression)
          ? node.expression.text
          : calleeText;
      if (opts.folds.has(last)) {
        for (const arg of node.arguments) valuesOf(arg, withheld, out, seen, true);
        return;
      }
      if (opts.helpers.has(last) || fileHelpers.has(last)) return;
      if (NUMERIC_CALL.test(calleeText)) return;
      if (ts.isPropertyAccessExpression(node.expression)) {
        const receiver = node.expression.expression;
        if (NUMERIC_METHOD.has(last)) return;
        if (KEEPS_RECEIVER.has(last)) return valuesOf(receiver, withheld, out, seen, folded);
      }
      if (calleeText === 'String' && node.arguments[0]) return valuesOf(node.arguments[0], withheld, out, seen, folded);
      // A local arrow function: what it returns, each parameter standing for the argument it was called with.
      if (ts.isIdentifier(node.expression)) {
        const local = constInit(node.expression);
        if (local && ts.isArrowFunction(local) && !bindings.has(local)) {
          const args = new Map<string, ts.Expression>();
          local.parameters.forEach((param, i) => {
            const given = node.arguments[i] ?? param.initializer;
            if (ts.isIdentifier(param.name) && given) args.set(param.name.text, given);
          });
          bindings.set(local, args);
          // A fresh `seen`: the same body inlined for another call has other arguments.
          const bodySeen = new Set<ts.Node>();
          if (!ts.isBlock(local.body)) valuesOf(local.body, withheld, out, bodySeen, folded);
          else {
            local.body.forEachChild(function returns(child: ts.Node): void {
              if (ts.isReturnStatement(child) && child.expression) valuesOf(child.expression, withheld, out, bodySeen, folded);
              else if (!ts.isFunctionLike(child)) child.forEachChild(returns);
            });
          }
          bindings.delete(local);
          return;
        }
      }
      // A join of several parts, or an accumulator's (however few it holds), is its parts.
      const parts = flatten(node);
      const ofAccumulator = ts.isPropertyAccessExpression(node.expression) && last === 'join' && !!accumulated(node.expression.expression);
      if (parts.length > 1 || ofAccumulator) {
        for (const part of parts) if ('expr' in part) valuesOf(part.expr, withheld, out, seen, folded);
        return;
      }
      // A call of a parameter (`guarded`'s `what()`) runs whatever its caller passed, so the same text in
      // another function is another value: keyed by its function, as every value is (`valueKey`).
      return add(node, last);
    }
    add(node);
  };

  /**
   * The values of a message, each marked withheld where the runner withholds
   * it: on the first line after its first ` failed: `, or on any later line
   * that is not one of the shapes the projection keeps (`reproduce:`,
   * `trace:`, a diff path `at ...`, a `last N ...:` heading).
   */
  const messageValues = (node: ts.Expression, projected: boolean): PrintValue[] => {
    const out: PrintValue[] = [];
    const seen = new Set<ts.Node>();
    let lineNo = 0;
    let lineText = '';
    let failedSeen = false;
    for (const part of flatten(node)) {
      if ('lit' in part) {
        const pieces = part.lit.split('\n');
        if (pieces.length > 1) {
          lineNo += pieces.length - 1;
          lineText = pieces[pieces.length - 1] ?? '';
        } else lineText += part.lit;
        if (lineNo === 0 && / failed: /.test(lineText)) failedSeen = true;
        continue;
      }
      const keptShape = /^\s*(?:reproduce|trace|at|last)\b/.test(lineText);
      const withheld = projected && ((lineNo === 0 && failedSeen) || (lineNo > 0 && !keptShape));
      valuesOf(part.expr, withheld, out, seen, projected);
      lineText += '\u0000';
    }
    return out;
  };

  const site = (node: ts.Node, kind: SinkKind, sink: string, values: PrintValue[], target?: string): void => {
    sites.push({ file, line: lineOf(node), fn: fnName(node), kind, sink, values, ...(target ? { target } : {}) });
  };
  /** The names a sink handed on is held in (`log = console.log`, `const { error } = console`); none when it is passed or stored elsewhere. */
  const holdersOf = (node: ts.Node): string[] => {
    if (ts.isBindingElement(node) || ts.isImportSpecifier(node) || ts.isNamespaceImport(node)) return [node.name.getText(source)];
    if (ts.isImportClause(node)) return node.name ? [node.name.text] : [];
    let at: ts.Node = node;
    while (ts.isParenthesizedExpression(at.parent) || ts.isAsExpression(at.parent) || ts.isNonNullExpression(at.parent)) at = at.parent;
    const holder = at.parent;
    if ((ts.isVariableDeclaration(holder) || ts.isParameter(holder)) && holder.initializer === at) {
      if (ts.isIdentifier(holder.name)) return [holder.name.text];
      return ts.isObjectBindingPattern(holder.name) ? holder.name.elements.map((element) => element.name.getText(source)) : [];
    }
    const assigned = ts.isBinaryExpression(holder) && holder.operatorToken.kind === ts.SyntaxKind.EqualsToken && holder.right === at;
    return assigned && ts.isIdentifier(holder.left) ? [holder.left.text] : [];
  };
  /** A sink handed on: a passthrough to list, held only where a declared wrapper makes its calls sites. */
  const reference = (node: ts.Node, target: string): void => {
    const holders = holdersOf(node);
    const held = holders.length > 0 && holders.every((name) => Object.hasOwn(wrappers, name));
    sites.push({ file, line: lineOf(node), fn: fnName(node), kind: 'passthrough', sink: 'reference', values: [], target, holders, held });
  };

  // The fs module and fs/promises under the names the file gives them (`fs` itself, `import * as
  // nodeFs from 'node:fs'`, `const fsp = require('fs/promises')`, `const fsp = fs.promises`, `const fsp =
  // await import('fs/promises')`), and `write` / `writev` / `open` taken from them by name (#576
  // W56-T1-NF1), and a file write or copy taken by name (`const { writeFileSync: w } = fs`, `import {
  // copyFile as c } from 'node:fs/promises'`), each local name to what it was taken as (#576 W74-NF1).
  const fsModules = new Set(['fs']);
  const fsPromisesModules = new Set<string>();
  const fsWrites = new Map<string, string>();
  const fsOpens = new Set<string>();
  const fsFileWrites = new Map<string, string>();
  /**
   * The module a `require('<module>')` or a dynamic `import('<module>')` loads, without `node:`; and
   * whether it is the import, whose call is a promise of the module (#576 W74-NF1).
   */
  const loaded = (expr: ts.Expression): { module: string; imported: boolean } | undefined => {
    if (!ts.isCallExpression(expr)) return undefined;
    const imported = expr.expression.kind === ts.SyntaxKind.ImportKeyword;
    if (!imported && !(ts.isIdentifier(expr.expression) && expr.expression.text === 'require')) return undefined;
    const spec = expr.arguments[0];
    return spec && ts.isStringLiteralLike(spec) ? { module: spec.text.replace(/^node:/, ''), imported } : undefined;
  };
  /** The module an expression is: `require('<module>')` or `await import('<module>')` (without `node:`), or a name or `.promises` of fs. */
  const moduleOf = (expr: ts.Expression): string | undefined => {
    let inner = expr;
    while (ts.isParenthesizedExpression(inner)) inner = inner.expression;
    const awaited = ts.isAwaitExpression(inner);
    if (ts.isAwaitExpression(inner)) inner = inner.expression;
    while (ts.isParenthesizedExpression(inner)) inner = inner.expression;
    const load = loaded(inner);
    // A dynamic import is the module once awaited; un-awaited it is a promise of it.
    if (load) return load.imported && !awaited ? undefined : load.module;
    if (ts.isIdentifier(expr)) return fsModules.has(expr.text) ? 'fs' : fsPromisesModules.has(expr.text) ? 'fs/promises' : undefined;
    if (ts.isPropertyAccessExpression(expr) && expr.name.text === 'promises' && moduleOf(expr.expression) === 'fs') return 'fs/promises';
    return undefined;
  };
  /** Note `local` as a name for `module`, or for what it takes from it (`taken`). */
  const bindModule = (module: string, local: string, taken?: string): void => {
    const name = module.replace(/^node:/, '');
    if (name === 'fs' && taken === undefined) fsModules.add(local);
    else if ((name === 'fs' && taken === 'promises') || (name === 'fs/promises' && taken === undefined)) fsPromisesModules.add(local);
    else if (name === 'fs' && taken !== undefined && FS_ASYNC_WRITE.has(taken)) fsWrites.set(local, taken);
    else if (name === 'fs/promises' && taken === 'open') fsOpens.add(local);
    else if ((name === 'fs' || name === 'fs/promises') && taken !== undefined && FS_FILE_WRITE.has(taken)) fsFileWrites.set(local, taken);
  };
  /** A destructure of `module`: each name it takes, and fs's `promises` destructured in turn (`const { promises: { writeFile: w } } = fs`). */
  const bindPattern = (module: string, pattern: ts.ObjectBindingPattern): void => {
    for (const element of pattern.elements) {
      const taken = text(element.propertyName ?? element.name);
      if (!ts.isObjectBindingPattern(element.name)) bindModule(module, element.name.getText(source), taken);
      else if (module === 'fs' && taken === 'promises') bindPattern('fs/promises', element.name);
    }
  };
  source.forEachChild(function modules(node: ts.Node): void {
    if (ts.isImportDeclaration(node) && ts.isStringLiteral(node.moduleSpecifier) && node.importClause) {
      const specifier = node.moduleSpecifier.text;
      const { name, namedBindings } = node.importClause;
      if (name) bindModule(specifier, name.text);
      if (namedBindings && ts.isNamespaceImport(namedBindings)) bindModule(specifier, namedBindings.name.text);
      if (namedBindings && ts.isNamedImports(namedBindings)) {
        for (const element of namedBindings.elements) bindModule(specifier, element.name.text, (element.propertyName ?? element.name).text);
      }
    }
    const held = ts.isVariableDeclaration(node) && node.initializer ? moduleOf(node.initializer) : undefined;
    if (held !== undefined && ts.isVariableDeclaration(node)) {
      if (ts.isIdentifier(node.name)) bindModule(held, node.name.text);
      if (ts.isObjectBindingPattern(node.name)) bindPattern(held, node.name);
    }
    node.forEachChild(modules);
  });
  /** `fs.write(fd, ..)` / `fs.writev(fd, ..)` under any name the file gives fs, or taken from it (`write(fd, ..)`). */
  const asyncFdWrite = (callee: ts.Expression): boolean =>
    ts.isPropertyAccessExpression(callee)
      ? FS_ASYNC_WRITE.has(callee.name.text) && moduleOf(callee.expression) === 'fs'
      : ts.isIdentifier(callee) && fsWrites.has(callee.text);
  /** fs or fs/promises, under any name the file gives it. */
  const isFsModule = (expr: ts.Expression): boolean => ['fs', 'fs/promises'].includes(moduleOf(expr) ?? '');
  /** fs/promises' `open` itself: `fsp.open`, `fs.promises.open`, `open` taken from it, or another name for one (#576 W78-NF). */
  const isOpen = (expr: ts.Expression): boolean =>
    ts.isPropertyAccessExpression(expr)
      ? expr.name.text === 'open' && moduleOf(expr.expression) === 'fs/promises'
      : ts.isIdentifier(expr) && fsOpens.has(expr.text);
  /** `await fs.promises.open(..)`, `fsp.open(..)`, `open(..)` taken from fs/promises: a FileHandle. */
  const opensHandle = (expr: ts.Expression): boolean => {
    let at = expr;
    while (ts.isAwaitExpression(at) || ts.isParenthesizedExpression(at)) at = at.expression;
    return ts.isCallExpression(at) && isOpen(at.expression);
  };
  /**
   * The function a name calls, declared in this file in a scope that encloses the call: a function
   * declaration, or a `const` arrow function or function expression, if that scope declares the name once
   * and no parameter between hides it (#576 W78-NF).
   */
  const localFunction = (id: ts.Identifier): ts.FunctionDeclaration | ts.ArrowFunction | ts.FunctionExpression | undefined => {
    for (let scope: ts.Node | undefined = id.parent; scope; scope = scope.parent) {
      if (ts.isFunctionLike(scope) && scope.parameters.some((param) => ts.isIdentifier(param.name) && param.name.text === id.text)) return undefined;
      if (!(ts.isBlock(scope) || ts.isSourceFile(scope) || ts.isCaseClause(scope) || ts.isModuleBlock(scope))) continue;
      const found: ts.Node[] = [];
      for (const statement of scope.statements) {
        if (ts.isFunctionDeclaration(statement) && statement.body && statement.name?.text === id.text) found.push(statement);
        if (!ts.isVariableStatement(statement)) continue;
        const constant = (statement.declarationList.flags & ts.NodeFlags.Const) !== 0;
        for (const decl of statement.declarationList.declarations) {
          // Any other declaration of the name hides the functions outside.
          if (ts.isIdentifier(decl.name) && decl.name.text === id.text) found.push(constant && decl.initializer ? decl.initializer : decl);
        }
      }
      if (found.length > 1) return undefined;
      const [only] = found;
      if (only) return ts.isFunctionDeclaration(only) || ts.isArrowFunction(only) || ts.isFunctionExpression(only) ? only : undefined;
    }
    return undefined;
  };
  /**
   * A call of a function of this file (`doWrite(fh)`, `writeWith(fsp.open)`): the parameter each argument
   * `is` holds for is noted in `names`, so that inside it stands for what it was given (#576 W78-NF). One
   * level: by name, as the handles are, and only what the pass has already noted by the call.
   */
  const handOn = (call: ts.CallExpression, is: (arg: ts.Expression) => boolean, names: Set<string>): void => {
    const callee = ts.isIdentifier(call.expression) && call.arguments.some(is) ? localFunction(call.expression) : undefined;
    if (!callee) return;
    for (let i = 0; i < call.arguments.length; i++) {
      const arg = call.arguments[i];
      // After a spread no argument is known to be at its parameter.
      if (!arg || ts.isSpreadElement(arg)) return;
      const param = callee.parameters[i];
      if (param && !param.dotDotDotToken && ts.isIdentifier(param.name) && is(arg)) names.add(param.name.text);
    }
  };
  // open under another name (`const o = fsp.open`), or handed to a function of this file (`writeWith(fsp.open)`):
  // that name, or the parameter it is given, opens a FileHandle too (#576 W78-NF). Before the handles below,
  // so that a handle the parameter opens is one.
  source.forEachChild(function opens(node: ts.Node): void {
    if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.initializer && isOpen(node.initializer)) fsOpens.add(node.name.text);
    if (ts.isCallExpression(node)) handOn(node, isOpen, fsOpens);
    node.forEachChild(opens);
  });

  // What a file stream is held in (`const log = fs.createWriteStream(..)`, `this.driverLog = ..`):
  // its `.write(..)` calls are writes to that file. And what a FileHandle is held in
  // (`const fh = await fs.promises.open(..)`): its `.write(..)`, `.writeFile(..)` too.
  const fileStreams = new Set<string>();
  const fileHandles = new Set<string>();
  source.forEachChild(function holders(node: ts.Node): void {
    const init = ts.isVariableDeclaration(node)
      ? node.initializer
      : ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.EqualsToken
        ? node.right
        : undefined;
    const holder = ts.isVariableDeclaration(node) ? node.name : ts.isBinaryExpression(node) ? node.left : undefined;
    const opens =
      init &&
      ts.isCallExpression(init) &&
      (/(?:^|\.)createWriteStream$/.test(text(init.expression)) ||
        (ts.isIdentifier(init.expression) && fsFileWrites.get(init.expression.text) === 'createWriteStream'));
    // Or another name for one (`const driverLog = this.driverLog`), declared after it.
    if (holder && init && (opens || fileStreams.has(text(init)))) fileStreams.add(text(holder));
    if (holder && init && (opensHandle(init) || fileHandles.has(text(init)))) fileHandles.add(text(holder));
    // Or the handle a callback is given (`fsp.open(p).then((fh) => fh.write(..))`, #576 W74-NF1), written
    // inline or a function of this file (`.then(writeIt)`, #576 W78-NF).
    if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression) && node.expression.name.text === 'then' && opensHandle(node.expression.expression)) {
      const callback = node.arguments[0];
      const inline = callback && (ts.isArrowFunction(callback) || ts.isFunctionExpression(callback)) ? callback : undefined;
      const param = (inline ?? (callback && ts.isIdentifier(callback) ? localFunction(callback) : undefined))?.parameters[0];
      if (param && ts.isIdentifier(param.name)) fileHandles.add(param.name.text);
    }
    // Or the parameter a handle is handed to, of a function of this file (`doWrite(fh)`, `doWrite(await fsp.open(p))`, #576 W78-NF).
    if (ts.isCallExpression(node)) handOn(node, (arg) => fileHandles.has(text(arg)) || opensHandle(arg), fileHandles);
    node.forEachChild(holders);
  });

  const visit = (node: ts.Node): void => {
    if (
      ts.isCallExpression(node) &&
      ts.isPropertyAccessExpression(node.expression) &&
      ['write', 'end'].includes(node.expression.name.text) &&
      fileStreams.has(text(node.expression.expression))
    ) {
      const values = node.arguments.slice(0, 1).flatMap((arg) => messageValues(arg, false));
      site(node, 'file', text(node.expression), values, `<stream ${text(node.expression.expression)}>`);
    }
    if (ts.isCallExpression(node)) {
      const callee = text(node.expression);
      // A file write or copy taken from fs by another name (`const { writeFileSync: w } = fs`) is that write.
      const taken = ts.isIdentifier(node.expression) ? fsFileWrites.get(node.expression.text) : undefined;
      const last = ts.isPropertyAccessExpression(node.expression) ? node.expression.name.text : (taken ?? callee);
      // The callee's name without whitespace or arguments (`context\n  .log` is `context.log`).
      const named = calleeName(node.expression);
      // A stream wrapper that holds the console or a stream (`const c = console`): its methods print too.
      const receiver = ts.isPropertyAccessExpression(node.expression) && ts.isIdentifier(node.expression.expression) ? node.expression.expression.text : '';
      const held = Object.hasOwn(wrappers, receiver) ? wrappers[receiver] : undefined;
      const wrapper = Object.hasOwn(wrappers, callee)
        ? wrappers[callee]
        : (typeof held === 'object' ? held.kind : held) === 'stream'
          ? 'stream'
          : undefined;
      const wrapped = typeof wrapper === 'object' ? wrapper.kind : wrapper;
      const printed = (args: readonly ts.Expression[]): readonly ts.Expression[] =>
        typeof wrapper === 'object'
          ? args.filter((_, i) => wrapper.args.includes(i))
          : named === 'console.assert'
            ? args.slice(1) // console.assert(condition, ...data) prints data, not the condition
            : args;
      const [first, ...rest] = node.arguments;
      const fdWrite = FD_WRITE.has(last) || asyncFdWrite(node.expression);
      // On a handle held in a name, or straight on the open (`(await fsp.open(p, 'w')).write(..)`, #576 W74-NF1).
      const handleWrite =
        ts.isPropertyAccessExpression(node.expression) &&
        HANDLE_WRITE.has(node.expression.name.text) &&
        (fileHandles.has(text(node.expression.expression)) || opensHandle(node.expression.expression));
      // `throw Error(message)`: the same error without `new`.
      if (ERROR_CALL.test(named) && first) site(node, 'error', callee, messageValues(first, errorsProjected));
      if (METRIC.test(named)) {
        // A metric's key prints in the PASS/FAIL line (`JSON.stringify(metrics.metrics)`); its value is a number.
        site(node, 'stream', callee, node.arguments.slice(0, 1).flatMap((arg) => messageValues(arg, false)));
      } else if (fdWrite && first && STD_FD.test(text(first))) {
        // fs.writeSync(1, text), fs.write(1, text, cb): stdout, and what it writes is the second argument.
        site(node, 'stream', callee, rest.slice(0, 1).flatMap((arg) => messageValues(arg, false)));
      } else if (STREAM.test(named) || wrapped === 'stream') {
        const values = printed(node.arguments).flatMap((arg) => messageValues(arg, false));
        site(node, inPage(node) ? 'page' : 'stream', callee, values);
      } else if (PROGRESS.test(named) || wrapped === 'progress') {
        site(node, 'progress', callee, printed(node.arguments).flatMap((arg) => messageValues(arg, true)));
      } else if (wrapped === 'error') {
        // An error wrapper prints its first argument, unless it names others.
        const message = typeof wrapper === 'object' ? printed(node.arguments) : node.arguments.slice(0, 1);
        site(node, 'error', callee, message.flatMap((arg) => messageValues(arg, errorsProjected)));
      } else if (handleWrite && ts.isPropertyAccessExpression(node.expression)) {
        // fh.write(data), fh.writeFile(data): the file the handle was opened on, and the data first.
        const values = node.arguments.slice(0, 1).flatMap((arg) => messageValues(arg, false));
        site(node, 'file', callee, values, `<handle ${text(node.expression.expression)}>`);
      } else if (FILE_WRITE.has(last) || fdWrite) {
        const values = rest.slice(0, 1).flatMap((arg) => messageValues(arg, false));
        site(node, 'file', callee, values, first ? text(first) : '');
      } else if (FILE_COPY.has(last)) {
        const target = node.arguments[1];
        site(node, 'file', callee, [], target ? text(target) : '');
      } else if (last === 'pipe' && node.arguments.some((a) => /^process\.(?:stdout|stderr)$/.test(text(a)))) {
        site(node, 'passthrough', callee, [], node.arguments.map(text).join(', '));
      } else if (EXEC_INHERITS_STDERR.has(last)) {
        // execSync(command, options); execFileSync(file, args?, options?).
        const second = node.arguments[1];
        const options =
          last === 'execSync' || (second !== undefined && ts.isObjectLiteralExpression(second)) ? second : node.arguments[2];
        const literal = options && ts.isObjectLiteralExpression(options) ? options : undefined;
        const setsStdio = literal?.properties.some((prop) => prop.name !== undefined && text(prop.name) === 'stdio');
        if (!setsStdio) site(node, 'passthrough', callee, [], literal || !options ? '<stderr by default>' : `<options ${text(options)}>`);
      }
    }
    // A sink handed on rather than called (`log = console.log`, `.catch(console.error)`):
    // whatever it is later called with prints.
    if (
      ts.isPropertyAccessExpression(node) &&
      STREAM.test(text(node)) &&
      !(ts.isCallExpression(node.parent) && node.parent.expression === node)
    ) {
      reference(node, text(node));
    }
    // An fd write handed on (`promisify(fs.write)`): whatever it is later called with is written.
    if (ts.isPropertyAccessExpression(node) && asyncFdWrite(node) && !(ts.isCallExpression(node.parent) && node.parent.expression === node)) {
      reference(node, text(node));
    }
    // A file write or copy handed on (`const w = fs.writeFileSync`, `promisify(fsp.writeFile)`), or one taken from
    // fs by name and handed on under that name (`const w = writeFileSync`, `promisify(write)`): whatever it is
    // later called with is written, and no call of it is a site (#576 W74-NF1).
    const called = node.parent !== undefined && ts.isCallExpression(node.parent) && node.parent.expression === node;
    if (!called && ts.isPropertyAccessExpression(node) && FS_FILE_WRITE.has(node.name.text) && isFsModule(node.expression)) {
      reference(node, text(node));
    }
    const takenFromFs = ts.isIdentifier(node) ? (fsFileWrites.get(node.text) ?? fsWrites.get(node.text)) : undefined;
    if (takenFromFs !== undefined && !called) {
      const parent = node.parent;
      // Its own declaration (`import { writeFileSync as w }`, `const { writeFileSync: w } = fs`), a property or a
      // declaration of the same name, or its type.
      const ownName =
        ts.isImportSpecifier(parent) ||
        (ts.isBindingElement(parent) && (parent.name === node || parent.propertyName === node)) ||
        ((ts.isPropertyAccessExpression(parent) ||
          ts.isPropertyAssignment(parent) ||
          ts.isVariableDeclaration(parent) ||
          ts.isParameter(parent) ||
          ts.isFunctionDeclaration(parent) ||
          ts.isMethodDeclaration(parent)) &&
          parent.name === node) ||
        ts.isTypeQueryNode(parent) ||
        ts.isQualifiedName(parent);
      if (!ownName) reference(node, `fs.${takenFromFs}`);
    }
    // The stream itself handed on (`const out = process.stderr`, `stdio: [.., process.stderr]`).
    if (
      ts.isPropertyAccessExpression(node) &&
      STD_STREAM.test(text(node)) &&
      !ts.isPropertyAccessExpression(node.parent) &&
      !(ts.isCallExpression(node.parent) && /\.pipe$/.test(text(node.parent.expression))) &&
      !(ts.isArrayLiteralExpression(node.parent) && ts.isPropertyAssignment(node.parent.parent) && text(node.parent.parent.name) === 'stdio')
    ) {
      reference(node, text(node));
    }
    if (ts.isNewExpression(node) && /(?:^|\.)\w*Error$/.test(text(node.expression)) && node.arguments?.[0]) {
      site(node, 'error', `new ${text(node.expression)}`, messageValues(node.arguments[0], errorsProjected));
    }
    // The console itself handed on (`const c = console`, `const { error } = console`, `run(console)`),
    // under the global object too (`const c = globalThis.console`).
    const consoleOf =
      (ts.isIdentifier(node) && node.text === 'console') || (ts.isPropertyAccessExpression(node) && GLOBAL_CONSOLE.test(text(node)))
        ? node.parent
        : undefined;
    // `console.x` is a call or a member reference, found above; the rest is the console itself.
    if (consoleOf && !(ts.isPropertyAccessExpression(consoleOf) && consoleOf.expression === node) && !inPage(node)) {
      const parent = consoleOf;
      const ownName =
        (ts.isPropertyAccessExpression(parent) && (parent.expression === node || parent.name === node)) ||
        ((ts.isVariableDeclaration(parent) || ts.isParameter(parent) || ts.isBindingElement(parent) || ts.isPropertyAssignment(parent)) &&
          parent.name === node) ||
        ts.isTypeOfExpression(parent) ||
        ts.isTypeQueryNode(parent) ||
        // A name in a type (`typeof globalThis.console`, `typeof console.log`): never a value (#576 W74-NF3).
        ts.isQualifiedName(parent);
      const element = ts.isElementAccessExpression(parent) && parent.expression === node ? parent : undefined;
      const call = element && ts.isCallExpression(element.parent) && element.parent.expression === element ? element.parent : undefined;
      // `console[method] = ..` (mcp/hygiene.ts) replaces a method: whatever replaces it prints and is checked there.
      const patched =
        element && ts.isBinaryExpression(element.parent) && element.parent.operatorToken.kind === ts.SyntaxKind.EqualsToken && element.parent.left === element;
      if (call) site(call, 'stream', text(call.expression), call.arguments.flatMap((arg) => messageValues(arg, false)));
      else if (!ownName && !patched) reference(node, text(node));
    }
    // `const { stdout } = process`: the stream (or `emitWarning`) under its own name.
    if (ts.isVariableDeclaration(node) && node.initializer && ts.isIdentifier(node.initializer) && node.initializer.text === 'process' && ts.isObjectBindingPattern(node.name)) {
      for (const element of node.name.elements) {
        const taken = text(element.propertyName ?? element.name);
        if (PROCESS_SINKS.has(taken)) reference(element, `process.${taken}`);
      }
    }
    // `import { stdout } from 'node:process'`, `import * as proc from 'process'`, anything from 'console'.
    if (ts.isImportDeclaration(node) && ts.isStringLiteral(node.moduleSpecifier) && STD_MODULE.test(node.moduleSpecifier.text)) {
      const module = node.moduleSpecifier.text.replace(/^node:/, '');
      const clause = node.importClause;
      const bindings = clause?.namedBindings;
      if (clause?.name && (module === 'console' || clause.name.text !== 'process')) {
        reference(clause, `import ${clause.name.text} from '${module}'`);
      }
      if (bindings && ts.isNamespaceImport(bindings)) reference(bindings, `import * from '${module}'`);
      if (bindings && ts.isNamedImports(bindings)) {
        for (const element of bindings.elements) {
          const taken = (element.propertyName ?? element.name).text;
          if (module === 'console' || PROCESS_SINKS.has(taken)) reference(element, `${module}.${taken}`);
        }
      }
    }
    // The same taken with `require` (#576 W56-T1-NF1): `const proc = require('process')`,
    // `const { stdout } = require('node:process')`, `require('process').stderr`, anything from `require('console')`.
    // And with a dynamic import, awaited (`const { stdout } = await import('process')`, `(await
    // import('process')).stderr`), or its promise handed on (`import('process').then(..)`, #576 W74-NF1).
    const load = ts.isCallExpression(node) ? loaded(node) : undefined;
    const required = load?.module;
    if (load && (required === 'process' || required === 'console')) {
      // What holds the module: the call, or for an import the `await` of it (in any parentheses).
      let at: ts.Node = node;
      let up = node.parent;
      while (load.imported && ts.isParenthesizedExpression(up)) up = up.parent;
      if (load.imported && ts.isAwaitExpression(up)) {
        at = up;
        while (ts.isParenthesizedExpression(at.parent)) at = at.parent;
      }
      const holder = at.parent;
      if (load.imported && at === node) {
        // The promise of the module handed on: whatever receives it holds the module.
        reference(node, `import('${required}')`);
      } else if (ts.isVariableDeclaration(holder) && holder.initializer === at && ts.isObjectBindingPattern(holder.name)) {
        for (const element of holder.name.elements) {
          const taken = text(element.propertyName ?? element.name);
          if (required === 'console' || PROCESS_SINKS.has(taken)) reference(element, `${required}.${taken}`);
        }
      } else if (ts.isPropertyAccessExpression(holder) && holder.expression === at) {
        if (required === 'console' || PROCESS_SINKS.has(holder.name.text)) reference(holder, `${required}.${holder.name.text}`);
      } else if (load.imported || !(required === 'process' && ts.isVariableDeclaration(holder) && ts.isIdentifier(holder.name) && holder.name.text === 'process')) {
        // `const process = require('process')`: its `process.stdout.write` is a stream site by name. Not an
        // import's namespace of it, whose `default` is the process again (`process.default.stdout`).
        reference(node, `${load.imported ? 'import' : 'require'}('${required}')`);
      }
    }
    if (ts.isPropertyAssignment(node) && text(node.name) === 'stdio') {
      const init = text(node.initializer);
      if (/'inherit'|"inherit"|process\.std(?:out|err)|(?:^|[[,\s])[12](?:[\],\s]|$)/.test(init)) {
        site(node, 'passthrough', 'stdio', [], init);
      }
    }
    // One stream's option (`new StdioClientTransport({ stderr: 'inherit' })`).
    if (ts.isPropertyAssignment(node) && /^(?:stdout|stderr)$/.test(text(node.name))) {
      const init = text(node.initializer);
      if (/^(?:'inherit'|"inherit"|process\.std(?:out|err))$/.test(init)) site(node, 'passthrough', text(node.name), [], init);
    }
    if (ts.isReturnStatement(node) && node.expression && builders.includes(fnName(node).split(' > ')[0] ?? '')) {
      site(node, 'return', 'return', messageValues(node.expression, false));
    }
    node.forEachChild(visit);
  };
  visit(source);

  // A wrapper is a sink in its own file only: exported, its calls in another file would be no site at all.
  const exported = new Map<string, ts.Node>();
  const isExported = (node: ts.Node): boolean =>
    ts.canHaveModifiers(node) && (ts.getModifiers(node) ?? []).some((modifier) => modifier.kind === ts.SyntaxKind.ExportKeyword);
  if (Object.keys(wrappers).length > 0) source.forEachChild(function exports(node: ts.Node): void {
    if ((ts.isFunctionDeclaration(node) || ts.isClassDeclaration(node)) && node.name && isExported(node)) exported.set(node.name.text, node);
    if (ts.isVariableStatement(node) && isExported(node)) {
      for (const decl of node.declarationList.declarations) if (ts.isIdentifier(decl.name)) exported.set(decl.name.text, decl);
    }
    if (ts.isExportDeclaration(node) && !node.moduleSpecifier && node.exportClause && ts.isNamedExports(node.exportClause)) {
      for (const element of node.exportClause.elements) exported.set((element.propertyName ?? element.name).text, element);
    }
    if (ts.isExportAssignment(node) && ts.isIdentifier(node.expression)) exported.set(node.expression.text, node);
    // CommonJS: `exports.P = P`, `module.exports = { P }`.
    if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.EqualsToken) {
      const named = /^(?:module\.)?exports\.(\w+)$/.exec(text(node.left));
      if (named?.[1]) exported.set(named[1], node);
      if (named && ts.isIdentifier(node.right)) exported.set(node.right.text, node);
      if (text(node.left) === 'module.exports' && ts.isObjectLiteralExpression(node.right)) {
        for (const prop of node.right.properties) {
          if (ts.isShorthandPropertyAssignment(prop)) exported.set(prop.name.text, prop);
          else if (ts.isPropertyAssignment(prop) && ts.isIdentifier(prop.initializer)) exported.set(prop.initializer.text, prop);
        }
      }
    }
    node.forEachChild(exports);
  });
  for (const name of Object.keys(wrappers)) {
    const at = exported.get(name);
    if (at) site(at, 'passthrough', 'export', [], `export ${name}`);
  }
  return sites;
}
