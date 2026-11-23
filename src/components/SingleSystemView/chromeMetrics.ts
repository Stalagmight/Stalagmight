/**
 * The app chrome's measurements, published as CSS variables on the view's
 * root so fixed-position overlays and floating menus can stay clear of it:
 *
 * - `--toolbar-h`: the toolbar's height (menus that flip upward).
 * - `--toolbar-bottom`: the toolbar's bottom edge in the viewport. A banner
 *   above the toolbar (the conversion notice of an upgraded file) pushes it
 *   down, so the full-screen data table starts there, not at the toolbar's
 *   height from the top: under a banner it hid its own header beneath the
 *   toolbar and took the toolbar's clicks (RS-FX6, P2-4).
 * - `--statusbar-h`: the status bar's height.
 */
export function publishChromeMetrics(root: HTMLElement, toolbar: HTMLElement, statusBar: HTMLElement | null): void {
  root.style.setProperty('--toolbar-h', `${toolbar.offsetHeight}px`);
  root.style.setProperty('--toolbar-bottom', `${Math.round(toolbar.getBoundingClientRect().bottom)}px`);
  if (statusBar) root.style.setProperty('--statusbar-h', `${statusBar.offsetHeight}px`);
}

/** The full-screen data table's top edge: the toolbar's bottom, else its height. */
export const FULLSCREEN_TOP = 'var(--toolbar-bottom, var(--toolbar-h, 48px))';
