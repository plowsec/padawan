//! Small fuzzy scorer for action titles and symbol names (files are matched by the engine).
//!
//! | Rule | Points |
//! |---|---|
//! | each matched char | +1 |
//! | consecutive with the previous match | +4 (+1 per run length) |
//! | at a word start (after ` _-./:`, or a lower→upper change, or index 0) | +6 |
//! | first match at index 0 | +4 |
//! | gap before a match | −1 per skipped char, max −3 |
//! | longer haystack | −len/16 |
//!
//! Case-insensitive, char-based, greedy with a word-start preference. `None`: not a subsequence.

/// Score and the byte offsets of the matched chars in `hay`.
pub fn score(query: &str, hay: &str) -> Option<(i32, Vec<usize>)> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Some((0, Vec::new()));
    }
    let h: Vec<(usize, char)> = hay.char_indices().collect();
    if h.len() > 4096 {
        return None;
    }
    let lower: Vec<char> = h.iter().map(|&(_, c)| c.to_lowercase().next().unwrap_or(c)).collect();
    let word_start = |j: usize| {
        j == 0 || {
            let p = h[j - 1].1;
            " _-./:\\".contains(p) || (p.is_lowercase() && h[j].1.is_uppercase())
        }
    };
    let mut pos = Vec::with_capacity(q.len());
    let (mut score, mut j, mut run) = (0i32, 0usize, 0i32);
    let mut last: Option<usize> = None;
    let rest_fits = |from: usize, rest: &[char]| {
        let mut k = from;
        rest.iter().all(|&c| match (k..lower.len()).find(|&x| lower[x] == c) {
            Some(x) => {
                k = x + 1;
                true
            }
            None => false,
        })
    };
    for (qi, &qc) in q.iter().enumerate() {
        // Prefer the next word start holding `qc` when the next plain occurrence is not adjacent,
        // as long as the rest of the query still fits after it.
        let first = (j..h.len()).find(|&k| lower[k] == qc)?;
        let pick = if Some(first) == last.map(|l| l + 1) || word_start(first) {
            first
        } else {
            (first..h.len()).find(|&k| lower[k] == qc && word_start(k) && rest_fits(k + 1, &q[qi + 1..])).unwrap_or(first)
        };
        score += 1;
        if Some(pick) == last.map(|l| l + 1) {
            run += 1;
            score += 4 + run;
        } else {
            run = 0;
            let gap = pick - last.map_or(0, |l| l + 1);
            score -= (gap as i32).min(3);
        }
        if word_start(pick) {
            score += 6;
        }
        if pick == 0 {
            score += 4;
        }
        pos.push(h[pick].0);
        last = Some(pick);
        j = pick + 1;
    }
    Some((score - (h.len() / 16) as i32, pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_and_ranks() {
        assert!(score("xyz", "Save All").is_none());
        assert_eq!(score("", "anything"), Some((0, vec![])));
        let (s1, p1) = score("sa", "Save All").unwrap();
        assert_eq!(p1, vec![0, 1]);
        let (s2, _) = score("sa", "Toggle Whitespace").unwrap();
        assert!(s1 > s2);
        // Word starts beat scattered letters.
        let (_, p) = score("sa", "Settings Save All").unwrap();
        assert_eq!(p.len(), 2);
        let (a, _) = score("gl", "Go to Line…").unwrap();
        let (b, _) = score("gl", "Toggle Line Numbers").unwrap();
        assert!(a > b, "{a} {b}");
        // Case-insensitive, camel case word starts.
        let (_, p) = score("ph", "parseHeader").unwrap();
        assert_eq!(p, vec![0, 5]);
    }

    #[test]
    fn hostile_and_unicode_never_panic() {
        assert!(score("é", "café").is_some());
        assert!(score("İ", "i̇x").is_some() || score("İ", "i̇x").is_none());
        assert!(score("a", &"b".repeat(10_000)).is_none());
        let s = "\u{202e}\0\n中文";
        let (_, p) = score("中", s).unwrap();
        assert!(s.is_char_boundary(p[0]));
    }
}
