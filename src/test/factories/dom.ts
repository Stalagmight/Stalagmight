/**
 * Stand-ins for browser APIs jsdom lacks (#576 E15).
 */

/**
 * A ResizeObserver that never fires: for a component that measures itself with
 * one but whose test does not drive a resize (`vi.stubGlobal('ResizeObserver',
 * NoopResizeObserver)`). A test that fires resizes keeps its own observer.
 */
export class NoopResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
