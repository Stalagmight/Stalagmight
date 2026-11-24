/**
 * RS-PROSE fix 6: a pill names a line by its display name. `useLineNameOf`
 * returned the stored name raw, so a line name holding a bidi override or a
 * control character (an import's) reached the pill's context and tooltip
 * unsanitised, where every other place names a line through
 * `lineDisplayName`. Injection: returning the raw `name` again fails both.
 */
import { describe, expect, it, vi } from 'vitest';
import { render, renderHook, screen } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import '../../../i18n';
import { EntityBlob } from '../EntityBlob';
import { useLineNameOf } from '../LinePill';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';

const HOSTILE = 'Ma‮in\u0007';

describe('line names in pills are display names (RS-PROSE fix 6)', () => {
  it('useLineNameOf drops bidi and control characters, and names an unnamed line', () => {
    useProjectStore.setState({
      project: { lines: [{ id: 1, name: HOSTILE, color: '#ff0', shots: [] }, { id: 2, name: '', color: '#00f', shots: [] }] } as never,
      surveyData: null,
    });
    const { result } = renderHook(() => useLineNameOf());
    expect(result.current(1)).toBe('Main');
    expect(result.current(2)).toBe('Unnamed line');
    expect(result.current(3)).toBeNull();
  });

  it("a station pill of another line names it, and its tooltip says it, sanitised", () => {
    useProjectStore.setState({
      project: { lines: [{ id: 1, name: HOSTILE, color: '#ff0', shots: [] }, { id: 2, name: 'Side', color: '#00f', shots: [] }] } as never,
      surveyData: null,
    });
    useUIStore.setState({ syntacticLevel: 'full', resolvedTheme: 'dark' });
    render(<EntityBlob entity="station" lineId={2} homeLineId={1} context="Side" testId="st">12</EntityBlob>);
    expect(screen.getByTestId('st-id')).toHaveTextContent(/^Main:$/);
    expect(screen.getByTestId('st')).toHaveAttribute('title', 'Belongs to Main');
  });
});
