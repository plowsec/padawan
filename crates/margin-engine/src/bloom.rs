//! Per-file trigram bloom filter over ASCII-lowercased bytes (the content index).
//!
//! | Choice | Value | Why |
//! |---|---|---|
//! | bits per distinct trigram | 8 | ~1 B/trigram keeps the index a fraction of the corpus |
//! | hashes | 3 | per-trigram false positive ≈ (1 − e^(−3/8))³ ≈ 3.1 %; a query needs ALL its trigrams, so a 6-byte literal (4 trigrams) passes a non-matching file with p ≈ 1e-6 if independent |
//! | size | n × 8 bits rounded up to 64 bits, 8 B .. 4 KiB | big files saturate (fall back to a scan); never a false negative |
//! | index | double hashing + fast range (`(h × m) >> 32`) | no power-of-two rounding (no 2× waste) |

use std::cell::RefCell;

pub const MAX_BLOOM: usize = 4096;
const BITS_PER_TRIGRAM: usize = 8;
const HASHES: u32 = 3;

pub fn fold(b: u8) -> u8 {
    b.to_ascii_lowercase()
}

#[inline]
fn tri(a: u8, b: u8, c: u8) -> u32 {
    (u32::from(fold(a)) << 16) | (u32::from(fold(b)) << 8) | u32::from(fold(c))
}

#[inline]
fn probes(t: u32, m_bits: u64) -> [u64; HASHES as usize] {
    let h = (u64::from(t) ^ 0x5bd1_e995_1234_5678).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let h1 = (h >> 32) as u32;
    let h2 = (h as u32) | 1;
    let mut out = [0u64; HASHES as usize];
    for (i, o) in out.iter_mut().enumerate() {
        let x = h1.wrapping_add((i as u32).wrapping_mul(h2));
        *o = (u64::from(x) * m_bits) >> 32;
    }
    out
}

struct Scratch {
    /// One bit per possible trigram (2^24).
    seen: Vec<u64>,
    list: Vec<u32>,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = const { RefCell::new(Scratch { seen: Vec::new(), list: Vec::new() }) };
}

/// Distinct folded trigrams of `data` → bloom bytes.
pub fn build(data: &[u8]) -> Box<[u8]> {
    SCRATCH.with(|s| {
        let mut s = s.borrow_mut();
        let Scratch { seen, list } = &mut *s;
        if seen.len() != 1 << 18 {
            *seen = vec![0u64; 1 << 18];
        }
        list.clear();
        for w in data.windows(3) {
            let t = tri(w[0], w[1], w[2]);
            let (word, bit) = ((t >> 6) as usize, 1u64 << (t & 63));
            if let Some(x) = seen.get_mut(word) {
                if *x & bit == 0 {
                    *x |= bit;
                    list.push(t);
                }
            }
        }
        let bits = list.len().saturating_mul(BITS_PER_TRIGRAM).div_ceil(64).max(1) * 64;
        let bytes = (bits / 8).min(MAX_BLOOM);
        let mut bloom = vec![0u8; bytes];
        let m = (bytes * 8) as u64;
        for &t in list.iter() {
            for p in probes(t, m) {
                if let Some(b) = bloom.get_mut((p >> 3) as usize) {
                    *b |= 1 << (p & 7);
                }
            }
            if let Some(x) = seen.get_mut((t >> 6) as usize) {
                *x = 0;
            }
        }
        bloom.into_boxed_slice()
    })
}

/// Distinct folded trigrams of a pattern.
pub fn trigrams(pat: &[u8]) -> Vec<u32> {
    let mut v: Vec<u32> = pat.windows(3).map(|w| tri(w[0], w[1], w[2])).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Every trigram may be present. An empty filter (bad cache data) is treated as "maybe".
pub fn may_contain(bloom: &[u8], tris: &[u32]) -> bool {
    if bloom.is_empty() {
        return true;
    }
    let m = (bloom.len() * 8) as u64;
    tris.iter().all(|&t| probes(t, m).iter().all(|&p| bloom.get((p >> 3) as usize).is_some_and(|b| b & (1 << (p & 7)) != 0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Xs(u64);
    impl Xs {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    fn naive_ci(hay: &[u8], pat: &[u8]) -> bool {
        let h: Vec<u8> = hay.iter().map(|&b| fold(b)).collect();
        let p: Vec<u8> = pat.iter().map(|&b| fold(b)).collect();
        h.windows(p.len()).any(|w| w == p.as_slice())
    }

    /// Property: whenever a naive scan finds the pattern, the bloom says "maybe".
    #[test]
    fn no_false_negatives() {
        let mut r = Xs(0x1234_5678_9abc_def1);
        let alpha = b"abcdeABCDE_ \n\x00\xff";
        let mut pruned = 0;
        let mut total = 0;
        for _ in 0..300 {
            let len = (r.next() % 3000) as usize;
            let data: Vec<u8> = (0..len).map(|_| alpha[(r.next() % alpha.len() as u64) as usize]).collect();
            let bloom = build(&data);
            assert!(bloom.len() <= MAX_BLOOM && !bloom.is_empty());
            for _ in 0..40 {
                let plen = 3 + (r.next() % 6) as usize;
                let pat: Vec<u8> = if len > plen && r.next().is_multiple_of(2) {
                    let s = (r.next() % (len - plen) as u64) as usize;
                    data[s..s + plen].to_vec()
                } else {
                    (0..plen).map(|_| alpha[(r.next() % alpha.len() as u64) as usize]).collect()
                };
                let tris = trigrams(&pat);
                total += 1;
                if naive_ci(&data, &pat) {
                    assert!(may_contain(&bloom, &tris), "false negative");
                } else if !may_contain(&bloom, &tris) {
                    pruned += 1;
                }
            }
        }
        assert!(pruned > total / 10, "bloom prunes too little: {pruned}/{total}");
    }

    #[test]
    fn sizes() {
        assert_eq!(build(b"").len(), 8);
        assert!(!may_contain(&build(b"ab"), &trigrams(b"abc")));
        let big: Vec<u8> = (0..200_000u32).flat_map(|i| i.to_le_bytes()).collect();
        assert_eq!(build(&big).len(), MAX_BLOOM);
    }
}
