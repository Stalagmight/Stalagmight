/**
 * #567 item 2: the suggestion phase's section-metadata form starts collapsed
 * behind a disclosure so it stops crowding the connection card + confidence
 * badge above it. `editState` lives in the parent dialog, not in this
 * component, so collapsing it must never lose in-progress edits.
 */
import { describe, expect, it, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { SuggestionSectionSettings } from '../SuggestionSectionSettings';
import type { SectionEditState } from '../../../utils/batchImportHelpers';

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys());

const editState = (overrides: Partial<SectionEditState> = {}): SectionEditState => ({
  name: 'Section A',
  surveyor: 'Alice',
  date: '2026-01-01',
  color: '#ffffff',
  direction: 'in',
  quality: 1,
  reverse: false,
  shots: [],
  ...overrides,
});

describe('SuggestionSectionSettings', () => {
  it('opens on a keyboard Enter, not just a mouse click', async () => {
    // Injection: revert Panel's collapsible header to a div[role=button] with
    // no keyboard handling (the pre-fix shape) -> Tab focuses it but Enter
    // dispatches no click, section-name-input never appears, and this goes red.
    const user = userEvent.setup();
    render(<SuggestionSectionSettings editState={editState()} onMetadataChange={vi.fn()} />);

    expect(screen.queryByTestId('section-name-input')).not.toBeInTheDocument();

    await user.tab();
    expect(screen.getByTestId('wizard-suggestion-section-settings-header')).toHaveFocus();
    // Focus alone must not open it.
    expect(screen.queryByTestId('section-name-input')).not.toBeInTheDocument();

    await user.keyboard('{Enter}');
    expect(screen.getByTestId('section-name-input')).toBeInTheDocument();
  });

  it('exposes aria-expanded on the header, tracking open state', () => {
    // Injection: drop `aria-expanded={isOpen}` from the header button ->
    // getByTestId(...).getAttribute('aria-expanded') is null and this goes red.
    render(<SuggestionSectionSettings editState={editState()} onMetadataChange={vi.fn()} />);
    const header = screen.getByTestId('wizard-suggestion-section-settings-header');

    expect(header).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(header);
    expect(header).toHaveAttribute('aria-expanded', 'true');
  });

  it('starts collapsed, hiding the section-name input', () => {
    // Injection: drop `defaultOpen={false}` (or flip it) on the wrapping
    // Panel -> the input renders on first paint and this goes red.
    render(<SuggestionSectionSettings editState={editState()} onMetadataChange={vi.fn()} />);

    expect(screen.queryByTestId('section-name-input')).not.toBeInTheDocument();
  });

  it('expanding the disclosure reveals the form with the current values', () => {
    // Injection: render the form unconditionally regardless of open state,
    // or wire the header click to something other than toggling open -> the
    // header click stops revealing the input and this goes red.
    render(<SuggestionSectionSettings editState={editState({ name: 'Section B' })} onMetadataChange={vi.fn()} />);

    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));

    const nameInput = screen.getByTestId('section-name-input') as HTMLInputElement;
    expect(nameInput.value).toBe('Section B');
  });

  it('collapsing again does not wipe values held by the parent', () => {
    // Injection: if the form's state lived inside this component instead of
    // the parent's editState, a collapse->expand cycle without a re-render
    // would still show the same value here (this test can't tell), but if
    // the component threw away the field content it renders on the next
    // expand (e.g. hard-coding a value on remount) this test goes red.
    const onMetadataChange = vi.fn();
    const { rerender } = render(
      <SuggestionSectionSettings editState={editState({ name: 'Section C' })} onMetadataChange={onMetadataChange} />,
    );

    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));
    fireEvent.change(screen.getByTestId('section-name-input'), { target: { value: 'Section C edited' } });
    expect(onMetadataChange).toHaveBeenCalledWith('name', 'Section C edited');

    // Simulate the parent committing the edit into editState, then collapse.
    rerender(
      <SuggestionSectionSettings editState={editState({ name: 'Section C edited' })} onMetadataChange={onMetadataChange} />,
    );
    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));
    expect(screen.queryByTestId('section-name-input')).not.toBeInTheDocument();

    // Expand again - the parent-held value survived the collapse.
    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));
    expect((screen.getByTestId('section-name-input') as HTMLInputElement).value).toBe('Section C edited');
  });

  it('reflects a parent-driven value the child never typed, across a collapse', () => {
    // Strengthens the test above: a component that ignored `editState` and
    // kept its own internal input state would still pass the previous test
    // (it echoes back the value the child itself typed), but would fail
    // this one, since 'Section D' never passes through the input's onChange
    // here - it can only appear by reading `editState.name` on every render.
    // Injection: cache the initial `editState.name` in local state on mount
    // (e.g. `useState(editState.name)`) instead of reading the prop directly
    // -> the input keeps showing 'Section C' after the parent renames it,
    // and this goes red.
    const onMetadataChange = vi.fn();
    const { rerender } = render(
      <SuggestionSectionSettings editState={editState({ name: 'Section C' })} onMetadataChange={onMetadataChange} />,
    );

    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));
    expect((screen.getByTestId('section-name-input') as HTMLInputElement).value).toBe('Section C');

    // Collapse, then the parent renames the section from elsewhere (not
    // through this component's own input).
    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));
    rerender(
      <SuggestionSectionSettings editState={editState({ name: 'Section D' })} onMetadataChange={onMetadataChange} />,
    );

    fireEvent.click(screen.getByTestId('wizard-suggestion-section-settings-header'));
    expect((screen.getByTestId('section-name-input') as HTMLInputElement).value).toBe('Section D');
  });
});
