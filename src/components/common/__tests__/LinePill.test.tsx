/**
 * The line pill (DetailLink and EntityBlob at the Full highlighting level):
 * the colour of its line in its leading dot (RS-LEADDOT, decorative), fill
 * and border (no marks after a name since RS-NODOTS, no entity symbol), the
 * muted context first with a colon, the main part, a station's Station ID as a
 * muted superscript, the accessible name, the missing-target variant, the
 * lower levels left as they were, and ownership (RS-OWNERSHIP): a station of
 * another line in that line's colour, a two-tone shot, the context giving way
 * first in a narrow place.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
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
import { missingPillColorsOf, pillColorsOf } from '../../../theme/pillColors';
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
    expect(cssVar(pill, '--pill-text')).toBe(colors.text);
    // The leading dot is the line colour itself (RS-LEADDOT); yellow stands
    // out from its dark fill alone, so it has no ring. It is decorative.
    expect(cssVar(pill, '--pill-dot')).toBe('#ffff00');
    expect(cssVar(pill, '--pill-ring')).toBe('transparent');
    const dot = screen.getByTestId(`station-${MAIN}-0-link-dot`);
    expect(dot).toHaveClass('sh-lpill-dot');
    expect(dot).toHaveAttribute('aria-hidden', 'true');
    expect(dot.textContent).toBe('');
    // No entity symbol, no marks after the name (RS-NODOTS).
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
    expect(cssVar(pill, '--pill-border')).toBe(pillColorsOf(SIDE_COLOR, 'light').border);
    // Navy on the light theme: the border and the dot are the line colour itself.
    expect(cssVar(pill, '--pill-border')).toBe('#000080');
    expect(cssVar(pill, '--pill-dot')).toBe('#000080');
    expect(screen.getByTestId(`line-${SIDE}-link-dot`)).toBeInTheDocument();
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

describe('ownership (RS-OWNERSHIP): a station of another line takes its colour, a shot is two-tone', () => {
  it("a station pill takes its home line's colour, dot and name, no second dot, and says whose it is", () => {
    render(
      <DetailLink
        type="station"
        target={{ lineId: MAIN, position: 1 }}
        displayText="St1 (Side)"
        fullText="Main Line / St1 (Side)"
        pillLabel="St1"
        pillContext="Main Line"
        homeLineId={SIDE}
        onSelect={() => {}}
      />,
    );
    const pill = screen.getByTestId(`station-${MAIN}-1-link`);
    // Injection: colouring by the slot's line fails these two.
    expect(cssVar(pill, '--pill-bg')).toBe(pillColorsOf(SIDE_COLOR, 'dark').bg);
    expect(cssVar(pill, '--pill-border')).toBe(pillColorsOf(SIDE_COLOR, 'dark').border);
    // Its one dot is the owner's colour, matching the pill (RS-LEADDOT).
    expect(cssVar(pill, '--pill-dot')).toBe(pillColorsOf(SIDE_COLOR, 'dark').dot);
    expect(pill.querySelectorAll('.sh-lpill-dot')).toHaveLength(1);
    // The context names the line the pill is coloured by, never the slot's.
    expect(screen.getByTestId(`station-${MAIN}-1-link-id`)).toHaveTextContent('Side:');
    expect(pill.querySelector('.sh-lpill-mark')).toBeNull();
    expect(pill).toHaveAttribute('title', 'Belongs to Side');
    // The button's accessible name keeps the plain words.
    expect(pill).toHaveAccessibleName('Select station Main Line / St1 (Side), Station ID 41');
  });

  it("a shot with one end of another line is split at the arrow, that half in its colour, the shot's dot leading", () => {
    render(
      <DetailLink
        type="shot"
        target={{ lineId: MAIN, position: 2 }}
        displayText="St1 → St2"
        shotHomeLineIds={{ to: SIDE }}
        onSelect={() => {}}
      />,
    );
    const pill = screen.getByTestId(`shot-${MAIN}-2-link`);
    expect(pill).toHaveClass('sh-lpill--split');
    expect(pill.getAttribute('style') ?? '').not.toContain('--pill-bg');
    const from = screen.getByTestId(`shot-${MAIN}-2-link-from`);
    const to = screen.getByTestId(`shot-${MAIN}-2-link-to`);
    expect(screen.queryByTestId(`shot-${MAIN}-2-link-head`)).toBeNull();
    expect(cssVar(from, '--pill-bg')).toBe(pillColorsOf(MAIN_COLOR, 'dark').bg);
    expect(cssVar(to, '--pill-bg')).toBe(pillColorsOf(SIDE_COLOR, 'dark').bg);
    // The leading dot and the from end in the shot's half, the dot in the
    // shot's colour (RS-LEADDOT); the arrow is a segment of its own whose
    // fill runs from that half's colour to the other's (RS-ARROW), so the
    // blend is centred on it.
    const dot = screen.getByTestId(`shot-${MAIN}-2-link-dot`);
    expect(from).toContainElement(dot);
    expect(from.firstElementChild).toBe(dot);
    expect(cssVar(from, '--pill-dot')).toBe(pillColorsOf(MAIN_COLOR, 'dark').dot);
    expect(screen.getByTestId(`shot-${MAIN}-2-link-from-end`).textContent).toBe('St1');
    // The to end's name, then its words for a screen reader.
    expect(screen.getByTestId(`shot-${MAIN}-2-link-to-end`).firstChild?.textContent).toBe('St2');
    const arrow = screen.getByTestId(`shot-${MAIN}-2-link-arrow`);
    expect(arrow.textContent).toBe('\u2192');
    const fill = cssVar(arrow, '--seg-fill');
    expect(fill.startsWith(`linear-gradient(to right, ${pillColorsOf(MAIN_COLOR, 'dark').bg} 0.00%`)).toBe(true);
    expect(fill.endsWith(`${pillColorsOf(SIDE_COLOR, 'dark').bg} 100.00%)`)).toBe(true);
    // The halves keep their own colour up to the arrow.
    expect(cssVar(from, '--seg-fill')).toBe(
      `linear-gradient(to right, ${pillColorsOf(MAIN_COLOR, 'dark').bg} 0.00%, ${pillColorsOf(MAIN_COLOR, 'dark').bg} 100.00%)`,
    );
    // The foreign end has no dot of its own (RS-NODOTS): its half's colour
    // shows it, its tooltip and its words say whose station it is. The
    // pill's one dot is the leading one.
    expect(screen.queryByTestId(`shot-${MAIN}-2-link-to-home-dot`)).toBeNull();
    expect(pill.querySelector('.sh-lpill-mark')).toBeNull();
    expect(pill.querySelectorAll('.sh-lpill-dot')).toHaveLength(1);
    const end = screen.getByTestId(`shot-${MAIN}-2-link-to-end`);
    expect(to).toContainElement(end);
    expect(end).toHaveAttribute('title', 'Belongs to Side');
    expect(screen.getByTestId(`shot-${MAIN}-2-link-to-owner`)).toHaveClass('sr-only');
    expect(screen.getByTestId(`shot-${MAIN}-2-link-from-end`)).not.toHaveAttribute('title');
    // The words are the plain shot's, the end's qualifier included.
    expect(pill.textContent).toBe('St1 \u2192 St2 (Side)');
  });

  it('a from end of another line gets its own half; the dot and the context keep the shot colour', () => {
    render(
      <EntityBlob
        entity="shot"
        lineId={MAIN}
        context="Main Line"
        shotEnds={{ from: 'St0', to: 'St1', fromHomeLineId: SIDE, fromOwnerLineId: SIDE }}
        testId="sh"
      >
        St0 → St1
      </EntityBlob>,
    );
    const head = screen.getByTestId('sh-head');
    expect(cssVar(head, '--pill-bg')).toBe(pillColorsOf(MAIN_COLOR, 'dark').bg);
    expect(head).toContainElement(screen.getByTestId('sh-dot'));
    expect(cssVar(head, '--pill-dot')).toBe(pillColorsOf(MAIN_COLOR, 'dark').dot);
    expect(head).toHaveTextContent(/^Main Line:$/);
    expect(cssVar(screen.getByTestId('sh-from'), '--pill-bg')).toBe(pillColorsOf(SIDE_COLOR, 'dark').bg);
    expect(cssVar(screen.getByTestId('sh-to'), '--pill-bg')).toBe(pillColorsOf(MAIN_COLOR, 'dark').bg);
    expect(screen.queryByTestId('sh-from-home-dot')).toBeNull();
    expect(screen.getByTestId('sh-from')).toContainElement(screen.getByTestId('sh-from-end'));
    expect(screen.getByTestId('sh-from-end')).toHaveAttribute('title', 'Belongs to Side');
  });

  it("a from end of another line with no context: a head of the shot's colour holds the dot alone (RS-LEADDOT)", () => {
    render(
      <EntityBlob entity="shot" lineId={MAIN} shotEnds={{ from: 'St0', to: 'St1', fromHomeLineId: SIDE, fromOwnerLineId: SIDE }} testId="sh">
        St0 → St1
      </EntityBlob>,
    );
    const head = screen.getByTestId('sh-head');
    expect(cssVar(head, '--pill-bg')).toBe(pillColorsOf(MAIN_COLOR, 'dark').bg);
    expect(head).toContainElement(screen.getByTestId('sh-dot'));
    expect(head.textContent).toBe('');
    expect(cssVar(screen.getByTestId('sh-from'), '--pill-bg')).toBe(pillColorsOf(SIDE_COLOR, 'dark').bg);
    expect(screen.getByTestId('sh').textContent).toBe('St0 (Side) \u2192 St1');
  });

  it('both ends of other lines: each half its own end\'s colour', () => {
    useProjectStore.setState((st) => ({
      project: { ...st.project!, lines: [...st.project!.lines, line(3, 'Third', '#00aa00', [])] },
    }));
    render(
      <EntityBlob
        entity="shot"
        lineId={MAIN}
        shotEnds={{ from: 'St0', to: 'St1', fromHomeLineId: SIDE, fromOwnerLineId: SIDE, toHomeLineId: 3, toOwnerLineId: 3 }}
        testId="sh"
      >
        St0 → St1
      </EntityBlob>,
    );
    expect(cssVar(screen.getByTestId('sh-from'), '--pill-bg')).toBe(pillColorsOf(SIDE_COLOR, 'dark').bg);
    expect(cssVar(screen.getByTestId('sh-to'), '--pill-bg')).toBe(pillColorsOf('#00aa00', 'dark').bg);
  });

  it('a shot whose ends are native stays one colour', () => {
    render(
      <EntityBlob entity="shot" lineId={MAIN} shotEnds={{ from: 'St0', to: 'St1', fromLoopEnd: true }} testId="sh">
        St0 → St1
      </EntityBlob>,
    );
    const pill = screen.getByTestId('sh');
    expect(pill).not.toHaveClass('sh-lpill--split');
    expect(cssVar(pill, '--pill-bg')).toBe(pillColorsOf(MAIN_COLOR, 'dark').bg);
    // The loop end says so in words and in its tooltip, with no mark (RS-NODOTS).
    expect(screen.getByTestId('sh-from-loop-end')).toHaveClass('sr-only');
    expect(screen.getByTestId('sh-from-end').getAttribute('title')).toMatch(/^Loop end/);
    expect(pill.querySelector('.sh-lpill-loop, svg')).toBeNull();
  });

  it('EntityBlob: a station of another line names it, says it in words and as a tooltip', () => {
    render(
      <>
        <EntityBlob entity="station" lineId={MAIN} homeLineId={SIDE} testId="bare">St1</EntityBlob>
        <EntityBlob entity="station" lineId={MAIN} homeLineId={SIDE} context="Main Line" testId="ctx">St1</EntityBlob>
      </>,
    );
    const bare = screen.getByTestId('bare');
    expect(cssVar(bare, '--pill-bg')).toBe(pillColorsOf(SIDE_COLOR, 'dark').bg);
    expect(bare).toHaveAttribute('title', 'Belongs to Side');
    expect(bare.querySelector('.sh-lpill-mark')).toBeNull();
    // Without a context, the words say whose it is: "St1 (Side)".
    expect(screen.getByTestId('bare-owner').textContent).toBe(' (Side)');
    // With one, the context names the owner and the words the slot's line.
    expect(screen.getByTestId('ctx-id')).toHaveTextContent('Side:');
    expect(screen.getByTestId('ctx-owner').textContent).toBe(' (on Main Line)');
  });

  it('without an owner there is no owner colour, words or tooltip', () => {
    const { container } = render(
      <PillParts testId="p" context="Main Line" main="St0" stationId={40} />,
    );
    expect(container.querySelector('.sh-lpill-mark')).toBeNull();
    expect(screen.queryByTestId('p-owner')).toBeNull();
  });
});

describe('the narrow-panel truncation (RS-OWNERSHIP)', () => {
  it('the context shrinks first: it may ellipsize, the name and Station ID are never squeezed out', () => {
    // jsdom does no layout: this pins the CSS contract the fix rests on.
    const css = readFileSync(resolve(__dirname, '../../../index.css'), 'utf8');
    const rule = (sel: string) => new RegExp(`\\n${sel.replace('.', '\\.')} \\{([^}]*)\\}`).exec(css)?.[1] ?? '';
    expect(rule('.sh-lpill-ctx')).toMatch(/flex: 0 1000 auto/);
    expect(rule('.sh-lpill-ctx')).toMatch(/text-overflow: ellipsis/);
    expect(rule('.sh-lpill-main')).toMatch(/flex: 0 1 auto/);
    expect(rule('.sh-lpill-sid')).toMatch(/flex: none/);
  });
});
