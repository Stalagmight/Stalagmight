/**
 * #576 Q-W27-1-D1: the busy indicator for an import whose file is still being
 * read. `routeOpenedFile` marks the parse in `importParse`; with no dialog up
 * nothing else says anything is happening (the Global View with no project open
 * looked untouched for the whole parse), so this does.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { act, render, screen } from '@testing-library/react';

// The real i18next instance, so the copy is the English string and this test
// fails if the key goes missing.
import '../../../i18n';
import { ImportParseIndicator } from '../ImportParseIndicator';
import { useUIStore } from '../../../store/uiStore';

beforeEach(() => {
  useUIStore.setState({ importParse: null, activeDialog: null, dialogData: {} });
});

describe('ImportParseIndicator', () => {
  it('names the file while its parse runs, and goes away when it settles', () => {
    render(<ImportParseIndicator />);
    expect(screen.queryByTestId('import-parse-indicator')).toBeNull();

    act(() => useUIStore.setState({ importParse: { path: '/cave/CaveA.dmp', token: 1 } }));
    const indicator = screen.getByTestId('import-parse-indicator');
    expect(indicator).toHaveTextContent('Reading CaveA.dmp...');
    expect(indicator).toHaveAttribute('role', 'status');

    act(() => useUIStore.setState({ importParse: null }));
    expect(screen.queryByTestId('import-parse-indicator')).toBeNull();
  });

  it('stays out of the way while a dialog owns the screen', () => {
    useUIStore.setState({
      importParse: { path: '/cave/CaveA.tml', token: 2 },
      activeDialog: 'import-as-new-project',
    });
    render(<ImportParseIndicator />);
    expect(screen.queryByTestId('import-parse-indicator')).toBeNull();
  });
});
