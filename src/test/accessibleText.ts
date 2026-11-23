/**
 * The text a screen reader reads in `el`: its text nodes in order, skipping
 * every `aria-hidden="true"` subtree (a prose pill's drawing) and keeping
 * `sr-only` text (the pill's plain words). RS-PROSE's tests compare it with
 * the sentence plain text wrote before pills, which must stay unchanged.
 */
export function accessibleText(el: Element): string {
  let out = '';
  const walk = (node: Node) => {
    if (node.nodeType === Node.TEXT_NODE) {
      out += node.textContent ?? '';
      return;
    }
    if (node instanceof Element && node.getAttribute('aria-hidden') === 'true') return;
    node.childNodes.forEach(walk);
  };
  walk(el);
  return out;
}
