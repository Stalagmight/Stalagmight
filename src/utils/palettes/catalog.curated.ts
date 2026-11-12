/**
 * Hand-authored gradient folders, in the spirit of an image editor's gradient
 * presets: plain two-to-six-colour ramps grouped by hue family, plus metallic
 * banded ramps. Original to Stalagmight (no third-party preset data or names).
 *
 * Colour values here are new colours: per the #380 rule, a change to any of
 * them needs a person to look at the rendered result (the palette contact
 * sheet) before it merges. Stops are evenly spaced unless a position is given.
 */
import type { GeneratedPalette } from './types';

type StopSpec = string | readonly [pos: number, hex: string];

interface CuratedSpec {
  readonly id: string;
  readonly name: string;
  readonly group: string;
  readonly stops: readonly StopSpec[];
}

const SPECS: readonly CuratedSpec[] = [
  // Basics: the classic two- and three-colour ramps.
  { id: 'grad.black-white', name: 'Black, White', group: 'grad-basics', stops: ['#000000', '#ffffff'] },
  { id: 'grad.red-green', name: 'Red, Green', group: 'grad-basics', stops: ['#c62828', '#2e7d32'] },
  { id: 'grad.violet-orange', name: 'Violet, Orange', group: 'grad-basics', stops: ['#4a148c', '#ff8f00'] },
  { id: 'grad.blue-red-yellow', name: 'Blue, Red, Yellow', group: 'grad-basics', stops: ['#0d47a1', '#c62828', '#fdd835'] },
  { id: 'grad.blue-yellow-blue', name: 'Blue, Yellow, Blue', group: 'grad-basics', stops: ['#1565c0', '#fdd835', '#1565c0'] },
  { id: 'grad.orange-yellow-orange', name: 'Orange, Yellow, Orange', group: 'grad-basics', stops: ['#ef6c00', '#ffee58', '#ef6c00'] },
  { id: 'grad.violet-green-orange', name: 'Violet, Green, Orange', group: 'grad-basics', stops: ['#6a1b9a', '#43a047', '#fb8c00'] },
  { id: 'grad.yellow-violet-orange-blue', name: 'Yellow, Violet, Orange, Blue', group: 'grad-basics', stops: ['#fdd835', '#7b1fa2', '#f57c00', '#1976d2'] },

  // Spectrum: full-hue and vivid multi-hue ramps.
  // Stops at magenta rather than wrapping back to red, so the two ends of a
  // data range never look alike.
  { id: 'grad.spectrum', name: 'Spectrum', group: 'grad-spectrum', stops: ['#ff0000', '#ffff00', '#00ff00', '#00ffff', '#0000ff', '#ff00ff'] },
  { id: 'grad.soft-rainbow', name: 'Soft Rainbow', group: 'grad-spectrum', stops: ['#e57373', '#ffb74d', '#fff176', '#81c784', '#64b5f6', '#9575cd'] },
  { id: 'grad.neon', name: 'Neon', group: 'grad-spectrum', stops: ['#ff00a0', '#ffe600', '#00ffd5', '#3d5afe'] },
  { id: 'grad.aurora', name: 'Aurora', group: 'grad-spectrum', stops: ['#0b3d2e', '#1de9b6', '#00b0ff', '#7c4dff', '#f50057'] },

  // Blues and water.
  { id: 'grad.deep-water', name: 'Deep Water', group: 'grad-blues', stops: ['#caf0f8', '#48cae4', '#0077b6', '#03045e'] },
  { id: 'grad.cenote', name: 'Cenote', group: 'grad-blues', stops: ['#b8f2e6', '#20a4a4', '#0b4f6c', '#04151f'] },
  { id: 'grad.ice', name: 'Ice', group: 'grad-blues', stops: ['#ffffff', '#b3e5fc', '#4fc3f7', '#0277bd'] },
  { id: 'grad.navy-sky', name: 'Navy, Sky', group: 'grad-blues', stops: ['#0a1931', '#185adb', '#87ceeb'] },
  { id: 'grad.lagoon', name: 'Lagoon', group: 'grad-blues', stops: ['#e0f7f4', '#4dd0c1', '#00897b', '#004d40'] },
  { id: 'grad.midnight', name: 'Midnight', group: 'grad-blues', stops: ['#000814', '#001d3d', '#003566', '#5c7cfa'] },

  // Greens.
  { id: 'grad.forest', name: 'Forest', group: 'grad-greens', stops: ['#e8f5e9', '#81c784', '#2e7d32', '#0b3d0b'] },
  { id: 'grad.moss', name: 'Moss', group: 'grad-greens', stops: ['#f1f8d6', '#aebd38', '#598216', '#2b3d0a'] },
  { id: 'grad.lime', name: 'Lime', group: 'grad-greens', stops: ['#ffffff', '#d4fc79', '#76b041', '#2a6f2b'] },
  { id: 'grad.emerald-sea', name: 'Emerald Sea', group: 'grad-greens', stops: ['#003d29', '#00a86b', '#7fffd4'] },

  // Reds and oranges.
  { id: 'grad.ember', name: 'Ember', group: 'grad-reds', stops: ['#1a0000', '#7f0000', '#e53935', '#ffcc80'] },
  { id: 'grad.sunset', name: 'Sunset', group: 'grad-reds', stops: ['#2d1b69', '#c2185b', '#ff7043', '#ffd54f'] },
  { id: 'grad.lava', name: 'Lava', group: 'grad-reds', stops: ['#000000', '#8b0000', '#ff4500', '#ffd700'] },
  { id: 'grad.coral', name: 'Coral', group: 'grad-reds', stops: ['#fff3e0', '#ffab91', '#ff7043', '#bf360c'] },
  { id: 'grad.amber', name: 'Amber', group: 'grad-reds', stops: ['#fff8e1', '#ffca28', '#ff8f00', '#e65100'] },

  // Purples and pinks.
  { id: 'grad.amethyst', name: 'Amethyst', group: 'grad-purples', stops: ['#f3e5f5', '#ba68c8', '#7b1fa2', '#38006b'] },
  { id: 'grad.orchid', name: 'Orchid', group: 'grad-purples', stops: ['#fce4ec', '#f48fb1', '#ad1457', '#560027'] },
  { id: 'grad.dusk', name: 'Dusk', group: 'grad-purples', stops: ['#0f0c29', '#302b63', '#8e44ad', '#f8a5c2'] },
  { id: 'grad.rose', name: 'Rose', group: 'grad-purples', stops: ['#fff0f3', '#ffb3c1', '#ff4d6d', '#a4133c'] },

  // Browns and earth: rock and sediment tones.
  { id: 'grad.limestone', name: 'Limestone', group: 'grad-earth', stops: ['#f4efe6', '#d8cbb0', '#a89474', '#6e5b40'] },
  { id: 'grad.clay', name: 'Clay', group: 'grad-earth', stops: ['#f6e0c8', '#d4915d', '#a0522d', '#5c2a12'] },
  { id: 'grad.sandstone', name: 'Sandstone', group: 'grad-earth', stops: ['#fbe7c6', '#e6b17e', '#c1693c', '#7b3b1e'] },
  { id: 'grad.sediment', name: 'Sediment', group: 'grad-earth', stops: ['#e9e4d4', '#9c8f6a', '#5f6b4e', '#2e3a2b'] },

  // Grays.
  { id: 'grad.graphite', name: 'Graphite', group: 'grad-grays', stops: ['#1c1c1c', '#5a5a5a', '#bdbdbd'] },
  { id: 'grad.fog', name: 'Fog', group: 'grad-grays', stops: ['#fafafa', '#cfd8dc', '#78909c', '#37474f'] },
  { id: 'grad.slate', name: 'Slate', group: 'grad-grays', stops: ['#e3e8ef', '#95a2b5', '#48566a', '#111827'] },

  // Metals: banded ramps that read as polished metal.
  { id: 'grad.copper', name: 'Copper', group: 'grad-metals', stops: ['#3b1e08', '#b87333', '#f6c9a0', '#8a4b20', '#e8a96b'] },
  { id: 'grad.chrome', name: 'Chrome', group: 'grad-metals', stops: [[0, '#2a5da8'], [0.45, '#e8f1fb'], [0.5, '#ffffff'], [0.52, '#6b4a2b'], [0.8, '#c9a27a'], [1, '#ffffff']] },
  { id: 'grad.gold', name: 'Gold', group: 'grad-metals', stops: ['#4d3200', '#b8860b', '#ffd700', '#fff4b0', '#b8860b'] },
  { id: 'grad.silver', name: 'Silver', group: 'grad-metals', stops: ['#3a3a3a', '#a8a9ad', '#f5f5f5', '#8a8a8a', '#d9d9d9'] },
  { id: 'grad.bronze', name: 'Bronze', group: 'grad-metals', stops: ['#3d2b1f', '#8c6239', '#cd7f32', '#f0c08a', '#8c6239'] },
];

function channels(hex: string): [number, number, number] {
  const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex);
  if (!m) throw new Error(`curated palette colour is not #rrggbb: ${hex}`);
  return [parseInt(m[1]!, 16), parseInt(m[2]!, 16), parseInt(m[3]!, 16)];
}

function toPalette(spec: CuratedSpec): GeneratedPalette {
  const last = spec.stops.length - 1;
  const data: number[] = [];
  spec.stops.forEach((stop, i) => {
    const [pos, hex] = typeof stop === 'string' ? [last === 0 ? 0 : i / last, stop] : stop;
    data.push(pos, ...channels(hex));
  });
  return { id: spec.id, name: spec.name, group: spec.group, kind: 'stops', data };
}

export const CURATED_PALETTES: readonly GeneratedPalette[] = SPECS.map(toPalette);
