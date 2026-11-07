import { useEffect, type RefObject } from 'react';

/**
 * Comprehensive selector for all focusable elements.
 */
const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not(:disabled)',
  'input:not(:disabled)',
  'textarea:not(:disabled)',
  'select:not(:disabled)',
  '[tabindex]:not([tabindex="-1"])',
  '[contenteditable]',
  'audio[controls]',
  'video[controls]',
  'details > summary',
].join(', ');

/**
 * Check if an element is inside an [inert] or [aria-hidden="true"] subtree.
 */
function isInHiddenSubtree(element: Element, container: HTMLElement): boolean {
  let current: Element | null = element;
  while (current) {
    if (
      current.hasAttribute('inert') ||
      current.getAttribute('aria-hidden') === 'true'
    ) {
      return true;
    }
    if (current === container) break;
    current = current.parentElement;
  }
  return false;
}

/**
 * Get all visible, focusable elements within the container,
 * excluding those inside [inert] or [aria-hidden="true"] subtrees.
 * Exported for a trap's initial focus (`ProjectLoadingOverlay`).
 */
export function getFocusableElements(container: HTMLElement): HTMLElement[] {
  const candidates = container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR);
  const result: HTMLElement[] = [];
  for (const el of candidates) {
    if (!isInHiddenSubtree(el, container)) {
      result.push(el);
    }
  }
  return result;
}

interface UseFocusTrapOptions {
  /** Whether the trap is active (default: true). */
  enabled?: boolean;
}

/**
 * Hook that traps Tab/Shift+Tab focus within a container element.
 *
 * Re-queries focusable elements on every Tab keypress to handle
 * dynamic content (elements added/removed after mount).
 *
 * @param containerRef - Ref to the container element to trap focus within
 * @param options - Configuration options
 */
export function useFocusTrap(
  containerRef: RefObject<HTMLElement>,
  options?: UseFocusTrapOptions,
): void {
  const enabled = options?.enabled ?? true;

  useEffect(() => {
    if (!enabled) return;

    const container = containerRef.current;
    if (!container) return;

    function handleKeyDown(event: KeyboardEvent): void {
      if (event.key !== 'Tab') return;

      const focusable = getFocusableElements(container!);
      if (focusable.length === 0) return;

      const first = focusable[0]!;
      const last = focusable[focusable.length - 1]!;

      if (event.shiftKey) {
        // Shift+Tab: if at first element (or focus is on the container itself), wrap to last
        if (
          document.activeElement === first ||
          document.activeElement === container
        ) {
          event.preventDefault();
          last.focus();
        }
      } else {
        // Tab: if at last element, wrap to first
        if (document.activeElement === last) {
          event.preventDefault();
          first.focus();
        }
      }
    }

    container.addEventListener('keydown', handleKeyDown);

    return () => {
      container.removeEventListener('keydown', handleKeyDown);
    };
  }, [containerRef, enabled]);
}
