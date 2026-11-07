import { describe, it, expect, beforeEach, afterAll, vi } from 'vitest';
import { pushDialog, pushDialogBeneath, popDialog, isTopmost } from '../dialogStack';

/**
 * Create a mock HTMLElement with the attributes API needed by dialogStack.
 */
function createMockElement(id: string): HTMLElement {
  const attrs = new Map<string, string>();
  return {
    id,
    setAttribute: vi.fn((name: string, value: string) => { attrs.set(name, value); }),
    removeAttribute: vi.fn((name: string) => { attrs.delete(name); }),
    hasAttribute: (name: string) => attrs.has(name),
    getAttribute: (name: string) => attrs.get(name) ?? null,
    // dialogStack stamps the paint layer here; a real element always has it.
    style: {} as CSSStyleDeclaration,
  } as unknown as HTMLElement;
}

/**
 * Reset the module-level stack between tests by popping all elements.
 * We track pushed elements and pop them in reverse order.
 */
let trackedElements: HTMLElement[] = [];

function pushAndTrack(el: HTMLElement): void {
  pushDialog(el);
  trackedElements.push(el);
}

function resetStack(): void {
  // Pop all tracked elements in reverse
  for (let i = trackedElements.length - 1; i >= 0; i--) {
    popDialog(trackedElements[i]!);
  }
  trackedElements = [];
}

// Mock document.getElementById to return a dialog-root with siblings
const mockDialogRoot = createMockElement('dialog-root');
const mockAppRoot = createMockElement('root');

const mockParent = {
  children: [mockAppRoot, mockDialogRoot],
};
Object.defineProperty(mockDialogRoot, 'parentElement', { value: mockParent });

vi.stubGlobal('document', {
  getElementById: (id: string) => {
    if (id === 'dialog-root') return mockDialogRoot;
    if (id === 'root') return mockAppRoot;
    return null;
  },
});

// Also stub import.meta.env for the dev warning
vi.stubGlobal('import', { meta: { env: { DEV: true } } });

