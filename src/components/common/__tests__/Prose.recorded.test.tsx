/**
 * RS-PROSE fix 2: a pill draws the words it was worded with. A history row's
 * parts are recorded when the edit is; if the owner line is renamed later,
 * the row's pill must keep the recorded name (the words its tooltip and the
 * screen reader read), and only take the line's live colour. Before, the pill
 * drew the live line name for an owned station, so "Old Main: 12" became
 * "New Main: 12" while the row's plain words still said "Old Main".
 * Injection: drawing `pill.lineNameOf(owner) ?? part.context` again fails
 * both tests.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import '../../../i18n';
import { ProsePill } from '../Prose';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { prosePillColorsOf } from '../../../theme/pillColors';
import type { ProsePart } from '../../../types/ipc/ProsePart';

const MAIN = 1;
const SIDE = 2;
/** A foreign slot's part as a history label recorded it, when Main was "Old Main". */
const RECORDED = {
  kind: 'station',
  text: 'Side / 12 (Old Main)',
  line_id: SIDE,
  context: 'Old Main',
  name: '12',
  home_line: MAIN,
  station_id: 11,
} as Exclude<ProsePart, { kind: 'text' }>;

function lines(mainName: string) {
  useProjectStore.setState({
    project: {
      lines: [
        { id: MAIN, name: mainName, color: '#ffff00', shots: [] },
        { id: SIDE, name: 'Side', color: 'navy', shots: [] },
      ],
    } as never,
    stations: [],
    surveyData: null,
  });
}

beforeEach(() => useUIStore.setState({ syntacticLevel: 'full', resolvedTheme: 'dark' }));

describe('a recorded part keeps its words (RS-PROSE fix 2)', () => {
  it('after the owner line is renamed, the pill still names it as recorded, in its live colour', () => {
    lines('New Main');
    render(<ProsePill part={RECORDED} testId="p" />);
    const pill = screen.getByTestId('p');
    const drawing = pill.querySelector('[aria-hidden="true"]')!;
    expect(drawing.textContent).toBe('Old Main: 12#11');
    expect(pill.querySelector('.sr-only')!.textContent).toBe(RECORDED.text);
    expect(pill).toHaveAttribute('title', 'Belongs to Old Main');
    expect(pill.style.getPropertyValue('--pill-dot')).toBe(prosePillColorsOf('#ffff00', 'dark').dot);
  });

  it('a live line with an empty name does not blank the recorded context', () => {
    lines('');
    render(<ProsePill part={RECORDED} testId="p" />);
    expect(screen.getByTestId('p').querySelector('[aria-hidden="true"]')!.textContent).toBe('Old Main: 12#11');
  });
});
