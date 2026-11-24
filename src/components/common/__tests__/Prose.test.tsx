/**
 * RS-PROSE: the prose pill. A sentence worded with prose tokens draws each
 * station, shot and line it names as an inline pill (the line's dot, the
 * muted context, the bare name, the home-line dot, the loop mark, the
 * Station ID), while a screen reader still reads, word for word, the
 * sentence plain text wrote: the drawing is aria-hidden and the plain words
 * sit beside it as sr-only text. Below Full the sentence is plain text.
 * Injections: rendering the token's words visibly fails "reads the plain
 * sentence"; dropping the sr-only words fails it too; ignoring the level
 * fails "below Full".
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import i18n from '../../../i18n';
import { BackendProse, Prose } from '../Prose';
import { accessibleText } from '../../../test/accessibleText';
import {
  hasProse,
  partsToPlain,
  proseParts,
  proseToPlain,
  proseTokens,
  stationPartOf,
} from '../../../utils/prose';
import { identityLabels } from '../../../utils/stationIdentity';
import { prosePillColorsOf } from '../../../theme/pillColors';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { surveyDataFromLegacy } from '../../../types/survey-data';
import type { Line, Project, Shot } from '../../../types/project';
import type { Station } from '../../../types/ipc/Station';
import type { ProsePart } from '../../../types/ipc/ProsePart';

const MAIN = 1;
const SIDE = 2;

function shot(id: number): Shot {
  return {
    id, azimuth_in: 90, azimuth_out: 90, length: 5, depth_in: 1, depth_out: 2,
    pitch_in: null, pitch_out: null, left: null, right: null, up: null, down: null,
    comment: null, kind: 'Normal', marker_index: null, temperature: null,
  } as Shot;
}

function line(id: number, name: string, color: string, shots: number[]): Line {
  return { id, name, color, direction: 'in', quality: 1, next_shot_id: 9, shots: shots.map(shot) } as unknown as Line;
}

function station(line_id: number, station_index: number, id: number, name: string, extra: Partial<Station> = {}): Station {
  return {
    station_ref: { line_id, station_index }, coordinates: { x: station_index, y: 0, z: 0 }, is_zero_datum: false,
    gps_coordinates: null, merged_with: [], id, name, ...extra,
  };
}

function source() {
  const s = useProjectStore.getState();
  return { surveyData: s.surveyData, stations: s.stations, lines: s.project?.lines, closures: [] };
}

beforeEach(() => {
  const lines = [line(MAIN, 'Main', '#ffff00', [101, 102]), line(SIDE, 'Side', 'navy', [201, 202])];
  const project = { lines, station_merges: [], zero_datums: [], loop_closures: [] } as unknown as Project;
  const list = [
    station(MAIN, 0, 3, '3'),
    station(MAIN, 1, 11, '12', { merged_with: [{ line_id: SIDE, station_index: 0 }] }),
    station(MAIN, 2, 13, '13'),
    station(SIDE, 0, 11, '12', {
      merged_with: [{ line_id: MAIN, station_index: 1 }],
      shown: { id: 11, home_line: MAIN, foreign: true, loop_end: false },
    }),
    station(SIDE, 1, 21, '1'),
    station(SIDE, 2, 22, '3', { shown: { id: 3, home_line: MAIN, foreign: true, loop_end: true, loop_closure: 9n } }),
  ];
  useProjectStore.setState({ project, stations: list, surveyData: surveyDataFromLegacy(lines, list) });
  useUIStore.setState({ syntacticLevel: 'full', resolvedTheme: 'dark' });
});

/** The sentence with tokens, and as plain text wrote it. */
function sentence(withPills: boolean) {
  const src = source();
  const names = withPills ? proseTokens(src) : null;
  const labels = identityLabels(src);
  const foreign = { line_id: SIDE, station_index: 0 };
  const loopEnd = { line_id: SIDE, station_index: 2 };
  const station = (ref: typeof foreign, withLine: boolean) =>
    names ? names.station(ref, withLine) : labels.station(ref, withLine);
  return i18n.t('dialogs.mergeStations.moves', {
    count: 1,
    piece: names ? names.line(SIDE) : labels.lineName(SIDE),
    distance: '2.0 m',
    station: `${station(foreign, true)} / ${station(loopEnd, false)} / ${names ? names.shotAt(SIDE, 0, true) : labels.shotAt(SIDE, 0, true)}`,
  });
}

