/**
 * #576 W51: every place the harness and the repo's Node tooling print or write
 * something, and every value each one prints, read off the source.
 * `printSites.test.ts` checks them against `rules.ts` and the group allow-lists:
 * a value reaches a shareable sink only through an approved helper or as a
 * value someone checked and gave a reason for. This file only finds the sites
 * and the values.
 *
 * A site is a sink call:
 * - `stream`: `console.*`, `process.stdout.write` / `process.stderr.write`, or a
 *   call of a function a file declares as a wrapper of one (`smoke.ts`' `log`,
 *   the repro report's `P`);
 * - `progress`: `ctx.log`, printed through `shareableProgressLine`;
 * - `stream` too: the key of `ctx.metric` / `ctx.setMetric`, which the runner's
 *   PASS/FAIL line prints (`JSON.stringify(metrics.metrics)`);
 * - `error`: `new <Name>Error(message)`, whose message a runner prints through
 *   `shareableErrorText` / `scenarioFailOutput`, which keep its first line
 *   (under `unprojected` prefixes - `scripts/` - the whole message prints);
 * - `return`: a `return` of a function a file declares as a builder of shared
 *   text (the repro report's `describeAction`);
 * - `file`: `writeFileSync`, `writeFile`, `appendFileSync`, `appendFile`,
 *   `createWriteStream`, a `.write` / `.end` on a stream one opened, and the
 *   copies `copyFileSync`, `copyFile`, `cpSync`;
 * - `passthrough`: a child process's `stdio` (or `stdout` / `stderr` option)
 *   that hands it this process's stdout or stderr (`'inherit'`, `1`, `2`,
 *   `process.stdout`), an `execSync` / `execFileSync` that sets no `stdio`
 *   (its stderr is ours), a `.pipe(process.stdout)`, and a sink handed on as a
 *   value (`log = console.log`, `const out = process.stderr`).
 *
 * A value is followed through a `const` to what it was made from, and through
 * a call of a local arrow function to what it returns, each parameter standing
 * for its argument. `.length`, arithmetic, comparisons and `Math.*` are numbers
 * and need no reason. A value is marked folded inside a `FOLDS` call, in a line
 * the runner projects (it folds every line), or as the parameter of a function a
 * `FOLDING_CALLBACKS` helper calls.
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
  /** The enclosing named function, or `<module>`. */
  fn: string;
  kind: SinkKind;
  /** The callee as written (`process.stdout.write`), or `return` / `stdio`. */
  sink: string;
  /** `file` sites: the target argument as written. */
  target?: string;
  values: PrintValue[];
}

export interface PrintValue {
  /** The expression as written, whitespace collapsed. */
  text: string;
  line: number;
  /** Where it sits makes the runner withhold it: after the first `failed: ` of the first line, or on a later line. */
  withheld: boolean;
  /** The value is a call of this function (the callee's last name), if any. */
  callee?: string;
  /** A bare identifier: the allow-list keys it by function too. */
  bare: boolean;
  /** The function the value is written in (a const or a local arrow function can be another than the site's). */
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

const STREAM = /^(?:console\.(?:log|error|warn|info|debug|trace|table|dir)|process\.(?:stdout|stderr)\.write)$/;
const FILE_WRITE = new Set(['writeFileSync', 'writeFile', 'appendFileSync', 'appendFile', 'createWriteStream']);
/** A scenario's metric: its key prints in the runner's PASS/FAIL line. */
const METRIC = /^ctx\.(?:metric|setMetric)$/;
/** Copies: the target is the second argument, the content another file. */
const FILE_COPY = new Set(['copyFileSync', 'copyFile', 'cpSync']);
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

