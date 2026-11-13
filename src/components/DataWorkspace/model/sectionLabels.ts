/**
 * Unique, <= 16 character section labels for row names.
 *
 * CONTRACT STUB - owned by the workspace-shell work; implement the real
 * unambiguous truncation (see the task brief), keep the signature.
 */
export const SECTION_LABEL_MAX = 16;

export function buildSectionLabels(sections: readonly { id: number; name: string | null }[], defaultName: (id: number) => string): Map<number, string> {
  return new Map(sections.map((s) => [s.id, (s.name?.trim() || defaultName(s.id)).slice(0, SECTION_LABEL_MAX)]));
}
