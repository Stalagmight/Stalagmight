/**
 * Stand-ins for the dialog frame and the common Button (#576 E15-NF1), for a
 * dialog test that is about the dialog's body, not its chrome (portal, focus
 * trap, dirty-close prompt). A file mocks the module with one:
 *
 *     vi.mock('../DialogWrapper', async () => (await import('<rel>/test/factories/components')).dialogWrapperStub());
 *     vi.mock('../../common/Button', async () => (await import('<rel>/test/factories/components')).buttonStub());
 *
 * `dialogWrapperStub` renders the children in a plain `<div>` - calling them
 * with a no-op `requestClose` when the dialog passes a render function - and,
 * as asked, the title and the dialog's `data-testid`:
 *
 * - `title: 'testid'`: the title in `<div data-testid="dialog-title">`;
 *   `title: 'h2'`: the title in an `<h2>`. Default: not rendered.
 * - `testId: true`: the dialog's `data-testid` on the wrapping `<div>`.
 * - `spy`: called on every render with every prop but `children`.
 *
 * A test that needs the real frame's behaviour (the dirty guard) keeps its own
 * stand-in.
 */
import type { ComponentProps, ReactNode } from 'react';

type DialogChildren = ReactNode | ((p: { requestClose: () => void }) => ReactNode);

interface DialogWrapperStubProps {
  children: DialogChildren;
  title?: ReactNode;
  'data-testid'?: string;
  [prop: string]: unknown;
}

export interface DialogWrapperStubOptions {
  /** Where the title prop is rendered; not at all by default. */
  title?: 'testid' | 'h2';
  /** Put the dialog's `data-testid` on the wrapping `<div>`. */
  testId?: boolean;
  /** Called on every render with every prop but `children`. */
  spy?: (props: Record<string, unknown>) => void;
}

/** A `DialogWrapper` module whose frame is a plain `<div>`. */
export function dialogWrapperStub(options: DialogWrapperStubOptions = {}) {
  return {
    DialogWrapper: ({ children, ...props }: DialogWrapperStubProps) => {
      options.spy?.(props);
      return (
        <div data-testid={options.testId ? props['data-testid'] : undefined}>
          {options.title === 'testid' && <div data-testid="dialog-title">{props.title}</div>}
          {options.title === 'h2' && <h2>{props.title}</h2>}
          {typeof children === 'function' ? children({ requestClose: () => {} }) : children}
        </div>
      );
    },
  };
}

/** A `common/Button` module whose `Button` is a plain `<button>` with the props passed through. */
export function buttonStub() {
  return {
    Button: ({ children, ...rest }: ComponentProps<'button'>) => <button {...rest}>{children}</button>,
  };
}
