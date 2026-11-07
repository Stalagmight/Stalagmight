import type * as maplibregl from 'maplibre-gl';
import type { ToolType } from '../../store/uiStore';
import { toolCursor } from '../../utils/toolUtils';

/**
 * The survey map's canvas cursor - the one place that writes it (#576 D3).
 *
 * Several parts of the map have an opinion about the cursor and each knows only
 * its own input: MapCanvas knows the tool, whether Ctrl is held, whether a
 * modifier pan is in progress and whether a placement is armed; SurveyLayer
 * knows what is under the pointer and whether that thing may be interacted
 * with (anchored / immovable features show no pointer). They used to write
 * `canvas.style.cursor` themselves, six writers in all, and the cursor on
 * screen was whichever ran last - which shipped two regressions (#24, #316) and
 * left a placement's crosshair to be replaced by a hover pointer or a pan's
 * grab. Now each pushes its input here and {@link resolveMapCursor} derives the
 * cursor from all of them at once, so the result no longer depends on order.
 *
 * Precedence, highest first (docs/move-tools.md "Cursor Behavior Summary"):
 *  1. a modifier pan that has claimed the gesture: `grabbing`;
 *  2. placement mode: `crosshair` - placement owns the cursor while it is on;
 *  3. a hovered feature that may be interacted with: `pointer`;
 *  4. Move Station / Move Shot with nothing movable hovered: the plain arrow,
 *     which stops MapLibre's own grab showing through - unless Ctrl is held for
 *     a pan, which outranks it;
 *  5. Ctrl held (any tool but Pan): `grab`;
 *  6. the tool's resting cursor ({@link toolCursor}).
 *
 * A move carry has no rule of its own: SurveyLayer freezes its hover while the
 * drag preview is up, so the carry keeps whatever rule 3/4 said at the pickup.
 */
export interface MapCursorInputs {
  tool: ToolType;
  /** Ctrl is held (the pan modifier). */
  ctrlHeld: boolean;
  /** A Ctrl+left or middle drag has claimed the gesture and is panning. */
  panning: boolean;
  /** A placement is armed (the import placement overlay). */
  placement: boolean;
  /** A feature is hovered AND may be interacted with (SurveyLayer's verdict). */
  hoverPointer: boolean;
}

const INITIAL_INPUTS: MapCursorInputs = {
  tool: 'select',
  ctrlHeld: false,
  panning: false,
  placement: false,
  hoverPointer: false,
};

/** The cursor for a set of inputs. Pure, so the precedence table is unit-testable. */
export function resolveMapCursor(inputs: MapCursorInputs): string {
  if (inputs.panning) return 'grabbing';
  if (inputs.placement) return 'crosshair';
  if (inputs.hoverPointer) return 'pointer';
  const ctrlPan = inputs.ctrlHeld && inputs.tool !== 'pan';
  const arrowFallback = inputs.tool === 'move_station' || inputs.tool === 'move_shot';
  if (arrowFallback && !ctrlPan) return 'default';
  if (ctrlPan) return 'grab';
  return toolCursor(inputs.tool);
}

/** Owns one canvas's cursor. Obtain it with {@link mapCursorFor}, never construct twice. */
export class MapCursor {
  private inputs: MapCursorInputs = { ...INITIAL_INPUTS };

  constructor(private readonly canvas: () => HTMLElement | null) {}

  /** Merge new inputs and re-derive the cursor. */
  set(patch: Partial<MapCursorInputs>): void {
    this.inputs = { ...this.inputs, ...patch };
    this.apply();
  }

  /** The current inputs (read-only snapshot, for tests and debugging). */
  get state(): Readonly<MapCursorInputs> {
    return this.inputs;
  }

  private apply(): void {
    const el = this.canvas();
    if (!el) return;
    const cursor = resolveMapCursor(this.inputs);
    if (el.style.cursor !== cursor) el.style.cursor = cursor;
  }
}

const owners = new WeakMap<maplibregl.Map, MapCursor>();

/**
 * The cursor owner of `map`, created on first use. Keyed by the map so every
 * writer of one map shares one owner without having to be handed it.
 */
export function mapCursorFor(map: maplibregl.Map): MapCursor {
  let owner = owners.get(map);
  if (!owner) {
    owner = new MapCursor(() => map.getCanvas());
    owners.set(map, owner);
  }
  return owner;
}
