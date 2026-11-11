/** Compact progress indicator; the surrounding status supplies its accessible label. */
export function LoadingSpinner({ testId }: { testId: string }) {
  return (
    <span
      data-testid={testId}
      aria-hidden="true"
      className="inline-block w-4 h-4 shrink-0 rounded-full border-2 border-current border-t-transparent animate-spin"
    />
  );
}
