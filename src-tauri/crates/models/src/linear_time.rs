//! A complexity check for the tests that pin a pass as linear (RS-FX10).
//!
//! A wall-clock bound such as "under 10 s" proves little and flakes: CI runs
//! debug builds on shared runners several times slower than a workstation,
//! with other tests on the same cores. [`assert_linear`] times one shape at
//! two sizes on the same machine in the same run and compares them, so the
//! runner's speed cancels out: a linear pass takes about [`SCALE`] times as
//! long at the larger size, a quadratic one about `SCALE` squared. A generous
//! absolute [`CEILING`] stays as the backstop against a hang.
//!
//! The callers pick sizes at which the quadratic forms they were written
//! against already dominate at the smaller size, so each of those would show
//! well over [`MAX_RATIO`]; each test's doc says what it pins.

use std::time::{Duration, Instant};

/// How many times larger the second size is.
pub const SCALE: usize = 4;

/// The largest time ratio between the two sizes a linear pass may show: it
/// takes about [`SCALE`] (4x), a quadratic one about 16x. 8x leaves a factor
/// of two for noise, cache and allocator effects on either side.
pub const MAX_RATIO: u32 = 8;

/// The backstop for the whole check (inputs built, every run): several
/// minutes, for the slowest runner; never the bound the test is about.
pub const CEILING: Duration = Duration::from_mins(5);

/// Runs per size in one round. Noise only ever adds time, so each size's
/// fastest run counts.
pub const RUNS: usize = 3;

/// Rounds at most: a ratio over [`MAX_RATIO`] after a round gets another,
/// since a busy stretch of a shared runner can slow the larger size more
/// than the smaller (it reaches past the caches the smaller fits in); a
/// quadratic pass stays over the bound however often it runs.
pub const ROUNDS: usize = 2;

/// A smaller time is not measured finer than this: a run that short is
/// mostly timer and scheduler noise, and its ratio means nothing.
pub const FLOOR: Duration = Duration::from_millis(10);

/// Assert that the pass `run` times scales linearly: `build(n)` and
/// `build(SCALE * n)` make the inputs, untimed, and `run` returns the time
/// of its own pass over one (it clones or prepares what it needs before
/// starting its clock, and checks its result after stopping it). The runs
/// alternate between the sizes, so a busy stretch slows both, and each
/// size's fastest run counts, over up to [`ROUNDS`] rounds of [`RUNS`].
/// Fails, naming `what` and the times, when the larger size still takes
/// more than [`MAX_RATIO`] times the smaller (at least [`FLOOR`]), or when
/// the whole check took [`CEILING`] or more.
pub fn assert_linear<T>(
    what: &str,
    n: usize,
    build: impl Fn(usize) -> T,
    mut run: impl FnMut(&T) -> Duration,
) {
    let started = Instant::now();
    let inputs = [build(n), build(SCALE * n)];
    let mut best = [Duration::MAX; 2];
    let linear = |[small, large]: [Duration; 2]| large <= small.max(FLOOR) * MAX_RATIO;
    for _ in 0..ROUNDS {
        for _ in 0..RUNS {
            for (input, best) in inputs.iter().zip(&mut best) {
                *best = (*best).min(run(input));
            }
        }
        if linear(best) {
            break;
        }
    }
    let [small, large] = best;
    let total = started.elapsed();
    let ratio = large.as_secs_f64() / small.max(FLOOR).as_secs_f64();
    assert!(
        linear(best),
        "{what}: {SCALE}x the input took {ratio:.1}x the time ({small:?} at {n}, {large:?} at {}); \
         a linear pass takes about {SCALE}x",
        SCALE * n
    );
    assert!(
        total < CEILING,
        "{what}: {total:?} in all, past the {CEILING:?} backstop"
    );
}
