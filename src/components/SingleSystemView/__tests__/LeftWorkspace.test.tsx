import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { LeftWorkspace } from '../LeftWorkspace';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));
vi.mock('../SpeleoWorkspace', () => ({ SpeleoWorkspace: () => <input aria-label="Metadata draft" data-testid="workspace-test-draft" /> }));
const link: SpeleoLinkState = {
  permission: 'READ_AND_WRITE', editState: 'read-only', lockHolder: null,
  lockedByOther: false, hasUnpushedEdits: false, unwound: false,
  undoMode: 'full', remoteState: 'up_to_date', noAccess: false, serverError: null,
};
beforeEach(() => {
  useSpeleoStore.setState({ link, session: null });
  useUIStore.setState({ appMode: 'survey', leftWorkspace: 'sections', surveyLeftWorkspace: 'sections' });
});
describe('Left workspace', () => {
  it('opens Sections first and preserves both tab bodies and drafts', () => {
    render(<LeftWorkspace sections={<input aria-label="Sections selection" data-testid="sections-test-selection" />} />);
    const sections = screen.getByRole('tab', { name: 'Sections' });
    expect(sections).toHaveAttribute('aria-selected', 'true');
    fireEvent.change(screen.getByTestId('sections-test-selection'), { target: { value: 'station 42' } });
    fireEvent.click(screen.getByRole('tab', { name: 'SpeleoDB' }));
    const draft = screen.getByRole('textbox', { name: 'Metadata draft' });
    fireEvent.change(draft, { target: { value: 'unfinished' } });
    fireEvent.click(sections);
    expect(draft).not.toBeVisible();
    expect(screen.getByTestId('sections-test-selection')).toHaveValue('station 42');
    fireEvent.click(screen.getByRole('tab', { name: 'SpeleoDB' }));
    expect(screen.getByRole('textbox', { name: 'Metadata draft' })).toBe(draft);
    expect(draft).toHaveValue('unfinished');
  });
  it('implements arrow-key tab navigation and roving focus', () => {
    render(<LeftWorkspace sections={<div />} />);
    const first = screen.getByRole('tab', { name: 'Sections' }); first.focus();
    fireEvent.keyDown(first, { key: 'ArrowRight' });
    expect(screen.getByRole('tab', { name: 'SpeleoDB' })).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowRight' });
    expect(first).toHaveFocus();
  });
  it('restores the Survey tab after all-mode remote navigation', () => {
    render(<LeftWorkspace sections={<div />} />);
    act(() => useUIStore.getState().setAppMode('plan'));
    expect(screen.queryByRole('tab', { name: 'Sections' })).toBeNull();
    expect(useUIStore.getState().speleoOutsideSurvey).toBe(false);
    act(() => useUIStore.getState().openSpeleoWorkspace('history'));
    expect(useUIStore.getState().speleoOutsideSurvey).toBe(true);
    act(() => useUIStore.getState().setAppMode('survey'));
    expect(screen.getByRole('tab', { name: 'Sections' })).toHaveAttribute('aria-selected', 'true');
  });
  it('exits fullscreen without clearing selection', () => {
    useUIStore.setState({ dataTableFullScreen: true, selection: { type: 'section', sectionId: 42 } });
    act(() => useUIStore.getState().openSpeleoWorkspace());
    expect(useUIStore.getState().dataTableFullScreen).toBe(false);
    expect(useUIStore.getState().selection).toEqual({ type: 'section', sectionId: 42 });
  });
});