  const fnName = (node: ts.Node): string => {
    for (let at: ts.Node | undefined = node.parent; at; at = at.parent) {
      if ((ts.isFunctionDeclaration(at) || ts.isMethodDeclaration(at)) && at.name) return at.name.getText(source);
      if ((ts.isArrowFunction(at) || ts.isFunctionExpression(at)) && at.parent) {
        const holder = at.parent;
        if ((ts.isVariableDeclaration(holder) || ts.isPropertyAssignment(holder)) && holder.name) {
          return holder.name.getText(source);
        }
      }
    }
    return '<module>';
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
    // `[a, b].join(sep)`: the elements, the separator between them.
    if (
      ts.isCallExpression(node) &&
      ts.isPropertyAccessExpression(node.expression) &&
      node.expression.name.text === 'join' &&
      ts.isArrayLiteralExpression(node.expression.expression)
    ) {
      const sepArg = node.arguments[0];
      const sep = sepArg && ts.isStringLiteralLike(sepArg) ? sepArg.text : ',';
      const parts: Part[] = [];
      node.expression.expression.elements.forEach((el, i) => {
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
      out.push({ text: text(n), line: lineOf(n), withheld, ...(callee ? { callee } : {}), bare: ts.isIdentifier(n), folded, fn: fnName(n) });
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
        out.push({ text: text(node), line: lineOf(node), withheld, bare: true, folded: true, fn: fnName(node) });
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
      if ([...flatten(node)].length > 1) {
        for (const part of flatten(node)) if ('expr' in part) valuesOf(part.expr, withheld, out, seen, folded);
        return;
      }
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

  // What a file stream is held in (`const log = fs.createWriteStream(..)`, `this.driverLog = ..`):
  // its `.write(..)` calls are writes to that file.
  const fileStreams = new Set<string>();
  source.forEachChild(function holders(node: ts.Node): void {
    const init = ts.isVariableDeclaration(node)
      ? node.initializer
      : ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.EqualsToken
        ? node.right
        : undefined;
    const holder = ts.isVariableDeclaration(node) ? node.name : ts.isBinaryExpression(node) ? node.left : undefined;
    const opens = init && ts.isCallExpression(init) && /(?:^|\.)createWriteStream$/.test(text(init.expression));
    // Or another name for one (`const driverLog = this.driverLog`), declared after it.
    if (holder && init && (opens || fileStreams.has(text(init)))) fileStreams.add(text(holder));
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
      const last = ts.isPropertyAccessExpression(node.expression) ? node.expression.name.text : callee;
      const wrapper = Object.hasOwn(wrappers, callee) ? wrappers[callee] : undefined;
      const wrapped = typeof wrapper === 'object' ? wrapper.kind : wrapper;
      const printed = (args: readonly ts.Expression[]): readonly ts.Expression[] =>
        typeof wrapper === 'object' ? args.filter((_, i) => wrapper.args.includes(i)) : args;
      if (METRIC.test(callee)) {
        // A metric's key prints in the PASS/FAIL line (`JSON.stringify(metrics.metrics)`); its value is a number.
        site(node, 'stream', callee, node.arguments.slice(0, 1).flatMap((arg) => messageValues(arg, false)));
      } else if (STREAM.test(callee) || wrapped === 'stream') {
        const values = printed(node.arguments).flatMap((arg) => messageValues(arg, false));
        site(node, inPage(node) ? 'page' : 'stream', callee, values);
      } else if (callee === 'ctx.log' || wrapped === 'progress') {
        site(node, 'progress', callee, printed(node.arguments).flatMap((arg) => messageValues(arg, true)));
      } else if (wrapped === 'error') {
        // An error wrapper prints its first argument, unless it names others.
        const message = typeof wrapper === 'object' ? printed(node.arguments) : node.arguments.slice(0, 1);
        site(node, 'error', callee, message.flatMap((arg) => messageValues(arg, errorsProjected)));
      } else if (FILE_WRITE.has(last)) {
        const [target, ...rest] = node.arguments;
        const values = rest.slice(0, 1).flatMap((arg) => messageValues(arg, false));
        site(node, 'file', callee, values, target ? text(target) : '');
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
      site(node, 'passthrough', 'reference', [], text(node));
    }
    // The stream itself handed on (`const out = process.stderr`, `stdio: [.., process.stderr]`).
    if (
      ts.isPropertyAccessExpression(node) &&
      /^process\.(?:stdout|stderr)$/.test(text(node)) &&
      !ts.isPropertyAccessExpression(node.parent) &&
      !(ts.isCallExpression(node.parent) && /\.pipe$/.test(text(node.parent.expression))) &&
      !(ts.isArrayLiteralExpression(node.parent) && ts.isPropertyAssignment(node.parent.parent) && text(node.parent.parent.name) === 'stdio')
    ) {
      site(node, 'passthrough', 'reference', [], text(node));
    }
    if (ts.isNewExpression(node) && /(?:^|\.)\w*Error$/.test(text(node.expression)) && node.arguments?.[0]) {
      site(node, 'error', `new ${text(node.expression)}`, messageValues(node.arguments[0], errorsProjected));
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
    if (ts.isReturnStatement(node) && node.expression && builders.includes(fnName(node))) {
      site(node, 'return', 'return', messageValues(node.expression, false));
    }
    node.forEachChild(visit);
  };
  visit(source);
  return sites;
}
