/**
 * [#576 NF-W6-2] The survey context menu, the panel error fallback and the entity
 * link render their text from en.json, not from English typed into the component.
 *
 * Two checks per string. (1) The key exists and renders the exact English the
 * component used to hard-code (snapshotted below), so the move changed no wording.
 * (2) With the key's en.json value swapped for a marker, the component shows the
 * marker: a string re-hard-coded in the component, or read from the wrong key,
 * fails here even though the English would still look right. The context menu is
 * also checked to pick its icons and its read-only exemption from the key, never
 * from the rendered label, so another language keeps both.
 *
 * Real i18n (not a key-echo mock). `.tsx` so vitest runs it under jsdom.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/react';

import i18n from '../../../i18n';
import ContextMenu from '../ContextMenu';
import { ErrorBoundary } from '../ErrorBoundary';
import { DetailLink, generateAriaLabel } from '../DetailLink';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import type { Project } from '../../../types/ipc/Project';
import type { Station } from '../../../types/ipc/Station';

/** Every moved string: [key, params, the English the component hard-coded before the move]. */
const OLD_ENGLISH: Array<[string, Record<string, unknown>, string]> = [
  ['contextMenu.editStation', {}, 'Edit Station'],
  ['contextMenu.mergeWith', {}, 'Merge With...'],
  ['contextMenu.unmergeStations', {}, 'Unmerge Stations'],
  ['contextMenu.removeClosure', {}, 'Remove Closure'],
  ['contextMenu.addShotBefore', {}, 'Add Shot Before'],
  ['contextMenu.addShotAfter', {}, 'Add Shot After'],
  ['contextMenu.setZeroDatum', {}, 'Set Zero Datum'],
  ['contextMenu.editSection', {}, 'Edit Section'],
  ['contextMenu.reverseSection', {}, 'Reverse Section'],
  ['contextMenu.reverseSections', { count: 2 }, 'Reverse 2 Sections'],
  ['contextMenu.reverseSections', { count: 12 }, 'Reverse 12 Sections'],
  ['contextMenu.deleteSection', {}, 'Delete Section'],
  ['contextMenu.deleteStation', {}, 'Delete Station'],
  ['contextMenu.editShot', {}, 'Edit Shot'],
  ['contextMenu.insertStation', {}, 'Insert Station...'],
  ['contextMenu.markAsJump', {}, 'Mark as Jump'],
  ['contextMenu.markAsReference', {}, 'Mark as Reference'],
  ['contextMenu.unmarkJump', {}, 'Unmark Jump'],
  ['contextMenu.unmarkReference', {}, 'Unmark Reference'],
  ['contextMenu.deleteShot', {}, 'Delete Shot'],
  ['contextMenu.adjustPosition', {}, 'Adjust Position...'],
  ['contextMenu.zoomToSection', {}, 'Zoom to Section'],
  ['contextMenu.sectionFallback', { id: 7 }, 'Section 7'],
  ['errorBoundary.panelError', { panel: 'Data Table' }, 'Data Table encountered an error'],
  ['errorBoundary.tryAgain', {}, 'Try Again'],
  ...(['station', 'shot', 'section'] as const).flatMap((type): Array<[string, Record<string, unknown>, string]> => [
    [`detailLink.${type}.notFound`, {}, `${type} not found`],
    [`detailLink.${type}.select`, {}, `Select ${type}`],
    [`detailLink.${type}.selectNamed`, { name: '2:5' }, `Select ${type} 2:5`],
    [`detailLink.${type}.selected`, { name: '2:5' }, `Selected ${type} 2:5`],
    [`detailLink.${type}.selected`, { name: '' }, `Selected ${type} `],
  ]),
];

// --- markers ----------------------------------------------------------------------

const saved = new Map<string, unknown>();

