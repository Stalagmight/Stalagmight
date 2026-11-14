/**
 * Component test (#737): the range-delete preview names, BEFORE the user
 * confirms, the loop closures closing the gap would remove because they would
 * span a single shot - by the stations they were created with.
 */
import { describe, it, expect } from 'vitest';
import { render, screen, within } from '@testing-library/react';
import DeleteShotImpactDetails, { type ShotImpactRelationships } from './DeleteShotImpactDetails';

const none: ShotImpactRelationships = {
  removed_merges: [],
  removed_closures: [],
  replacement_merges: [],
  replacement_closure_updates: [],
  closure_to_merge: [],
  merge_rewire_skipped: false,
  closure_rewire_skipped: false,
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
              station_a: { section_id: 0, station_index: 1 },
              station_b: { section_id: 0, station_index: 5 },
            },
          ],
        }}
      />
    );
    const block = screen.getByTestId('impact-closures-across-one-shot');
    expect(within(block).getByText('Loop closures removed (they would span a single shot)')).toBeInTheDocument();
    expect(block).toHaveTextContent('#3: 0:1 <-> 0:5');
    // Not mixed up with the closures removed because a station goes.
    expect(screen.queryByText('Removed closures')).toBeNull();
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
