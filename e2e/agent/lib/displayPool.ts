/**
 * The Xvfb display pool a session may use, and the rules for narrowing it.
 *
 * Lives here rather than in `session.ts` because it is pure logic with no
 * WebDriver in it: this is the directory `vite.config.ts` collects unit tests
 * from, and a pool rule that decides whether a fuzz run lands on the user's
 * desktop deserves tests.
 */

/**
 * Displays `startXvfb` tries, in order, taking the first that comes up.
 *
 * Five, because the parallel runner caps its workers at that: each worker gets
 * a pool of exactly ONE display keyed off its slot index, which is what makes
 * concurrent sessions safe. The default first-that-starts walk relies on Xvfb
 * losing the O_EXCL race on an already-locked display and exiting inside the
 * 250 ms probe window, so two sessions launched together can otherwise both
 * believe they own `:89`.
 */
export const DEFAULT_XVFB_DISPLAYS = [':89', ':88', ':87', ':86', ':85'];

/**
 * Parse `STALAGMIGHT_AGENT_DISPLAYS`, rejecting anything that would silently
 * shrink the pool.
 *
 * Narrowing spends every retry the default walk would have had - that is the
 * point of it - so the caller has no fallback left and an entry that is not a
 * display, or a value that filters down to nothing, must stop the session
 * rather than reach `startXvfb` as a shorter list than the caller thinks it
 * passed.
 */
export function parseDisplayPool(raw: string | undefined): string[] {
  if (raw === undefined) return DEFAULT_XVFB_DISPLAYS;
  const entries = raw.split(',').map((entry) => entry.trim()).filter(Boolean);
  if (entries.length === 0) {
    throw new Error(`STALAGMIGHT_AGENT_DISPLAYS is set but empty ("${raw}"); unset it to use the default pool`);
  }
  const malformed = entries.filter((entry) => !/^:\d+$/.test(entry));
  if (malformed.length > 0) {
    // The entries are the user's own text, so the line a runner prints counts
    // them and the next line, which it withholds, names them (#576 W51).
    throw new Error(
      `STALAGMIGHT_AGENT_DISPLAYS has ${malformed.length} malformed display(s); ` +
        `entries must look like ":87"\n  malformed: ${JSON.stringify(malformed)}`,
    );
  }
  return entries;
}
