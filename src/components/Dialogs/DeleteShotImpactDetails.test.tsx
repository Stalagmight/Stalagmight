/**
 * Component test (#737): the range-delete preview names, BEFORE the user
 * confirms, the loop closures closing the gap would remove because they would
 * span a single shot - by the stations they were created with.
 */
import { beforeEach, describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import DeleteShotImpactDetails, { type ShotImpactRelationships } from './DeleteShotImpactDetails';
import { useProjectStore } from '../../store/projectStore';

// Names that differ from the positional indices: a label built from the
// handles (`0:1`) can never pass for one built from the names.
beforeEach(() => {
  useProjectStore.setState({
    project: { lines: [{ id: 0, name: 'Main Line', shots: [] }] },
    surveyData: null,
    stations: [
      { station_ref: { line_id: 0, station_index: 1 }, name: 'A1' },
      { station_ref: { line_id: 0, station_index: 5 }, name: 'A5' },
    ],
  } as never);
});

const none: ShotImpactRelationships = {
  removed_merges: [],
  removed_closures: [],
  closure_to_merge: [],
};

describe('DeleteShotImpactDetails - closures across one shot (#737)', () => {
  it('lists the closures the gap-close removes, by their pre-delete stations', () => {
    render(
      <DeleteShotImpactDetails
        impact={{
          ...none,
          closures_removed_across_one_shot: [
            {
              closure_id: 3n,
              station_a: { line_id: 0, station_index: 1 },
              station_b: { line_id: 0, station_index: 5 },
            },
          ],
        }}
      />
    );
    const block = screen.getByTestId('impact-closures-across-one-shot');
    // Names the button that removes them: Split removes nothing.
    expect(block).toHaveTextContent(
      'Close Gap & Delete removes these loop closures (they would span a single shot)'
    );
    // Full highlighting: line pills, the line as the context.
    expect(block).toHaveTextContent(/#3: Main Line: A1( Station ID \d+)? <-> Main Line: A5/);
    expect(block.textContent).not.toMatch(/\d+:\d+ /);
    // Not mixed up with the closures removed because a station goes.
    expect(screen.queryByText('Removed closures')).toBeNull();
  });

  it('names the action that removes them', () => {
    render(
      <DeleteShotImpactDetails
        gapCloseAction="Delete Shots"
        impact={{
          ...none,
          closures_removed_across_one_shot: [
            {
              closure_id: 4n,
              station_a: { line_id: 0, station_index: 2 },
              station_b: { line_id: 0, station_index: 6 },
            },
          ],
        }}
      />
    );
    expect(screen.getByTestId('impact-closures-across-one-shot')).toHaveTextContent(
      'Delete Shots removes these loop closures (they would span a single shot)'
    );
  });

  it('shows nothing for an empty or absent prediction', () => {
    const { container, rerender } = render(
      <DeleteShotImpactDetails impact={{ ...none, closures_removed_across_one_shot: [] }} />
    );
    expect(container).toBeEmptyDOMElement();
    rerender(<DeleteShotImpactDetails impact={none} />);
    expect(container).toBeEmptyDOMElement();
  });
});

describe('DeleteShotImpactDetails - the station graph (plan 5.4)', () => {
  it('says the line leaves a station it shared, and which closures join their stations', () => {
    render(
      <DeleteShotImpactDetails
        impact={{
          ...none,
          removed_merges: [{ station_a: { line_id: 0, station_index: 5 }, station_b: { line_id: 0, station_index: 1 } }],
          closure_to_merge: [{ station_a: { line_id: 0, station_index: 1 }, station_b: { line_id: 0, station_index: 5 } }],
        }}
      />
    );
    expect(screen.getByText('No longer shared with another line')).toBeInTheDocument();
    expect(screen.getByText('Loop closures that join their two stations into one')).toBeInTheDocument();
    // Nothing is re-wired on the station graph: no replacement rows exist.
    expect(screen.queryByText(/Replacement/)).toBeNull();
    expect(screen.queryByText(/could not be/)).toBeNull();
  });
});