/** The marker `mark` puts in place of `key`'s value: the key plus the value's placeholders. */
function markerOf(key: string, original: unknown): string {
  const placeholders = typeof original === 'string' ? original.match(/\{\{\w+\}\}/g) ?? [] : [];
  return `@@${key}${placeholders.map((p) => `|${p}`).join('')}@@`;
}

/** Swap each key's en.json value for its marker until the test ends. */
function mark(...keys: string[]): void {
  for (const key of keys) {
    const original: unknown = i18n.getResource('en', 'translation', key);
    if (!saved.has(key)) saved.set(key, original);
    i18n.addResource('en', 'translation', key, markerOf(key, original));
  }
}

afterEach(() => {
  for (const [key, value] of saved) i18n.addResource('en', 'translation', key, value as string);
  saved.clear();
});

describe('moved strings keep their English (#576 NF-W6-2)', () => {
  it.each(OLD_ENGLISH)('%s %j renders the old English', (key, params, english) => {
    const resolved = key === 'contextMenu.reverseSections' ? `${key}_other` : key;
    expect(i18n.exists(resolved)).toBe(true);
    expect(i18n.t(key, params)).toBe(english);
  });
});

// --- ContextMenu ------------------------------------------------------------------

const shot = (id: number, kind: string) => ({
  id,
  kind,
  length: 10,
  azimuth_in: 0,
  azimuth_out: null,
  depth_in: 1,
  depth_out: 1,
  pitch_in: null,
  pitch_out: null,
});

const project = {
  sections: [
    { id: 1, name: 'Main', shots: [shot(11, 'Normal')] },
    { id: 2, name: 'Side', shots: [shot(21, 'Jump'), shot(22, 'Reference'), shot(23, 'Normal')] },
  ],
  station_merges: [],
} as unknown as Project;

const ref = (section_id: number, station_index: number) => ({ section_id, station_index });

const stations = [
  {
    station_ref: ref(1, 0),
    coordinates: { x: 0, y: 0, z: 0 },
    is_zero_datum: false,
    gps_coordinates: null,
    merged_with: [ref(2, 0)],
    closures_with: [{ closure_id: 3, station_ref: ref(2, 1), via_station: null, is_transitive: false }],
  },
] as unknown as Station[];

/** Each target the survey menu serves, with the keys of the entries it lists, in order. */
const SCENARIOS: Array<{ name: string; target: unknown; multi?: unknown[]; keys: string[] }> = [
  {
    name: 'first station (merged, with a closure)',
    target: { type: 'station', ref: ref(1, 0) },
    keys: [
      'contextMenu.editStation',
      'contextMenu.mergeWith',
      'contextMenu.unmergeStations',
      'contextMenu.removeClosure',
      'contextMenu.addShotBefore',
      'contextMenu.setZeroDatum',
      'contextMenu.editSection',
      'contextMenu.reverseSection',
      'contextMenu.deleteSection',
      'contextMenu.deleteStation',
    ],
  },
  {
    name: 'last station',
    target: { type: 'station', ref: ref(1, 1) },
    keys: [
      'contextMenu.editStation',
      'contextMenu.mergeWith',
      'contextMenu.addShotAfter',
      'contextMenu.setZeroDatum',
      'contextMenu.editSection',
      'contextMenu.reverseSection',
      'contextMenu.deleteSection',
      'contextMenu.deleteStation',
    ],
  },
  {
    name: 'permanent shot',
    target: { type: 'shot', sectionId: 1, shotId: 11 },
    keys: [
      'contextMenu.editShot',
      'contextMenu.insertStation',
      'contextMenu.markAsJump',
      'contextMenu.markAsReference',
      'contextMenu.addShotBefore',
      'contextMenu.addShotAfter',
      'contextMenu.editSection',
      'contextMenu.reverseSection',
      'contextMenu.deleteSection',
      'contextMenu.deleteShot',
    ],
  },
  {
    name: 'jump shot',
    target: { type: 'shot', sectionId: 2, shotId: 21 },
    keys: [
      'contextMenu.editShot',
      'contextMenu.insertStation',
      'contextMenu.unmarkJump',
      'contextMenu.markAsReference',
      'contextMenu.addShotBefore',
      'contextMenu.editSection',
      'contextMenu.reverseSection',
      'contextMenu.deleteSection',
      'contextMenu.deleteShot',
    ],
  },
  {
    name: 'reference shot',
    target: { type: 'shot', sectionId: 2, shotId: 22 },
    keys: [
      'contextMenu.editShot',
      'contextMenu.insertStation',
      'contextMenu.unmarkReference',
      'contextMenu.markAsJump',
      'contextMenu.editSection',
      'contextMenu.reverseSection',
      'contextMenu.deleteSection',
      'contextMenu.deleteShot',
    ],
  },
  {
    name: 'section, two sections selected',
    target: { type: 'section', sectionId: 1 },
    multi: [
      { type: 'section', sectionId: 1 },
      { type: 'section', sectionId: 2 },
    ],
    keys: [
      'contextMenu.editSection',
      'contextMenu.reverseSection',
      'contextMenu.reverseSections_other',
      'contextMenu.adjustPosition',
      'contextMenu.zoomToSection',
      'contextMenu.deleteSection',
    ],
  },
  {
    name: 'data-table shot',
    target: { type: 'datatable-shot', sectionId: 1, shotId: 11, isFirst: true, isLast: true, totalShots: 1, sectionName: 'Main' },
    keys: ['contextMenu.addShotBefore', 'contextMenu.addShotAfter', 'contextMenu.deleteShot'],
  },
];

