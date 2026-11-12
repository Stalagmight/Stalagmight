/**
 * PeopleInput: the surveyors / explorers pill field. Covers committing
 * (Enter, ';', paste, Tab, blur), suggestion picking, the Escape layering a
 * host dialog relies on, Backspace's two-step removal, partial pills and
 * their tooltips, read-only mode, duplicates and same-name disambiguation.
 *
 * `.tsx` so vitest runs it under jsdom; the real i18n instance renders the
 * English strings.
 */
import '../../../i18n';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { PeopleInput, type PeopleInputProps, type PeopleInputValue } from '../PeopleInput';
import { DialogWrapper } from '../../Dialogs/DialogWrapper';
import { personKey, type PersonLike, type Suggestion } from '../../../utils/people';

const JOHN: PersonLike = { name: 'John Smith', email: 'jsmith@caves.org' };
const JANE: PersonLike = { name: 'Jane Roe', email: 'jane@caves.org' };
const ROB: PersonLike = { name: 'Rob Ray' };

const SUGGESTIONS: Suggestion[] = [
  { person: JOHN, source: 'project' },
  { person: JANE, source: 'speleodb' },
  { person: ROB, source: 'recent' },
];

type HarnessProps = Partial<Omit<PeopleInputProps, 'value'>> & { initial?: PeopleInputValue[] };

/** Owns the value like a real parent, with functional updates. */
function Harness({ initial = [], onAdd, onRemove, ...rest }: HarnessProps) {
  const [value, setValue] = useState<PeopleInputValue[]>(initial);
  return (
    <PeopleInput
      label="Surveyors"
      testId="people"
      suggestions={SUGGESTIONS}
      {...rest}
      value={value}
      onAdd={(person) => {
        onAdd?.(person);
        setValue((prev) => [...prev, { person }]);
      }}
      onRemove={(person) => {
        onRemove?.(person);
        setValue((prev) => prev.filter((entry) => personKey(entry.person) !== personKey(person)));
      }}
    />
  );
}

const input = () => screen.getByTestId('people-input') as HTMLInputElement;
const pillLabels = () => screen.queryAllByTestId(/^people-pill-\d+-label$/).map((el) => el.textContent);
const list = () => screen.queryByTestId('people-suggestions');

