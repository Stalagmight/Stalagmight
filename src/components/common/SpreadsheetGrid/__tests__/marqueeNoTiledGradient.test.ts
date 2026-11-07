/**
 * #576 HFI-9: no animated rule paints a TILED gradient.
 *
 * On WebKitGTK's GPU painter a gradient repeated as a tile (`background-repeat`
 * other than `no-repeat` with a `background-size` smaller than the box) is drawn
 * as a pattern of a GPU-backed image - a Skia promise image - while one untiled
 * gradient is filled directly. The grid's copy and cut marquees animated such a
 * tile, so every frame for as long as a marquee showed made a promise image. A
 * critical memory release that lands while one is in flight deadlocks the web
 * process for good (the compositor thread re-takes its own Skia GL-context lock
 * while fulfilling it), and with the app's memory budget a large page gets one
 * every second: the window froze moments after a copy or cut in a big grid
 * (measured on the real app: profile + stack traces in the #576 HFI-9 report).
 *
 * What this pins is the count of animated tiled gradients in the app's CSS - the
 * repaint-every-frame source of those images - which must stay zero, and that
 * the marquee that replaced them still marches the same way: one 8px dash period
 * per cycle, clockwise. jsdom cannot paint, so the CSS is read as text.
 */
import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SRC = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../..');

interface CssRule {
  file: string;
  selector: string;
  decls: Map<string, string>;
}

/** Every CSS file under src/. */
function cssFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return e.name === 'node_modules' ? [] : cssFiles(p);
    return e.name.endsWith('.css') ? [p] : [];
  });
}

/**
 * Style rules (selector + declarations), at any nesting depth, skipping the
 * frames of `@keyframes` blocks. Enough of a parser for the app's own CSS:
 * comments stripped, braces matched, declarations split on top-level `;`.
 */
function parseRules(file: string): { rules: CssRule[]; keyframes: Map<string, string> } {
  const css = fs.readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
  const rules: CssRule[] = [];
  const keyframes = new Map<string, string>();
  const walk = (text: string, inKeyframes: string | null) => {
    let i = 0;
    while (i < text.length) {
      const open = text.indexOf('{', i);
      if (open < 0) return;
      const prelude = text.slice(i, open).trim();
      let depth = 1;
      let j = open + 1;
      while (j < text.length && depth > 0) {
        if (text[j] === '{') depth += 1;
        else if (text[j] === '}') depth -= 1;
        j += 1;
      }
      const body = text.slice(open + 1, j - 1);
      const kf = /^@(?:-webkit-)?keyframes\s+([\w-]+)/.exec(prelude);
      if (kf) {
        keyframes.set(kf[1] as string, body);
      } else if (prelude.startsWith('@') || body.includes('{')) {
        walk(body, inKeyframes);
      } else if (!inKeyframes) {
        const decls = new Map<string, string>();
        for (const part of splitTopLevel(body, ';')) {
          const colon = part.indexOf(':');
          if (colon < 0) continue;
          decls.set(part.slice(0, colon).trim().toLowerCase(), part.slice(colon + 1).trim());
        }
        rules.push({ file: path.relative(SRC, file), selector: prelude.replace(/\s+/g, ' '), decls });
      }
      i = j;
    }
  };
  walk(css, null);
  return { rules, keyframes };
}

/** Split on `sep` outside parentheses. */
function splitTopLevel(text: string, sep: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let cur = '';
  for (const ch of text) {
    if (ch === '(') depth += 1;
    else if (ch === ')') depth -= 1;
    if (ch === sep && depth === 0) {
      out.push(cur);
      cur = '';
    } else {
      cur += ch;
    }
  }
  if (cur.trim()) out.push(cur);
  return out.map((s) => s.trim()).filter(Boolean);
}

