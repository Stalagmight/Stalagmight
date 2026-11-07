/**
 * [#576 RV4-FE-MH-2] mapCursor.ts is the one writer of the survey map's cursor
 * (#576 D3): six writers used to set `canvas.style.cursor` themselves and the cursor
 * on screen was whichever ran last, which shipped #24 and #316. mapCursor.test.ts
 * exercises only the owner, so a direct write re-introduced anywhere else in the
 * map would pass it. This pins the property itself: outside mapCursor.ts, nothing
 * under src/components/Map writes a cursor, except the rows below, each a different
 * element with a written reason.
 */
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { describe, expect, it } from 'vitest';

const MAP_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const ALLOWED: Record<string, string> = {
  'mapCursor.ts': 'the owner',
  'useGasPlanDropMarkers.ts': "a stage-drop marker's own DOM element (grab / grabbing while it is dragged), not the map canvas",
};

/** `x.style.cursor = ...` (not `==`), or `style.setProperty('cursor', ...)`. */
const CURSOR_WRITE = /\.style\.cursor\s*=(?!=)|\.style\.setProperty\(\s*['"`]cursor['"`]/;

function sources(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === '__tests__' ? [] : sources(full);
    return /\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name) ? [full] : [];
  });
}

describe('the survey map cursor has one writer (#576 D3, RV4-FE-MH-2)', () => {
  it('no file under src/components/Map writes a cursor except mapCursor.ts and the allowed rows', () => {
    const files = sources(MAP_DIR);
    expect(files.length, 'the walk found no map sources').toBeGreaterThan(20);
    const writers = files
      .filter((file) => !(path.basename(file) in ALLOWED))
      .flatMap((file) =>
        fs
          .readFileSync(file, 'utf8')
          .split('\n')
          .map((line, i) => ({ line: line.trim(), at: `${path.relative(MAP_DIR, file)}:${i + 1}` }))
          .filter(({ line }) => !line.startsWith('//') && !line.startsWith('*') && CURSOR_WRITE.test(line))
          .map(({ line, at }) => `${at}  ${line}`),
      );
    expect(writers, 'push the input to mapCursor.ts instead of writing the cursor').toEqual([]);
  });

  it('every allowed row still writes a cursor (no stale row)', () => {
    for (const name of Object.keys(ALLOWED)) {
      const file = sources(MAP_DIR).find((f) => path.basename(f) === name);
      expect(file, `${name} is gone; drop its row`).toBeDefined();
      expect(CURSOR_WRITE.test(fs.readFileSync(file!, 'utf8')), `${name} no longer writes a cursor; drop its row`).toBe(true);
    }
  });
});