describe('PeopleInput - committing typed text', () => {
  it('Enter creates a pill from the typed name, whitespace normalized, and clears the input', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(<Harness onAdd={onAdd} suggestions={[]} />);
    await user.type(input(), '  Jane   Doe {Enter}');
    expect(onAdd).toHaveBeenCalledTimes(1);
    expect(onAdd).toHaveBeenCalledWith({ name: 'Jane Doe' });
    expect(pillLabels()).toEqual(['Jane Doe']);
    expect(input().value).toBe('');
  });

  it("';' commits like Enter and is never typed into the field", async () => {
    const user = userEvent.setup();
    render(<Harness suggestions={[]} />);
    await user.type(input(), 'Ann;Bob;');
    expect(pillLabels()).toEqual(['Ann', 'Bob']);
    expect(input().value).toBe('');
  });

  it('does not split on commas', async () => {
    const user = userEvent.setup();
    render(<Harness suggestions={[]} />);
    await user.type(input(), 'Doe, John{Enter}');
    expect(pillLabels()).toEqual(['Doe, John']);
  });

  it("pasting text with ';' creates every name at once", async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(<Harness onAdd={onAdd} suggestions={[]} />);
    await user.click(input());
    await user.paste('Ann; Bob;  ; Doe, John');
    expect(pillLabels()).toEqual(['Ann', 'Bob', 'Doe, John']);
    expect(onAdd).toHaveBeenCalledTimes(3);
    expect(input().value).toBe('');
  });

  it('pasting a line-separated column creates a pill per line', async () => {
    const user = userEvent.setup();
    render(<Harness suggestions={[]} />);
    await user.click(input());
    await user.paste('Ann\nBob\r\nCy');
    expect(pillLabels()).toEqual(['Ann', 'Bob', 'Cy']);
  });

  it('pasting a single name just fills the input', async () => {
    const user = userEvent.setup();
    render(<Harness suggestions={[]} />);
    await user.click(input());
    await user.paste('Jane Doe');
    expect(pillLabels()).toEqual([]);
    expect(input().value).toBe('Jane Doe');
  });

  it('Enter on an empty input is left to the host (not prevented)', async () => {
    const user = userEvent.setup();
    const seen: boolean[] = [];
    render(
      <div onKeyDown={(e) => e.key === 'Enter' && seen.push(e.defaultPrevented)}>
        <Harness suggestions={[]} />
      </div>,
    );
    await user.click(input());
    await user.keyboard('{Enter}');
    expect(seen).toEqual([false]);
  });

  it('Tab commits the typed name once and lets focus move on', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(
      <>
        <Harness onAdd={onAdd} suggestions={[]} />
        <button type="button" data-testid="after">
          after
        </button>
      </>,
    );
    await user.type(input(), 'Jane');
    await user.tab();
    expect(onAdd).toHaveBeenCalledTimes(1);
    expect(onAdd).toHaveBeenCalledWith({ name: 'Jane' });
    expect(screen.getByTestId('after')).toHaveFocus();
  });

  it('Tab picks the highlighted suggestion, the same way Enter would', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(
      <>
        <Harness onAdd={onAdd} />
        <button type="button" data-testid="after">
          after
        </button>
      </>,
    );
    await user.type(input(), 'jo');
    await user.tab();
    expect(onAdd).toHaveBeenCalledTimes(1);
    expect(onAdd).toHaveBeenCalledWith(JOHN);
  });

  it('blur commits the typed text as typed, never the loose highlight', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(
      <>
        <Harness onAdd={onAdd} />
        <button type="button" data-testid="after">
          after
        </button>
      </>,
    );
    await user.type(input(), 'jo');
    expect(screen.getByTestId('people-suggestion-0')).toHaveAttribute('aria-selected', 'true');
    await user.click(screen.getByTestId('after'));
    expect(onAdd).toHaveBeenCalledTimes(1);
    expect(onAdd).toHaveBeenCalledWith({ name: 'jo' });
    expect(list()).not.toBeInTheDocument();
  });

  it('blur reuses a known person when the typed name is exactly that identity', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(<Harness onAdd={onAdd} />);
    await user.type(input(), 'rob  ray');
    act(() => input().blur());
    expect(onAdd).toHaveBeenCalledWith(ROB);
  });

  it('keeps typed text when the window, not the field, loses focus', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(<Harness onAdd={onAdd} suggestions={[]} />);
    await user.type(input(), 'Jane');
    // A window blur fires blur on the input while it stays the active element.
    fireEvent.blur(input());
    expect(onAdd).not.toHaveBeenCalled();
    expect(input().value).toBe('Jane');
  });

  it('reports pending text through onCommitPending only when it changes', async () => {
    const user = userEvent.setup();
    const onCommitPending = vi.fn();
    render(<Harness onCommitPending={onCommitPending} suggestions={[]} />);
    expect(onCommitPending).not.toHaveBeenCalled();
    await user.type(input(), 'Ja');
    expect(onCommitPending.mock.calls).toEqual([[true]]);
    await user.keyboard('{Enter}');
    expect(onCommitPending.mock.calls).toEqual([[true], [false]]);
  });
});

