/**
 * A stand-in for `@tanstack/react-virtual` (#576 E15): jsdom has no layout, so
 * the real virtualizer measures a zero-height scroller and renders no rows. This
 * one renders every row, at a fixed 33 px pitch below a 33 px header, so a grid
 * test can find each row by test id. A file mocks the module with it:
 *
 *     vi.mock('@tanstack/react-virtual', () => import('<rel>/test/factories/virtualizer'));
 *
 * or, for a layout the default does not match, builds one with `fakeVirtualizer`:
 *
 *     vi.mock('@tanstack/react-virtual', async () =>
 *       (await import('<rel>/test/factories/virtualizer')).fakeVirtualizer({ headerOffset: 0 }));
 */

/** Row height (px) the fake lays rows out at. */
export const FAKE_ROW_HEIGHT = 33;

export interface FakeVirtualizerOptions {
  /** Where the first row starts (px): the header row's height. Default 33. */
  headerOffset?: number;
  /** Render the rows (default) or none at all (`getVirtualItems()` is empty). */
  renderRows?: boolean;
}

/** A `@tanstack/react-virtual` module whose `useVirtualizer` lays out every row. */
export function fakeVirtualizer(options: FakeVirtualizerOptions = {}) {
  const { headerOffset = FAKE_ROW_HEIGHT, renderRows = true } = options;
  return {
    useVirtualizer: ({ count }: { count: number }) => ({
      getVirtualItems: () =>
        renderRows
          ? Array.from({ length: count }, (_, index) => ({
              index,
              key: index,
              start: headerOffset + index * FAKE_ROW_HEIGHT,
              size: FAKE_ROW_HEIGHT,
            }))
          : [],
      getTotalSize: () => count * FAKE_ROW_HEIGHT,
      scrollToIndex: () => {},
    }),
  };
}

/** The default layout, so the module itself can stand in for `@tanstack/react-virtual`. */
export const { useVirtualizer } = fakeVirtualizer();
