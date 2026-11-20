//! Ordered lists (datums, placements, closures, Sections, import hashes, the
//! line order) as hunks: where the two states differ, each hunk holds the
//! stretch it replaces and the stretch it puts in, so a step either way
//! writes only those.
//!
//! The diff is Myers' O((N + M) D) over the stretch between the common
//! prefix and suffix, with `D` capped; past the cap the stretch is one hunk.

use serde::{Deserialize, Serialize};

use super::bits::Same;
use super::Direction;

/// One replaced stretch: at `at` in the BEFORE list, `before` gives way to
/// `after`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Hunk<T>(pub(crate) u32, pub(crate) Vec<T>, pub(crate) Vec<T>);

/// How a list changed: the lengths of both states and the hunks, in order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ListChange<T> {
    /// `[before length, after length]`.
    pub(crate) n: [u32; 2],
    pub(crate) h: Vec<Hunk<T>>,
}

/// The most edit steps the diff looks for before it settles for one hunk.
const MAX_EDIT_STEPS: usize = 512;

impl<T: Same + Clone> ListChange<T> {
    /// The change from `before` to `after`, or `None` when they are the same.
    pub(crate) fn between(before: &[T], after: &[T]) -> Option<Self> {
        if before.same(after) {
            return None;
        }
        let prefix = before
            .iter()
            .zip(after)
            .take_while(|(a, b)| a.same(b))
            .count();
        let suffix = before[prefix..]
            .iter()
            .rev()
            .zip(after[prefix..].iter().rev())
            .take_while(|(a, b)| a.same(b))
            .count();
        let (b, a) = (
            &before[prefix..before.len() - suffix],
            &after[prefix..after.len() - suffix],
        );
        let mut hunks = Vec::new();
        match myers(b, a) {
            Some(script) => {
                for (bi, bn, ai, an) in script {
                    hunks.push(Hunk(
                        (prefix + bi) as u32,
                        b[bi..bi + bn].to_vec(),
                        a[ai..ai + an].to_vec(),
                    ));
                }
            }
            None => hunks.push(Hunk(prefix as u32, b.to_vec(), a.to_vec())),
        }
        Some(Self {
            n: [before.len() as u32, after.len() as u32],
            h: hunks,
        })
    }

    /// The list `live` becomes when it steps `direction`, or `None` when
    /// `live` is not the state the change starts from (its length, or a
    /// stretch a hunk replaces, differs from the record).
    pub(crate) fn step(&self, live: &[T], direction: Direction) -> Option<Vec<T>> {
        let (from_len, to_len) = match direction {
            Direction::Redo => (self.n[0], self.n[1]),
            Direction::Undo => (self.n[1], self.n[0]),
        };
        if live.len() != from_len as usize {
            return None;
        }
        let mut out = Vec::with_capacity(to_len as usize);
        let mut read = 0usize;
        let mut shift: i64 = 0;
        for Hunk(at, before, after) in &self.h {
            let (from, to) = match direction {
                Direction::Redo => (before, after),
                Direction::Undo => (after, before),
            };
            // `at` is a BEFORE index; stepping back from the after list,
            // earlier hunks have shifted it.
            let start = match direction {
                Direction::Redo => i64::from(*at),
                Direction::Undo => i64::from(*at) + shift,
            };
            shift += after.len() as i64 - before.len() as i64;
            let start = usize::try_from(start).ok()?;
            let end = start.checked_add(from.len())?;
            if start < read || end > live.len() || !live[start..end].same(from.as_slice()) {
                return None;
            }
            out.extend_from_slice(&live[read..start]);
            out.extend(to.iter().cloned());
            read = end;
        }
        out.extend_from_slice(&live[read..]);
        (out.len() == to_len as usize).then_some(out)
    }
}

impl<T> ListChange<T> {
    /// The structural rules a recorded change keeps (hostile input): hunks in
    /// order and apart, inside the before list, and the two lengths
    /// consistent with them.
    pub(crate) fn shape_ok(&self) -> bool {
        let mut end = 0u64;
        let mut after_len = i64::from(self.n[0]);
        for Hunk(at, before, after) in &self.h {
            let at = u64::from(*at);
            if at < end || (before.is_empty() && after.is_empty()) {
                return false;
            }
            end = at + before.len() as u64;
            after_len += after.len() as i64 - before.len() as i64;
        }
        !self.h.is_empty() && end <= u64::from(self.n[0]) && after_len == i64::from(self.n[1])
    }

    /// Every element the AFTER (`after == true`) or BEFORE list gains over
    /// the other, hunk by hunk.
    pub(crate) fn side(&self, after: bool) -> impl Iterator<Item = &T> + '_ {
        self.h
            .iter()
            .flat_map(move |h| if after { h.2.iter() } else { h.1.iter() })
    }
}

/// Myers' shortest edit script between `a` and `b` as replace hunks
/// `(a_at, a_len, b_at, b_len)`, or `None` past [`MAX_EDIT_STEPS`].
fn myers<T: Same>(a: &[T], b: &[T]) -> Option<Vec<(usize, usize, usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = (a.len() + b.len()).min(MAX_EDIT_STEPS) as isize;
    let offset = max + 1;
    let at = |k: isize| (k + offset) as usize;
    let mut v = vec![0isize; (2 * max + 3) as usize];
    // trace[d]: the frontier as round d found it (after round d - 1).
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut end = None;
    'rounds: for d in 0..=max {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize].same(&b[y as usize]) {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                end = Some(d);
                break 'rounds;
            }
            k += 2;
        }
    }
    let d_end = end?;
    // Walk back from (n, m): each round ends in a snake and starts with one
    // edit, a deletion from `a` (a step right) or an insertion from `b` (down).
    let mut edits: Vec<(usize, usize, bool)> = Vec::new(); // (x, y, is_insertion)
    let (mut x, mut y) = (n, m);
    for d in (0..=d_end).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = v[at(prev_k)];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            edits.push((prev_x as usize, prev_y as usize, x == prev_x));
        }
        x = prev_x;
        y = prev_y;
    }
    edits.reverse();
    let mut hunks: Vec<(usize, usize, usize, usize)> = Vec::new();
    for (x, y, insertion) in edits {
        match hunks.last_mut() {
            Some(h) if h.0 + h.1 == x && h.2 + h.3 == y => {
                if insertion {
                    h.3 += 1;
                } else {
                    h.1 += 1;
                }
            }
            _ => hunks.push(if insertion {
                (x, 0, y, 1)
            } else {
                (x, 1, y, 0)
            }),
        }
    }
    Some(hunks)
}
