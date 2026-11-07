/**
 * The survey map's one cursor owner (#576 D3). The precedence table is the spec in
 * docs/move-tools.md ("Cursor Behavior Summary"), plus the two modes that outrank
 * every hover: a claimed modifier pan and placement.
 */
import { describe, expect, it } from 'vitest';
import type * as maplibregl from 'maplibre-gl';
import type { ToolType } from '../../../store/uiStore';
import { MapCursor, mapCursorFor, resolveMapCursor, type MapCursorInputs } from '../mapCursor';

const TOOLS: ToolType[] = ['select', 'pan', 'zoom', 'merge', 'measure', 'move_station', 'move_segment', 'move_shot', 'comment', 'move_comment'];
const RESTING: Record<ToolType, string> = {
  select: 'default',
  pan: 'grab',
  zoom: 'zoom-in',
  merge: 'cell',
  measure: 'crosshair',
  move_station: 'default', // the arrow fallback, not the tool's own `move`
  move_segment: 'move',
  move_shot: 'default',
  comment: 'crosshair',
  move_comment: 'crosshair',
};

const base: MapCursorInputs = { tool: 'select', ctrlHeld: false, panning: false, placement: false, hoverPointer: false };

describe('resolveMapCursor', () => {
  it.each(TOOLS)('%s: nothing hovered shows the resting cursor', (tool) => {
    expect(resolveMapCursor({ ...base, tool })).toBe(RESTING[tool]);
  });

  it.each(TOOLS)('%s: an interactive hover shows the pointer', (tool) => {
    expect(resolveMapCursor({ ...base, tool, hoverPointer: true })).toBe('pointer');
  });

  it.each(TOOLS.filter((t) => t !== 'pan'))('%s: Ctrl held shows the pan grab, also over the move-tool arrow', (tool) => {
    expect(resolveMapCursor({ ...base, tool, ctrlHeld: true })).toBe('grab');
  });

  it('pan: Ctrl changes nothing (the tool already pans)', () => {
    expect(resolveMapCursor({ ...base, tool: 'pan', ctrlHeld: true })).toBe('grab');
  });

  it('a hovered interactive feature keeps the pointer while Ctrl is held (Ctrl+click toggles it)', () => {
    expect(resolveMapCursor({ ...base, ctrlHeld: true, hoverPointer: true })).toBe('pointer');
  });

  it.each(TOOLS)('%s: placement shows the crosshair whatever is hovered or held', (tool) => {
    for (const hoverPointer of [false, true]) {
      for (const ctrlHeld of [false, true]) {
        expect(resolveMapCursor({ ...base, tool, placement: true, hoverPointer, ctrlHeld })).toBe('crosshair');
      }
    }
  });

  it.each(TOOLS)('%s: a claimed modifier pan shows grabbing over everything', (tool) => {
    for (const placement of [false, true]) {
      for (const hoverPointer of [false, true]) {
        expect(resolveMapCursor({ ...base, tool, panning: true, placement, hoverPointer })).toBe('grabbing');
      }
    }
  });
});

/** A canvas stand-in that counts writes to `style.cursor`. */
function fakeCanvas(): { el: HTMLElement; writes: string[] } {
  const writes: string[] = [];
  let cursor = '';
  const style = {
    get cursor() {
      return cursor;
    },
    set cursor(v: string) {
      writes.push(v);
      cursor = v;
    },
  };
  return { el: { style } as unknown as HTMLElement, writes };
}

describe('MapCursor', () => {
  it('derives from all inputs whatever order they arrive in (#24, #316)', () => {
    // Tool switch to Zoom while nothing is hovered, then the hover owner re-derives:
    // the hover verdict must not erase the tool cursor.
    const { el } = fakeCanvas();
    const owner = new MapCursor(() => el);
    owner.set({ tool: 'zoom' });
    owner.set({ hoverPointer: false });
    expect(el.style.cursor).toBe('zoom-in');
    // Measure tool, pointer leaves the map: back to the measure crosshair.
    owner.set({ tool: 'measure', hoverPointer: true });
    expect(el.style.cursor).toBe('pointer');
    owner.set({ hoverPointer: false });
    expect(el.style.cursor).toBe('crosshair');
  });

  it('does not rewrite an unchanged cursor', () => {
    const { el, writes } = fakeCanvas();
    const owner = new MapCursor(() => el);
    owner.set({ tool: 'measure' });
    owner.set({ hoverPointer: false });
    owner.set({ ctrlHeld: false });
    expect(writes).toEqual(['crosshair']);
  });

  it('tolerates a missing canvas', () => {
    const owner = new MapCursor(() => null);
    expect(() => owner.set({ tool: 'zoom' })).not.toThrow();
    expect(owner.state.tool).toBe('zoom');
  });

  it('mapCursorFor gives every writer of one map the same owner', () => {
    const { el } = fakeCanvas();
    const map = { getCanvas: () => el } as unknown as maplibregl.Map;
    const other = { getCanvas: () => fakeCanvas().el } as unknown as maplibregl.Map;
    expect(mapCursorFor(map)).toBe(mapCursorFor(map));
    expect(mapCursorFor(map)).not.toBe(mapCursorFor(other));
    mapCursorFor(map).set({ placement: true });
    expect(el.style.cursor).toBe('crosshair');
  });
});
