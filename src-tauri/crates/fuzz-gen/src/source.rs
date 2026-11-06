//! [`ByteSource`]: the one source of every choice a generator makes.
//!
//! A generator never calls a random number generator. It asks a `ByteSource`
//! for its next decision, so the same bytes always render the same file. Under
//! libFuzzer the bytes are the fuzzer's input (coverage-guided mutation of the
//! decisions); in the stable fixed-seed tests they are [`seed_bytes`] of a
//! numbered seed. A source that runs dry answers zero to everything, which every
//! generator treats as "the smallest valid choice", so a short input still
//! renders a small, well-formed file.

/// Decision stream over a byte slice. Exhaustion is not an error: every read
/// past the end yields zero.
#[derive(Debug, Clone)]
pub struct ByteSource<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteSource<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// True once every byte has been consumed.
    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// Bytes left to consume.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn u8(&mut self) -> u8 {
        let b = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos = self.pos.saturating_add(1);
        b
    }

    pub fn u16(&mut self) -> u16 {
        u16::from_le_bytes([self.u8(), self.u8()])
    }

    pub fn u32(&mut self) -> u32 {
        u32::from_le_bytes([self.u8(), self.u8(), self.u8(), self.u8()])
    }

    pub fn u64(&mut self) -> u64 {
        u64::from(self.u32()) | (u64::from(self.u32()) << 32)
    }

    pub fn bool(&mut self) -> bool {
        self.u8() & 1 == 1
    }

    /// True with probability `num`/256 (so `chance(0)` is never, and a dry
    /// source never takes the rare branch).
    pub fn chance(&mut self, num: u8) -> bool {
        self.u8() > u8::MAX - num
    }

    /// Uniform-ish in `0..n`; 0 when `n` is 0.
    pub fn below(&mut self, n: usize) -> usize {
        if n <= 1 {
            return 0;
        }
        if n <= 256 {
            usize::from(self.u8()) % n
        } else if n <= 65_536 {
            usize::from(self.u16()) % n
        } else {
            (self.u32() as usize) % n
        }
    }

    /// Inclusive `lo..=hi` (returns `lo` when the range is empty).
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        let span = (hi - lo).unsigned_abs().saturating_add(1);
        let pick = if span <= 256 {
            u64::from(self.u8()) % span
        } else if span <= 65_536 {
            u64::from(self.u16()) % span
        } else {
            self.u64() % span
        };
        lo.saturating_add(pick as i64)
    }

    /// One element of a non-empty slice (the first one when the source is dry).
    pub fn pick<'b, T>(&mut self, items: &'b [T]) -> Option<&'b T> {
        let i = self.below(items.len());
        items.get(i)
    }

    /// A copyable element of a non-empty slice, or `fallback` for an empty one.
    pub fn pick_or<T: Copy>(&mut self, items: &[T], fallback: T) -> T {
        self.pick(items).copied().unwrap_or(fallback)
    }

    /// A float in `[lo, hi]` with 1/65535 resolution.
    pub fn f64_in(&mut self, lo: f64, hi: f64) -> f64 {
        let t = f64::from(self.u16()) / f64::from(u16::MAX);
        lo + (hi - lo) * t
    }

    /// Up to `max` raw bytes (fewer when the source runs dry).
    pub fn bytes(&mut self, max: usize) -> &'a [u8] {
        let want = self.below(max.saturating_add(1));
        let start = self.pos.min(self.data.len());
        let end = start.saturating_add(want).min(self.data.len());
        self.pos = self.pos.saturating_add(want);
        self.data.get(start..end).unwrap_or(&[])
    }

    /// A short ASCII identifier from `[A-Za-z0-9]`, at least one character.
    pub fn ident(&mut self, max_len: usize) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let len = 1 + self.below(max_len.max(1));
        (0..len)
            .map(|_| char::from(self.pick_or(ALPHABET, b'A')))
            .collect()
    }

    /// Free text that is mostly printable but sometimes carries the characters
    /// parsers trip on: separators, quotes, markup, control bytes, non-ASCII.
    pub fn text(&mut self, max_len: usize) -> String {
        const NASTY: &[&str] = &[
            ";",
            ",",
            "\"",
            "'",
            "<",
            ">",
            "&",
            "&amp;",
            "]]>",
            "\t",
            "\r",
            "\n",
            "\\",
            "#",
            "$",
            "%",
            "{",
            "}",
            "(",
            ")",
            "=",
            "|",
            "\u{0}",
            "\u{7f}",
            "\u{a0}",
            "\u{e9}",
            "\u{20ac}",
            "\u{fffd}",
            "\u{1f600}",
            "..",
            "/",
        ];
        let len = self.below(max_len.saturating_add(1));
        let mut out = String::new();
        for _ in 0..len {
            if self.chance(24) {
                out.push_str(self.pick_or(NASTY, ";"));
            } else {
                out.push(char::from(b' ' + (self.u8() % 95)));
            }
        }
        out
    }

    /// A float drawn mostly from `[lo, hi]` but sometimes from the values
    /// numeric parsing and unit scaling get wrong: zero, negative zero, the
    /// edges of the range, huge and tiny magnitudes, and non-finite values.
    pub fn number(&mut self, lo: f64, hi: f64) -> f64 {
        if !self.chance(20) {
            return self.f64_in(lo, hi);
        }
        const EDGES: &[f64] = &[
            0.0,
            -0.0,
            -1.0,
            1e-9,
            -1e-9,
            1e9,
            -1e9,
            f64::MAX / 3.0,
            -f64::MAX / 3.0,
            f64::MIN_POSITIVE,
            f64::MAX,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            360.0,
            -360.0,
            720.5,
            90.0,
            -90.0,
            9999.0,
            -9999.0,
            999.0,
            -999.0,
        ];
        match self.below(3) {
            0 => lo,
            1 => hi,
            _ => self.pick_or(EDGES, 0.0),
        }
    }
}

/// A deterministic byte buffer for seed `seed` (splitmix64), so a fixed-seed
/// test and a farm run with a recorded seed regenerate the exact same input.
pub fn seed_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed ^ 0x5DEE_CE66_D1CE_4E5B;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        out.extend_from_slice(&z.to_le_bytes());
    }
    out.truncate(len);
    out
}

#[cfg(test)]
mod tests {
    use super::{seed_bytes, ByteSource};

    #[test]
    fn dry_source_answers_zero_and_smallest_choices() {
        let mut s = ByteSource::new(&[]);
        assert_eq!(s.u32(), 0);
        assert_eq!(s.below(10), 0);
        assert_eq!(s.range(3, 9), 3);
        assert_eq!(s.ident(5), "A");
        assert!(!s.chance(255));
        assert!((s.f64_in(2.0, 4.0) - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn seed_bytes_are_deterministic_and_seed_dependent() {
        assert_eq!(seed_bytes(7, 64), seed_bytes(7, 64));
        assert_ne!(seed_bytes(7, 64), seed_bytes(8, 64));
        assert_eq!(seed_bytes(1, 13).len(), 13);
    }
}
