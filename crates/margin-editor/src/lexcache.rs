//! Lexer state at the start of each line, filled lazily with a time budget. Pure, no egui.
//!
//! | Field | Meaning |
//! |---|---|
//! | `states[i]` | state at the start of line `i` |
//! | `valid` | `states[..valid]` are right |
//! | `dirty_end..tail_end` | old states after the edited lines: right again as soon as one of them matches |
//!
//! Edit: `valid` drops to the first edited line + 1; later states shift with the lines.
//! Re-lex: when the state computed for a line `>= dirty_end` equals the stored one, jump `valid` to `tail_end`.

use std::time::{Duration, Instant};

use margin_code::LineState;

pub struct LexCache {
    states: Vec<LineState>,
    valid: usize,
    dirty_end: usize,
    tail_end: usize,
}

impl Default for LexCache {
    fn default() -> Self {
        LexCache { states: vec![LineState::default()], valid: 1, dirty_end: 0, tail_end: 0 }
    }
}

impl LexCache {
    pub fn reset(&mut self) {
        *self = LexCache::default();
    }

    /// Lines `0..valid()` have a known start state.
    #[cfg(test)]
    pub fn valid(&self) -> usize {
        self.valid
    }

    /// State at the start of `line`, if known.
    pub fn state(&self, line: usize) -> Option<LineState> {
        (line < self.valid).then(|| self.states[line])
    }

    /// Line `first` changed; `removed` lines after it are gone, `inserted` new ones follow it.
    pub fn on_edit(&mut self, first: usize, removed: usize, inserted: usize) {
        // Entries for lines first+1 ..= first+removed are replaced by `inserted` placeholders.
        let lo = (first + 1).min(self.states.len());
        let hi = (first + 1 + removed).min(self.states.len());
        let removed_here = hi - lo;
        let insert_here = if hi == first + 1 + removed { inserted } else { 0 };
        if removed_here > 0 || insert_here > 0 {
            self.states.splice(lo..hi, std::iter::repeat_n(LineState::default(), insert_here));
        }
        // Old index x (> first + removed) → new index.
        let shift = |x: usize| x + inserted - removed;
        let last_changed = first + removed; // old coordinates
        let edit_end = first + 1 + inserted; // new: first entry whose line chain is untouched
        let (v, d, t) = (self.valid, self.dirty_end, self.tail_end);
        // Candidate consistent chains after the edit; keep the one reaching furthest.
        let mut best = (0usize, 0usize);
        let mut offer = |a: usize, b: usize| {
            if a < b && b > best.1 {
                best = (a, b);
            }
        };
        if v > last_changed + 1 {
            offer(edit_end, shift(v));
        }
        if d < t {
            if first >= t {
                offer(d, t);
            } else if last_changed < d {
                offer(shift(d), shift(t));
            } else if last_changed + 1 < t {
                offer(edit_end, shift(t));
            }
        }
        let len = self.states.len();
        self.dirty_end = best.0.min(len);
        self.tail_end = best.1.min(len);
        if self.dirty_end >= self.tail_end {
            self.dirty_end = 0;
            self.tail_end = 0;
        }
        self.valid = v.min(first + 1).max(1);
    }

    /// Lex forward until line `upto` (exclusive) has a start state, or `budget` runs out.
    /// `lex(state, line_index) -> next state`. Returns true when done.
    pub fn advance(&mut self, upto: usize, line_count: usize, budget: Duration, mut lex: impl FnMut(LineState, usize) -> LineState) -> bool {
        let upto = upto.min(line_count);
        if self.valid >= upto {
            return true;
        }
        let t0 = Instant::now();
        let mut n = 0u32;
        while self.valid < upto {
            let i = self.valid - 1;
            let next = lex(self.states[i], i);
            let j = i + 1;
            if j < self.states.len() {
                let converged = j >= self.dirty_end && j < self.tail_end && self.states[j] == next;
                self.states[j] = next;
                if converged {
                    self.valid = self.tail_end;
                    self.dirty_end = 0;
                    self.tail_end = 0;
                    continue;
                }
                if j >= self.tail_end {
                    self.dirty_end = 0;
                    self.tail_end = 0;
                }
            } else {
                self.states.push(next);
            }
            self.valid = j + 1;
            n += 1;
            if n.is_multiple_of(256) && t0.elapsed() >= budget {
                break;
            }
        }
        self.valid >= upto
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testrng::Rng;

    /// Toy lexer: state 1 inside `/* */`.
    fn toy(s: LineState, line: &str) -> LineState {
        let mut st = s.0;
        let b = line.as_bytes();
        let mut i = 0;
        while i + 1 < b.len() {
            if st == 0 && &b[i..i + 2] == b"/*" {
                st = 1;
                i += 2;
            } else if st == 1 && &b[i..i + 2] == b"*/" {
                st = 0;
                i += 2;
            } else {
                i += 1;
            }
        }
        LineState(st)
    }

    fn full(lines: &[String]) -> Vec<LineState> {
        let mut v = vec![LineState::default()];
        for l in lines.iter().take(lines.len().saturating_sub(1)) {
            let s = *v.last().unwrap_or(&LineState::default());
            v.push(toy(s, l));
        }
        v
    }

    #[test]
    fn random_edits_match_full_lex() {
        let mut r = Rng::new(7);
        let pieces = ["a", "/*", "*/", "x y"];
        let mut lines: Vec<String> = (0..200).map(|i| pieces[i % 4].to_owned()).collect();
        let mut c = LexCache::default();
        let mut lexed = 0usize;
        for round in 0..3000 {
            // Random edit: replace lines first..=first+removed with 1+inserted new lines.
            let first = r.below(lines.len());
            let removed = r.below(3).min(lines.len() - 1 - first);
            let inserted = r.below(3);
            let new: Vec<String> = (0..=inserted).map(|_| pieces[r.below(4)].to_owned()).collect();
            lines.splice(first..=first + removed, new);
            c.on_edit(first, removed, inserted);
            // Partial or full re-lex.
            let upto = if round % 3 == 0 { lines.len() } else { r.below(lines.len() + 1) };
            let ls = &lines;
            c.advance(upto, ls.len(), Duration::from_secs(1), |s, i| {
                lexed += 1;
                toy(s, &ls[i])
            });
            let want = full(&lines);
            for (i, w) in want.iter().enumerate().take(c.valid()) {
                assert_eq!(c.state(i), Some(*w), "round {round} line {i}");
            }
            assert!(c.valid() >= upto.min(lines.len()).max(1));
        }
        assert!(lexed > 0);
    }

    #[test]
    fn stops_relexing_when_state_converges() {
        let lines: Vec<String> = (0..10_000).map(|i| format!("line {i}")).collect();
        let mut c = LexCache::default();
        let n = lines.len();
        c.advance(n, n, Duration::from_secs(5), |s, i| toy(s, &lines[i]));
        assert_eq!(c.valid(), n);
        c.on_edit(10, 0, 0);
        let mut count = 0;
        c.advance(n, n, Duration::from_secs(5), |s, i| {
            count += 1;
            toy(s, &lines[i])
        });
        assert_eq!(c.valid(), n);
        assert!(count <= 3, "re-lexed {count} lines");
    }
}
