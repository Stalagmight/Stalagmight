//! [#576 FZ-DET] The one place a shot's fresh identity is minted.
//!
//! A shipped build mints a random v4 uuid, always. A test build
//! (`test-support`) can arm a SEEDED source on one thread with
//! [`seed_on_this_thread`]: the determinism harness
//! (core/state/determinism_tests.rs) replays one scenario twice and needs both
//! replays to mint the same identities, or every output ordered by a shot's
//! uuid - a station key after a shot is - would differ between them by
//! construction rather than by a defect.

/// A fresh shot identity: a v4 uuid.
pub fn mint() -> String {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(seeded) = seeded::next() {
        return seeded;
    }
    uuid::Uuid::new_v4().to_string()
}

#[cfg(any(test, feature = "test-support"))]
pub use seeded::{seed_on_this_thread, SeededUuids};

#[cfg(any(test, feature = "test-support"))]
mod seeded {
    use std::cell::Cell;

    thread_local! {
        /// `(seed, minted so far)` while armed on this thread.
        static SOURCE: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
    }

    /// splitmix64: a well-mixed 64-bit step, so seeded uuids sort in no
    /// particular order, as random ones do.
    const fn mix(mut z: u64) -> u64 {
        z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub(super) fn next() -> Option<String> {
        SOURCE.with(|source| {
            let (seed, count) = source.get()?;
            source.set(Some((seed, count + 1)));
            let high = mix(seed ^ mix(count));
            let low = mix(high ^ count);
            let bytes = ((u128::from(high) << 64) | u128::from(low)).to_be_bytes();
            Some(
                uuid::Builder::from_random_bytes(bytes)
                    .into_uuid()
                    .to_string(),
            )
        })
    }

    /// Guard for [`seed_on_this_thread`]: dropping it restores random minting.
    pub struct SeededUuids {
        previous: Option<(u64, u64)>,
    }

    impl Drop for SeededUuids {
        fn drop(&mut self) {
            SOURCE.with(|source| source.set(self.previous));
        }
    }

    /// From now until the guard drops, every uuid minted ON THIS THREAD comes
    /// from `seed`: the same sequence on every thread armed with it.
    #[must_use = "minting goes back to random when the guard drops"]
    pub fn seed_on_this_thread(seed: u64) -> SeededUuids {
        SeededUuids {
            previous: SOURCE.with(|source| source.replace(Some((seed, 0)))),
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod tests {
    use super::{mint, seed_on_this_thread};

    #[test]
    fn a_seeded_thread_mints_one_sequence_and_unseeded_mints_v4() {
        let sequence = |seed| {
            let _seeded = seed_on_this_thread(seed);
            (0..4).map(|_| mint()).collect::<Vec<_>>()
        };
        assert_eq!(sequence(7), sequence(7));
        assert_ne!(sequence(7), sequence(8));
        for minted in sequence(7) {
            let parsed = uuid::Uuid::parse_str(&minted).unwrap();
            assert_eq!(parsed.get_version_num(), 4, "{minted} is shaped as a v4");
        }
        assert_ne!(
            mint(),
            mint(),
            "the guard dropped, so minting is random again"
        );
    }
}