describe('PeopleInput - suggestions', () => {
  it('shows ranked suggestions with the match in bold, the email and a SpeleoDB badge', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.type(input(), 'ro');
    // Rob Ray: name prefix; Jane Roe: word start.
    const first = screen.getByTestId('people-suggestion-0');
    const second = screen.getByTestId('people-suggestion-1');
    expect(first).toHaveTextContent('Rob Ray');
    expect(within(first).getByText('Ro').tagName).toBe('STRONG');
    expect(second).toHaveTextContent('Jane Roe');
    expect(second).toHaveTextContent('jane@caves.org');
    expect(screen.getByTestId('people-suggestion-1-badge')).toHaveTextContent('SpeleoDB');
    expect(screen.queryByTestId('people-suggestion-0-badge')).not.toBeInTheDocument();
  });

  it('picks a suggestion by click without committing the typed text, and keeps focus', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(<Harness onAdd={onAdd} />);
    await user.type(input(), 'smi');
    await user.click(screen.getByTestId('people-suggestion-0'));
    expect(onAdd).toHaveBeenCalledTimes(1);
    expect(onAdd).toHaveBeenCalledWith(JOHN);
    expect(input()).toHaveFocus();
    expect(input().value).toBe('');
    expect(list()).not.toBeInTheDocument();
  });

  it('picks a suggestion by keyboard and tracks aria-activedescendant', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(<Harness onAdd={onAdd} />);
    await user.type(input(), 'ro');
    const first = screen.getByTestId('people-suggestion-0');
    const second = screen.getByTestId('people-suggestion-1');
    expect(input()).toHaveAttribute('aria-activedescendant', first.id);
    await user.keyboard('{ArrowDown}');
    expect(input()).toHaveAttribute('aria-activedescendant', second.id);
    expect(second).toHaveAttribute('aria-selected', 'true');
    await user.keyboard('{Enter}');
    expect(onAdd).toHaveBeenCalledWith(JANE);
  });

  it('wraps the highlight around the list, including the Add row', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.type(input(), 'ro');
    await user.keyboard('{ArrowUp}');
    expect(screen.getByTestId('people-suggestion-add')).toHaveAttribute('aria-selected', 'true');
    await user.keyboard('{ArrowDown}');
    expect(screen.getByTestId('people-suggestion-0')).toHaveAttribute('aria-selected', 'true');
  });

  it('highlights nothing by default unless the query is a name prefix, so Enter creates the typed name', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(<Harness onAdd={onAdd} />);
    await user.type(input(), 'smi');
    expect(screen.getByTestId('people-suggestion-0')).toHaveAttribute('aria-selected', 'false');
    expect(input()).not.toHaveAttribute('aria-activedescendant');
    await user.keyboard('{Enter}');
    expect(onAdd).toHaveBeenCalledWith({ name: 'smi' });
  });

  it('highlights the row the mouse moves over', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    fireEvent.mouseMove(window, { clientX: 40, clientY: 40 });
    await user.type(input(), 'ro');
    fireEvent.mouseMove(screen.getByTestId('people-suggestion-1'), { clientX: 60, clientY: 52 });
    expect(screen.getByTestId('people-suggestion-1')).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByTestId('people-suggestion-0')).toHaveAttribute('aria-selected', 'false');
  });

  // The data-table popover opens by double-click, so its list appears right
  // where the pointer rests. The row that slides under it gets mouseenter /
  // mouseover (and a same-coordinates mousemove); following those moved the
  // highlight onto "Add <typed>", and Enter created a new person instead of
  // taking the default-highlighted prefix match.
  it('keeps the default highlight when the list opens under a resting pointer', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(<Harness onAdd={onAdd} />);
    fireEvent.mouseMove(window, { clientX: 120, clientY: 90 });
    // Typed with the keyboard: no click, so the pointer stays where it rests.
    input().focus();
    await user.type(input(), 'Ro', { skipClick: true });
    const add = screen.getByTestId('people-suggestion-add');
    fireEvent.mouseEnter(add, { clientX: 120, clientY: 90 });
    fireEvent.mouseOver(add, { clientX: 120, clientY: 90 });
    fireEvent.mouseMove(add, { clientX: 120, clientY: 90 });
    expect(screen.getByTestId('people-suggestion-0')).toHaveAttribute('aria-selected', 'true');
    expect(add).toHaveAttribute('aria-selected', 'false');

    // A real movement over the list does move it.
    fireEvent.mouseMove(add, { clientX: 126, clientY: 93 });
    expect(add).toHaveAttribute('aria-selected', 'true');

    // And Enter takes what is highlighted.
    fireEvent.mouseMove(screen.getByTestId('people-suggestion-0'), { clientX: 126, clientY: 70 });
    await user.keyboard('{Enter}');
    expect(onAdd).toHaveBeenCalledWith(ROB);
  });

  it('offers "Add <typed>" unless the typed text is exactly a known identity', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(<Harness onAdd={onAdd} />);
    await user.type(input(), 'Zed');
    expect(screen.getByTestId('people-suggestion-add')).toHaveTextContent('Add "Zed"');
    await user.clear(input());
    // Rob Ray has no email: typing his name IS him, so no Add row.
    await user.type(input(), 'rob ray');
    expect(screen.queryByTestId('people-suggestion-add')).not.toBeInTheDocument();
    await user.clear(input());
    // John Smith has an email: a name-only John Smith would be someone else.
    await user.type(input(), 'John Smith');
    await user.click(screen.getByTestId('people-suggestion-add'));
    expect(onAdd).toHaveBeenCalledWith({ name: 'John Smith' });
  });

  it('ArrowDown on an empty input opens every suggestion', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(input());
    expect(list()).not.toBeInTheDocument();
    await user.keyboard('{ArrowDown}');
    expect(screen.getAllByRole('option')).toHaveLength(3);
    expect(screen.getByTestId('people-suggestion-0')).toHaveAttribute('aria-selected', 'true');
  });

  it('does not suggest people already selected', async () => {
    const user = userEvent.setup();
    render(<Harness initial={[{ person: JOHN }]} />);
    await user.type(input(), 'j');
    const rows = screen.getAllByRole('option').map((el) => el.textContent);
    expect(rows.some((row) => row?.includes('John Smith'))).toBe(false);
    expect(rows.some((row) => row?.includes('Jane Roe'))).toBe(true);
  });
});