describe('dialogStack', () => {
  beforeEach(() => {
    resetStack();
    vi.clearAllMocks();
  });

  afterAll(() => {
    vi.unstubAllGlobals();
  });

  describe('pushDialog', () => {
    it('makes the pushed element topmost', () => {
      const el = createMockElement('dialog-a');
      pushAndTrack(el);
      expect(isTopmost(el)).toBe(true);
    });

    it('sets inert on app root siblings when pushing', () => {
      const el = createMockElement('dialog-a');
      pushAndTrack(el);
      expect(mockAppRoot.setAttribute).toHaveBeenCalledWith('inert', '');
    });

    it('sets inert on previously stacked dialogs', () => {
      const a = createMockElement('dialog-a');
      const b = createMockElement('dialog-b');
      pushAndTrack(a);
      pushAndTrack(b);

      expect(a.setAttribute).toHaveBeenCalledWith('inert', '');
      expect(isTopmost(b)).toBe(true);
      expect(isTopmost(a)).toBe(false);
    });

    it('ensures pushed element is not inert', () => {
      const el = createMockElement('dialog-a');
      pushAndTrack(el);
      expect(el.removeAttribute).toHaveBeenCalledWith('inert');
    });

    it('de-duplicates: moves existing element to top', () => {
      const a = createMockElement('dialog-a');
      const b = createMockElement('dialog-b');
      pushAndTrack(a);
      pushAndTrack(b);

      // Push A again - should move to top
      pushDialog(a);
      trackedElements.push(a); // track for cleanup

      expect(isTopmost(a)).toBe(true);
      expect(isTopmost(b)).toBe(false);
    });
  });

  describe('popDialog', () => {
    it('removes element from stack', () => {
      const el = createMockElement('dialog-a');
      pushAndTrack(el);
      popDialog(el);
      trackedElements = [];

      expect(isTopmost(el)).toBe(false);
    });

    it('clears inert from popped element', () => {
      const el = createMockElement('dialog-a');
      pushAndTrack(el);
      popDialog(el);
      trackedElements = [];

      expect(el.removeAttribute).toHaveBeenCalledWith('inert');
    });

    it('restores interactivity to new topmost dialog', () => {
      const a = createMockElement('dialog-a');
      const b = createMockElement('dialog-b');
      pushAndTrack(a);
      pushAndTrack(b);

      popDialog(b);
      trackedElements.pop();

      expect(isTopmost(a)).toBe(true);
      // a should have inert removed when it becomes topmost again
      expect(a.removeAttribute).toHaveBeenCalledWith('inert');
    });

    it('removes app inert when stack is empty', () => {
      const el = createMockElement('dialog-a');
      pushAndTrack(el);
      vi.clearAllMocks();

      popDialog(el);
      trackedElements = [];

      expect(mockAppRoot.removeAttribute).toHaveBeenCalledWith('inert');
    });

    it('does nothing for unknown element', () => {
      const el = createMockElement('unknown');
      vi.clearAllMocks();
      popDialog(el);
      // Should not throw or modify anything
      expect(mockAppRoot.removeAttribute).not.toHaveBeenCalled();
    });

    it('handles non-LIFO removal correctly', () => {
      const a = createMockElement('dialog-a');
      const b = createMockElement('dialog-b');
      const c = createMockElement('dialog-c');
      pushAndTrack(a);
      pushAndTrack(b);
      pushAndTrack(c);

      // Remove middle element (non-LIFO)
      popDialog(b);
      trackedElements = trackedElements.filter(e => e !== b);

      expect(isTopmost(c)).toBe(true);
      expect(isTopmost(a)).toBe(false);
    });
  });

  // [W71-NF1] The project loading window: a modal layer that never covers a
  // dialog, whichever opened first (an import starts a load under its own dialog).
  describe('pushDialogBeneath', () => {
    it('on an empty stack, is the interactive top layer with the app inert', () => {
      const loading = createMockElement('loading');
      pushDialogBeneath(loading);
      trackedElements.push(loading);

      expect(isTopmost(loading)).toBe(true);
      expect(loading.hasAttribute('inert')).toBe(false);
      expect(mockAppRoot.setAttribute).toHaveBeenCalledWith('inert', '');
    });

    it('goes beneath a dialog already open: inert, painted below, the dialog stays topmost', () => {
      const dialog = createMockElement('import-dialog');
      pushAndTrack(dialog);
      const loading = createMockElement('loading');
      pushDialogBeneath(loading);
      trackedElements.push(loading);

      expect(isTopmost(dialog)).toBe(true);
      expect(dialog.hasAttribute('inert')).toBe(false);
      expect(loading.hasAttribute('inert')).toBe(true);
      expect(Number(loading.style.zIndex)).toBeLessThan(Number(dialog.style.zIndex));
    });

    it('takes the top back when the dialog above it closes, and leaves the app inert', () => {
      const loading = createMockElement('loading');
      pushDialogBeneath(loading);
      trackedElements.push(loading);
      const dialog = createMockElement('dialog');
      pushAndTrack(dialog);
      expect(loading.hasAttribute('inert')).toBe(true);
      vi.clearAllMocks();

      popDialog(dialog);
      trackedElements = trackedElements.filter((e) => e !== dialog);

      expect(isTopmost(loading)).toBe(true);
      expect(loading.hasAttribute('inert')).toBe(false);
      expect(mockAppRoot.removeAttribute).not.toHaveBeenCalledWith('inert');
    });

    it('leaves the dialog above it in place when it closes first', () => {
      const dialog = createMockElement('import-dialog');
      pushAndTrack(dialog);
      const loading = createMockElement('loading');
      pushDialogBeneath(loading);
      vi.clearAllMocks();

      popDialog(loading);

      expect(isTopmost(dialog)).toBe(true);
      expect(dialog.hasAttribute('inert')).toBe(false);
      expect(loading.style.zIndex).toBe('');
      expect(mockAppRoot.removeAttribute).not.toHaveBeenCalledWith('inert');
    });
  });

  describe('isTopmost', () => {
    it('returns false for empty stack', () => {
      const el = createMockElement('dialog-a');
      expect(isTopmost(el)).toBe(false);
    });

    it('returns true only for the last element', () => {
      const a = createMockElement('dialog-a');
      const b = createMockElement('dialog-b');
      pushAndTrack(a);
      pushAndTrack(b);

      expect(isTopmost(a)).toBe(false);
      expect(isTopmost(b)).toBe(true);
    });
  });

  describe('paint order', () => {
    // The reason these exist: `inert` and z-index used to be decided by different
    // mechanisms, so the topmost dialog could be the interactive one while being
    // painted UNDERNEATH the dialog that raised it (#258 - the file picker buried
    // by the revert dialog). A user clicking controls they cannot see is worse
    // than either problem alone, so the two orders are now stamped together.
    it('paints each dialog above the one below it', () => {
      const a = createMockElement('dialog-a');
      const b = createMockElement('dialog-b');
      pushAndTrack(a);
      pushAndTrack(b);

      expect(Number(b.style.zIndex)).toBeGreaterThan(Number(a.style.zIndex));
    });

    it('DO NOT REGRESS: the topmost dialog is both interactive AND on top', () => {
      // Pinning them together, because the defect was precisely that they
      // disagreed - each half looked correct on its own.
      const host = createMockElement('host');
      const overlay = createMockElement('overlay');
      pushAndTrack(host);
      pushAndTrack(overlay);

      expect(isTopmost(overlay)).toBe(true);
      expect(overlay.hasAttribute('inert')).toBe(false);
      expect(host.hasAttribute('inert')).toBe(true);
      expect(Number(overlay.style.zIndex)).toBeGreaterThan(Number(host.style.zIndex));
    });

    it('hands the top layer back when the overlay closes', () => {
      const a = createMockElement('dialog-a');
      const b = createMockElement('dialog-b');
      pushAndTrack(a);
      pushAndTrack(b);
      const wasOnTop = Number(b.style.zIndex);

      popDialog(b);

      // The popped container keeps no stale layer - it may be re-used later, and
      // starting from wherever it last sat would re-create this class of bug.
      expect(b.style.zIndex).toBe('');
      expect(Number(a.style.zIndex)).toBeLessThan(wasOnTop);
      expect(isTopmost(a)).toBe(true);
    });

    it('re-ranks a dialog that is re-pushed rather than leaving it buried', () => {
      // `pushDialog` de-duplicates by moving an existing entry to the top; the
      // layer has to follow, or a re-raised dialog stays painted where it was.
      const a = createMockElement('dialog-a');
      const b = createMockElement('dialog-b');
      pushAndTrack(a);
      pushAndTrack(b);
      pushDialog(a);

      expect(isTopmost(a)).toBe(true);
      expect(Number(a.style.zIndex)).toBeGreaterThan(Number(b.style.zIndex));
    });
  });
});
