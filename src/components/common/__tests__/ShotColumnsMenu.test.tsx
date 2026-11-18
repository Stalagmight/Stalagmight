import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';
import { ShotColumnsMenu } from '../ShotColumnsMenu';

/**
 * The eye menu is shared by the data table, the new-line dialog and the shot
 * editor, and the whole point of sharing it is that a row cannot exist in one and
 * not another by accident. These pin the parts a host is allowed to vary - which
 * optional rows appear, which angle families get a mode switch, which rows go red
 * - and the parts it is not: the row set and its grouping.
 */

const open = () => fireEvent.click(screen.getByTestId('eye'));

const renderMenu = (props: Partial<React.ComponentProps<typeof ShotColumnsMenu>> = {}) =>
  render(
    <ShotColumnsMenu
      hidden={new Set()}
      onToggle={vi.fn()}
      buttonTestId="eye"
      dropdownTestId="drop"
      {...props}
    />
  );

describe('ShotColumnsMenu', () => {
  it('always renders the shot rows, with LRUD as one grouped toggle', () => {
    renderMenu();
    open();
    for (const key of ['length', 'depth_in', 'depth_out', 'kind', 'comment']) {
      expect(screen.getByTestId(`col-vis-${key}`)).toBeInTheDocument();
    }
    // Four columns, one checkbox, keyed by the first of the group.
    expect(screen.getByTestId('col-vis-left')).toBeInTheDocument();
    expect(screen.queryByTestId('col-vis-right')).toBeNull();
  });

  it('omits the optional rows unless the host declares the column', () => {
    renderMenu();
    open();
    expect(screen.queryByTestId('col-vis-_warnings')).toBeNull();
    expect(screen.queryByTestId('col-vis-line_id')).toBeNull();
    expect(screen.queryByTestId('col-vis-shot_id')).toBeNull();
  });

  it('renders each optional row the host does declare', () => {
    renderMenu({ extraRows: { warnings: true, lineId: true, shotId: true } });
    open();
    expect(screen.getByTestId('col-vis-_warnings')).toBeInTheDocument();
    expect(screen.getByTestId('col-vis-line_id')).toBeInTheDocument();
    expect(screen.getByTestId('col-vis-shot_id')).toBeInTheDocument();
  });

  it('shows an angle switch only for the families the host passes', () => {
    renderMenu({ angleModes: { azimuth: { mode: 'avg', onPick: vi.fn() } } });
    open();
    expect(screen.getByTestId('col-azimuth-avg')).toBeInTheDocument();
    expect(screen.queryByTestId('col-inclination-avg')).toBeNull();
  });

  it('reports the picked angle mode', () => {
    const onPick = vi.fn();
    renderMenu({ angleModes: { inclination: { mode: 'off', onPick } } });
    open();
    fireEvent.click(screen.getByTestId('col-inclination-inout'));
    expect(onPick).toHaveBeenCalledWith('inout');
  });

  it('checks a grouped row only when every key is visible', () => {
    renderMenu({ hidden: new Set(['up']) });
    open();
    expect(screen.getByTestId<HTMLInputElement>('col-vis-left').checked).toBe(false);
    expect(screen.getByTestId<HTMLInputElement>('col-vis-length').checked).toBe(true);
  });

  it('toggles every key of a group together', () => {
    const onToggle = vi.fn();
    renderMenu({ onToggle });
    open();
    fireEvent.click(screen.getByTestId('col-vis-left'));
    expect(onToggle).toHaveBeenCalledWith(['left', 'right', 'up', 'down']);
  });

  it('reds the button and the row when a hidden column holds a missing value', () => {
    renderMenu({ problems: new Set(['length']) });
    expect(screen.getByTestId('eye').className).toContain('text-error');
    open();
    expect(screen.getByTestId('col-vis-length').closest('label')?.className).toContain(
      'text-error'
    );
    expect(screen.getByTestId('col-vis-comment').closest('label')?.className).not.toContain(
      'text-error'
    );
  });

  it('prefixes every test ID for a host that needs its own namespace', () => {
    renderMenu({
      testIdPrefix: 'shot-editor-',
      angleModes: { azimuth: { mode: 'avg', onPick: vi.fn() } },
    });
    open();
    expect(screen.getByTestId('shot-editor-col-vis-length')).toBeInTheDocument();
    expect(screen.getByTestId('shot-editor-col-azimuth-avg')).toBeInTheDocument();
    expect(screen.queryByTestId('col-vis-length')).toBeNull();
  });

  it('replaces the shot rows entirely when the host supplies its own', () => {
    renderMenu({ rowsOverride: [{ keys: ['_x', '_y'], label: 'Coordinates' }] });
    open();
    expect(screen.getByTestId('col-vis-_x')).toBeInTheDocument();
    expect(screen.queryByTestId('col-vis-length')).toBeNull();
  });
});
