/**
 * #567 item 2: the suggestion phase's line-metadata form starts collapsed
 * behind a disclosure so it stops crowding the connection card + confidence
 * badge above it. `editState` lives in the parent dialog, not in this
 * component, so collapsing it must never lose in-progress edits.
 */
import { describe, expect, it, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { SuggestionSectionSettings } from '../SuggestionSectionSettings';
import type { LineEditState } from '../../../utils/batchImportHelpers';
import { listPeopleProps } from '../../common/LineMetadataForm';

vi.mock('react-i18next', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react-i18next')>()),
  useTranslation: () => ({ t: (key: string) => key }),
}));

const editState = (overrides: Partial<LineEditState> = {}): LineEditState => ({
  name: 'Line A',
  surveyors: [{ name: 'Alice' }],
  date: '2026-01-01',
  color: '#ffffff',
  direction: 'in',
  quality: 1,
  reverse: false,
  shots: [],
  ...overrides,
});

/** The host resolves the people (its own lists, else the wizard defaults) and hands them in. */
const people = (update = vi.fn()) => listPeopleProps({ surveyors: [{ name: 'Alice' }], explorers: [] }, update);

describe('SuggestionSectionSettings', () => {
  it('opens on a keyboard Enter, not just a mouse click', async () => {
    // Injection: revert Panel's collapsible header to a div[role=button] with
    // no keyboard handling (the pre-fix shape) -> Tab focuses it but Enter
    // dispatches no click, line-name-input never appears, and this goes red.
    const user = userEvent.setup();
    render(<SuggestionSectionSettings editState={editState()} people={people()} onMetadataChange={vi.fn()} />);

    expect(screen.queryByTestId('line-name-input')).not.toBeInTheDocument();

    await user.tab();
    expect(screen.getByTestId('wizard-suggestion-section-settings-header')).toHaveFocus();
    // Focus alone must not open it.
    expect(screen.queryByTestId('line-name-input')).not.toBeInTheDocument();

    await user.keyboard('{Enter}');
    expect(screen.getByTestId('line-name-input')).toBeInTheDocument();
  });

  it('exposes aria-expanded on the header, tracking open state', () => {
    // Injection: drop `aria-expanded={isOpen}` from the header button ->
    // getByTestId(...).getAttribute('aria-expanded') is null and this goes red.
    render(<SuggestionSectionSettings editState={editState()} people={people()} onMetadataChange={vi.fn()} />);
    const header = screen.getByTestId('wizard-suggestion-section-settings-header');

    expect(header).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(header);
    expect(header).toHaveAttribute('aria-expanded', 'true');
  });

  it('starts collapsed, hiding the line-name input', () => {
    // Injection: drop `defaultOpen={false}` (or flip it) on the wrapping
    // Panel -> the input renders on first paint and this goes red.
    render(<SuggestionSectionSettings editState={editState()} people={people()} onMetadataChange={vi.fn()} />);

    expect(screen.queryByTestId('line-name-input')).not.toBeInTheDocument();
  });

  it('expanding the disclosure reveals the form with the current values', () => {
    // Injection: render the form unconditionally regardless of open state,
    // or wire the header click to something other than toggling open -> the
    // header click stops revealing the input and this goes red.
    render(<SuggestionSectionSettings editState={editState({ name: 'Line B' })} people={people()} onMetadataChange={vi.fn()} />);

    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));

    const nameInput = screen.getByTestId('line-name-input') as HTMLInputElement;
    expect(nameInput.value).toBe('Line B');
  });

  it('collapsing again does not wipe values held by the parent', () => {
    // Injection: if the form's state lived inside this component instead of
    // the parent's editState, a collapse->expand cycle without a re-render
    // would still show the same value here (this test can't tell), but if
    // the component threw away the field content it renders on the next
    // expand (e.g. hard-coding a value on remount) this test goes red.
    const onMetadataChange = vi.fn();
    const { rerender } = render(
      <SuggestionSectionSettings editState={editState({ name: 'Line C' })} people={people()} onMetadataChange={onMetadataChange} />,
    );

    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));
    fireEvent.change(screen.getByTestId('line-name-input'), { target: { value: 'Line C edited' } });
    expect(onMetadataChange).toHaveBeenCalledWith('name', 'Line C edited');

    // Simulate the parent committing the edit into editState, then collapse.
    rerender(
      <SuggestionSectionSettings editState={editState({ name: 'Line C edited' })} people={people()} onMetadataChange={onMetadataChange} />,
    );
    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));
    expect(screen.queryByTestId('line-name-input')).not.toBeInTheDocument();

    // Expand again - the parent-held value survived the collapse.
    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));
    expect((screen.getByTestId('line-name-input') as HTMLInputElement).value).toBe('Line C edited');
  });

  it('reflects a parent-driven value the child never typed, across a collapse', () => {
    // Strengthens the test above: a component that ignored `editState` and
    // kept its own internal input state would still pass the previous test
    // (it echoes back the value the child itself typed), but would fail
    // this one, since 'Line D' never passes through the input's onChange
    // here - it can only appear by reading `editState.name` on every render.
    // Injection: cache the initial `editState.name` in local state on mount
    // (e.g. `useState(editState.name)`) instead of reading the prop directly
    // -> the input keeps showing 'Line C' after the parent renames it,
    // and this goes red.
    const onMetadataChange = vi.fn();
    const { rerender } = render(
      <SuggestionSectionSettings editState={editState({ name: 'Line C' })} people={people()} onMetadataChange={onMetadataChange} />,
    );

    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));
    expect((screen.getByTestId('line-name-input') as HTMLInputElement).value).toBe('Line C');

    // Collapse, then the parent renames the line from elsewhere (not
    // through this component's own input).
    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));
    rerender(
      <SuggestionSectionSettings editState={editState({ name: 'Line D' })} people={people()} onMetadataChange={onMetadataChange} />,
    );

    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));
    expect((screen.getByTestId('line-name-input') as HTMLInputElement).value).toBe('Line D');
  });

  it('shows the line people as pills and hands an added name to the host', async () => {
    // Injection: drop `people={people}` from the LineMetadataForm inside ->
    // no pill renders and the update is never called, and this goes red.
    const user = userEvent.setup();
    const update = vi.fn();
    render(<SuggestionSectionSettings editState={editState()} people={people(update)} onMetadataChange={vi.fn()} />);
    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));

    expect(screen.getByTestId('line-surveyors-pill-0-label')).toHaveTextContent('Alice');
    await user.type(screen.getByTestId('line-explorers-input'), 'Bob{Enter}');
    expect(update).toHaveBeenCalledWith('explorers', expect.any(Function));
    const reducer = update.mock.calls[0]![1] as (prev: Array<{ name: string }>) => Array<{ name: string }>;
    expect(reducer([])).toEqual([{ name: 'Bob' }]);
  });
});
