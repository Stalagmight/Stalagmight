import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { mapVizTokenNames, themeTokens } from '../tokens';
import { getDepthColor, hexToHSL } from '../../utils/colors';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const cssPath = path.resolve(__dirname, '../../index.css');
const cssSource = fs.readFileSync(cssPath, 'utf8');
const htmlPath = path.resolve(__dirname, '../../../index.html');
const htmlSource = fs.readFileSync(htmlPath, 'utf8');
/** index.html with comments stripped, so prose cannot satisfy a rule below. */
const htmlRules = htmlSource.replace(/\/\*[\s\S]*?\*\//g, '');

function extractBlock(source: string, marker: string): string {
  const start = source.indexOf(marker);
  if (start === -1) {
    throw new Error(`Missing theme block for marker: ${marker}`);
  }
  const open = source.indexOf('{', start);
  if (open === -1) {
    throw new Error(`Missing opening brace for marker: ${marker}`);
  }
  const close = source.indexOf('}', open);
  if (close === -1) {
    throw new Error(`Missing closing brace for marker: ${marker}`);
  }
  return source.slice(open + 1, close);
}

function extractTokens(block: string): Record<string, string> {
  const tokens: Record<string, string> = {};
  const tokenRegex = /(--cc-[a-z0-9-]+)\s*:\s*([^;]+);/gi;
  let match: RegExpExecArray | null = null;
  while ((match = tokenRegex.exec(block)) !== null) {
    const name = match[1];
    const value = match[2];
    if (!name || !value) continue;
    tokens[name] = value.trim();
  }
  return tokens;
}

function sortedKeys(tokens: Record<string, string>): string[] {
  return Object.keys(tokens).sort();
}

/** WCAG relative luminance. */
function luminance(hex: string): number {
  const channel = (v: number) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  };
  const [r, g, b] = [1, 3, 5].map((i) => channel(parseInt(hex.slice(i, i + 2), 16)));
  return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!;
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi! + 0.05) / (lo! + 0.05);
}

/**
 * The map canvas a theme actually paints on. Chrome-only themes
 * (`solarized_light`, `catppuccin_latte`) declare no `--cc-map-background` of
 * their own and inherit `:root`'s dark canvas - mirrors `mapCanvasFor` in
 * src/types/config.ts, duplicated here rather than imported so this suite
 * doesn't couple to that module's own in-flight changes.
 */
function mapCanvasFor(theme: string): string {
  const tokens = (themeTokens as Record<string, Record<string, string> | undefined>)[theme];
  return tokens?.['--cc-map-background'] ?? themeTokens.dark['--cc-map-background'];
}

type ThemeName = keyof typeof themeTokens;

const ALL_THEME_NAMES = Object.keys(themeTokens) as ThemeName[];

const cssDarkTokens = extractTokens(extractBlock(cssSource, ':root,'));
const cssLightTokens = extractTokens(extractBlock(cssSource, '[data-theme="light"]'));
const cssCaveTokens = extractTokens(extractBlock(cssSource, '[data-theme="cave"]'));
const cssDraculaTokens = extractTokens(extractBlock(cssSource, '[data-theme="dracula"]'));
const cssOneDarkTokens = extractTokens(extractBlock(cssSource, '[data-theme="one_dark"]'));
const cssSolarizedLightTokens = extractTokens(extractBlock(cssSource, '[data-theme="solarized_light"]'));
const cssCatppuccinLatteTokens = extractTokens(extractBlock(cssSource, '[data-theme="catppuccin_latte"]'));
const cssNordTokens = extractTokens(extractBlock(cssSource, '[data-theme="nord"]'));
const cssCatppuccinMochaTokens = extractTokens(extractBlock(cssSource, '[data-theme="catppuccin_mocha"]'));
const cssKaleidoEinkTokens = extractTokens(extractBlock(cssSource, '[data-theme="kaleido_eink"]'));

/** Dark themes must include all dark token keys (including map visualization). */
const FULL_DARK_THEMES = ['cave', 'dracula', 'one_dark', 'nord', 'catppuccin_mocha'] as const;

/** Light themes only override UI chrome + controls (no map viz tokens). */
const LIGHT_THEMES = ['solarized_light', 'catppuccin_latte'] as const;

/**
 * Light themes with full token sets (including map visualization overrides).
 *
 * This list is about the token SHAPE of a light-CHROME theme, and it is only
 * ever used by the shape checks below. It is deliberately NOT the input to any
 * perceptual rule: a theme can have light chrome and still paint on the dark
 * `:root` canvas. Anything that depends on the canvas measures the canvas -
 * see `rampFloorFor`.
 */
const FULL_LIGHT_THEMES = ['kaleido_eink', 'light'] as const;

const cssTokensByTheme: Record<string, Record<string, string>> = {
  dark: cssDarkTokens,
  light: cssLightTokens,
  cave: cssCaveTokens,
  dracula: cssDraculaTokens,
  one_dark: cssOneDarkTokens,
  solarized_light: cssSolarizedLightTokens,
  catppuccin_latte: cssCatppuccinLatteTokens,
  nord: cssNordTokens,
  catppuccin_mocha: cssCatppuccinMochaTokens,
  kaleido_eink: cssKaleidoEinkTokens,
};

