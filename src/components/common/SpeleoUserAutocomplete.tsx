import { useEffect, useRef, useState } from 'react';
import { invokeIpc } from '../../utils/ipc';
import type { SpeleoUserSuggestion } from '../../types/ipc.generated';

// The email typeahead queries the server on a short debounce; the backend ignores
// queries under this length (matches the server's 3-char minimum), so we skip the
// call entirely below it rather than provoke a 400.
const AUTOCOMPLETE_MIN_CHARS = 3;
const AUTOCOMPLETE_DEBOUNCE_MS = 250;

/**
 * Server-backed, debounced email typeahead for granting SpeleoDB access, shared by
 * the Project Settings admin roster (#215) and the Publish dialog's collaborator
 * picker. It owns only the transient suggestion dropdown; the chosen email is a
 * controlled `value`/`onChange` pair so the parent keeps the source of truth. The
 * lookup (`speleodb_search_users`) needs only a session token - not an open project
 * - so it works during Publish, before the project exists on the server.
 *
 * A stale in-flight response is dropped (the cleanup flips `cancelled`), and picking
 * a suggestion suppresses the follow-up search that the resulting value change would
 * otherwise fire (which would immediately re-open the dropdown over the chosen entry).
 * All test-ids are passed in so each host keeps its existing selectors.
 */
export function SpeleoUserAutocomplete({
  value,
  onChange,
  disabled = false,
  enabled = true,
  className,
  placeholder,
  ariaLabel,
  inputTestId,
  listTestId,
  suggestionTestIdPrefix,
}: {
  value: string;
  onChange: (email: string) => void;
  disabled?: boolean;
  /** When false, no server searches run (e.g. a non-admin who cannot add). */
  enabled?: boolean;
  className?: string;
  placeholder?: string;
  ariaLabel?: string;
  inputTestId: string;
  listTestId: string;
  /** Each suggestion is `<prefix>-<position>`: a test id never carries the email (#576 RV19b-P1). */
  suggestionTestIdPrefix: string;
}) {
  const [suggestions, setSuggestions] = useState<SpeleoUserSuggestion[]>([]);
  // Holds the email just filled by PICKING a suggestion, so the debounce effect can
  // skip the search the value change would otherwise fire (which would reopen the
  // dropdown over the chosen entry). Keyed by the picked value rather than a plain
  // boolean: if the pick equals the current value, `onChange` is a no-op, the effect
  // never runs, and a boolean flag would stay stuck and swallow the NEXT real search.
  const pickedValue = useRef<string | null>(null);

  useEffect(() => {
    if (!enabled) return;
    if (pickedValue.current !== null) {
      const picked = pickedValue.current;
      pickedValue.current = null;
      // Only skip when THIS render is the pick landing; if the value has already moved
      // on, fall through and search normally (no stuck-suppress).
      if (picked === value) return;
    }
    const query = value.trim();
    if (query.length < AUTOCOMPLETE_MIN_CHARS) {
      setSuggestions([]);
      return;
    }
    let cancelled = false;
    const handle = setTimeout(() => {
      invokeIpc('speleodb_search_users', { query })
        .then((r) => {
          if (!cancelled) setSuggestions(r);
        })
        .catch(() => {
          // A failed lookup just yields no suggestions; the field stays free-text.
          if (!cancelled) setSuggestions([]);
        });
    }, AUTOCOMPLETE_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(handle);
    };
  }, [value, enabled]);

  const pick = (email: string) => {
    pickedValue.current = email;
    onChange(email);
    setSuggestions([]);
  };

  // The blur handler defers closing the list so a suggestion click can land first; the
  // handle is tracked so an unmount within that window can't fire setState on a gone
  // component.
  const blurHandle = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(blurHandle.current), []);

  return (
    <div className="relative flex-1">
      <input
        type="email"
        value={value}
        disabled={disabled}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        // Let a suggestion click register before the blur closes the list.
        onBlur={() => {
          blurHandle.current = window.setTimeout(() => setSuggestions([]), 150);
        }}
        className={className}
        role="combobox"
        aria-expanded={suggestions.length > 0}
        aria-autocomplete="list"
        aria-label={ariaLabel}
        data-testid={inputTestId}
      />
      {suggestions.length > 0 && (
        <ul
          className="absolute z-10 mt-1 w-full max-h-48 overflow-auto bg-background border border-border rounded shadow-lg"
          data-testid={listTestId}
        >
          {suggestions.map((s, i) => (
            <li key={s.email}>
              <button
                type="button"
                // mousedown fires before the input's blur; preventing its default
                // keeps focus so the click below still lands.
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => pick(s.email)}
                className="w-full text-left px-2 py-1 text-sm truncate hover:bg-surface"
                data-testid={`${suggestionTestIdPrefix}-${i}`}
              >
                {s.label}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