const englishOf = (key: string): string =>
  key === 'contextMenu.reverseSections_other' ? i18n.t('contextMenu.reverseSections', { count: 2 }) : i18n.t(key);

/** Render the menu for `target` and read each entry's label, icon markup and disabled state. */
function openMenu(target: unknown, multi: unknown[] = []) {
  useUIStore.setState({
    dialogData: { target, position: { x: 10, y: 10 }, survivesScroll: true },
    multiSelection: multi as never,
  });
  const { unmount } = render(<ContextMenu />);
  const rows = within(screen.getByTestId('context-menu')).getAllByRole('button');
  const read = rows.map((row) => ({
    // The label span only: the shot-kind icons carry a letter badge (J / R) as text.
    label: row.querySelector('span.flex-1')?.textContent ?? null,
    icon: row.querySelector('span[aria-hidden="true"]')?.innerHTML ?? null,
    disabled: (row as HTMLButtonElement).disabled,
  }));
  unmount();
  return read;
}

describe('ContextMenu labels come from en.json (#576 NF-W6-2)', () => {
  beforeEach(() => {
    useUIStore.setState({ appMode: 'survey', geometryMode: 'measured', syntacticLevel: 'full' });
    useProjectStore.setState({ project, stations });
  });
  afterEach(() => {
    useUIStore.setState({ dialogData: {}, multiSelection: [], geometryMode: 'measured' });
    useProjectStore.setState({ project: null, stations: [] });
  });

  it.each(SCENARIOS)('$name: English labels, then the marker of each key', ({ target, multi, keys }) => {
    const english = openMenu(target, multi);
    expect(english.map((r) => r.label)).toEqual(keys.map(englishOf));

    mark(...keys);
    const marked = openMenu(target, multi);
    expect(marked.map((r) => r.label)).toEqual(
      keys.map((k) => markerOf(k, saved.get(k)).replace('{{count}}', '2')),
    );
    // The icons are picked from the key, so the marker text leaves every icon as it was.
    expect(marked.map((r) => r.icon)).toEqual(english.map((r) => r.icon));
    expect(english.every((r) => r.icon !== null && r.icon !== '')).toBe(true);
  });

  it('read-only view: only Zoom to Section stays enabled, whatever the label text', () => {
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    const { target, multi, keys } = SCENARIOS.find((s) => s.name.startsWith('section'))!;
    mark(...keys);
    const rows = openMenu(target, multi);
    const enabled = rows.filter((r) => !r.disabled).map((r) => r.label);
    expect(enabled).toEqual([markerOf('contextMenu.zoomToSection', saved.get('contextMenu.zoomToSection'))]);
  });
});