describe('theme tokens', () => {
  it('dark tokens match CSS :root block', () => {
    expect(sortedKeys(cssDarkTokens)).toEqual(sortedKeys(themeTokens.dark));
    for (const [key, value] of Object.entries(themeTokens.dark)) {
      expect(cssDarkTokens[key]).toBe(value);
    }
  });

  it('light tokens match CSS light block', () => {
    expect(sortedKeys(cssLightTokens)).toEqual(sortedKeys(themeTokens.light));
    for (const [key, value] of Object.entries(themeTokens.light)) {
      expect(cssLightTokens[key]).toBe(value);
    }
  });

  it('cave tokens match CSS cave block', () => {
    expect(sortedKeys(cssCaveTokens)).toEqual(sortedKeys(themeTokens.cave));
    for (const [key, value] of Object.entries(themeTokens.cave)) {
      expect(cssCaveTokens[key]).toBe(value);
    }
  });

  it.each(FULL_DARK_THEMES)('%s tokens include all dark token keys (full dark theme)', (theme) => {
    expect(sortedKeys(themeTokens[theme])).toEqual(sortedKeys(themeTokens.dark));
  });

  it.each(FULL_DARK_THEMES)('%s tokens match CSS block', (theme) => {
    const cssTokens = cssTokensByTheme[theme]!;
    expect(sortedKeys(cssTokens)).toEqual(sortedKeys(themeTokens[theme]));
    for (const [key, value] of Object.entries(themeTokens[theme])) {
      expect(cssTokens[key]).toBe(value);
    }
  });

  it.each(LIGHT_THEMES)('%s tokens only override UI chrome and controls', (theme) => {
    const themeKeys = new Set(Object.keys(themeTokens[theme]));
    const missingKeys = Object.keys(themeTokens.dark).filter((key) => !themeKeys.has(key));

    expect(missingKeys.sort()).toEqual([...mapVizTokenNames].sort());

    const cssTokens = cssTokensByTheme[theme]!;
    for (const key of mapVizTokenNames) {
      expect(themeKeys.has(key)).toBe(false);
      expect(Object.prototype.hasOwnProperty.call(cssTokens, key)).toBe(false);
    }
  });

  it.each(LIGHT_THEMES)('%s tokens match CSS block', (theme) => {
    const cssTokens = cssTokensByTheme[theme]!;
    expect(sortedKeys(cssTokens)).toEqual(sortedKeys(themeTokens[theme]));
    for (const [key, value] of Object.entries(themeTokens[theme])) {
      expect(cssTokens[key]).toBe(value);
    }
  });

  it.each(FULL_LIGHT_THEMES)('%s tokens include all dark token keys (full light theme)', (theme) => {
    expect(sortedKeys(themeTokens[theme])).toEqual(sortedKeys(themeTokens.dark));
  });

  it.each(FULL_LIGHT_THEMES)('%s tokens match CSS block', (theme) => {
    const cssTokens = cssTokensByTheme[theme]!;
    expect(sortedKeys(cssTokens)).toEqual(sortedKeys(themeTokens[theme]));
    for (const [key, value] of Object.entries(themeTokens[theme])) {
      expect(cssTokens[key]).toBe(value);
    }
  });
});

/**
 * The depth gradient is the one map colour a theme can get wrong without any token
 * being missing: `getDepthColor` interpolates between the two endpoints along the
 * SHORTEST HSL hue path, so a pair that looks fine at both ends can pass through a
 * near-invisible hue in the middle. The dark theme's teal -> coral does exactly that
 * on a light canvas, bottoming out at 1.14:1. Assert the whole ramp, against each
 * theme's own canvas, using the production interpolation rather than a copy of it.
 */