describe('PeopleInput - Escape layering', () => {
  it('closes the list, then clears the text, and only then lets the dialog close', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(
      <DialogWrapper title="Edit shots" onClose={onClose}>
        <Harness />
      </DialogWrapper>,
    );
    await user.type(input(), 'jo');
    expect(list()).toBeInTheDocument();

    await user.keyboard('{Escape}');
    expect(list()).not.toBeInTheDocument();
    expect(input().value).toBe('jo');
    expect(onClose).not.toHaveBeenCalled();

    await user.keyboard('{Escape}');
    expect(input().value).toBe('');
    expect(onClose).not.toHaveBeenCalled();

    await user.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('marks the root data-dropdown-open while the list shows (the app-wide Escape guard)', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    expect(screen.getByTestId('people')).not.toHaveAttribute('data-dropdown-open');
    await user.type(input(), 'jo');
    expect(screen.getByTestId('people')).toHaveAttribute('data-dropdown-open');
  });
});

describe('PeopleInput - removing', () => {
  it('Backspace in an empty input selects the last pill, a second Backspace removes it', async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    render(<Harness onRemove={onRemove} initial={[{ person: { name: 'Ann' } }, { person: { name: 'Bob' } }]} />);
    await user.click(input());
    await user.keyboard('{Backspace}');
    expect(screen.getByTestId('people-pill-1')).toHaveAttribute('data-armed');
    expect(screen.getByTestId('people-status')).toHaveTextContent('Press Backspace again to remove Bob');
    expect(onRemove).not.toHaveBeenCalled();
    await user.keyboard('{Backspace}');
    expect(onRemove).toHaveBeenCalledWith({ name: 'Bob' });
    expect(pillLabels()).toEqual(['Ann']);
    expect(screen.getByTestId('people-pill-0')).not.toHaveAttribute('data-armed');
  });

  it('any other key clears the selected pill', async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    render(<Harness onRemove={onRemove} initial={[{ person: { name: 'Ann' } }]} />);
    await user.click(input());
    await user.keyboard('{Backspace}');
    expect(screen.getByTestId('people-pill-0')).toHaveAttribute('data-armed');
    await user.keyboard('{ArrowLeft}');
    expect(screen.getByTestId('people-pill-0')).not.toHaveAttribute('data-armed');
    await user.keyboard('{Backspace}');
    expect(onRemove).not.toHaveBeenCalled();
  });

  it('Backspace with text edits the text and never touches the pills', async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    render(<Harness onRemove={onRemove} suggestions={[]} initial={[{ person: { name: 'Ann' } }]} />);
    await user.type(input(), 'B{Backspace}');
    expect(input().value).toBe('');
    expect(screen.getByTestId('people-pill-0')).not.toHaveAttribute('data-armed');
    expect(onRemove).not.toHaveBeenCalled();
  });

  it('the x button removes its person, without committing the typed text', async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    const onAdd = vi.fn();
    render(
      <Harness
        onAdd={onAdd}
        onRemove={onRemove}
        suggestions={[]}
        initial={[{ person: { name: 'Ann' } }, { person: { name: 'Bob' } }]}
      />,
    );
    await user.type(input(), 'Zo');
    const remove = screen.getByTestId('people-pill-0-remove');
    expect(remove).toHaveAccessibleName('Remove Ann');
    await user.click(remove);
    expect(onRemove).toHaveBeenCalledWith({ name: 'Ann' });
    expect(onAdd).not.toHaveBeenCalled();
    expect(input().value).toBe('Zo');
    expect(pillLabels()).toEqual(['Bob']);
  });

  it('the x button is keyboard reachable', async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    render(<Harness onRemove={onRemove} initial={[{ person: { name: 'Ann' } }]} />);
    await user.tab();
    expect(screen.getByTestId('people-pill-0-remove')).toHaveFocus();
    await user.keyboard('{Enter}');
    expect(onRemove).toHaveBeenCalledWith({ name: 'Ann' });
    expect(input()).toHaveFocus();
  });
});