describe('prose pills (RS-PROSE)', () => {
  it('draws every name of the sentence as a pill and reads the plain sentence', () => {
    const plain = sentence(false);
    expect(plain).toContain('Side / 12 (Main)');
    expect(plain).toContain('3 (loop end on Side)');
    render(
      <p data-testid="p">
        <Prose text={sentence(true)} />
      </p>,
    );
    const p = screen.getByTestId('p');
    const pills = p.querySelectorAll('.sh-ppill');
    expect([...pills].map((el) => el.getAttribute('data-prose-kind'))).toEqual(['line', 'station', 'station', 'shot']);
    expect(accessibleText(p)).toBe(plain);
    // The drawing: the foreign slot in the colour of the line it belongs to
    // (RS-OWNERSHIP), named by it, bare name and Station ID, no second dot.
    const foreign = pills[1]! as HTMLElement;
    const drawing = foreign.querySelector('[aria-hidden="true"]')!;
    expect(drawing.textContent).toBe('Main: 12#11');
    expect(foreign.querySelector('[data-prose-mark="home"]')).toBeNull();
    expect(foreign.getAttribute('title')).toBe('Belongs to Main');
    expect(foreign.getAttribute('data-owner-line')).toBe(String(MAIN));
    expect(foreign.style.getPropertyValue('--pill-dot')).toBe(prosePillColorsOf('#ffff00', 'dark').dot);
    // The loop end: its loop mark, the Station ID it shows, its kept end's (Main's) colour.
    const loopEnd = pills[2]! as HTMLElement;
    expect(loopEnd.querySelector('[data-prose-mark="loop"] svg[data-glyph="loop"]')).not.toBeNull();
    expect(loopEnd.querySelector('[data-prose-mark="loop"]')).toHaveAttribute('title', 'Loop end: a loop closure shows this station here');
    expect(loopEnd.querySelector('[aria-hidden="true"]')!.textContent).toBe('3#3');
    expect(loopEnd.style.getPropertyValue('--pill-dot')).toBe(prosePillColorsOf('#ffff00', 'dark').dot);
    // The shot: two tints, the foreign from end's half in Main's after a
    // head in Side's, the end's dot kept; no Station ID.
    const shotPill = pills[3]!;
    expect(shotPill.querySelector('[aria-hidden="true"]')!.textContent).toBe('Side: 12 → 1');
    expect(shotPill.querySelectorAll('[data-prose-mark="home"]')).toHaveLength(1);
    const seg = (name: string) => shotPill.querySelector(`[data-prose-seg="${name}"]`) as HTMLElement;
    expect(seg('head').textContent).toBe('Side: ');
    expect(seg('from').textContent).toBe('12');
    // RS-ARROW: the arrow and its spaces are a segment of their own, whose
    // tint runs from the from half's to the to half's; each half's tint
    // blends toward its neighbour.
    expect(seg('arrow').textContent).toBe(' → ');
    expect(seg('to').textContent).toBe('1');
    const tint = (el: HTMLElement) => el.style.getPropertyValue('--seg-tint');
    for (const name of ['head', 'from', 'arrow', 'to']) expect(tint(seg(name))).toMatch(/^url\("data:image\/svg\+xml,/);
    expect(tint(seg('from'))).not.toBe(tint(seg('to')));
    // A wrap never separates the arrow from either end: the from half, the
    // arrow and the to half are joined (nowrap).
    expect(seg('from').parentElement).toHaveClass('sh-ppill-join');
    expect(seg('arrow').parentElement).toBe(seg('to').parentElement);
    expect(seg('from')).toContainElement(shotPill.querySelector('[data-prose-mark="home"]') as HTMLElement);
    expect(shotPill.querySelector('[data-prose-sid]')).toBeNull();
    // Colours come from the line (custom properties of pillColors.ts).
    expect((foreign as HTMLElement).style.getPropertyValue('--pill-dot')).not.toBe('');
  });

  it('tints a two-tone shot per line once laid out, and a moving layout never loops the update (RS-BLEND)', () => {
    // Every measure sees another box, as while a panel slides open: a hook
    // that measured on every render would never settle (React #185).
    let calls = 0;
    const rects = vi.spyOn(HTMLElement.prototype, 'getClientRects').mockImplementation(function (this: HTMLElement) {
      calls += 1;
      const width = 20 + (calls % 2) * 10;
      const rect = { top: 0, bottom: 16, left: 0, right: width, width, height: 16, x: 0, y: 0, toJSON: () => ({}) };
      return Object.assign([rect], { item: (i: number) => (i === 0 ? rect : null) }) as unknown as DOMRectList;
    });
    try {
      const { rerender } = render(
        <p data-testid="p">
          <Prose text={sentence(true)} />
        </p>,
      );
      rerender(
        <p data-testid="p">
          <Prose text={sentence(true)} />
        </p>,
      );
      const seg = screen.getByTestId('p').querySelector('[data-prose-seg="from"]') as HTMLElement;
      // Laid out: one layer per line, placed over its fragment.
      expect(seg.style.getPropertyValue('--seg-tint-size')).toBe('100.0000% 100%');
    } finally {
      rects.mockRestore();
    }
  });

  it('is the plain sentence below Full', () => {
    useUIStore.setState({ syntacticLevel: 'off' });
    render(
      <p data-testid="p">
        <Prose text={sentence(true)} />
      </p>,
    );
    const p = screen.getByTestId('p');
    expect(p.querySelector('.sh-ppill')).toBeNull();
    expect(p.textContent).toBe(sentence(false));
  });

  it('turns tokens back into plain words for plain-text contexts', () => {
    const text = sentence(true);
    expect(hasProse(text)).toBe(true);
    expect(proseToPlain(text)).toBe(sentence(false));
    expect(proseToPlain('no names')).toBe('no names');
    // A cut token shows no marker.
    const cut = text.slice(0, text.indexOf('\u0002'));
    expect(proseToPlain(cut)).not.toMatch(/[\u0001\u0002]/);
  });

  it('renders a backend text by its parts, or its plain string without', () => {
    const part = stationPartOf(source(), { line_id: SIDE, station_index: 0 }, true);
    const parts: ProsePart[] = [{ kind: 'text', text: 'The depth readings at ' }, part, { kind: 'text', text: ' differ' }];
    const plain = partsToPlain(parts);
    expect(plain).toBe('The depth readings at Side / 12 (Main) differ');
    render(
      <>
        <p data-testid="with">
          <BackendProse text={plain} parts={parts} />
        </p>
        <p data-testid="without">
          <BackendProse text={plain} />
        </p>
      </>,
    );
    expect(screen.getByTestId('with').querySelectorAll('.sh-ppill')).toHaveLength(1);
    expect(accessibleText(screen.getByTestId('with'))).toBe(plain);
    expect(screen.getByTestId('without').textContent).toBe(plain);
    expect(proseParts(plain)).toEqual([{ kind: 'text', text: plain }]);
  });
});
