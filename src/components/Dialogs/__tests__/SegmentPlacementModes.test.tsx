/**
 * #567 item 3: the placement-mode card list gets its own bounded scroll
 * region so the "Place on Map" action stays reachable on short windows
 * without scrolling past every card. The load-bearing structural fact is
 * that the action row is a DOM sibling of the scroll region, not a
 * descendant of it.
 */
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { SegmentPlacementModes, type SegmentPlacementModesProps } from '../SegmentPlacementModes';

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys());

const baseProps = (overrides: Partial<SegmentPlacementModesProps> = {}): SegmentPlacementModesProps => ({
  segmentSkippedSuggestion: null,
  placementMode: 'manual',
  onPlacementModeChange: vi.fn(),
  currentSegment: [0],
  segmentAnchor: { sectionIndex: 0, stationIndex: 0 },
  onCommitSegmentAnchor: vi.fn(),
  sectionEdits: new Map(),
  dmpSections: [],
  onAnchorSectionHover: vi.fn(),
  onAnchorStationHover: vi.fn(),
  projectSections: [],
  fromExistingSectionId: null,
  onFromExistingSectionChange: vi.fn(),
  fromExistingStationIdx: 0,
  onFromExistingStationChange: vi.fn(),
  fromExistingStationCount: 0,
  onPlaceOnMap: vi.fn(),
  ...overrides,
});

describe('SegmentPlacementModes', () => {
  it('renders the Place on Map action row outside the scrolling card list', () => {
    // Injection: move the Button back inside the manual card's expanded
    // content (its old location) -> the action row's closest scroll-region
    // ancestor is no longer null and this goes red.
    render(<SegmentPlacementModes {...baseProps()} />);

    const actionRow = screen.getByTestId('segment-manual-action-row');
    const scrollRegion = screen.getByTestId('segment-placement-modes-scroll');
    expect(scrollRegion.contains(actionRow)).toBe(false);

    // And the group's own container is the actual scroll ancestor.
    const group = screen.getByTestId('segment-placement-modes');
    expect(group.contains(scrollRegion)).toBe(true);
    expect(group.contains(actionRow)).toBe(true);
  });

  it('does not render the action row when manual is not the selected mode', () => {
    // Injection: drop the `placementMode === 'manual'` guard around the
    // action row -> it renders unconditionally and this goes red.
    render(<SegmentPlacementModes {...baseProps({ placementMode: 'discard' })} />);

    expect(screen.queryByTestId('segment-manual-action-row')).not.toBeInTheDocument();
  });

  it('the scroll region is bounded so it can engage independently of the page scroller', () => {
    // Injection: drop the max-height class (or overflow-y-auto) from the
    // scroll wrapper -> neither class is present and this goes red.
    render(<SegmentPlacementModes {...baseProps()} />);

    const scrollRegion = screen.getByTestId('segment-placement-modes-scroll');
    expect(scrollRegion.className).toMatch(/max-h-\[/);
    expect(scrollRegion.className).toContain('overflow-y-auto');
  });
});