describe('Left workspace tab icons and SpeleoDB state', () => {
  const speleoTab = () => screen.getByRole('tab', { name: 'SpeleoDB' });
  const badge = () => screen.queryByTestId('workspace-tab-speleodb-icon-state-badge');
  beforeEach(() => useProjectStore.setState({ isDirty: false }));

  it('gives Sections its icon and SpeleoDB the bare mark while editing', () => {
    useSpeleoStore.setState({ link: { ...link, editState: 'editing' } });
    render(<LeftWorkspace sections={<div />} />);
    const sections = screen.getByRole('tab', { name: 'Sections' });
    expect(sections.querySelector('[data-testid="workspace-tab-sections-icon"]')).not.toBeNull();
    expect(speleoTab()).toContainElement(screen.getByTestId('workspace-tab-speleodb-icon-logo'));
    expect(speleoTab()).toHaveAttribute('data-state', 'normal');
    expect(badge()).toBeNull();
    expect(speleoTab()).not.toHaveAttribute('aria-describedby');
    expect(speleoTab().className).not.toContain('text-warning');
  });

  it('a local project shows the bare mark (no SpeleoDB state to report)', () => {
    useSpeleoStore.setState({ link: null });
    render(<LeftWorkspace sections={<div />} />);
    expect(speleoTab()).toHaveAttribute('data-state', 'normal');
    expect(badge()).toBeNull();
  });

  it('read-only (no write lock) badges the mark with a lock and says so', () => {
    render(<LeftWorkspace sections={<div />} />);
    expect(speleoTab()).toHaveAttribute('data-state', 'read-only');
    expect(badge()).not.toBeNull();
    expect(badge()).toHaveClass('text-text-primary');
    expect(badge()!.querySelector('svg')).not.toBeNull();
    expect(speleoTab()).toHaveAccessibleDescription(/Read-only/);
    expect(speleoTab().className).not.toContain('text-warning');
  });

  it.each([
    ['signed out', null, 'READ_AND_WRITE', 'Read-only copy - connect to SpeleoDB to edit'],
    ['view-only access', { loggedIn: true, userEmail: 'me@example.com', endpoint: 'https://example.com' }, 'READ_ONLY',
      'Read-only - your SpeleoDB access does not allow editing'],
    ['not editing', { loggedIn: true, userEmail: 'me@example.com', endpoint: 'https://example.com' }, 'READ_AND_WRITE',
      'Read-only - you are not editing this project'],
  ] as const)('read-only says why, in the pill and header words, when %s', (_label, session, permission, words) => {
    useSpeleoStore.setState({ session, link: { ...link, permission } });
    render(<LeftWorkspace sections={<div />} />);
    expect(speleoTab()).toHaveAttribute('data-state', 'read-only');
    expect(speleoTab()).toHaveAttribute('title', words);
    expect(speleoTab()).toHaveAccessibleDescription(words);
  });

  it('labels the tab list and both tabs through i18n', () => {
    render(<LeftWorkspace sections={<div />} />);
    expect(screen.getByRole('tablist', { name: 'Project workspace' })).toBeInTheDocument();
    expect(screen.getByRole('tab', { name: 'Sections' })).toBeInTheDocument();
    expect(speleoTab()).toBeInTheDocument();
  });

  /**
   * The mark takes the tab's own text colour: it paints in `currentColor` (no tint)
   * and nothing between it and the tab sets a text colour of its own, so the icon
   * and the label can never disagree.
   */
  const expectIconInTabColour = () => {
    const logo = screen.getByTestId('workspace-tab-speleodb-icon-logo');
    expect(logo.style.backgroundColor).toBe('currentcolor');
    expect(logo).not.toHaveAttribute('data-tint');
    for (let el = logo.parentElement; el && el !== speleoTab(); el = el.parentElement) {
      expect(el.className).not.toMatch(/\btext-(?!sm\b|xs\b)/);
    }
  };

  it.each([
    ['saved but not uploaded', () => useSpeleoStore.setState({ link: { ...link, editState: 'editing', hasUnpushedEdits: true } })],
    ['unsaved edits', () => { useSpeleoStore.setState({ link: { ...link, editState: 'editing' } }); useProjectStore.setState({ isDirty: true }); }],
  ])('un-uploaded changes (%s) turn the icon and text yellow, with no "!" badge', (_label, seed) => {
    seed();
    render(<LeftWorkspace sections={<div />} />);
    expect(speleoTab()).toHaveAttribute('data-state', 'unuploaded');
    // Not an error: no badge on the mark at all.
    expect(badge()).toBeNull();
    expect(speleoTab().querySelector('svg')).toBeNull();
    // Not selected (Sections is): the warning text, dimmed, on the label and the mark alike.
    expect(speleoTab()).toHaveClass('text-warning-text-dim');
    expectIconInTabColour();
    expect(speleoTab().className).not.toMatch(/text-error|text-danger|bg-error|bg-danger/);
    // The note says what is going on, without calling it an error.
    const note = 'Changes on this computer have not been uploaded to SpeleoDB yet.';
    expect(speleoTab()).toHaveAccessibleDescription(note);
    expect(speleoTab()).toHaveAttribute('title', note);
    expect(note).not.toMatch(/error|fail|problem|warning|!/i);
  });

  it('un-uploaded wins over read-only when both apply, still without a badge', () => {
    useSpeleoStore.setState({ link: { ...link, editState: 'read-only', hasUnpushedEdits: true } });
    render(<LeftWorkspace sections={<div />} />);
    expect(speleoTab()).toHaveAttribute('data-state', 'unuploaded');
    expect(badge()).toBeNull();
    expect(speleoTab()).toHaveAccessibleDescription(/not been uploaded/);
  });

  it('a copy only stepped back to an older version is not flagged as un-uploaded (#303)', () => {
    useSpeleoStore.setState({ link: { ...link, editState: 'editing', hasUnpushedEdits: true, unwound: true } });
    render(<LeftWorkspace sections={<div />} />);
    expect(speleoTab()).toHaveAttribute('data-state', 'normal');
  });

  it('keeps the warning visible from Plan and Export, where only the SpeleoDB tab exists', () => {
    useSpeleoStore.setState({ link: { ...link, hasUnpushedEdits: true } });
    useUIStore.setState({ appMode: 'plan', leftWorkspace: 'speleodb' });
    render(<LeftWorkspace sections={<div />} />);
    expect(screen.queryByRole('tab', { name: 'Sections' })).toBeNull();
    expect(speleoTab()).toHaveAttribute('data-state', 'unuploaded');
  });

  /** The tab's look, without the layout classes every tab shares. */
  const look = (tab: HTMLElement) => tab.className.split(/\s+/)
    .filter((c) => /^(bg-|text-(?!sm$)|font-|shadow|border|ring)/.test(c)).sort();
  const unuploaded = () => useSpeleoStore.setState({ link: { ...link, editState: 'editing', hasUnpushedEdits: true } });

  it('un-uploaded, not selected: only the dimmed warning text - no wash, no rule, no selected weight', () => {
    unuploaded();
    render(<LeftWorkspace sections={<div />} />);
    expect(speleoTab()).toHaveAttribute('aria-selected', 'false');
    expect(look(speleoTab())).toEqual(['text-warning-text-dim']);
    expect(speleoTab().className).not.toMatch(/bg-warning|shadow|inset/);
    // Sections, the selected tab, keeps the selected indicator.
    expect(look(screen.getByRole('tab', { name: 'Sections' }))).toEqual(['bg-accent-10', 'font-medium']);
  });

  it('un-uploaded, selected: the same selected indicator as Sections, with the full-strength warning text', () => {
    unuploaded();
    const { unmount } = render(<LeftWorkspace sections={<div />} />);
    const sectionsSelected = look(screen.getByRole('tab', { name: 'Sections' }));
    unmount();
    useUIStore.setState({ leftWorkspace: 'speleodb', surveyLeftWorkspace: 'speleodb' });
    render(<LeftWorkspace sections={<div />} />);
    expect(speleoTab()).toHaveAttribute('aria-selected', 'true');
    expect(look(speleoTab())).toEqual([...sectionsSelected, 'text-warning-text'].sort());
    expect(speleoTab().className).not.toMatch(/bg-warning|shadow|inset|text-warning-text-dim/);
    // Full-strength yellow on the mark too, and still no badge.
    expectIconInTabColour();
    expect(badge()).toBeNull();
  });

  it.each(['selected', 'not selected'] as const)('no warning, %s: exactly the Sections tab look', (which) => {
    useSpeleoStore.setState({ link: { ...link, editState: 'editing' } });
    if (which === 'selected') useUIStore.setState({ leftWorkspace: 'speleodb', surveyLeftWorkspace: 'speleodb' });
    render(<LeftWorkspace sections={<div />} />);
    const sections = screen.getByRole('tab', { name: 'Sections' });
    expect(look(speleoTab())).toEqual(which === 'selected' ? ['bg-accent-10', 'font-medium'] : ['text-text-secondary']);
    expect(look(sections)).toEqual(which === 'selected' ? ['text-text-secondary'] : ['bg-accent-10', 'font-medium']);
  });
});
