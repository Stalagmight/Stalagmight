/**
 * #576 NF-W12DMP-1 (#605): no CSS `filter` in the app's own styles.
 *
 * On WebKitGTK's GPU painter an element under a CSS filter (or a backdrop filter) is painted
 * into an offscreen buffer and drawn back as a GPU promise image on every repaint. A critical
 * memory release that lands while one is in flight deadlocks the web process for good (the
 * compositor re-takes its own Skia GL-context lock while fulfilling it), and with the app's
 * memory budget a large page gets one every second. The grid marquee's tiled gradient was the
 * hot source (HFI-9, `marqueeNoTiledGradient.test.ts`); the filters were the rest: the map
 * zoom icons' `brightness(0) invert(1)` on dark themes, the toolbar's and the docked panels'
 * placement-mode `grayscale`, and the Plan-mode gas markers' drop shadow, all now drawn
 * without one (white icons; desaturated colour tokens, `src/utils/grayscaleTokens.ts`, and
 * `src/hooks/useGrayscalePaint.ts` for the non-token colours; halo strokes,
 * `src/components/Map/gasMarkerShadow.ts`).
 *
 * What this inventories, in `src/` outside tests: `filter` / `backdrop-filter` declarations
 * in CSS; Tailwind filter utilities (`grayscale`, `blur-*`, `drop-shadow-*`, `backdrop-*`,
 * ...) in class strings; `filter:` in inline CSS text; `filter` / `backdropFilter` style
 * properties; `.style.filter`; SVG `<filter>` and `filter=`. None may be left (#576
 * NF-W13FE-CSS-D1): there is no allowlist. jsdom cannot paint, so the sources are read as
 * text (CSS) and as a TypeScript syntax tree (TS/TSX).
 */
import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const SRC = path.join(REPO, 'src');

function walk(dir: string, keep: (file: string) => boolean): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return e.name === '__tests__' || e.name === 'test' ? [] : walk(p, keep);
    return keep(p) ? [p] : [];
  });
}

const rel = (file: string) => path.relative(REPO, file).split(path.sep).join('/');

/** `filter` / `backdrop-filter` declarations (any vendor prefix) in the app's CSS. */
function cssFilterDeclarations(): string[] {
  return walk(SRC, (f) => f.endsWith('.css')).flatMap((file) => {
    // Comments blanked to the same length so line numbers survive.
    const css = fs.readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, ' '));
    const found: string[] = [];
    const decl = /(^|[{;\s])((?:-[a-z]+-)?(?:backdrop-)?filter)\s*:\s*([^;}]*)/g;
    for (let m = decl.exec(css); m !== null; m = decl.exec(css)) {
      const line = css.slice(0, m.index).split('\n').length;
      found.push(`${rel(file)}:${line}: ${m[2]}: ${(m[3] ?? '').trim()}`);
    }
    return found;
  });
}

/** Tailwind v3 filter utilities (every one but `*-none` composes a `filter:` declaration). */
const TW_FILTER =
  /^(?:[\w-]+:|\[[^\]]*\]:)*!?-?(?:blur|brightness|contrast|drop-shadow|grayscale|hue-rotate|invert|saturate|sepia|filter|backdrop-(?:blur|brightness|contrast|grayscale|hue-rotate|invert|opacity|saturate|sepia|filter))(?:-\S+)?$/;
