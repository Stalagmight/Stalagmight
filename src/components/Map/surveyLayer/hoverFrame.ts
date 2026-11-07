/**
 * Per-frame coalescing for map hover hit-testing.
 *
 * A mousemove only queues its event; one animation frame later the LATEST queued
 * event is hit-tested, so a burst of moves costs one hit-test per frame instead
 * of a backlog of stale ones (the hit-test reprojects candidate sections).
 *
 * The queued frame is a promise about where the pointer IS, so whatever ends the
 * hover has to take it back with {@link HoverFrame.cancel}: the pointer leaving
 * the canvas, or the layer going away. Otherwise a move dispatched just before
 * the leave - a fast flick off the canvas edge across a station, which WebKit
 * delivers as move + leave inside one frame - is hit-tested AFTER the leave
 * cleared the hover and re-applies the highlight (and its cross-highlight in the
 * panels) with the pointer already gone, until it returns (#576 D39(h)).
 */
export class HoverFrame<E> {
  private pending: E | null = null;
  private frame: number | null = null;

  /**
   * `schedule` / `unschedule` default to the window's animation-frame pair,
   * looked up at call time.
   */
  constructor(
    private readonly run: (event: E) => void,
    private readonly schedule: (cb: () => void) => number = (cb) => requestAnimationFrame(cb),
    private readonly unschedule: (id: number) => void = (id) => cancelAnimationFrame(id),
  ) {}

  /** Queue `event` for the next frame, replacing any event already queued. */
  push(event: E): void {
    this.pending = event;
    if (this.frame !== null) return;
    this.frame = this.schedule(() => {
      this.frame = null;
      const queued = this.pending;
      this.pending = null;
      if (queued !== null) this.run(queued);
    });
  }

  /** Drop the queued event and its frame. Safe when nothing is queued. */
  cancel(): void {
    if (this.frame !== null) this.unschedule(this.frame);
    this.frame = null;
    this.pending = null;
  }
}