describe('PeopleInput - pills', () => {
  const partialValue: PeopleInputValue[] = [
    { person: { name: 'Ann Lee', email: 'ann@caves.org' }, count: 12, total: 30 },
    { person: { name: 'Bob' }, count: 30, total: 30 },
  ];

  it('renders a partial pill with a count suffix and a full one without', () => {
    render(<Harness initial={partialValue} />);
    expect(screen.getByTestId('people-pill-0')).toHaveAttribute('data-partial');
    expect(screen.getByTestId('people-pill-0-count')).toHaveTextContent('12/30');
    expect(screen.getByTestId('people-pill-1')).not.toHaveAttribute('data-partial');
    expect(screen.queryByTestId('people-pill-1-count')).not.toBeInTheDocument();
  });

  it('shows the full details in a tooltip on hover', async () => {
    const user = userEvent.setup();
    render(<Harness initial={partialValue} />);
    await user.hover(screen.getByTestId('people-pill-0-label'));
    const tooltip = await screen.findByTestId('people-pill-0-tooltip');
    expect(tooltip).toHaveTextContent('Ann Lee');
    expect(tooltip).toHaveTextContent('ann@caves.org');
    expect(tooltip).toHaveTextContent('On 12 of 30 shots');
    expect(tooltip).not.toHaveTextContent('SpeleoDB collaborator');
  });

  it('shows the tooltip on keyboard focus too', () => {
    render(<Harness initial={partialValue} />);
    act(() => screen.getByTestId('people-pill-1-remove').focus());
    expect(screen.getByTestId('people-pill-1-tooltip')).toHaveTextContent('Bob');
    // A full pill over several shots says so; only partial pills say "N of M".
    expect(screen.getByTestId('people-pill-1-tooltip')).toHaveTextContent('On all 30 shots');
    expect(screen.getByTestId('people-pill-1-tooltip')).not.toHaveTextContent('of 30');
  });

  it('names a SpeleoDB collaborator in the tooltip', async () => {
    const user = userEvent.setup();
    render(<Harness initial={[{ person: JANE }]} />);
    await user.hover(screen.getByTestId('people-pill-0-label'));
    expect(await screen.findByTestId('people-pill-0-tooltip')).toHaveTextContent('SpeleoDB collaborator');
  });

  it('clicking the body of a partial pill promotes it when onPromote is given', async () => {
    const user = userEvent.setup();
    const onPromote = vi.fn();
    render(<Harness initial={partialValue} onPromote={onPromote} />);
    expect(screen.queryByTestId('people-pill-1-promote')).not.toBeInTheDocument();
    await user.click(screen.getByTestId('people-pill-0-promote'));
    expect(onPromote).toHaveBeenCalledWith(partialValue[0]!.person);
    await user.hover(screen.getByTestId('people-pill-0-label'));
    expect(await screen.findByTestId('people-pill-0-tooltip')).toHaveTextContent('Click to add to all shots');
  });

  it('has no promote control without onPromote, and x still removes a partial pill', async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    render(<Harness initial={partialValue} onRemove={onRemove} />);
    expect(screen.queryByTestId('people-pill-0-promote')).not.toBeInTheDocument();
    await user.click(screen.getByTestId('people-pill-0-remove'));
    expect(onRemove).toHaveBeenCalledWith(partialValue[0]!.person);
  });

  it('disambiguates two people named John Smith', async () => {
    const user = userEvent.setup();
    const suggestions: Suggestion[] = [
      { person: JOHN, source: 'project' },
      { person: { name: 'John Smith', email: 'john.smith@diving.net' }, source: 'project' },
    ];
    render(<Harness suggestions={suggestions} initial={[{ person: JOHN }]} />);
    expect(pillLabels()).toEqual(['John Smith (jsmith)']);
    expect(screen.getByTestId('people-pill-0-remove')).toHaveAccessibleName('Remove John Smith (jsmith)');
    // A third, email-less John Smith through the Add row.
    await user.type(input(), 'John Smith');
    await user.click(screen.getByTestId('people-suggestion-add'));
    expect(pillLabels()).toEqual(['John Smith (jsmith)', 'John Smith (no email)']);
  });

  it('shows a lone name without any hint', () => {
    render(<Harness initial={[{ person: JOHN }]} />);
    expect(pillLabels()).toEqual(['John Smith']);
  });
});

