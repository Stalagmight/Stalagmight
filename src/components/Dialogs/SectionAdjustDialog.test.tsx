/**
 * Component test (#161) - Section Adjust dialog accepts negative / partial input.
 *
 * Regression for the controlled-number-input bug: dx/dy were a numeric
 * `value={dx}` coerced with `parseFloat(e.target.value) || 0`, so every
 * intermediate keystroke ('-', '', '.') collapsed to 0 and the controlled
 * re-render wiped it - negatives were impossible and a leading 0 couldn't be
 * cleared. The fix backs the inputs with string state that starts empty.
 *
 * `userEvent.type` simulates real per-keystroke entry, which is what exercises
 * the bug - and crucially these tests do NOT clear/select first, so they cover
 * the natural click-then-type flow a user actually hits. (Playwright's `.fill()`
 * sets the value atomically and never reaches the buggy intermediate state, which
 * is why the existing e2e `dyInput.fill('-3.2')` passed while the bug was live -
 * so this guard lives at the component tier.)
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/telemetry', async () => (await import('../../test/factories/telemetry')).telemetryWith());

import SectionAdjustDialog from './SectionAdjustDialog';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { Project, SectionId } from '../../types/project';
import type { CommandResult } from '../../types/commands';

const SECTION_ID = 1 as SectionId;

let moveSection: ReturnType<typeof vi.fn>;

beforeEach(() => {
  moveSection = vi.fn(async (): Promise<CommandResult> => ({ success: true, error: null }) as CommandResult);
  useProjectStore.setState({
    // The dialog only reads project.sections and section.id/name.
    project: { sections: [{ id: SECTION_ID, name: 'Main Passage' }] } as unknown as Project,
    moveSection,
    getConnectedSectionIds: vi.fn(async (): Promise<number[]> => [SECTION_ID]),
  });
  useUIStore.setState({
    closeDialog: vi.fn(),
    dialogData: { sectionId: SECTION_ID },
  });
});

describe('SectionAdjustDialog negative / partial input (#161)', () => {
  it('types a leading minus with no prior clear/select (the natural flow)', async () => {
    render(<SectionAdjustDialog />);
    const dx = await screen.findByTestId('dx-input');
    expect(dx).toHaveValue(''); // starts empty; placeholder shows "0"

    // The exact #161 failure: a lone '-' is the intermediate state the old
    // numeric control snapped back to 0, making negatives untypable.
    await userEvent.type(dx, '-');
    expect(dx).toHaveValue('-');

    await userEvent.type(dx, '2.5');
    expect(dx).toHaveValue('-2.5');
  });

  it('can clear a typed value back to empty (no pinned leading zero)', async () => {
    render(<SectionAdjustDialog />);
    const dy = await screen.findByTestId('dy-input');

    await userEvent.type(dy, '7');
    expect(dy).toHaveValue('7');

    await userEvent.clear(dy);
    expect(dy).toHaveValue(''); // the bug pinned this to '0'
  });

  it('accepts a negative and a leading-dot decimal in dy', async () => {
    // dy goes through handleDyChange - a separate handler/closure from dx, so it
    // needs its own coverage. fireEvent.change is atomic (no debounce-interleave),
    // and the per-keystroke negative path is already covered by the dx test above.
    render(<SectionAdjustDialog />);
    const dy = await screen.findByTestId('dy-input');

    fireEvent.change(dy, { target: { value: '-1.5' } });
    expect(dy).toHaveValue('-1.5');

    fireEvent.change(dy, { target: { value: '.5' } }); // leading dot is valid
    expect(dy).toHaveValue('.5');
  });

  it('drops input the partial-decimal gate rejects', async () => {
    render(<SectionAdjustDialog />);
    const dx = await screen.findByTestId('dx-input');

    // fireEvent.change is atomic + synchronous, so it exercises the reject branch
    // deterministically - per-keystroke userEvent.type of rejected chars races
    // React's controlled-value restore under load (a flake we already hit once).
    fireEvent.change(dx, { target: { value: '-2' } });
    expect(dx).toHaveValue('-2');

    fireEvent.change(dx, { target: { value: '-2e3' } }); // 'e' fails /^-?\d*\.?\d*$/
    expect(dx).toHaveValue('-2'); // rejected → field unchanged
  });
});

// #576 D20: "Adjusting: <name>" is a <Trans> template now (`Adjusting:
// <blob>{{name}}</blob>`). A section name that looks like markup must render as
// typed inside the section pill, not be parsed as a tag of the template.
describe('SectionAdjustDialog section name', () => {
  it('renders a name that looks like markup as typed', async () => {
    useProjectStore.setState({
      project: { sections: [{ id: SECTION_ID, name: 'Main <A> & </blob>B' }] } as unknown as Project,
    });
    render(<SectionAdjustDialog />);
    const pill = await screen.findByText('Main <A> & </blob>B');
    expect(pill.parentElement?.textContent).toBe('Adjusting: Main <A> & </blob>B');
  });
});
