import { useEffect, useState } from 'react';
import { useProjectStore } from '../../store/projectStore';
import { defaultSplitNames, lineNameProblemMessage, splitNameProblems } from '../../utils/nameValidation';

/**
 * The two line names a delete-and-split (Delete Shot in mid-line, a bulk delete
 * of an interior range) gives its halves. Defaults to `<name> A` / `<name> B`
 * made unique; each name must be a usable, project-unique line name (the split
 * line's own name is free: half A keeps the line) and the two must differ.
 */
export function useSplitNames(lineId: number | undefined, active: boolean) {
  const lines = useProjectStore((s) => s.project?.lines);
  const [nameA, setNameA] = useState('');
  const [nameB, setNameB] = useState('');
  const lineName = lines?.find((l) => l.id === lineId)?.name;

  // Prefill once the dialog knows it splits; later name changes elsewhere must
  // not overwrite what the user typed.
  useEffect(() => {
    if (!active || lineId === undefined) return;
    const names = defaultSplitNames(lineName, lines, lineId);
    setNameA(names.a);
    setNameB(names.b);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, lineId]);

  const problems = splitNameProblems(nameA, nameB, lines, lineId);
  const errorA = active && problems.a ? lineNameProblemMessage(problems.a, nameA) : null;
  const errorB = active && problems.b ? lineNameProblemMessage(problems.b, nameB) : null;
  return {
    nameA: nameA.trim(),
    nameB: nameB.trim(),
    fieldsProps: { nameA, nameB, setNameA, setNameB, errorA, errorB },
    valid: errorA === null && errorB === null,
    /** The first problem, for the submit button's title. */
    error: errorA ?? errorB,
  };
}

interface SplitNameFieldsProps {
  nameA: string;
  nameB: string;
  setNameA: (v: string) => void;
  setNameB: (v: string) => void;
  errorA: string | null;
  errorB: string | null;
  labelA: string;
  labelB: string;
  disabled?: boolean;
}

/** The two name inputs, each with its inline error (`split-name-a-error`, ...). */
export function SplitNameFields({ nameA, nameB, setNameA, setNameB, errorA, errorB, labelA, labelB, disabled }: SplitNameFieldsProps) {
  const field = (key: 'a' | 'b', label: string, value: string, set: (v: string) => void, error: string | null) => (
    <div>
      <label className="block text-sm text-text-secondary mb-1" htmlFor={`split-name-${key}`}>
        {label}
      </label>
      <input
        id={`split-name-${key}`}
        type="text"
        value={value}
        onChange={(e) => set(e.target.value)}
        className="w-full"
        disabled={disabled}
        aria-invalid={error !== null}
        {...(error ? { 'aria-describedby': `split-name-${key}-error` } : {})}
        data-testid={`split-name-${key}`}
      />
      {error && (
        <p id={`split-name-${key}-error`} className="mt-1 text-sm text-error" data-testid={`split-name-${key}-error`}>
          {error}
        </p>
      )}
    </div>
  );
  return (
    <div className="space-y-3">
      {field('a', labelA, nameA, setNameA, errorA)}
      {field('b', labelB, nameB, setNameB, errorB)}
    </div>
  );
}