describe('PeopleInput - duplicates', () => {
  it('re-adding a listed person clears the input and flashes their pill instead', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(<Harness onAdd={onAdd} suggestions={[]} initial={[{ person: { name: 'Ann Lee' } }]} />);
    await user.type(input(), 'ann  lee{Enter}');
    expect(onAdd).not.toHaveBeenCalled();
    expect(input().value).toBe('');
    expect(screen.getByTestId('people-pill-0')).toHaveAttribute('data-flash');
    expect(screen.getByTestId('people-status')).toHaveTextContent('Ann Lee is already listed');
    await waitFor(() => expect(screen.getByTestId('people-pill-0')).not.toHaveAttribute('data-flash'), {
      timeout: 2000,
    });
  });

  it('a pasted list skips listed people and repeats within itself', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(<Harness onAdd={onAdd} suggestions={[]} initial={[{ person: { name: 'Ann' } }]} />);
    await user.click(input());
    await user.paste('ann; Cy; CY ;Dee');
    expect(onAdd.mock.calls.map(([p]) => (p as PersonLike).name)).toEqual(['Cy', 'Dee']);
    expect(pillLabels()).toEqual(['Ann', 'Cy', 'Dee']);
    expect(screen.getByTestId('people-pill-0')).toHaveAttribute('data-flash');
  });

  it('does not offer "Add" for a name that is already selected', async () => {
    const user = userEvent.setup();
    render(<Harness suggestions={[]} initial={[{ person: { name: 'Ann' } }]} />);
    await user.type(input(), 'ann');
    expect(list()).not.toBeInTheDocument();
  });
});

describe('PeopleInput - read-only', () => {
  it('shows the pills without remove buttons or an input', () => {
    render(<Harness disabled initial={[{ person: { name: 'Ann' } }, { person: JOHN }]} />);
    expect(pillLabels()).toEqual(['Ann', 'John Smith']);
    expect(screen.queryByTestId('people-input')).not.toBeInTheDocument();
    expect(screen.queryByTestId('people-pill-0-remove')).not.toBeInTheDocument();
    // Pills stay keyboard-reachable so their tooltips can be read.
    expect(screen.getByTestId('people-pill-0')).toHaveAttribute('tabIndex', '0');
  });

  it('keeps tooltips but never promotes', async () => {
    const user = userEvent.setup();
    const onPromote = vi.fn();
    render(<Harness disabled onPromote={onPromote} initial={[{ person: { name: 'Ann' }, count: 1, total: 4 }]} />);
    expect(screen.queryByTestId('people-pill-0-promote')).not.toBeInTheDocument();
    await user.hover(screen.getByTestId('people-pill-0-label'));
    expect(await screen.findByTestId('people-pill-0-tooltip')).toHaveTextContent('On 1 of 4 shots');
  });

  it('says None when empty', () => {
    render(<Harness disabled />);
    expect(screen.getByTestId('people-field')).toHaveTextContent('None');
  });
});