describe('depth gradient legibility', () => {
  /** Worst contrast anywhere along the drawn ramp, plus where it occurs. */
  /**
   * Themes that declare their own map-visualization tokens, derived from the
   * tokens themselves rather than hand-listed. A hand-maintained union is a
   * silent coverage hole: a theme added to tokens.ts and forgotten here would
   * get NONE of the three properties below (contrast floor, endpoint hue bands,
   * hue traversal) and nothing would fail. Declaring `--cc-map-background` is
   * the marker because the map-viz tokens are all-or-nothing - the
   * "only override UI chrome and controls" test above asserts a chrome-only
   * theme is missing exactly `mapVizTokenNames`, never a subset of it.
   *
   * The same derivation runs at the type level so `KNOWN_BELOW_FLOOR` still
   * cannot name a theme without a ramp, and so `themeTokens[theme]` keeps its
   * literal types instead of degrading to `Record<string, string>`.
   */
  type FullTheme = {
    [K in ThemeName]: (typeof themeTokens)[K] extends { '--cc-map-background': string } ? K : never;
  }[ThemeName];

  /**
   * Hue as a SIGNED offset from red, i.e. 355 reads as -5 rather than 355. Reds
   * straddle the 0/360 seam, so raw hue makes a perfectly ordinary green -> red
   * ramp look like it jumps backwards at the last step.
   */
  function signedHue(hex: string): number {
    const hsl = hexToHSL(hex);
    expect(hsl).not.toBeNull();
    return hsl!.h > 180 ? hsl!.h - 360 : hsl!.h;
  }

  /**
   * The whole drawn ramp, sampled the way the renderer draws it. Endpoint-only
   * checks are the trap this suite exists for: `getDepthColor` interpolates along
   * the shortest HSL hue path, so two perfectly good endpoints can be joined by a
   * detour through blue and magenta that no endpoint assertion can see.
   */
  function sweepRamp(theme: FullTheme) {
    const tokens = themeTokens[theme];
    const canvas = tokens['--cc-map-background'];
    const shallow = tokens['--cc-depth-shallow'];
    const deep = tokens['--cc-depth-deep'];
    const samples: { color: string; ratio: number; hue: number }[] = [];
    for (let i = 0; i <= 40; i++) {
      // Depths are negative elevations, so minDepth is the DEEPER end. Passing them
      // the other way round makes getDepthColor's own clamp degenerate (min > max)
      // and the sweep only ever returns the two endpoints - which silently turns
      // this into an endpoint check and misses the mid-ramp hue entirely.
      const color = getDepthColor(-40 + i, shallow, deep, -40, 0);
      expect(color).not.toBeNull();
      samples.push({
        color: color!,
        ratio: contrast(color!, canvas),
        hue: signedHue(color!),
      });
    }
    // Sample 0 is the SHALLOW end (depth -40 is the shallowest elevation passed).
    return { samples, canvas, shallow, deep };
  }

  function worstOnRamp(theme: FullTheme) {
    const { samples, canvas } = sweepRamp(theme);
    let worst = Number.POSITIVE_INFINITY;
    let at = '';
    for (const s of samples) {
      if (s.ratio < worst) {
        worst = s.ratio;
        at = s.color;
      }
    }
    return { worst, at, canvas };
  }

  // 3:1 is the WCAG floor for non-text graphical objects, which is what a survey line
  // is. Full themes only: the chrome-only light themes inherit the dark canvas, so
  // their ramp is judged against that one.
  const RAMP_FLOOR = 3;

  /**
   * Light-canvas themes are held to 2:1 instead, and this is a deliberate,
   * user-directed trade rather than an oversight.
   *
   * The ramp runs cyan -> green -> yellow -> red. Its YELLOW MIDPOINT is the
   * brightest sample on the sweep, so on a near-white canvas the midpoint sets
   * the worst ratio, and holding it at 3:1 forces the WHOLE ramp dark - which is
   * what made `light` read as a flat olive-to-brown smear with no visible cyan or
   * green in it. The same anchors on a dark canvas measure 3.5:1; the constraint
   * is the canvas, not the palette.
   *
   * Measured cost of each option on `light` (worst sample vs #faf7f2):
   *   #0b7d77 -> #8f1206   3.50:1   clears 3:1, but visibly muted
   *   #1aa89c -> #b81a0c   2.10:1   <- chosen: vivid and still legible
   *   #3fd6c8 -> #d92a20   1.42:1   matches dark, midpoint near-invisible
   *
   * 2:1 keeps the midpoint distinguishable while letting the ends read as real
   * colours. Dark-canvas themes are UNAFFECTED and still assert the full 3:1 -
   * do not "simplify" this back to one constant.
   */
  const RAMP_FLOOR_LIGHT_CANVAS = 2;

  /**
   * Whether `theme`'s ramp is drawn on a light canvas, MEASURED off that
   * canvas rather than read off a list of light-chrome themes.
   *
   * The distinction is load-bearing: the 2:1 trade above exists because of the
   * canvas the ramp is painted on, so selecting by chrome would hold a future
   * light-chrome/dark-canvas theme to 2:1 while it draws on `#1a1a2e` - a real
   * accessibility regression with no failing test. `solarized_light` and
   * `catppuccin_latte` are exactly that shape today (light chrome, inherited
   * dark canvas); they are out of scope here only because they declare no ramp.
   *
   * `> 0.5` relative luminance is the same rule `isLightCanvasTheme`
   * (src/utils/colors.ts) applies to pick a section palette, and the rule
   * `LIGHT_CANVAS_THEMES` in src/types/config.ts is pinned against. Recomputed
   * from this file's own helpers rather than imported, so this stays an
   * independent check instead of re-asserting the module's own definition.
   */
  const isLightCanvas = (theme: FullTheme): boolean => luminance(mapCanvasFor(theme)) > 0.5;

  /** The floor that applies to `theme`, by the canvas its ramp is drawn on. */
  const rampFloorFor = (theme: FullTheme): number =>
    isLightCanvas(theme) ? RAMP_FLOOR_LIGHT_CANVAS : RAMP_FLOOR;

  /**
   * Themes still below the floor, with the value measured when the exception was
   * granted. Empty today: `one_dark` was the last entry and #357 item 8 took its
   * shallow endpoint off the indigo arc (2.599 -> 4.38), so the entry was deleted
   * exactly as the "once it clears the floor, remove it" assertion below demands.
   *
   * Carry any future value to 3dp on purpose. The assertion below is `worst >= known`,
   * so every digit dropped here is slack the ratchet grants a real regression.
   */
  const KNOWN_BELOW_FLOOR: Partial<Record<FullTheme, number>> = {};

  const ALL_FULL_THEMES = ALL_THEME_NAMES.filter(
    (name): name is FullTheme => '--cc-map-background' in themeTokens[name]
  );

  /**
   * Deep must be red (signed hue, so red straddles 0); shallow must sit anywhere
   * on the warm side of the 180 line.
   *
   * The ONLY thing this band exists to forbid is a shallow endpoint at hue >= 180,
   * because that is the side of the wheel from which `getDepthColor`'s shortest
   * path to red runs UP through blue and magenta instead of DOWN through green
   * and yellow. 180 is therefore the real boundary, and the max is set just under
   * it rather than at some arbitrary "green enough" point.
   *
   * It used to be 155, which additionally encoded a product preference for a
   * green shallow end. That preference is gone: the shipped ramp is cyan -> green
   * -> yellow -> red (the pre-#357 look), whose shallow anchors run 169.8-174.9.
   * Those descend through green and yellow exactly like a green shallow does -
   * they just start further round the wheel and cover MORE of it, which is why
   * they measure better on discriminability and on deuteranopia than the green
   * ramp that briefly replaced them. Narrowing this band back below 178 would
   * reject the intended colours, not catch a defect.
   */
  const SHALLOW_HUE = { min: 85, max: 178 };
  const DEEP_HUE = { min: -25, max: 25 };

  /**
   * End-to-end contrast between the two endpoints. Green -> red is the classic
   * red/green confusable pair, so the ramp has to carry its information in
   * luminance too, or it is a flat grey ramp under deuteranopia / protanopia.
   * Note this is a LUMINANCE ratio, not HSL lightness: yellow and blue at equal
   * HSL L differ about 5x in relative luminance, so equal-L endpoints prove
   * nothing.
   */
  const MIN_END_SEPARATION = 1.6;

  it('grants the 2:1 light-canvas floor to exactly light and kaleido_eink', () => {
    // Pins the measured predicate to the set it is expected to produce TODAY, in
    // both directions. `rampFloorFor` is deliberately derived, so without this a
    // theme joining or leaving the light-canvas set would silently change which
    // floor it is judged against - the 2:1 trade was granted per-canvas by a
    // human and a new theme inheriting it is a decision, not a detail.
    // These are the same two names as `LIGHT_CANVAS_THEMES` in src/types/config.ts.
    expect(ALL_FULL_THEMES.filter(isLightCanvas).slice().sort()).toEqual(['kaleido_eink', 'light']);
    expect(ALL_FULL_THEMES.filter((t) => rampFloorFor(t) === RAMP_FLOOR_LIGHT_CANVAS).slice().sort()).toEqual([
      'kaleido_eink',
      'light',
    ]);
  });

  it.each(ALL_FULL_THEMES)(
    '%s keeps the whole depth ramp above the contrast floor for its own canvas',
    (theme) => {
      const { worst, at, canvas } = worstOnRamp(theme);
      const floor = rampFloorFor(theme);
      const known = KNOWN_BELOW_FLOOR[theme];
      if (known === undefined) {
        expect(
          worst,
          `${theme}: ${at} on ${canvas} is ${worst.toFixed(2)}:1, below the ${floor}:1 floor`
        ).toBeGreaterThanOrEqual(floor);
        return;
      }
      // Not allowed to get worse...
      expect(
        worst,
        `${theme} regressed: ${at} on ${canvas} is now ${worst.toFixed(2)}:1, was ${known}:1`
      ).toBeGreaterThanOrEqual(known);
      // ...and once it clears the floor, this entry is stale - delete it.
      // Compared against THIS theme's floor, not the bare RAMP_FLOOR: a
      // light-canvas theme is held to 2:1, so measuring 2.0-3.0 clears its own
      // floor while still sitting below 3. Hard-coding 3 here would let such an
      // entry survive forever, which defeats the whole point of a staleness
      // check that exists to force exemptions back off the list.
      expect(
        worst,
        `${theme} now clears ${floor}:1 (${worst.toFixed(2)}:1) - remove it from KNOWN_BELOW_FLOOR`
      ).toBeLessThan(floor);
    }
  );

  it.each(ALL_FULL_THEMES)('%s ramps from green (shallower) to red (deeper)', (theme) => {
    const { shallow, deep } = sweepRamp(theme);
    const hs = signedHue(shallow);
    const hd = signedHue(deep);
    expect(hs, `${theme}: shallow ${shallow} is at hue ${hs.toFixed(0)}, not green`).toBeGreaterThanOrEqual(SHALLOW_HUE.min);
    expect(hs, `${theme}: shallow ${shallow} is at hue ${hs.toFixed(0)}, not green`).toBeLessThanOrEqual(SHALLOW_HUE.max);
    expect(hd, `${theme}: deep ${deep} is at hue ${hd.toFixed(0)}, not red`).toBeGreaterThanOrEqual(DEEP_HUE.min);
    expect(hd, `${theme}: deep ${deep} is at hue ${hd.toFixed(0)}, not red`).toBeLessThanOrEqual(DEEP_HUE.max);
  });

  it.each(ALL_FULL_THEMES)(
    '%s traverses green -> yellow -> orange -> red, with no detour through blue',
    (theme) => {
      // THIS is the assertion the endpoints cannot make. Two green/red endpoints
      // joined the wrong way round the wheel still satisfy the test above; they
      // fail here, because the sweep's hue climbs towards blue (180+) instead of
      // falling towards red.
      const { samples } = sweepRamp(theme);
      const hues = samples.map((s) => s.hue);
      for (let i = 1; i < hues.length; i++) {
        expect(
          hues[i]!,
          `${theme}: hue rose from ${hues[i - 1]!.toFixed(0)} to ${hues[i]!.toFixed(0)} at ` +
            `${samples[i]!.color} - the ramp is taking the long way round through blue/magenta`
        ).toBeLessThanOrEqual(hues[i - 1]! + 1e-9);
      }
      for (const s of samples) {
        expect(
          s.hue,
          `${theme}: ${s.color} sits at hue ${s.hue.toFixed(0)}, outside the green -> red band`
        ).toBeGreaterThanOrEqual(DEEP_HUE.min);
        expect(
          s.hue,
          `${theme}: ${s.color} sits at hue ${s.hue.toFixed(0)}, outside the green -> red band`
        ).toBeLessThanOrEqual(SHALLOW_HUE.max);
      }
    }
  );

  it.each(ALL_FULL_THEMES)('%s separates its two ends in luminance, not just hue', (theme) => {
    const { shallow, deep } = sweepRamp(theme);
    const separation = contrast(shallow, deep);
    expect(
      separation,
      `${theme}: ${shallow} and ${deep} are only ${separation.toFixed(2)}:1 apart - a ` +
        `red/green ramp this flat in luminance is unreadable under deuteranopia`
    ).toBeGreaterThanOrEqual(MIN_END_SEPARATION);
  });

  it('would reject a ramp that takes the long way round the wheel', () => {
    // Proves the traversal test above can fail, on the exact pairing `one_dark`
    // shipped (#357 item 8): its #56b6c2 cyan sits at hue 187, just past the 180
    // line, so the shortest path to the #e06c75 red at 355 climbs through indigo
    // instead of falling through yellow. Both ENDPOINTS look defensible in
    // isolation - which is the whole reason the sweep exists.
    const canvas = themeTokens.one_dark['--cc-map-background'];
    const samples: { hue: number; ratio: number }[] = [];
    for (let i = 0; i <= 40; i++) {
      const color = getDepthColor(-40 + i, '#56b6c2', '#e06c75', -40, 0)!;
      samples.push({ hue: signedHue(color), ratio: contrast(color, canvas) });
    }

    // 1. The endpoint check alone does NOT catch it: the deep end is a perfectly
    //    ordinary red. Only the shallow end is out of band, and a ramp built from
    //    two in-band endpoints could take this same detour.
    expect(signedHue('#e06c75')).toBeGreaterThanOrEqual(DEEP_HUE.min);
    expect(signedHue('#e06c75')).toBeLessThanOrEqual(DEEP_HUE.max);

    // 2. The traversal check does catch it, twice over. Signed hue puts the
    //    blue/magenta arc at -173..-5, so the sweep both RISES (the band is meant
    //    to fall, green -> red) and leaves the band on the blue side.
    const rose = samples.some((s, i) => i > 0 && s.hue > samples[i - 1]!.hue + 1e-9);
    expect(rose, 'the bad ramp should show a rising hue').toBe(true);
    expect(Math.min(...samples.map((s) => s.hue))).toBeLessThan(DEEP_HUE.min);

    // 3. And the mid it detours through is what breaks the contrast floor.
    expect(Math.min(...samples.map((s) => s.ratio))).toBeLessThan(RAMP_FLOOR);
  });

  it('would reject the dark endpoints on the light canvas', () => {
    // Proves the test above can fail: this is the exact pairing light shipped with
    // before the endpoints were themed.
    const light = themeTokens.light['--cc-map-background'];
    let worst = Number.POSITIVE_INFINITY;
    for (let i = 0; i <= 40; i++) {
      const color = getDepthColor(
        -40 + i,
        themeTokens.dark['--cc-depth-shallow'],
        themeTokens.dark['--cc-depth-deep'],
        -40,
        0
      );
      worst = Math.min(worst, contrast(color!, light));
    }
    expect(worst).toBeLessThan(2);
  });
});

