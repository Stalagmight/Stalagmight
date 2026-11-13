/**
 * Find / replace for the Data workspace: turn a set of matches into parsed cell
 * edits, one edit per cell with every chosen match in that cell replaced at once.
 *
 * - Regex mode expands the replacement like `String.prototype.replace`:
 *   `$1`..`$99`, `$&`, `` $` ``, `$'`, `$<name>` and `$$`. Literal mode inserts
 *   the replacement exactly as typed.
 * - A match in a locked cell (computed column, frozen geometry, read-only
 *   workspace, not applicable) is skipped and counted.
 * - The new text goes through the column's own `parse` (the same coercion as
 *   typing into the cell); `undefined` means invalid - skipped and reported.
 * - A cell whose text would not change is left alone.
 */
import type { DataColumnDef, DataColumnKey, DataCellEdit, DataModelCtx, DataRow, SearchMatch } from '../types';
import { compileSearch, execAll, type SearchPattern } from './search';

export interface ReplaceSpec extends SearchPattern {
  replacement: string;
}

export interface ReplaceInvalidCell {
  rowKey: string;
  colKey: DataColumnKey;
  /** The text the replacement produced, which the column refused. */
  text: string;
  matches: number;
}

export interface ReplacePlan {
  edits: DataCellEdit[];
  /** How many matches each edit carries (parallel to `edits`). */
  editMatches: number[];
  /** The cell text before / after the replacement (parallel to `edits`). */
  editTexts: { before: string; after: string }[];
  /** Matches that become edits. */
  replacedMatches: number;
  /** Matches skipped because their cell is locked. */
  lockedMatches: number;
  lockedCells: number;
  invalid: ReplaceInvalidCell[];
  /** Cells whose text the replacement leaves unchanged. */
  unchangedCells: number;
}

export interface ReplaceModel {
  rowByKey: ReadonlyMap<string, DataRow>;
  columnByKey: ReadonlyMap<DataColumnKey, DataColumnDef>;
}

/**
 * Expand a regex replacement template for one match, per ECMAScript
 * GetSubstitution: `$$`, `$&`, `` $` ``, `$'`, `$n` / `$nn` (only when that
 * group exists, else literal), `$<name>` (only when the regex has named groups).
 */
export function expandReplacement(template: string, m: RegExpExecArray, text: string): string {
  if (!template.includes('$')) return template;
  const groupCount = m.length - 1;
  const start = m.index;
  const end = start + m[0].length;
  let out = '';
  for (let i = 0; i < template.length; i++) {
    const ch = template[i]!;
    if (ch !== '$' || i === template.length - 1) {
      out += ch;
      continue;
    }
    const next = template[i + 1]!;
    if (next === '$') {
      out += '$';
      i++;
    } else if (next === '&') {
      out += m[0];
      i++;
    } else if (next === '`') {
      out += text.slice(0, start);
      i++;
    } else if (next === "'") {
      out += text.slice(end);
      i++;
    } else if (next >= '0' && next <= '9') {
      const two = template.slice(i + 1, i + 3);
      const twoNum = /^\d\d$/.test(two) ? Number(two) : NaN;
      const oneNum = Number(next);
      if (twoNum >= 1 && twoNum <= groupCount) {
        out += m[twoNum] ?? '';
        i += 2;
      } else if (oneNum >= 1 && oneNum <= groupCount) {
        out += m[oneNum] ?? '';
        i++;
      } else {
        out += ch;
      }
    } else if (next === '<' && m.groups !== undefined) {
      const close = template.indexOf('>', i + 2);
      if (close < 0) {
        out += ch;
      } else {
        out += m.groups[template.slice(i + 2, close)] ?? '';
        i = close;
      }
    } else {
      out += ch;
    }
  }
  return out;
}

/**
 * Replace the given `[start, end)` ranges of `text` (matches of `spec` in that
 * text). Ranges that are no longer a match of `spec` are left alone. Returns the
 * new text and how many ranges were replaced.
 */
export function replaceInText(
  text: string,
  ranges: readonly { start: number; end: number }[],
  spec: ReplaceSpec,
): { text: string; replaced: number } {
  const compiled = compileSearch(spec);
  if (!compiled.regex) return { text, replaced: 0 };
  const found = execAll(text, compiled);
  const sorted = [...ranges].sort((a, b) => a.start - b.start);
  let out = '';
  let cursor = 0;
  let replaced = 0;
  for (const r of sorted) {
    const m = found.get(r.start);
    if (!m || r.start + m[0].length !== r.end || r.start < cursor) continue;
    out += text.slice(cursor, r.start);
    out += spec.regex ? expandReplacement(spec.replacement, m, text) : spec.replacement;
    cursor = r.end;
    replaced++;
  }
  out += text.slice(cursor);
  return { text: out, replaced };
}

/**
 * Plan the edits for `matches` (all of them for Replace all, the current one
 * for Replace). Cells are visited in the order their first match appears.
 */
export function planReplace(
  matches: readonly SearchMatch[],
  model: ReplaceModel,
  ctx: DataModelCtx,
  spec: ReplaceSpec,
): ReplacePlan {
  const plan: ReplacePlan = {
    edits: [],
    editMatches: [],
    editTexts: [],
    replacedMatches: 0,
    lockedMatches: 0,
    lockedCells: 0,
    invalid: [],
    unchangedCells: 0,
  };
  const cells = new Map<string, SearchMatch[]>();
  for (const m of matches) {
    const key = `${m.rowKey}\u0000${m.colKey}`;
    let list = cells.get(key);
    if (!list) cells.set(key, (list = []));
    list.push(m);
  }
  for (const list of cells.values()) {
    const first = list[0]!;
    const row = model.rowByKey.get(first.rowKey);
    const col = model.columnByKey.get(first.colKey);
    if (!row || row.kind === 'section' || !col) continue;
    const lock = col.lockReason(row, ctx);
    if (lock !== null) {
      plan.lockedMatches += list.length;
      plan.lockedCells++;
      continue;
    }
    const oldText = col.text(row, ctx);
    const { text: newText, replaced } = replaceInText(oldText, list, spec);
    if (replaced === 0 || newText === oldText) {
      plan.unchangedCells++;
      continue;
    }
    const value = col.parse ? col.parse(newText, row, ctx) : undefined;
    if (value === undefined) {
      plan.invalid.push({ rowKey: row.key, colKey: col.key, text: newText, matches: replaced });
      continue;
    }
    plan.edits.push({ row, colKey: col.key, value });
    plan.editMatches.push(replaced);
    plan.editTexts.push({ before: oldText, after: newText });
    plan.replacedMatches += replaced;
  }
  return plan;
}
