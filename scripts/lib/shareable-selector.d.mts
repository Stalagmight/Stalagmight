export type SelectorPart = { t: string } | { n: number };
export function codeWords(repoRoot?: string): Set<string>;
export function valueParts(value: unknown, words?: Set<string>): SelectorPart[];
export function selectorParts(selector: unknown, words?: Set<string>): SelectorPart[];
export function testIdParts(testId: unknown, words?: Set<string>): SelectorPart[];
export function renderParts(parts: readonly SelectorPart[], max?: number): string;
export function shareableTestId(testId: unknown, words?: Set<string>): string;
export function shareableSelector(selector: unknown, words?: Set<string>): string;
