/**
 * A multi-line value as its one-line cell shows it: its lines, trimmed, the
 * empty ones dropped, joined by " / " (demo finding N-9). HTML collapses a
 * line break to a space, so a merge's joined comment ("T-junction, line
 * arrow points out" and "Silt bank, low visibility", joined by a blank line)
 * read as one run-on sentence. Only the display changes: the cell's tooltip,
 * its editor and the clipboard keep the text with its line breaks.
 */
export function oneLineCellText(text: string): string {
  if (!/[\r\n]/.test(text)) return text;
  return text
    .split(/\r\n|\r|\n/)
    .map((part) => part.trim())
    .filter((part) => part !== '')
    .join(' / ');
}
