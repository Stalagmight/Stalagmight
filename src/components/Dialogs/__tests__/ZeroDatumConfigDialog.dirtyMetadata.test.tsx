/**
 * The dirty guard (#F9) originally checked only GPS text, the name field and
 * shot edits - three of the eight fields the confirm payload actually sends
 * (surveyors/explorers, date, color, direction and reverse are the other five, all live
 * in `SectionMetadataForm`). A stub that always reports "clean" on those five
 * would pass the main dirty-guard suite (it never touches them) but silently
 * drop a surveyor/date/color/direction/reverse edit on Cancel or Back - this
 * file is isolated from `ZeroDatumConfigDialog.test.tsx` specifically to
 * exercise them, with a minimal `SectionMetadataForm` stub instead of that
 * file's `() => null` one.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () => [],
    getTotalSize: () => count * 33,
    scrollToIndex: () => {},
  }),
}));

vi.mock('react-i18next', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react-i18next')>()),
  useTranslation: () => ({ t: (k: string) => k }),
}));

vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../SectionPreview', () => ({ default: () => null }));

vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ project: null, config: null }),
}));

const mockDialogWrapperSpy = vi.fn();
vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({
    children,
    isDirty,
    ...props
  }: {
    children: React.ReactNode | ((p: { requestClose: () => void }) => React.ReactNode);
    isDirty?: boolean;
    [k: string]: unknown;
  }) => {
    mockDialogWrapperSpy({ isDirty, ...props });
    return <div>{typeof children === 'function' ? children({ requestClose: () => {} }) : children}</div>;
  },
}));

// Minimal stand-in: only the one control each test needs, wired to the real
// callback props - everything else in the real form is irrelevant here.
vi.mock('../../common/SectionMetadataForm', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../common/SectionMetadataForm')>()),
  SectionMetadataForm: ({
    people,
    date,
    color,
    direction,
    reverse,
    onDateChange,
    onColorChange,
    onDirectionChange,
    onReverseChange,
  }: {
    people: import('../../common/SectionMetadataForm').SectionPeopleProps;
    date: string;
    color: string;
    direction?: string;
    reverse?: boolean;
    onDateChange: (v: string) => void;
    onColorChange: (v: string) => void;
    onDirectionChange?: (v: 'in' | 'out') => void;
    onReverseChange?: (v: boolean) => void;
  }) => (
    <div>
      <span data-testid="stub-surveyors">{people.surveyors.map((v) => v.person.name).join('|')}</span>
      <span data-testid="stub-explorers">{people.explorers.map((v) => v.person.name).join('|')}</span>
      <button type="button" data-testid="stub-add-surveyor" onClick={() => people.onAdd('surveyors', { name: 'New Surveyor' })} />
      <button type="button" data-testid="stub-remove-surveyor" onClick={() => people.onRemove('surveyors', { name: 'New Surveyor' })} />
      <button type="button" data-testid="stub-add-explorer" onClick={() => people.onAdd('explorers', { name: 'New Explorer' })} />
      <button type="button" data-testid="stub-pending-on" onClick={() => people.onCommitPending?.(true)} />
      <button type="button" data-testid="stub-pending-off" onClick={() => people.onCommitPending?.(false)} />
      <input data-testid="stub-date" value={date} onChange={(e) => onDateChange(e.target.value)} />
      <input data-testid="stub-color" value={color} onChange={(e) => onColorChange(e.target.value)} />
      <button type="button" data-testid="stub-direction-toggle" onClick={() => onDirectionChange?.(direction === 'in' ? 'out' : 'in')} />
      <input
        type="checkbox"
        data-testid="stub-reverse"
        checked={reverse}
        onChange={(e) => onReverseChange?.(e.target.checked)}
      />
    </div>
  ),
}));

import ZeroDatumConfigDialog from '../ZeroDatumConfigDialog';
import type { DmpFile, DmpSection, DmpShot } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';

function dmpShot(): DmpShot {
  return {
    shot_type: 'Standard',
    heading_in: 10,
    heading_out: 10,
    length: 5,
    depth_in: 0,
    depth_out: 0,
    pitch_in: 0,
    pitch_out: 0,
    left: 0,
    right: 0,
    up: 0,
    down: 0,
    temperature: 0,
    hour: 0,
    minute: 0,
    second: 0,
    marker_index: 0,
  };
}

function dmpFile(): DmpFile {
  const section: DmpSection = {
    version: 1,
    name: 'Anchor',
    direction: 'in',
    date: '2026-01-01',
    shots: Array.from({ length: 2 }, dmpShot),
    content_hash: 'hash-anchor',
  };
  return { version: 1, sections: [section] } as unknown as DmpFile;
}

const selectionConfig = {
  selectedIndices: new Set([0]),
  defaultSurveyors: [{ name: 'S' }],
  defaultExplorers: [],
  defaultDate: '2026-01-01',
} as unknown as DmpSelectionConfig;

function renderDialog() {
  render(
    <ZeroDatumConfigDialog
      dmpFile={dmpFile()}
      selectionConfig={selectionConfig}
      sectionIndex={0}
      filePath="/tmp/anchor.dmp"
      onCancel={vi.fn()}
      onBack={vi.fn()}
      onConfirm={vi.fn()}
    />,
  );
}

function lastIsDirty(): boolean {
  const calls = mockDialogWrapperSpy.mock.calls;
  const last = calls[calls.length - 1]?.[0] as { isDirty?: boolean } | undefined;
  return Boolean(last?.isDirty);
}

describe('ZeroDatumConfigDialog dirty guard - metadata fields', () => {
  beforeAll(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('opens holding the step-1 default people', () => {
    renderDialog();
    expect(screen.getByTestId('stub-surveyors')).toHaveTextContent('S');
    expect(screen.getByTestId('stub-explorers')).toHaveTextContent('');
    expect(lastIsDirty()).toBe(false);
  });

  it('goes dirty when a surveyor alone is added, and clean again when removed', () => {
    renderDialog();
    fireEvent.click(screen.getByTestId('stub-add-surveyor'));
    expect(screen.getByTestId('stub-surveyors')).toHaveTextContent('S|New Surveyor');
    expect(lastIsDirty()).toBe(true);
    fireEvent.click(screen.getByTestId('stub-remove-surveyor'));
    expect(lastIsDirty()).toBe(false);
  });

  it('goes dirty when an explorer alone is added', () => {
    renderDialog();
    fireEvent.click(screen.getByTestId('stub-add-explorer'));
    expect(lastIsDirty()).toBe(true);
  });

  it('goes dirty while a name is typed but not added yet', () => {
    renderDialog();
    fireEvent.click(screen.getByTestId('stub-pending-on'));
    expect(lastIsDirty()).toBe(true);
    fireEvent.click(screen.getByTestId('stub-pending-off'));
    expect(lastIsDirty()).toBe(false);
  });

  it('goes dirty when date alone is changed', () => {
    renderDialog();
    fireEvent.change(screen.getByTestId('stub-date'), { target: { value: '2026-02-02' } });
    expect(lastIsDirty()).toBe(true);
  });

  it('goes dirty when color alone is changed', () => {
    renderDialog();
    fireEvent.change(screen.getByTestId('stub-color'), { target: { value: '#000000' } });
    expect(lastIsDirty()).toBe(true);
  });

  it('goes dirty when direction alone is changed', () => {
    renderDialog();
    fireEvent.click(screen.getByTestId('stub-direction-toggle'));
    expect(lastIsDirty()).toBe(true);
  });

  it('goes dirty when reverse alone is toggled', () => {
    renderDialog();
    fireEvent.click(screen.getByTestId('stub-reverse'));
    expect(lastIsDirty()).toBe(true);
  });
});
