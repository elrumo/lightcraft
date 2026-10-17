//! Sorting a big list on all cores.

use std::cmp::Ordering;

/// Lists shorter than this are sorted on one thread (starting threads would cost more than they save).
const MIN_LEN: usize = 50_000;

/// `v` sorted by `cmp`, which must be a total order (equal elements may come out in either order). Past
/// [`MIN_LEN`] each core sorts a part and the sorted parts are merged.
pub(crate) fn sort_by<T: Copy + Send + Sync>(v: &mut Vec<T>, cmp: impl Fn(&T, &T) -> Ordering + Sync) {
    // (the web build has no threads)
    let threads = if cfg!(target_arch = "wasm32") { 1 } else { std::thread::available_parallelism().map_or(1, |n| n.get()).min(8) };
    if threads < 2 || v.len() < MIN_LEN {
        v.sort_unstable_by(cmp);
        return;
    }
    let part = v.len().div_ceil(threads);
    let sorted = std::thread::scope(|scope| {
        let workers: Vec<_> = v.chunks_mut(part).map(|c| scope.spawn(|| c.sort_unstable_by(&cmp))).collect();
        // (a part that panicked, a comparison that does: sorted again below, on this thread)
        let joined: Vec<bool> = workers.into_iter().map(|w| w.join().is_ok()).collect();
        joined.iter().all(|ok| *ok)
    });
    if !sorted {
        v.sort_unstable_by(cmp);
        return;
    }
    let mut runs: Vec<Vec<T>> = v.chunks(part).map(<[T]>::to_vec).collect();
    while runs.len() > 1 {
        let mut merged = Vec::with_capacity(runs.len().div_ceil(2));
        let mut pairs = runs.into_iter();
        while let Some(a) = pairs.next() {
            merged.push(match pairs.next() {
                Some(b) => merge(&a, &b, &cmp),
                None => a,
            });
        }
        runs = merged;
    }
    if let Some(all) = runs.pop() {
        *v = all;
    }
}

fn merge<T: Copy>(a: &[T], b: &[T], cmp: &impl Fn(&T, &T) -> Ordering) -> Vec<T> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while let (Some(x), Some(y)) = (a.get(i), b.get(j)) {
        if cmp(y, x).is_lt() {
            out.push(*y);
            j += 1;
        } else {
            out.push(*x);
            i += 1;
        }
    }
    out.extend_from_slice(a.get(i..).unwrap_or_default());
    out.extend_from_slice(b.get(j..).unwrap_or_default());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parts, ties, a list that is already in order or in reverse, and a short one: the same as `sort`.
    #[test]
    fn sorts_like_sort() {
        let mix = |i: u64| i.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40;
        let cases: Vec<Vec<(u64, u64)>> = vec![
            (0..200_001u64).map(|i| (mix(i) % 1000, i)).collect(),
            (0..120_000u64).map(|i| (i, i)).collect(),
            (0..120_000u64).rev().map(|i| (i, i)).collect(),
            (0..10u64).map(|i| (mix(i), i)).collect(),
            vec![],
        ];
        for case in cases {
            let mut want = case.clone();
            want.sort_unstable();
            let mut got = case;
            sort_by(&mut got, |a, b| a.cmp(b));
            assert!(got == want);
        }
    }
}