const TW_ARBITRARY = /^(?:[\w-]+:)*!?\[(?:-[a-z]+-)?(?:backdrop-)?filter:/;
const CSS_TEXT_FILTER = /(?:^|[;{\s"'`])(?:-[a-z]+-)?(?:backdrop-)?filter\s*:/;
const STYLE_KEYS = new Set(['filter', 'backdropFilter', 'WebkitFilter', 'WebkitBackdropFilter']);
const PREFILTER = /grayscale|blur|brightness-|contrast-|drop-shadow|hue-rotate|invert|saturate-|sepia|backdrop-|filter\s*[:=]|Filter\s*:|<filter|\.filter\s*=/;

/** Whether `node` sits where a class list is built: a JSX `className`, or a `*Cls` / `*Class*` binding. */
function inClassContext(node: ts.Node): boolean {
  for (let p: ts.Node | undefined = node.parent; p; p = p.parent) {
    if (ts.isJsxAttribute(p)) return /^(className|class)$/.test(p.name.getText());
    if (ts.isVariableDeclaration(p) || ts.isPropertyAssignment(p)) return /cls$|class/i.test(p.name.getText());
    if (ts.isStatement(p)) return false;
  }
  return false;
}

function literalText(node: ts.Node): string | null {
  if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) return node.text;
  if (ts.isTemplateHead(node) || ts.isTemplateMiddle(node) || ts.isTemplateTail(node)) return node.text;
  return null;
}

/** Filter uses in the TS/TSX under `root` (the app's `src/`), as `file:line: what`. */
function tsFilterUses(root = SRC): string[] {
  const files = walk(root, (f) => /\.(ts|tsx)$/.test(f) && !/\.(test|spec)\.tsx?$/.test(f) && !f.includes(`${path.sep}types${path.sep}ipc`));
  const found: string[] = [];
  for (const file of files) {
    const text = fs.readFileSync(file, 'utf8');
    if (!PREFILTER.test(text)) continue;
    const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
    const at = (node: ts.Node, what: string) => {
      const line = source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1;
      found.push(`${rel(file)}:${line}: ${what}`);
    };
    const visit = (node: ts.Node) => {
      const lit = literalText(node);
      if (lit !== null) {
        if (CSS_TEXT_FILTER.test(lit)) at(node, `inline CSS filter: ${lit.trim().slice(0, 80)}`);
        const tokens = lit.split(/\s+/).filter(Boolean);
        if (tokens.length > 1 || inClassContext(node)) {
          for (const token of tokens) {
            if (/^!?-?(?:backdrop-)?filter-none$/.test(token)) continue;
            if (TW_FILTER.test(token) || TW_ARBITRARY.test(token)) at(node, `class ${token}`);
          }
        }
      }
      // `filter: 'grayscale(1)'` / `backdropFilter: ...` in a style object (a MapLibre layer
      // `filter` is an expression array, not a string, and is not CSS).
      if (ts.isPropertyAssignment(node) && STYLE_KEYS.has(node.name.getText().replace(/['"]/g, ''))) {
        const init = node.initializer;
        if (ts.isStringLiteral(init) || ts.isTemplateExpression(init) || ts.isNoSubstitutionTemplateLiteral(init) || ts.isConditionalExpression(init)) {
          at(node, `style ${node.name.getText()}: ${init.getText(source).slice(0, 60)}`);
        }
      }
      if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.EqualsToken && /\.style\.(filter|backdropFilter|webkitFilter)$/.test(node.left.getText(source))) {
        at(node, `assignment ${node.left.getText(source)}`);
      }
      if (ts.isCallExpression(node) && /\.style\.setProperty$/.test(node.expression.getText(source))) {
        const [prop] = node.arguments;
        if (prop && ts.isStringLiteral(prop) && /(^|-)filter$/.test(prop.text)) at(node, `setProperty ${prop.text}`);
      }
      if ((ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) && node.tagName.getText(source) === 'filter') at(node, 'SVG <filter>');
      if (ts.isJsxAttribute(node) && node.name.getText(source) === 'filter') at(node, `SVG filter=${node.initializer?.getText(source) ?? ''}`);
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
  return found;
}

/**
 * The filters that stay, each with its reason. Matched on `file` and a substring of the
 * finding (line numbers drift); every entry must still match something.
 */
describe('no CSS filter in the app styles (#576 NF-W12DMP-1, #605)', () => {
  const css = cssFilterDeclarations();
  const tsUses = tsFilterUses();

  it('RULE css-filter: no filter declaration in the app CSS', () => {
    expect(
      css,
      'RULE css-filter: on WebKitGTK a filtered element repaints through a GPU promise image (#605); ' +
        'draw it without a filter (see the zoom-icon rule in src/index.css)',
    ).toEqual([]);
  });

  it('RULE ts-filter: no filter in the TS/TSX sources', () => {
    expect(
      tsUses,
      'RULE ts-filter: a CSS filter (Tailwind utility, inline CSS, style property, SVG filter) repaints through ' +
        'a GPU promise image on WebKitGTK (#605); draw it without one (src/utils/grayscaleTokens.ts and ' +
        'src/hooks/useGrayscalePaint.ts for a gray dim, src/components/Map/gasMarkerShadow.ts for a soft shadow)',
    ).toEqual([]);
  });

  it('is not vacuous: it would see the uses it replaced', () => {
    const probe = (snippet: string) => {
      const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'filter-probe-'));
      try {
        fs.writeFileSync(path.join(dir, 'probe.tsx'), snippet);
        return tsFilterUses(dir);
      } finally {
        fs.rmSync(dir, { recursive: true, force: true });
      }
    };
    // The toolbar's old dim, in a template class list and in a `*Cls` binding.
    expect(probe("export const A = (on: boolean) => <div className={`flex ${on ? 'opacity-45 grayscale' : ''}`} />;\n")).toHaveLength(1);
    expect(probe("export const dimCls = 'grayscale';\n")).toHaveLength(1);
    // The docked panels' old dim and the gas markers' old shadow (#576 NF-W13FE-CSS-D1).
    expect(probe("const on = true;\nexport const dimCls = on ? 'pointer-events-none select-none opacity-45 grayscale' : '';\n")).toHaveLength(1);
    expect(probe("export const mark = (el: HTMLElement) => { el.style.cssText = 'line-height:0;filter:drop-shadow(0 0 1px rgba(0,0,0,0.6));'; };\n")).toHaveLength(1);
    expect(probe("export const s = { filter: 'blur(2px)' };\nexport const t = 'a:b;filter:invert(1)';\n")).toHaveLength(2);
    // Not filters: an event name, a MapLibre layer filter, Array#filter.
    expect(probe("window.addEventListener('blur', () => {});\nexport const l = { filter: ['==', 'a', 1] };\nexport const f = [1].filter(Boolean);\n")).toEqual([]);
  });
});

describe('the dark-theme zoom icons drawn white without a filter (#576 NF-W12DMP-1)', () => {
  const css = fs.readFileSync(path.join(SRC, 'index.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
  const darkThemes = new Set<string>();
  const block = /([^{}]+)\{([^{}]*)\}/g;
  for (let m = block.exec(css); m !== null; m = block.exec(css)) {
    if (!/color-scheme:\s*dark/.test(m[2] ?? '')) continue;
    for (const t of (m[1] ?? '').matchAll(/\[data-theme="([\w-]+)"\]/g)) darkThemes.add(t[1] ?? '');
  }
  const iconRule = (button: 'in' | 'out') => {
    const re = new RegExp(`(:is\\([^{]*\\)) \\.maplibregl-ctrl-zoom-${button} \\.maplibregl-ctrl-icon \\{([^}]*)\\}`);
    const m = re.exec(css);
    return { selector: m?.[1] ?? '', body: m?.[2] ?? '' };
  };
  /** MapLibre's own icon for the button: the path its stylesheet draws filled #333. */
  const maplibrePath = (button: 'in' | 'out') => {
    const ml = fs.readFileSync(path.join(REPO, 'node_modules/maplibre-gl/dist/maplibre-gl.css'), 'utf8');
    const rule = new RegExp(`\\.maplibregl-ctrl-zoom-${button} \\.maplibregl-ctrl-icon\\{background-image:url\\("([^"]*fill='%23333'[^"]*)"\\)`).exec(ml);
    return /d='([^']+)'/.exec(rule?.[1] ?? '')?.[1];
  };

  it('lists exactly the themes whose block sets color-scheme: dark', () => {
    expect([...darkThemes].sort()).toEqual(['catppuccin_mocha', 'cave', 'dark', 'dracula', 'nord', 'one_dark']);
    for (const button of ['in', 'out'] as const) {
      const listed = [...iconRule(button).selector.matchAll(/\[data-theme="([\w-]+)"\]/g)].map((t) => t[1]).sort();
      expect(listed, `zoom-${button}`).toEqual([...darkThemes].sort());
      // Before useTheme stamps data-theme the root is the dark block too.
      expect(iconRule(button).selector).toContain(':root:not([data-theme])');
    }
  });

  it("draws MapLibre's own glyph, filled white", () => {
    for (const button of ['in', 'out'] as const) {
      const { body } = iconRule(button);
      expect(body).toContain("fill='%23fff'");
      const ours = /d='([^']+)'/.exec(body)?.[1];
      expect(ours, `zoom-${button}`).toBeTruthy();
      expect(ours).toBe(maplibrePath(button));
    }
  });
});
