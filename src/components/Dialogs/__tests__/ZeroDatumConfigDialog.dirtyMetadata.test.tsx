/**
 * The dirty guard (#F9) originally checked only GPS text, the name field and
 * shot edits - three of the eight fields the confirm payload actually sends
 * (surveyor, date, color, direction and reverse are the other five, all live
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

vi.mock('@tanstack/react-virtual', async () =>
  (await import('../../../test/factories/virtualizer')).fakeVirtualizer({ renderRows: false }),
);

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys());

vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../SectionPreview', () => ({ default: () => null }));

vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ project: null, config: null }),
}));

const mockDialogWrapperSpy = vi.fn();
vi.mock('../DialogWrapper', async () =>
  (await import('../../../test/factories/components')).dialogWrapperStub({
    spy: (props) => mockDialogWrapperSpy(props),
  }),
);

// Minimal stand-in: only the one control each test needs, wired to the real
// callback props - everything else in the real form is irrelevant here.
vi.mock('../../common/SectionMetadataForm', () => ({
  SectionMetadataForm: ({
    surveyor,
    date,
    color,
    direction,
    reverse,
    onSurveyorChange,
    onDateChange,
    onColorChange,
    onDirectionChange,
    onReverseChange,
  }: {
    surveyor: string;
    date: string;
    color: string;
    direction?: string;
    reverse?: boolean;
    onSurveyorChange: (v: string) => void;
    onDateChange: (v: string) => void;
    onColorChange: (v: string) => void;
    onDirectionChange?: (v: 'in' | 'out') => void;
    onReverseChange?: (v: boolean) => void;
  }) => (
    <div>
      <input data-testid="stub-surveyor" value={surveyor} onChange={(e) => onSurveyorChange(e.target.value)} />
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
import type { DmpFile, DmpSection } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import { makeDmpSection, makeDmpShots } from '../../../test/factories/dmp';

function dmpFile(): DmpFile {
  const section: DmpSection = makeDmpSection({ shots: makeDmpShots(2) });
  return { version: 1, sections: [section] } as unknown as DmpFile;
}

const selectionConfig = {
  selectedIndices: new Set([0]),
  defaultSurveyor: 'S',
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

  it('goes dirty when surveyor alone is changed', () => {
    renderDialog();
    fireEvent.change(screen.getByTestId('stub-surveyor'), { target: { value: 'New Surveyor' } });
    expect(lastIsDirty()).toBe(true);
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
