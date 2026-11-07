// @vitest-environment jsdom
/**
 * Unit tests for mapDragRelease - forwarding mouse releases that land outside
 * a map's canvas container, so drags released over chrome still end (#409).
 */

import { describe, it, expect, afterEach } from 'vitest';
import type { Map as MaplibreMap } from 'maplibre-gl';
import { attachDragReleaseForwarder } from '../mapDragRelease';
import { makeFakeMap } from '../../test/factories/fakeMap';

/** Dispatch a real capture-phase-visible mouseup from `target`. */
function releaseOver(target: EventTarget): MouseEvent {
  const event = new MouseEvent('mouseup', { bubbles: true, cancelable: true });
  target.dispatchEvent(event);
  return event;
}

afterEach(() => {
  document.body.innerHTML = '';
});

describe('attachDragReleaseForwarder', () => {
  it('forwards a release that lands outside the canvas container', () => {
    const { map, handleEvent } = makeFakeMap({ dom: {} });
    const chrome = document.createElement('button');
    document.body.appendChild(chrome);
    const detach = attachDragReleaseForwarder(map);

    const event = releaseOver(chrome);

    expect(handleEvent).toHaveBeenCalledTimes(1);
    expect(handleEvent).toHaveBeenCalledWith(event);
    detach();
  });

  it('does not forward when no drag is in flight', () => {
    // `handleEvent` runs every handler's `mouseup`, not just dragEnd - on
    // macOS that replays a contextmenu deferred at mousedown - so releases
    // unrelated to a live drag must never be forwarded.
    const { map, handleEvent } = makeFakeMap({ dom: { dragActive: false } });
    const chrome = document.createElement('button');
    document.body.appendChild(chrome);
    const detach = attachDragReleaseForwarder(map);

    releaseOver(chrome);

    expect(handleEvent).not.toHaveBeenCalled();
    detach();
  });

  it('does not forward a release inside the canvas container', () => {
    const { map, canvasContainer, canvas, handleEvent } = makeFakeMap({ dom: {} });
    const detach = attachDragReleaseForwarder(map);

    // MapLibre's own container listener already ends these; forwarding would
    // dispatch `mouseup` to the handlers twice.
    releaseOver(canvas);
    releaseOver(canvasContainer);

    expect(handleEvent).not.toHaveBeenCalled();
    detach();
  });

  it('resolves an accessor at event time, and no-ops while it is empty', () => {
    const { map, handleEvent } = makeFakeMap({ dom: {} });
    let current: MaplibreMap | null = null;
    const detach = attachDragReleaseForwarder(() => current);
    const chrome = document.createElement('button');
    document.body.appendChild(chrome);

    releaseOver(chrome);
    expect(handleEvent).not.toHaveBeenCalled();

    current = map;
    releaseOver(chrome);
    expect(handleEvent).toHaveBeenCalledTimes(1);
    detach();
  });

  it('does not throw when the map has no handler manager', () => {
    const { map } = makeFakeMap({ dom: { withHandlers: false } });
    const withoutHandleEvent = {
      getCanvasContainer: map.getCanvasContainer.bind(map),
      dragPan: { isActive: () => true },
      _handlers: {},
    } as unknown as MaplibreMap;
    const chrome = document.createElement('button');
    document.body.appendChild(chrome);

    const detachNoHandlers = attachDragReleaseForwarder(map);
    expect(() => releaseOver(chrome)).not.toThrow();
    detachNoHandlers();

    const detachNoHandleEvent = attachDragReleaseForwarder(withoutHandleEvent);
    expect(() => releaseOver(chrome)).not.toThrow();
    detachNoHandleEvent();
  });

  it('stops forwarding once the returned unregister runs', () => {
    const { map, handleEvent } = makeFakeMap({ dom: {} });
    const chrome = document.createElement('button');
    document.body.appendChild(chrome);
    const detach = attachDragReleaseForwarder(map);

    releaseOver(chrome);
    expect(handleEvent).toHaveBeenCalledTimes(1);

    detach();
    releaseOver(chrome);
    expect(handleEvent).toHaveBeenCalledTimes(1);
  });

  it('gives each of several mounted maps its own forwarder', () => {
    const first = makeFakeMap({ dom: {} });
    const second = makeFakeMap({ dom: {} });
    const detachFirst = attachDragReleaseForwarder(first.map);
    const detachSecond = attachDragReleaseForwarder(second.map);

    // A release over the second map's canvas is "outside" for the first map, so
    // it is forwarded there when (as here) the first map's drag is live; a map
    // whose dragPan is not active is skipped entirely by the guard above.
    releaseOver(second.canvas);

    expect(first.handleEvent).toHaveBeenCalledTimes(1);
    expect(second.handleEvent).not.toHaveBeenCalled();

    detachFirst();
    detachSecond();
  });
});
