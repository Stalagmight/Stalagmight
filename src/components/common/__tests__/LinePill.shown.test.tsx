/**
 * RS 6: pills show the identity a slot shows. A station pill on a foreign
 * slot (its line knew another station there; the junction shows the kept
 * one) ends with the shown Station ID and takes the colour of the line its
 * shown station belongs to (RS-OWNERSHIP), its leading dot that line's
 * colour (RS-LEADDOT) and, raised after its name, its home dot in the same
 * colour (RS-LEADDOT: a station shown on a line it does not belong to); a
 * loop end takes its kept end's colour. No pill draws a loop mark
 * (RS-NODOTS): a foreign end and a loop end say what they are in a tooltip
 * and in words after the end's bare name (RS-FX5: the words are the
 * plain-text qualifier), and a shot pill is two-tone when an end belongs to
 * another line, its leading dot the shot's line's, the foreign end's home
 * dot on its own half. The plain text below Full says whose station it is,
 * with or without the line.
 * Injections: reading the node id for the superscript fails "ends with the
 * shown Station ID"; dropping the default owner fails "takes the home line's
 * colour"; the qualified label in the pill fails "names the station bare";
 * a `bySlot` pill that drops `homeDotLineId` fails "says whose station it
 * shows".
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import '../../../i18n';
import { DetailLink } from '../DetailLink';
import { EntityBlob, StationBlob } from '../EntityBlob';
import { shotEndMarks } from '../shotEndMarks';
import { identityLabels } from '../../../utils/stationIdentity';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { homeDotOn, pillColorsOf } from '../../../theme/pillColors';
import { surveyDataFromLegacy } from '../../../types/survey-data';
import type { Line, Project, Shot } from '../../../types/project';
import type { Station } from '../../../types/ipc/Station';

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

beforeEach(() => {
  const lines = [line(MAIN, 'Main', '#ffff00', [101, 102]), line(SIDE, 'Side', 'navy', [201, 202])];
  const project = { lines, station_merges: [], zero_datums: [], loop_closures: [] } as unknown as Project;
  const list = [
    station(MAIN, 0, 3, '3'),
    station(MAIN, 1, 11, '12', { merged_with: [{ line_id: SIDE, station_index: 0 }] }),
    station(MAIN, 2, 13, '13'),
    // Side 0 shows the junction's kept station Main / 12 (node 11).
    station(SIDE, 0, 11, '12', {
      merged_with: [{ line_id: MAIN, station_index: 1 }],
      shown: { id: 11, home_line: MAIN, foreign: true, loop_end: false },
    }),
    station(SIDE, 1, 21, '1'),
    // Side 2 (node 22) is a loop end shown as Main / 3.
    station(SIDE, 2, 22, '3', { shown: { id: 3, home_line: MAIN, foreign: true, loop_end: true, loop_closure: 9n } }),
  ];
  useProjectStore.setState({ project, stations: list, surveyData: surveyDataFromLegacy(lines, list) });
  useUIStore.setState({ syntacticLevel: 'full', resolvedTheme: 'dark' });
});

describe('station pills of foreign slots and loop ends (RS 6)', () => {
  it("takes the home line's colour on a foreign slot, its own line's on a native one, its dot too, and the home dot after a foreign name", () => {
    render(
      <>
        <StationBlob station={{ line_id: SIDE, station_index: 0 }} testId="foreign" />
        <StationBlob station={{ line_id: MAIN, station_index: 1 }} testId="native" />
      </>,
    );
    const bg = (id: string) => screen.getByTestId(id).style.getPropertyValue('--pill-bg');
    expect(bg('foreign')).toBe(pillColorsOf('#ffff00', 'dark').bg);
    expect(bg('native')).toBe(pillColorsOf('#ffff00', 'dark').bg);
    expect(screen.getByTestId('foreign')).toHaveAttribute('title', 'Belongs to Main');
    expect(screen.getByTestId('native')).not.toHaveAttribute('title');
    expect(screen.getByTestId('foreign').style.getPropertyValue('--pill-dot')).toBe(pillColorsOf('#ffff00', 'dark').dot);
    expect(screen.getByTestId('foreign').querySelectorAll('.sh-lpill-dot')).toHaveLength(1);
    expect(screen.getByTestId('foreign-station-id')).toHaveTextContent('#11');
    // RS-LEADDOT: the foreign station's home dot, right after its name and
    // before its Station ID, decorative, in its owner's colour (the pill's own).
    const home = screen.getByTestId('foreign-home-dot');
    expect(home).toHaveClass('sh-lpill-home');
    expect(home).toHaveAttribute('aria-hidden', 'true');
    expect(home.textContent).toBe('');
    expect(home.previousElementSibling).toHaveClass('sh-lpill-main');
    expect(home.compareDocumentPosition(screen.getByTestId('foreign-station-id')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByTestId('foreign').style.getPropertyValue('--home-dot')).toBe(pillColorsOf('#ffff00', 'dark').dot);
    expect(screen.queryByTestId('native-home-dot')).toBeNull();
    expect(screen.getByTestId('native').querySelector('.sh-lpill-home')).toBeNull();
  });

  it('with its line, a foreign slot names the line it belongs to and says the slot\'s line in words', () => {
    render(<StationBlob station={{ line_id: SIDE, station_index: 0 }} withLine testId="foreign" />);
    expect(screen.getByTestId('foreign-id')).toHaveTextContent('Main:');
    expect(screen.getByTestId('foreign-owner').textContent).toBe(' (on Side)');
  });

  it('names the station bare in the pill: its words and tooltip say whose', () => {
    render(
      <>
        <StationBlob station={{ line_id: SIDE, station_index: 0 }} testId="foreign" />
        <StationBlob station={{ line_id: SIDE, station_index: 2 }} testId="loop" />
      </>,
    );
    // A screen reader reads "(Main)" (RS-FX6, P2-5), the tooltip says it.
    expect(screen.getByTestId('foreign')).toHaveTextContent(/^12\s*\(Main\)\s*Station ID\s*#11$/);
    expect(screen.getByTestId('loop')).toHaveTextContent(/^3\s*\(Main\)\s*loop end\s*Station ID\s*#3$/);
    expect(screen.getByTestId('foreign')).toHaveAttribute('title', 'Belongs to Main');
    expect(screen.getByTestId('loop').getAttribute('title')).toBe(
      'Belongs to Main\nLoop end: a loop closure shows this station here',
    );
  });

  it('ends a loop end with the shown Station ID and its words, no mark', () => {
    render(<StationBlob station={{ line_id: SIDE, station_index: 2 }} testId="loop" />);
    expect(screen.getByTestId('loop-station-id')).toHaveTextContent(/#3$/);
    expect(screen.getByTestId('loop-station-id')).not.toHaveTextContent('22');
    expect(screen.getByTestId('loop-loop-end')).toHaveTextContent('loop end');
    expect(screen.getByTestId('loop-loop-end')).toHaveClass('sr-only');
    // The kept end is Main's: the loop end takes Main's colour and dot, and
    // shown on Side it has the home dot (RS-LEADDOT); no loop mark.
    expect(screen.getByTestId('loop-home-dot')).toHaveClass('sh-lpill-home');
    expect(screen.getByTestId('loop').style.getPropertyValue('--pill-dot')).toBe(pillColorsOf('#ffff00', 'dark').dot);
    expect(screen.getByTestId('loop').style.getPropertyValue('--pill-bg')).toBe(pillColorsOf('#ffff00', 'dark').bg);
  });

  it('draws no loop mark (RS-NODOTS), only the leading dot (RS-LEADDOT): the tooltip and the words say it', () => {
    // The user: "get rid of the circle and squiggle indicators"; the leading
    // dot came back, the marks after a name did not.
    render(<StationBlob station={{ line_id: SIDE, station_index: 2 }} testId="loop" />);
    const pill = screen.getByTestId('loop');
    expect(pill.querySelector('svg, .sh-lpill-loop, .sh-lpill-mark, [data-glyph]')).toBeNull();
    expect(pill.firstElementChild).toHaveClass('sh-lpill-dot');
    expect(pill.querySelectorAll('.sh-lpill-dot')).toHaveLength(1);
    expect(pill.getAttribute('title')).toContain('Loop end: a loop closure shows this station here');
    expect(screen.getByTestId('loop-loop-end')).toHaveClass('sr-only');
  });

  it("bySlot: a list of slots keeps each slot's line, and still says whose station it shows", () => {
    render(<StationBlob station={{ line_id: SIDE, station_index: 0 }} withLine bySlot testId="slot" />);
    const pill = screen.getByTestId('slot');
    expect(pill.style.getPropertyValue('--pill-bg')).toBe(pillColorsOf('navy', 'dark').bg);
    // Its dot matches its colour: the slot's line.
    expect(pill.style.getPropertyValue('--pill-dot')).toBe(pillColorsOf('navy', 'dark').dot);
    expect(screen.getByTestId('slot-id')).toHaveTextContent('Side:');
    expect(pill).toHaveAttribute('title', 'Belongs to Main');
    expect(screen.getByTestId('slot-owner').textContent).toBe(' (Main)');
    // Its home dot says it too: Main's colour, ringed against Side's fill
    // where it needs it (RS-LEADDOT).
    expect(screen.getByTestId('slot-home-dot')).toHaveClass('sh-lpill-home');
    const on = homeDotOn('#ffff00', pillColorsOf('navy', 'dark'), 'dark');
    expect(pill.style.getPropertyValue('--home-dot')).toBe(on.dot);
    expect(pill.style.getPropertyValue('--home-ring')).toBe(on.ring ?? 'transparent');
    expect(on.dot).toBe('#ffff00');
  });

  it("lets a caller keep the slot's colour", () => {
    render(<StationBlob station={{ line_id: SIDE, station_index: 0 }} homeLineId={null} testId="none" />);
    expect(screen.getByTestId('none').style.getPropertyValue('--pill-bg')).toBe(pillColorsOf('navy', 'dark').bg);
    expect(screen.getByTestId('none')).not.toHaveAttribute('title');
    expect(screen.queryByTestId('none-home-dot')).toBeNull();
  });

  it('says whose station it is in plain text below Full, with or without the line (U-P3-8, RS-FX5)', () => {
    useUIStore.setState({ syntacticLevel: 'off' });
    render(
      <>
        <span data-testid="plain-foreign"><StationBlob station={{ line_id: SIDE, station_index: 0 }} withLine /></span>
        <span data-testid="plain-loop"><StationBlob station={{ line_id: SIDE, station_index: 2 }} withLine /></span>
        <span data-testid="bare-foreign"><StationBlob station={{ line_id: SIDE, station_index: 0 }} /></span>
        <span data-testid="bare-loop"><StationBlob station={{ line_id: SIDE, station_index: 2 }} /></span>
      </>,
    );
    expect(screen.getByTestId('plain-foreign')).toHaveTextContent('Side / 12 (Main)');
    expect(screen.getByTestId('plain-loop')).toHaveTextContent('Main / 3 (loop end on Side)');
    expect(screen.getByTestId('bare-foreign')).toHaveTextContent(/^12 \(Main\)$/);
    expect(screen.getByTestId('bare-loop')).toHaveTextContent(/^3 \(loop end on Side\)$/);
  });

  it("gives a station link the same: shown ID, the owner's colour, loop end words, and a spoken whose", () => {
    render(
      <DetailLink
        type="station"
        target={{ lineId: SIDE, position: 2 }}
        displayText="3"
        fullText="Main / 3 (loop end on Side)"
        onSelect={() => {}}
      />,
    );
    const id = `station-${SIDE}-2-link`;
    expect(screen.getByTestId(`${id}-station-id`)).toHaveTextContent(/#3$/);
    expect(screen.getByTestId(`${id}-loop-end`)).toHaveClass('sr-only');
    // Main's station shown on Side: the home dot, in the pill's (Main's) colour.
    expect(screen.getByTestId(`${id}-home-dot`)).toHaveAttribute('aria-hidden', 'true');
    expect(screen.getByTestId(id).style.getPropertyValue('--home-dot')).toBe(pillColorsOf('#ffff00', 'dark').dot);
    expect(screen.getByTestId(id).querySelector('svg')).toBeNull();
    expect(screen.getByTestId(id).style.getPropertyValue('--pill-bg')).toBe(pillColorsOf('#ffff00', 'dark').bg);
    expect(screen.getByTestId(id).getAttribute('title')).toBe('Belongs to Main\nLoop end: a loop closure shows this station here');
    expect(screen.getByTestId(id)).toHaveAccessibleName('Select station Main / 3 (loop end on Side), Station ID 3');
  });

  it("gives a shot link each foreign end's words and tooltip, with the ends' bare names, the shot's dot leading", () => {
    // Side's first shot runs from the foreign junction slot to Side 1.
    render(
      <DetailLink type="shot" target={{ lineId: SIDE, position: 1 }} displayText="12 (Main) → 1" onSelect={() => {}} />,
    );
    const id = `shot-${SIDE}-1-link`;
    // RS-LEADDOT: the foreign from end has its home dot, on its own half
    // (Main's), right after its name; the native to end has none.
    const home = screen.getByTestId(`${id}-from-home-dot`);
    expect(screen.getByTestId(`${id}-from`)).toContainElement(home);
    expect(home.previousElementSibling).toContainElement(screen.getByTestId(`${id}-from-end`));
    expect(screen.getByTestId(`${id}-from`).style.getPropertyValue('--home-dot')).toBe('#ffff00');
    expect(screen.queryByTestId(`${id}-to-home-dot`)).toBeNull();
    expect(screen.getByTestId(`${id}-from-owner`)).toHaveClass('sr-only');
    expect(screen.getByTestId(`${id}-from-end`)).toHaveAttribute('title', 'Belongs to Main');
    expect(screen.queryByTestId(`${id}-to-owner`)).toBeNull();
    expect(screen.getByTestId(`${id}-to-end`)).not.toHaveAttribute('title');
    expect(screen.queryByTestId(`${id}-from-loop-end`)).toBeNull();
    // Two-tone: the foreign from end's half in Main's colour, after a head in
    // Side's that holds the dot alone (RS-LEADDOT: the shot's dot sits in the
    // shot line's part at the start).
    expect(screen.getByTestId(id)).toHaveClass('sh-lpill--split');
    expect(screen.getByTestId(`${id}-from`).style.getPropertyValue('--pill-bg')).toBe(pillColorsOf('#ffff00', 'dark').bg);
    const head = screen.getByTestId(`${id}-head`);
    expect(head.style.getPropertyValue('--pill-bg')).toBe(pillColorsOf('navy', 'dark').bg);
    expect(head).toContainElement(screen.getByTestId(`${id}-dot`));
    expect(head.style.getPropertyValue('--pill-dot')).toBe(pillColorsOf('navy', 'dark').dot);
    expect(screen.getByTestId(`${id}-to`).style.getPropertyValue('--pill-bg')).toBe(pillColorsOf('navy', 'dark').bg);
    expect(screen.getByTestId(id)).toHaveTextContent(/^12\s*\(Main\)\s*→\s*1$/);
    expect(screen.getByTestId(id)).toHaveAccessibleName('Select shot 12 (Main) → 1');
  });

  it("gives a shot link a loop end's words and tooltip, with the ends' bare names (RS-FX5)", () => {
    // Side's second shot runs from Side 1 to the loop end Side 2.
    render(
      <DetailLink
        type="shot"
        target={{ lineId: SIDE, position: 2 }}
        displayText="1 → 3 (loop end on Side)"
        onSelect={() => {}}
      />,
    );
    const id = `shot-${SIDE}-2-link`;
    expect(screen.getByTestId(`${id}-to-loop-end`)).toHaveClass('sr-only');
    expect(screen.queryByTestId(`${id}-from-loop-end`)).toBeNull();
    // The loop end shows Main's station on Side: its home dot on its half.
    expect(screen.getByTestId(`${id}-to`)).toContainElement(screen.getByTestId(`${id}-to-home-dot`));
    expect(screen.queryByTestId(`${id}-from-home-dot`)).toBeNull();
    expect(screen.getByTestId(`${id}-to-end`).getAttribute('title')).toBe(
      'Belongs to Main\nLoop end: a loop closure shows this station here',
    );
    expect(screen.getByTestId(id).querySelector('svg')).toBeNull();
    expect(screen.getByTestId(id)).toHaveTextContent(/^1\s*→\s*3\s*\(Main\)\s*loop end$/);
  });

  it("gives a shot pill (EntityBlob) the same words through shotEndMarks", () => {
    const labels = identityLabels({
      surveyData: useProjectStore.getState().surveyData,
      stations: useProjectStore.getState().stations,
      lines: useProjectStore.getState().project?.lines,
    });
    const ends = shotEndMarks(labels, SIDE, 1);
    expect(ends).toEqual({
      from: '1',
      to: '3',
      fromHomeLineId: null,
      toHomeLineId: MAIN,
      fromLoopEnd: false,
      toLoopEnd: true,
      fromOwnerLineId: null,
      toOwnerLineId: MAIN,
    });
    // A shot with no dotted end draws no marks.
    expect(shotEndMarks(labels, MAIN, 0)).toBeNull();
    render(
      <EntityBlob entity="shot" lineId={SIDE} shotEnds={ends} testId="shot-pill">
        {labels.shotPillAt(SIDE, 1)}
      </EntityBlob>,
    );
    expect(screen.getByTestId('shot-pill-to-loop-end')).toHaveClass('sr-only');
    expect(screen.getByTestId('shot-pill-to-owner')).toHaveClass('sr-only');
    expect(screen.getByTestId('shot-pill-to')).toContainElement(screen.getByTestId('shot-pill-to-home-dot'));
    expect(screen.queryByTestId('shot-pill-from-home-dot')).toBeNull();
    // The loop end's half takes its kept end's (Main's) colour.
    expect(screen.getByTestId('shot-pill-to').style.getPropertyValue('--pill-bg')).toBe(pillColorsOf('#ffff00', 'dark').bg);
    expect(screen.getByTestId('shot-pill-from').style.getPropertyValue('--pill-bg')).toBe(pillColorsOf('navy', 'dark').bg);
  });
});
