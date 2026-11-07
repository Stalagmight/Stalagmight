/**
 * Module-level stack of open dialog container elements.
 * The last element is the topmost (interactive) dialog.
 */
const stack: HTMLElement[] = [];

/**
 * Subscribers for the React hook.
 */
const listeners = new Set<() => void>();

function emitChange(): void {
  for (const listener of listeners) {
    listener();
  }
}

/**
 * Set `inert` on all siblings of `#dialog-root` (i.e., the app root and any
 * other siblings). This makes the entire app non-interactive when a dialog is open.
 *
 * Requires `<div id="dialog-root"></div>` in index.html as a sibling of `#root`.
 * Dialogs must be portaled into `#dialog-root` so they are NOT affected by the
 * inert attribute set on `#root`.
 */
function setAppInert(inert: boolean): void {
  const dialogRoot = document.getElementById('dialog-root');
  if (!dialogRoot?.parentElement) {
    if (import.meta.env.DEV) {
      console.warn(
        '[dialogStack] #dialog-root element not found in DOM. ' +
        'Ensure <div id="dialog-root"></div> exists in index.html after #root.',
      );
    }
    return;
  }

  const siblings = dialogRoot.parentElement.children;
  for (const sibling of siblings) {
    if (sibling === dialogRoot) continue;
    if (inert) {
      sibling.setAttribute('inert', '');
    } else {
      sibling.removeAttribute('inert');
    }
  }
}

/**
 * The z-index the FIRST stacked dialog gets; each one above it gets one more.
 *
 * 51 because app chrome inside `#root` tops out at `z-50` and `#dialog-root`
 * creates no stacking context of its own, so dialogs compete with that chrome
 * directly in the root stacking context. Staying just above it leaves the whole
 * 60..9999 band free for the layers that must out-rank every dialog - toasts at
 * `z-[99999]`, tooltips, onboarding.
 */
const BASE_Z = 51;

/**
 * Give every stacked container a z-index matching its position in the stack.
 *
 * Paint order has to be OWNED here, because nothing else can own it correctly.
 * Dialog containers are `position: fixed` with a z-index, and they portal into a
 * `#dialog-root` that is a bare div - no position, no transform, no isolation - so
 * they do not form a local stacking context and are sorted against each other in
 * the ROOT one. Painting compares z-index first and only falls back to DOM order
 * on a tie, so a later-mounted dialog with a lower class-declared z-index can
 * never come out on top however it is inserted.
 *
 * That is not hypothetical: `DialogWrapper` declares `z-50` and `AlertDialog`
 * declares `z-[60]`, so ANY overlay built on the former was painted underneath any
 * dialog built on the latter - visible as the #258 file picker being buried by the
 * revert dialog that raised it. Worse, it was buried while still being the
 * interactive one, because this module's `inert` bookkeeping was right: the user
 * could click controls they could not see.
 *
 * An inline style rather than a class, deliberately - it beats the Tailwind
 * declaration by specificity, so the classes stay as documentation of each
 * dialog's default layer, and it is a real computed value that a test can assert
 * without a stylesheet loaded.
 */
function restack(): void {
  stack.forEach((element, index) => {
    element.style.zIndex = String(BASE_Z + index);
  });
}

/**
 * Push a dialog container onto the stack.
 *
 * - Sets `inert` on all siblings of `#dialog-root` (app root)
 * - Sets `inert` on all previously-stacked dialog containers
 *   so only the topmost dialog is interactive
 * - Re-ranks every container's z-index so the topmost is also the one PAINTED on
 *   top (see [`restack`])
 */
export function pushDialog(element: HTMLElement): void {
  // De-duplicate: if already in stack, move to top
  const existing = stack.indexOf(element);
  if (existing !== -1) {
    stack.splice(existing, 1);
  }

  // Set inert on all previously-stacked dialog containers
  for (const prev of stack) {
    prev.setAttribute('inert', '');
  }

  stack.push(element);

  // Ensure the new topmost dialog is interactive
  element.removeAttribute('inert');

  // Always set app siblings inert when any dialog is open
  setAppInert(true);

  restack();

  emitChange();
}

/**
 * Push a modal container onto the BOTTOM of the stack, beneath every open dialog.
 *
 * For a modal layer that must never cover a dialog, whichever of the two opened
 * first: the project loading window (`ProjectLoadingOverlay`). A survey-file
 * import starts a load while its own dialog is open and keeps that dialog as the
 * interactive surface (its footer Cancel stops the import); `pushDialog` would put
 * the loading window over it. Otherwise the same bookkeeping as `pushDialog`: the
 * app root goes inert, the container is interactive only while nothing is above
 * it, and the layers are re-ranked. A dialog pushed later goes on top as usual,
 * and `popDialog` removes the container wherever it sits.
 */
export function pushDialogBeneath(element: HTMLElement): void {
  const existing = stack.indexOf(element);
  if (existing !== -1) {
    stack.splice(existing, 1);
  }

  stack.unshift(element);

  if (stack.length > 1) {
    element.setAttribute('inert', '');
  } else {
    element.removeAttribute('inert');
  }

  setAppInert(true);

  restack();

  emitChange();
}

/**
 * Remove a dialog container from the stack.
 *
 * - Removes `inert` from the new topmost dialog (if any)
 * - Removes `inert` from app siblings only if stack is now empty
 * - Clears the popped container's stamped z-index and re-ranks the rest
 */
export function popDialog(element: HTMLElement): void {
  const index = stack.indexOf(element);
  if (index === -1) return;

  stack.splice(index, 1);

  // Clear inert AND the stamped layer from the popped element, so a container
  // that is re-used later starts from its declared class again rather than from
  // wherever it happened to sit last time.
  element.removeAttribute('inert');
  element.style.zIndex = '';

  if (stack.length === 0) {
    // No dialogs left - restore app interactivity
    setAppInert(false);
  } else {
    // Restore interactivity to the new topmost dialog
    const topmost = stack[stack.length - 1]!;
    topmost.removeAttribute('inert');
  }

  restack();

  emitChange();
}

/**
 * Check if the given element is the topmost dialog in the stack.
 */
export function isTopmost(element: HTMLElement): boolean {
  return stack.length > 0 && stack[stack.length - 1] === element;
}