/**
 * Read-only contrast audit findings 1-3 (#357). Each pairing failed WCAG on at
 * least one theme without any token being missing - the CSS/tokens.ts sync
 * checks above cannot see this class of defect, only a real luminance
 * comparison can. Guards live here (not a parallel harness) so a future retune
 * of any of these three tokens is checked the same way the depth ramp is.
 */
describe('contrast audit (#357)', () => {
  // Finding 1: --cc-text-secondary on --cc-panel. Drives the vertical "Sections"
  // tab label (11px text, needs 4.5:1) and the pin/close icons (needs 3:1) in
  // PanelOpenerTab.tsx - one shared box, one shared token. 4.5:1 is the binding
  // threshold since it's stricter than the icon floor.
  const TEXT_SECONDARY_ON_PANEL_FLOOR = 4.5;

  it.each(ALL_THEME_NAMES)(
    '%s: --cc-text-secondary on --cc-panel clears 4.5:1 (11px label) and 3:1 (icons)',
    (theme) => {
      const ratio = contrast(themeTokens[theme]['--cc-text-secondary'], themeTokens[theme]['--cc-panel']);
      expect(
        ratio,
        `${theme}: --cc-text-secondary ${themeTokens[theme]['--cc-text-secondary']} on ` +
          `--cc-panel ${themeTokens[theme]['--cc-panel']} is ${ratio.toFixed(2)}:1, below 4.5:1`
      ).toBeGreaterThanOrEqual(TEXT_SECONDARY_ON_PANEL_FLOOR);
    }
  );

  // Finding 2: --cc-map-scale-border on --cc-map-background. 3:1 is the WCAG
  // floor for a non-text UI boundary (the "200 m" scale box border).
  const MAP_SCALE_BORDER_FLOOR = 3;

  it.each(ALL_THEME_NAMES)(
    '%s: --cc-map-scale-border on its map canvas clears 3:1',
    (theme) => {
      const canvas = mapCanvasFor(theme);
      const ratio = contrast(themeTokens[theme]['--cc-map-scale-border'], canvas);
      expect(
        ratio,
        `${theme}: --cc-map-scale-border ${themeTokens[theme]['--cc-map-scale-border']} on ` +
          `map canvas ${canvas} is ${ratio.toFixed(2)}:1, below 3:1`
      ).toBeGreaterThanOrEqual(MAP_SCALE_BORDER_FLOOR);
    }
  );

  // Finding 3: --cc-map-scale-text on --cc-map-background. --cc-map-scale-bg is
  // transparent on every theme, so this text sits directly on the map canvas -
  // which for the chrome-only light themes is the DARK :root canvas, not their
  // own light panel.
  const MAP_SCALE_TEXT_FLOOR = 4.5;

  it.each(ALL_THEME_NAMES)('%s: --cc-map-scale-text on its map canvas clears 4.5:1', (theme) => {
    const canvas = mapCanvasFor(theme);
    const ratio = contrast(themeTokens[theme]['--cc-map-scale-text'], canvas);
    expect(
      ratio,
      `${theme}: --cc-map-scale-text ${themeTokens[theme]['--cc-map-scale-text']} on ` +
        `map canvas ${canvas} is ${ratio.toFixed(2)}:1, below 4.5:1`
    ).toBeGreaterThanOrEqual(MAP_SCALE_TEXT_FLOOR);
  });
});

