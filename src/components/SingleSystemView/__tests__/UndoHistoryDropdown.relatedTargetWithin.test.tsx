import { describe, it, expect } from 'vitest';

import { relatedTargetWithin } from '../UndoHistoryDropdown';

// `.tsx` so this runs in the jsdom tier (see vite.config.ts environmentMatchGlobs),
// giving us a real DOM `Node`/`window` to exercise the WebKit crash guard.
describe('relatedTargetWithin - Node.contains guard', () => {
  it('returns true when the related target is a node inside the container', () => {
    const container = document.createElement('div');
    const child = document.createElement('button');
    container.appendChild(child);
    expect(relatedTargetWithin(container, child)).toBe(true);
  });

  it('returns false when the related target is a node outside the container', () => {
    const container = document.createElement('div');
    const outside = document.createElement('span');
    expect(relatedTargetWithin(container, outside)).toBe(false);
  });

  it('does not throw and returns false when the related target is window (WebKit leave-into-chrome case)', () => {
    const container = document.createElement('div');
    // window is an EventTarget but not a Node - bare container.contains(window)
    // throws a TypeError in WebKit. The guard must swallow it.
    expect(() => relatedTargetWithin(container, window)).not.toThrow();
    expect(relatedTargetWithin(container, window)).toBe(false);
  });

  it('returns false for a null or undefined related target', () => {
    const container = document.createElement('div');
    expect(relatedTargetWithin(container, null)).toBe(false);
    expect(relatedTargetWithin(container, undefined)).toBe(false);
  });

  it('returns false when the container ref is null', () => {
    const child = document.createElement('button');
    expect(relatedTargetWithin(null, child)).toBe(false);
  });
});