// --- ErrorBoundary ----------------------------------------------------------------

function Boom(): never {
  throw new Error('kaboom');
}

describe('ErrorBoundary fallback text comes from en.json (#576 NF-W6-2)', () => {
  beforeEach(() => {
    // React reports the caught render error on console.error; the test expects it.
    vi.spyOn(console, 'error').mockImplementation(() => {});
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('renders the English, then the markers', () => {
    const first = render(
      <ErrorBoundary panelName="Map">
        <Boom />
      </ErrorBoundary>,
    );
    const fallback = screen.getByTestId('error-boundary-fallback');
    expect(fallback).toHaveTextContent('Map encountered an error');
    expect(screen.getByTestId('error-boundary-reset')).toHaveTextContent(/^Try Again$/);
    first.unmount();

    mark('errorBoundary.panelError', 'errorBoundary.tryAgain');
    render(
      <ErrorBoundary panelName="Map">
        <Boom />
      </ErrorBoundary>,
    );
    expect(screen.getByTestId('error-boundary-fallback')).toHaveTextContent('@@errorBoundary.panelError|Map@@');
    expect(screen.getByTestId('error-boundary-reset')).toHaveTextContent(/^@@errorBoundary\.tryAgain@@$/);
  });
});

// --- DetailLink -------------------------------------------------------------------

describe('DetailLink text comes from en.json (#576 NF-W6-2)', () => {
  const target = { sectionId: 2, position: 5 };

  it.each(['station', 'shot', 'section'] as const)('%s: not-found title, aria-label and announcement', (type) => {
    const announce = vi.fn();
    const missing = render(
      <DetailLink type={type} target={target} displayText="2:5" onSelect={() => {}} isMissing testId="link" />,
    );
    const link = screen.getByTestId('link');
    expect(link).toHaveAttribute('title', `${type} not found`);
    expect(link).toHaveAttribute('aria-label', `Select ${type} 2:5`);
    expect(link.querySelector('span[title]')).toHaveAttribute('title', `${type} not found`);
    missing.unmount();
    const present = render(
      <DetailLink type={type} target={target} displayText="2:5" onSelect={() => {}} onAnnounce={announce} testId="link" />,
    );
    fireEvent.click(screen.getByTestId('link'));
    expect(announce).toHaveBeenLastCalledWith(`Selected ${type} 2:5`);
    present.unmount();
    expect(generateAriaLabel(type, '  ')).toBe(`Select ${type}`);

    const k = (name: string) => `detailLink.${type}.${name}`;
    mark(k('notFound'), k('select'), k('selectNamed'), k('selected'));
    render(<DetailLink type={type} target={target} displayText="2:5" onSelect={() => {}} isMissing testId="marked" />);
    const marked = screen.getByTestId('marked');
    expect(marked).toHaveAttribute('title', `@@${k('notFound')}@@`);
    expect(marked).toHaveAttribute('aria-label', `@@${k('selectNamed')}|2:5@@`);
    expect(marked.querySelector('span[title]')).toHaveAttribute('title', `@@${k('notFound')}@@`);
    render(
      <DetailLink type={type} target={target} displayText="2:5" onSelect={() => {}} onAnnounce={announce} testId="marked-ok" />,
    );
    fireEvent.click(screen.getByTestId('marked-ok'));
    expect(announce).toHaveBeenLastCalledWith(`@@${k('selected')}|2:5@@`);
    expect(generateAriaLabel(type, '')).toBe(`@@${k('select')}@@`);
  });
});