/**
 * index.html paints the frame before the CSS bundle loads. Nothing else in the
 * repo reads that file, so these are the only guards on it: #332 was caused by a
 * hard-coded background there, and the first fix for it reintroduced a different
 * hard-coded guess (an OS colour-scheme media query, which disagrees with the
 * shipped default of theme "dark").
 */
describe('index.html anti-flash cover', () => {
  it('uses the same hex as :root --cc-background', () => {
    const cover = /html\s*\{\s*background-color:\s*(#[0-9a-f]{3,8})\s*;/i.exec(
      htmlRules
    );
    expect(cover).not.toBeNull();
    expect(cover![1]!.toLowerCase()).toBe(
      themeTokens.dark['--cc-background'].toLowerCase()
    );
  });

  it('hands over to the live token once data-theme is stamped', () => {
    expect(htmlRules).toMatch(
      /html\[data-theme\]\s*\{\s*background-color:\s*var\(\s*--cc-background\s*\)\s*;/
    );
  });

  it('does not pin the background with an inline style on body (#332)', () => {
    const bodyTag = /<body[^>]*>/i.exec(htmlRules);
    expect(bodyTag).not.toBeNull();
    expect(bodyTag![0]).not.toMatch(/style\s*=/i);
  });

  it('declares every depth endpoint as a plain #rrggbb literal (#360)', () => {
    // These two tokens are the only colour rule that crosses into Rust: the
    // image export is sent whatever the document resolves them to, and the
    // Rust side accepts ONLY `#rrggbb` (src-tauri/src/export/image.rs,
    // `parse_hex_endpoint`), falling back to the dark ramp for anything else.
    // That fallback is SILENT by design - a malformed colour must not paint a
    // black survey - so a future theme written as `rgb(...)`, `oklch(...)` or
    // `#fff` would revert #360 with no error and no failing test anywhere.
    // This is the only place the contract can be pinned in the language that
    // owns the tokens.
    //
    // Absence is a FAILURE here, not a skip. This loop used to `continue` past
    // an undefined value, which made it silently vacuous in two directions: a
    // theme whose endpoints stopped being parsed out of the CSS (a renamed
    // token, a reformatted block `extractTokens` no longer matches) checked
    // nothing, and a theme missing from the hand-maintained `cssTokensByTheme`
    // was never visited at all. Both are pinned below instead.
    expect(
      Object.keys(cssTokensByTheme).slice().sort(),
      'cssTokensByTheme is hand-maintained - a theme missing from it is never checked by this test'
    ).toEqual([...ALL_THEME_NAMES].slice().sort());

    // The themes that declare depth endpoints, pinned in both directions. The
    // other two (solarized_light, catppuccin_latte) are chrome-only and inherit
    // :root's ramp - see the "only override UI chrome and controls" test above.
    const DEPTH_ENDPOINT_THEMES = [
      'catppuccin_mocha',
      'cave',
      'dark',
      'dracula',
      'kaleido_eink',
      'light',
      'nord',
      'one_dark',
    ];

    const declaring: string[] = [];
    for (const [theme, tokens] of Object.entries(cssTokensByTheme)) {
      const shallow = tokens['--cc-depth-shallow'];
      const deep = tokens['--cc-depth-deep'];
      // All-or-nothing: the map-viz tokens ship as one block, so half a ramp is
      // a defect regardless of which half is missing.
      expect(
        deep === undefined,
        `${theme} declares one depth endpoint but not the other`
      ).toBe(shallow === undefined);
      if (shallow === undefined) continue;
      declaring.push(theme);
      for (const [name, value] of [
        ['--cc-depth-shallow', shallow],
        ['--cc-depth-deep', deep!],
      ] as const) {
        expect(value, `${theme} ${name} is not #rrggbb`).toMatch(/^#[0-9a-fA-F]{6}$/);
      }
    }
    expect(
      declaring.slice().sort(),
      'a theme dropped out of (or joined) the set with depth endpoints'
    ).toEqual(DEPTH_ENDPOINT_THEMES);
  });

  it('does not guess the theme from the OS colour scheme', () => {
    // The OS preference is only consulted when appearance.theme is "auto"
    // (src/hooks/useTheme.ts), so keying the cover off it repaints the shipped
    // default the wrong colour on a light-appearance machine.
    expect(htmlRules).not.toMatch(/@media[^{]*prefers-color-scheme/i);
  });
});

/** `#rrggbb`, or `rgba(r, g, b, a)` composited over `under` (a `#rrggbb`). */
function solid(value: string, under: string): string {
  if (value.startsWith('#')) return value;
  const m = /^rgba\((\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\)$/.exec(value);
  if (!m) throw new Error(`unparsed colour ${value}`);
  const alpha = Number(m[4]);
  const base = [1, 3, 5].map((i) => parseInt(under.slice(i, i + 2), 16));
  return `#${[m[1], m[2], m[3]]
    .map((c, i) => Math.round(alpha * Number(c) + (1 - alpha) * base[i]!))
    .map((c) => c.toString(16).padStart(2, '0'))
    .join('')}`;
}

/**
 * Warning PROSE (`text-warning-text`) sits on the panel, on the page background and
 * on the warning washes (`bg-warning-10` / `bg-warning-20`, composited over either):
 * the remove-copy and purge warnings, the insecure-HTTP note, the workspace's
 * un-uploaded line. Every theme must keep it at WCAG AA for normal text, 4.5:1, on all six.
 * `--cc-warning` itself is a fill colour and is not held to this.
 */
describe('warning text contrast', () => {
  for (const theme of ALL_THEME_NAMES) {
    it(`${theme}: warning-text reads at AA on the panel, the background and both washes`, () => {
      const tokens = themeTokens[theme] as Record<string, string>;
      const text = tokens['--cc-warning-text']!;
      expect(text).toMatch(/^#[0-9a-fA-F]{6}$/);
      for (const surface of ['--cc-panel', '--cc-background']) {
        const under = tokens[surface]!;
        for (const bg of [under, solid(tokens['--cc-warning-10']!, under), solid(tokens['--cc-warning-20']!, under)]) {
          const ratio = contrast(text, bg);
          expect(ratio, `${theme}: --cc-warning-text ${text} on ${bg} (over ${surface}) is ${ratio.toFixed(2)}:1`)
            .toBeGreaterThanOrEqual(4.5);
        }
      }
    });
  }

  it('would reject the light theme\'s fill yellow as text on its own wash', () => {
    const tokens = themeTokens.light as Record<string, string>;
    // 2.64:1 - what the un-uploaded tab and the warning boxes used to show.
    expect(contrast(tokens['--cc-warning']!, solid(tokens['--cc-warning-20']!, tokens['--cc-panel']!))).toBeLessThan(4.5);
  });
});

/**
 * The SpeleoDB tab with un-uploaded changes says so by its text colour alone
 * (src/components/SingleSystemView/LeftWorkspace.tsx). Selected, it is the warning
 * text on the selected tab's accent wash (`bg-accent-10` over the panel): AA,
 * 4.5:1. Not selected, it is dimmed toward the panel the way idle tabs are dimmer
 * than the selected one (`text-warning-text-dim`, a `color-mix` in
 * tailwind.config.js), and must still read: 3:1 or better on the panel.
 */
describe('warning tab text contrast', () => {
  const twSource = fs.readFileSync(path.resolve(__dirname, '../../../tailwind.config.js'), 'utf8');
  const dim = /'warning-text-dim':\s*'color-mix\(in srgb, var\(--cc-warning-text\) (\d+)%, var\(--cc-panel\)\)'/.exec(twSource);

  it('dims the warning text by mixing it toward the panel', () => {
    expect(dim, "tailwind.config.js 'warning-text-dim'").not.toBeNull();
  });

  const share = Number(dim?.[1] ?? 0) / 100;
  for (const theme of ALL_THEME_NAMES) {
    it(`${theme}: selected at AA on the accent wash, idle at 3:1 dimmed on the panel`, () => {
      const tokens = themeTokens[theme] as Record<string, string>;
      const panel = tokens['--cc-panel']!;
      const text = tokens['--cc-warning-text']!;
      const selected = contrast(text, solid(tokens['--cc-accent-10']!, panel));
      expect(selected, `${theme}: selected ${selected.toFixed(2)}:1`).toBeGreaterThanOrEqual(4.5);
      const rgb = [1, 3, 5].map((i) => parseInt(text.slice(i, i + 2), 16));
      const dimmed = solid(`rgba(${rgb.join(', ')}, ${share})`, panel);
      const idle = contrast(dimmed, panel);
      expect(idle, `${theme}: dimmed ${dimmed} on ${panel} is ${idle.toFixed(2)}:1`).toBeGreaterThanOrEqual(3);
      // Visibly dimmer than the full-strength text the selected tab shows.
      expect(idle).toBeLessThan(contrast(text, panel) * 0.8);
    });
  }
});

/**
 * The `secondary` Button fill (`bg-button-secondary` in tailwind.config.js) is the
 * panel mixed a little toward the text colour, so the button stands apart from the
 * panel it sits on, and its label must still read: AA (4.5:1) at rest and on hover
 * in every theme whose text colour reaches AA on its plain panel. Solarized light's
 * text colour does not (4.39:1 on the panel, already on master; follow-up #671), so
 * that theme alone is held to 3.5:1 until its text colour is fixed.
 */
describe('secondary button fill', () => {
  const twSource = fs.readFileSync(path.resolve(__dirname, '../../../tailwind.config.js'), 'utf8');
  const rule = (name: string) => new RegExp(
    `'${name}':\\s*'color-mix\\(in srgb, var\\(--cc-panel\\) (\\d+)%, var\\(--cc-text-primary\\)\\)'`,
  ).exec(twSource);
  const fill = rule('button-secondary');
  const hover = rule('button-secondary-hover');

  it('is the panel mixed toward the text colour, with a stronger hover', () => {
    expect(fill, "tailwind.config.js 'button-secondary'").not.toBeNull();
    expect(hover, "tailwind.config.js 'button-secondary-hover'").not.toBeNull();
    expect(Number(hover?.[1])).toBeLessThan(Number(fill?.[1]));
  });

  /** CSS color-mix(in srgb, panel p%, text): straight interpolation of the encoded channels. */
  const mixed = (panel: string, text: string, share: number) => {
    const ch = (hex: string) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
    const [p, t] = [ch(panel), ch(text)];
    return `#${p.map((v, i) => Math.round(v * share + t[i]! * (1 - share)).toString(16).padStart(2, '0')).join('')}`;
  };

  for (const theme of ALL_THEME_NAMES) {
    it(`${theme}: the fill stands apart from the panel and its label reads`, () => {
      const tokens = themeTokens[theme] as Record<string, string>;
      const panel = tokens['--cc-panel']!;
      const text = tokens['--cc-text-primary']!;
      const floor = contrast(text, panel) >= 4.5 ? 4.5 : 3.5;
      if (theme !== 'solarized_light') expect(floor, `${theme} text on its panel`).toBe(4.5);
      for (const share of [Number(fill?.[1]) / 100, Number(hover?.[1]) / 100]) {
        const bg = mixed(panel, text, share);
        expect(contrast(bg, panel), `${theme}: ${bg} vs panel ${panel}`).toBeGreaterThan(1.08);
        const label = contrast(text, bg);
        expect(label, `${theme}: label ${text} on ${bg} is ${label.toFixed(2)}:1`).toBeGreaterThanOrEqual(floor);
      }
    });
  }
});
