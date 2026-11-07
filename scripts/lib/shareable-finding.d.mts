export interface ShareableFinding {
  severity: string;
  kind: string;
  code?: string;
  message: string;
  detail?: unknown;
}
export function shareableFinding(finding: unknown): ShareableFinding;
