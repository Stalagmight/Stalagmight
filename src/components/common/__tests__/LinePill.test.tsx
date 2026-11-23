/**
 * The line pill (DetailLink and EntityBlob at the Full highlighting level):
 * the colour of its line, a dot instead of the old entity symbol, the muted
 * context first with a colon, the main part, a station's Station ID as a
 * muted superscript, the accessible name, the missing-target variant, the
 * lower levels left as they were, and the home-line mark (prepared, not wired).
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import '../../../i18n';
import { DetailLink } from '../DetailLink';
import { EntityBlob, StationBlob } from '../EntityBlob';
import { PillParts } from '../LinePill';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { missingPillColorsOf, pillColorsOf, pillMarkOf } from '../../../theme/pillColors';
import { surveyDataFromLegacy } from '../../../types/survey-data';
import type { Line, Project, Shot } from '../../../types/project';
import type { Station } from '../../../types/ipc/Station';

const MAIN = 1;
const SIDE = 2;
const MAIN_COLOR = '#ffff00';
const SIDE_COLOR = 'navy';

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

function station(line_id: number, station_index: number, id: number, name: string): Station {
  return {
    station_ref: { line_id, station_index }, coordinates: { x: station_index, y: 0, z: 0 }, is_zero_datum: false,
    gps_coordinates: null, merged_with: [], id, name,
  };
}

beforeEach(() => {
  const lines = [line(MAIN, 'Main Line', MAIN_COLOR, [101, 102]), line(SIDE, 'Side', SIDE_COLOR, [201])];
  const project = { lines, station_merges: [], zero_datums: [], loop_closures: [] } as unknown as Project;
  const list = [
    station(MAIN, 0, 40, 'St0'),
    station(MAIN, 1, 41, 'St1'),
    station(MAIN, 2, 42, 'St2'),
    station(SIDE, 0, 50, 'A'),
    station(SIDE, 1, 51, 'B'),
  ];
  useProjectStore.setState({ project, stations: list, surveyData: surveyDataFromLegacy(lines, list) });
  useUIStore.setState({ syntacticLevel: 'full', resolvedTheme: 'dark' });
});

const cssVar = (el: HTMLElement, name: string) => el.style.getPropertyValue(name);

describe('DetailLink at the Full level', () => {
  it('is a pill in its line colour: dot, muted context with a colon, main part, Station ID superscript', () => {
    render(
      <DetailLink
        type="station"
        target={{ lineId: MAIN, position: 0 }}
        displayText="St0"
        fullText="Main Line / St0"
        pillLabel="St0"
        pillContext="Main Line"
        onSelect={() => {}}
      />,
    );
    const pill = screen.getByTestId(`station-${MAIN}-0-link`);
    expect(pill).toHaveClass('sh-lpill');
    const colors = pillColorsOf(MAIN_COLOR, 'dark');
    expect(cssVar(pill, '--pill-bg')).toBe(colors.bg);
    expect(cssVar(pill, '--pill-border')).toBe(colors.border);
    expect(cssVar(pill, '--pill-dot')).toBe('#ffff00');
    expect(cssVar(pill, '--pill-text')).toBe(colors.text);
    // The dot replaced the entity symbol.
    expect(screen.getByTestId(`station-${MAIN}-0-link-dot`)).toHaveClass('sh-lpill-dot');
    expect(screen.queryByTestId(`station-${MAIN}-0-link-symbol`)).toBeNull();
    expect(pill.querySelector('svg')).toBeNull();
    // Context first, smaller and muted, then the name, then the Station ID.
    const context = screen.getByTestId(`station-${MAIN}-0-link-id`);
    expect(context).toHaveClass('sh-lpill-ctx');
    expect(context).toHaveTextContent(/^Main Line:$/);
    const sid = screen.getByTestId(`station-${MAIN}-0-link-station-id`);
    expect(sid.tagName).toBe('SUP');
    expect(sid).toHaveClass('sh-lpill-sid');
    expect(sid).toHaveTextContent(/^#40$/);
    expect(pill.textContent).toBe('Main Line: St0 #40');
    const order = [...pill.children].map((c) => c.className);
    expect(order).toEqual(['sh-lpill-dot', 'sh-lpill-ctx', 'sh-lpill-main', 'sh-lpill-sid']);
  });

  it('reads naturally: the Station ID is in the accessible name and the announcement', async () => {
    const announce = vi.fn();
    render(
      <DetailLink
        type="station"
        target={{ lineId: MAIN, position: 1 }}
        displayText="St1"
        fullText="Main Line / St1"
        onSelect={() => {}}
        onAnnounce={announce}
      />,
    );
    const pill = screen.getByTestId(`station-${MAIN}-1-link`);
    expect(pill).toHaveAccessibleName('Select station Main Line / St1, Station ID 41');
    await userEvent.click(pill);
    expect(announce).toHaveBeenCalledWith('Selected station Main Line / St1, Station ID 41');
  });

  it('takes the colour of the line in its target, and follows the theme', () => {
    useUIStore.setState({ resolvedTheme: 'light' });
    render(<DetailLink type="line" target={{ lineId: SIDE }} displayText="Side" onSelect={() => {}} />);
    const pill = screen.getByTestId(`line-${SIDE}-link`);
    expect(cssVar(pill, '--pill-bg')).toBe(pillColorsOf(SIDE_COLOR, 'light').bg);
    expect(cssVar(pill, '--pill-dot')).toBe('#000080');
    // A line pill: no context, no Station ID.
    expect(pill.textContent).toBe('Side');
    expect(screen.queryByTestId(`line-${SIDE}-link-station-id`)).toBeNull();
  });

  it('a shot pill has no Station ID', () => {
    render(
      <DetailLink type="shot" target={{ lineId: MAIN, position: 1 }} displayText="St0 → St1" pillContext="Main Line" onSelect={() => {}} />,
    );
    expect(screen.getByTestId(`shot-${MAIN}-1-link`).textContent).toBe('Main Line: St0 → St1');
    expect(screen.queryByTestId(`shot-${MAIN}-1-link-station-id`)).toBeNull();
  });

  it('stationIdShown={false} drops the superscript (the "Also on" rows name a line)', () => {
    render(
      <DetailLink type="station" target={{ lineId: SIDE, position: 0 }} displayText="Side" stationIdShown={false} onSelect={() => {}} />,
    );
    expect(screen.queryByTestId(`station-${SIDE}-0-link-station-id`)).toBeNull();
    expect(screen.getByTestId(`station-${SIDE}-0-link`)).toHaveAccessibleName('Select station Side');
  });

  it('a missing target is the neutral dashed pill with a warning glyph, no dot and no Station ID', () => {
    render(
      <DetailLink type="station" target={{ lineId: MAIN, position: 0 }} displayText="St0" isMissing onSelect={() => {}} />,
    );
    const pill = screen.getByTestId(`station-${MAIN}-0-link`);
    expect(pill).toHaveClass('sh-lpill', 'sh-lpill--missing');
    expect(pill).toHaveAttribute('aria-disabled', 'true');
    const missing = missingPillColorsOf('dark');
    expect(cssVar(pill, '--pill-bg')).toBe(missing.bg);
    expect(cssVar(pill, '--pill-warning')).toBe(missing.warning);
    expect(screen.getByTestId(`station-${MAIN}-0-link-missing`)).toBeInTheDocument();
    expect(screen.queryByTestId(`station-${MAIN}-0-link-dot`)).toBeNull();
    expect(screen.queryByTestId(`station-${MAIN}-0-link-station-id`)).toBeNull();
  });
});

describe('below the Full level nothing changed', () => {
  it.each(['off', 'toolbar_only', 'icons'] as const)('%s: DetailLink is the plain underline link', (level) => {
    useUIStore.setState({ syntacticLevel: level });
    render(
      <DetailLink
        type="station"
        target={{ lineId: MAIN, position: 0 }}
        displayText="St0"
        fullText="Main Line / St0"
        pillContext="Main Line"
        onSelect={() => {}}
      />,
    );
    const link = screen.getByTestId(`station-${MAIN}-0-link`);
    expect(link).toHaveClass('underline');
    expect(link).not.toHaveClass('sh-lpill');
    expect(link.textContent).toBe('St0');
    expect(link).toHaveAccessibleName('Select station Main Line / St0');
  });

  it.each(['off', 'toolbar_only', 'icons'] as const)('%s: StationBlob is its plain label', (level) => {
    useUIStore.setState({ syntacticLevel: level });
    const { container } = render(<StationBlob station={{ line_id: MAIN, station_index: 2 }} withLine />);
    expect(container.textContent).toBe('Main Line / St2');
    expect(container.querySelector('.sh-lpill')).toBeNull();
  });
});

describe('EntityBlob', () => {
  it('a station: line colour, context, name, and the Station ID read with its label', () => {
    render(<StationBlob station={{ line_id: SIDE, station_index: 1 }} withLine testId="blob" />);
    const pill = screen.getByTestId('blob');
    expect(pill).toHaveClass('sh-lpill');
    expect(pill).toHaveAttribute('data-entity', 'station');
    expect(cssVar(pill, '--pill-bg')).toBe(pillColorsOf(SIDE_COLOR, 'dark').bg);
    expect(screen.getByTestId('blob-id')).toHaveTextContent(/^Side:$/);
    // A span pill has no aria-label, so the superscript carries its words.
    expect(screen.getByTestId('blob-station-id')).toHaveTextContent(/^Station ID #51$/);
    expect(pill.textContent).toBe('Side: B Station ID #51');
  });

  it('a shot or a line: no Station ID even when one is passed; trailing content sits inside', () => {
    render(
      <EntityBlob entity="line" lineId={MAIN} stationId={7} trailing={<span data-testid="chev" />} testId="blob">
        Main Line
      </EntityBlob>,
    );
    const pill = screen.getByTestId('blob');
    expect(screen.queryByTestId('blob-station-id')).toBeNull();
    expect(pill).toContainElement(screen.getByTestId('chev'));
    expect(pill.textContent).toBe('Main Line');
  });

  it('a null line is a neutral pill', () => {
    render(<EntityBlob entity="shot" lineId={null} testId="blob">4 → 5</EntityBlob>);
    expect(cssVar(screen.getByTestId('blob'), '--pill-bg')).toBe(pillColorsOf(null, 'dark').bg);
  });
});

describe('the home-line mark (prepared for the archived-station model, not wired yet)', () => {
  it('a station pill shows a second dot in its home line colour right after the name', () => {
    render(
      <DetailLink
        type="station"
        target={{ lineId: MAIN, position: 1 }}
        displayText="St1"
        pillContext="Main Line"
        homeLineId={SIDE}
        onSelect={() => {}}
      />,
    );
    const mark = screen.getByTestId(`station-${MAIN}-1-link-home-dot`);
    const expected = pillMarkOf(SIDE_COLOR, pillColorsOf(MAIN_COLOR, 'dark'), 'dark');
    expect(cssVar(mark, '--mark-dot')).toBe(expected.dot);
    expect(cssVar(mark, '--mark-ring')).toBe(expected.ring ?? 'transparent');
    // RS-FX6, P2-5: the dot says whose station it is in words too, a
    // tooltip and screen-reader text ("St1 (Side)"), as the loop mark does.
    // Injection: render the mark `aria-hidden` with no words again.
    expect(mark).toHaveAttribute('title', 'Belongs to Side');
    expect(mark).not.toHaveAttribute('aria-hidden');
    expect(mark.querySelector('.sr-only')?.textContent).toBe(' (Side)');
    // Right after the name, before the Station ID.
    expect(mark.previousElementSibling).toHaveClass('sh-lpill-main');
    expect(mark.nextElementSibling).toHaveClass('sh-lpill-sid');
  });

  it('a shot pill marks each end that is shown away from its home line, after that end', () => {
    render(
      <DetailLink
        type="shot"
        target={{ lineId: MAIN, position: 2 }}
        displayText="St1 → St2"
        shotHomeLineIds={{ to: SIDE }}
        onSelect={() => {}}
      />,
    );
    const main = screen.getByTestId(`shot-${MAIN}-2-link`).querySelector('.sh-lpill-main')!;
    // The end's dot reads "(Side)" to a screen reader (RS-FX6, P2-5).
    expect(main.textContent).toBe('St1 → St2 (Side)');
    expect(screen.queryByTestId(`shot-${MAIN}-2-link-from-home-dot`)).toBeNull();
    const to = screen.getByTestId(`shot-${MAIN}-2-link-to-home-dot`);
    expect(to.previousSibling?.textContent).toBe('St2');
  });

  it('EntityBlob takes the same marks', () => {
    render(
      <>
        <EntityBlob entity="station" lineId={MAIN} homeLineId={SIDE} testId="st">St1</EntityBlob>
        <EntityBlob entity="shot" lineId={MAIN} shotEnds={{ from: 'St0', to: 'St1', fromHomeLineId: SIDE }} testId="sh">
          St0 → St1
        </EntityBlob>
      </>,
    );
    expect(screen.getByTestId('st-home-dot')).toHaveAttribute('title', 'Belongs to Side');
    expect(screen.getByTestId('sh-from-home-dot')).toHaveAttribute('title', 'Belongs to Side');
    expect(screen.getByTestId('sh-from-home-dot').previousSibling?.textContent).toBe('St0');
    expect(screen.queryByTestId('sh-to-home-dot')).toBeNull();
  });

  it('no call site passes one yet: without the prop there is no mark', () => {
    const { container } = render(
      <PillParts testId="p" context="Main Line" main="St0" stationId={40} />,
    );
    expect(container.querySelector('.sh-lpill-mark')).toBeNull();
  });
});

describe('a home-line mark without a line name (RS-FX6)', () => {
  it('stays a silent dot when the line is unknown', () => {
    render(<PillParts testId="p" main="St0" mark={{ dot: '#f00', ring: null }} />);
    const mark = screen.getByTestId('p-home-dot');
    expect(mark).toHaveAttribute('aria-hidden', 'true');
    expect(mark).not.toHaveAttribute('title');
  });
});