const animates = (r: CssRule) => r.decls.has('animation') || r.decls.has('animation-name');
const paintsGradient = (r: CssRule) => /gradient\(/.test(`${r.decls.get('background-image') ?? ''} ${r.decls.get('background') ?? ''}`);
/** Untiled only when every layer says `no-repeat` (CSS repeats by default). */
const tiles = (r: CssRule) => {
  const repeat = r.decls.get('background-repeat') ?? '';
  const shorthand = r.decls.get('background') ?? '';
  const layers = splitTopLevel(repeat || shorthand, ',');
  return layers.length === 0 || layers.some((l) => !/\bno-repeat\b/.test(l));
};

const ALL = cssFiles(SRC).map(parseRules);
const RULES = ALL.flatMap((p) => p.rules);

describe('animated CSS never tiles a gradient (#576 HFI-9)', () => {
  it('reads the app stylesheets (the inventory below is not vacuous)', () => {
    expect(RULES.some((r) => r.selector.includes('.sg-marching-ants'))).toBe(true);
    expect(RULES.some((r) => r.selector.includes('.sg-cut-marquee'))).toBe(true);
  });

  it('RULE animated-tiled-gradient: no animated rule repeats a gradient tile', () => {
    const offenders = RULES.filter((r) => animates(r) && paintsGradient(r) && tiles(r)).map(
      (r) => `${r.file}: ${r.selector} (background-repeat: ${r.decls.get('background-repeat') ?? '(default: repeat)'})`,
    );
    expect(
      offenders,
      'RULE animated-tiled-gradient: an animated, tiled gradient makes a GPU promise image on every frame ' +
        'on WebKitGTK; draw each edge as one untiled repeating-linear-gradient instead (see SpreadsheetGrid.css)',
    ).toEqual([]);
  });
});

describe('the copy / cut marquee still marches one 8px period clockwise (#576 HFI-9)', () => {
  const marquee = RULES.find((r) => r.selector.includes('.sg-marching-ants') && r.decls.has('animation'));
  const grid = ALL.find((p) => p.keyframes.has('sg-ants'));

  /** One layer's `x y` offset in px along its moving axis; `100%` / `0` pin the other axis. */
  const offsets = (value: string) =>
    splitTopLevel(value, ',').map((layer) =>
      layer.split(/\s+/).map((v) => (v === '0' ? 0 : v.endsWith('px') ? Number(v.slice(0, -2)) : v)),
    );

  it('is drawn by the same rule for both marquees, untiled, four edges', () => {
    expect(marquee?.selector).toBe('.sg-marching-ants, .sg-cut-marquee');
    expect(splitTopLevel(marquee?.decls.get('background-image') ?? '', ',')).toHaveLength(4);
    expect(marquee?.decls.get('background-repeat')).toBe('no-repeat');
    expect(marquee?.decls.get('animation')).toBe('sg-ants 0.6s linear infinite');
  });

  it('moves top right, right down, bottom left and left up by exactly 8px a cycle', () => {
    const from = offsets(marquee?.decls.get('background-position') ?? '');
    const toDecl = /background-position:\s*([^;]+);/.exec(grid?.keyframes.get('sg-ants') ?? '');
    const to = offsets(toDecl?.[1] ?? '');
    // [layer]: [axis index that moves, expected delta]
    const moves: Array<[number, number]> = [
      [0, +8], // top edge: x, rightwards
      [0, -8], // bottom edge: x, leftwards
      [1, -8], // left edge: y, upwards
      [1, +8], // right edge: y, downwards
    ];
    moves.forEach(([axis, delta], layer) => {
      const a = from[layer]?.[axis];
      const b = to[layer]?.[axis];
      expect.soft(typeof a === 'number' && typeof b === 'number' ? b - a : `${String(a)} -> ${String(b)}`, `layer ${layer}`).toBe(delta);
      // The other axis does not move.
      expect.soft(to[layer]?.[1 - axis], `layer ${layer} fixed axis`).toEqual(from[layer]?.[1 - axis]);
    });
  });
});
