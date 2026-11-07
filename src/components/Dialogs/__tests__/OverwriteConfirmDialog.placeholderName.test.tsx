/**
 * [#576 RV6b-F4] A file name is shown exactly as it is, even when it contains the
 * message's own placeholder.
 *
 * The overwrite prompt passed the name to `<Trans>` as the `{{fileName}}` value.
 * Trans interpolates the translated string, then interpolates each text node of
 * it again with the same values, so a name that itself contained "{{fileName}}"
 * was substituted twice: "{{fileName}}.stalx" rendered "{{fileName}}.stalx.stalx".
 * The name is now the `<name>` component's own content, which no interpolation
 * reaches.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

import OverwriteConfirmDialog from '../OverwriteConfirmDialog';
import { useUIStore } from '../../../store/uiStore';

beforeEach(() => {
  useUIStore.setState({ activeDialog: 'confirm-overwrite' });
});

describe('the overwrite prompt names the file as typed (#576 RV6b-F4)', () => {
  it.each([
    ['{{fileName}}.stalx', 'overwrite-confirm-message', false],
    ['Cave {{ fileName }}.stalx', 'overwrite-confirm-message', false],
    ['{{fileName}}.stalx', 'overwrite-confirm-unknown', true],
    ['CaveA <B>.stalx', 'overwrite-confirm-message', false],
  ] as const)('%s (%s)', (fileName, testId, existenceUnknown) => {
    useUIStore.setState({ dialogData: { path: `/caves/${fileName}`, fileName, existenceUnknown } });
    render(<OverwriteConfirmDialog />);
    const text = screen.getByTestId(testId).textContent ?? '';
    console.log(`[RV6b-F4] name ${JSON.stringify(fileName)} renders as: ${JSON.stringify(text)}`);
    expect(text).toContain(fileName);
    expect(text.split(fileName).length - 1).toBe(1);
    expect(text).not.toContain('.stalx.stalx');
  });
});