describe('PeopleInput - accessibility and layout', () => {
  it('wires the combobox, listbox and label together', async () => {
    const user = userEvent.setup();
    render(<Harness initial={[{ person: { name: 'Ann' } }]} />);
    const combo = screen.getByRole('combobox', { name: 'Surveyors' });
    expect(combo).toBe(input());
    expect(combo).toHaveAttribute('aria-expanded', 'false');
    expect(combo).not.toHaveAttribute('aria-controls');
    await user.type(combo, 'jo');
    const listbox = screen.getByRole('listbox', { name: 'Surveyors' });
    expect(combo).toHaveAttribute('aria-expanded', 'true');
    expect(combo).toHaveAttribute('aria-controls', listbox.id);
    expect(within(listbox).getAllByRole('option').length).toBeGreaterThan(0);
    const pills = screen.getByRole('list', { name: 'Surveyors' });
    expect(within(pills).getAllByRole('listitem')).toHaveLength(1);
  });

  it('clicking the empty part of the field focuses the input', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByTestId('people-field'));
    expect(input()).toHaveFocus();
  });

  it('uses the given id for the input and honours autoFocus', () => {
    render(<Harness id="surveyors-field" autoFocus />);
    expect(input()).toHaveAttribute('id', 'surveyors-field');
    expect(input()).toHaveFocus();
  });

  it('shows the placeholder only while there are no pills', async () => {
    const user = userEvent.setup();
    render(<Harness placeholder="Add a surveyor" suggestions={[]} />);
    expect(input()).toHaveAttribute('placeholder', 'Add a surveyor');
    await user.type(input(), 'Ann{Enter}');
    expect(input()).not.toHaveAttribute('placeholder');
  });

  it('compact keeps the label for assistive tech but hides it visually', () => {
    render(<Harness compact />);
    expect(screen.getByText('Surveyors')).toHaveClass('sr-only');
    expect(screen.getByRole('combobox', { name: 'Surveyors' })).toBeInTheDocument();
  });
});

describe('PeopleInput - typed names resolve to SpeleoDB collaborators', () => {
  const ALICE: PersonLike = { name: 'Alice Ng', email: 'alice@caves.org' };
  const SMITH_A: PersonLike = { name: 'John Smith', email: 'jsmith@caves.org' };
  const SMITH_B: PersonLike = { name: 'John Smith', email: 'john.smith@divers.example' };

  it('an email typed in full becomes the one collaborator with that email', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(<Harness onAdd={onAdd} suggestions={[{ person: ALICE, source: 'speleodb' }]} />);
    await user.type(input(), 'ALICE@caves.org');
    // An email match is not a name prefix, so nothing is highlighted...
    expect(screen.getByTestId('people-suggestion-0')).toHaveAttribute('aria-selected', 'false');
    await user.keyboard('{Enter}');
    // ...and the typed text still takes the collaborator's identity.
    expect(onAdd).toHaveBeenCalledWith(ALICE);
    expect(pillLabels()).toEqual(['Alice Ng']);
  });

  it('a name typed in full and left with Tab becomes that collaborator, email included', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(<Harness onAdd={onAdd} suggestions={[{ person: ALICE, source: 'speleodb' }, { person: ROB, source: 'recent' }]} />);
    await user.type(input(), 'alice ng');
    await user.keyboard('{Tab}');
    expect(onAdd).toHaveBeenCalledWith(ALICE);
  });

  it('two same-named collaborators: Enter asks instead of guessing, the list tells them apart', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(
      <Harness
        onAdd={onAdd}
        suggestions={[
          { person: SMITH_A, source: 'speleodb' },
          { person: SMITH_B, source: 'speleodb' },
        ]}
      />,
    );
    await user.type(input(), 'John Smith');
    await user.keyboard('{Enter}');
    expect(onAdd).not.toHaveBeenCalled();
    expect(input().value).toBe('John Smith');
    expect(screen.getByTestId('people-status')).toHaveTextContent('Several people are called John Smith');
    const rows = within(screen.getByTestId('people-suggestions')).getAllByRole('option');
    expect(rows.map((row) => row.textContent)).toEqual(
      expect.arrayContaining([expect.stringContaining('jsmith@caves.org'), expect.stringContaining('john.smith@divers.example')]),
    );
    await user.click(screen.getByTestId('people-suggestion-1'));
    expect(onAdd).toHaveBeenCalledTimes(1);
    // The pill shows just enough to tell the two apart.
    expect(pillLabels()[0]).toMatch(/^John Smith \((jsmith|john\.smith)\)$/);
  });

  it('a name typed in full that two collaborators share stays a plain name on blur', async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(
      <Harness
        onAdd={onAdd}
        suggestions={[
          { person: SMITH_A, source: 'speleodb' },
          { person: SMITH_B, source: 'speleodb' },
        ]}
      />,
    );
    await user.type(input(), 'John Smith');
    act(() => input().blur());
    expect(onAdd).toHaveBeenCalledWith({ name: 'John Smith' });
  });

  it('starts with defaultText and its suggestions open', () => {
    render(<Harness defaultText="Ja" />);
    expect(input().value).toBe('Ja');
    expect(list()).toBeInTheDocument();
  });
});
