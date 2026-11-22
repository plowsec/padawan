//! Undo/redo stacks of edit groups. Pure, no egui.
//!
//! | Rule | Value |
//! |---|---|
//! | coalesce | same kind (`Type` or `Delete`), ≤ 600 ms since the last edit, cursor where the group left it, group is not the saved point |
//! | break | pause, cursor jump, other kind, newline / compound edits (`Other`) |
//! | memory cap | `CAP` bytes of stored text (undo + redo); oldest undo groups dropped first, the newest group always kept |
//! | dirty | `state_id()` = id of the top undo group (or of the last dropped one); compared with the saved id |
//!
//! A group stores the inverse edits in application order. Undo applies them in reverse; the inverses of
//! those become the redo group (same id), and redo does the same in the other direction.

use std::collections::VecDeque;

use crate::buffer::Edit;
use crate::Pos;

pub const CAP: usize = 64 << 20;
const COALESCE_S: f64 = 0.6;

/// Cursor + anchor, restored by undo/redo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sel {
    pub anchor: Pos,
    pub head: Pos,
}

impl Sel {
    pub fn caret(p: Pos) -> Self {
        Sel { anchor: p, head: p }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Type,
    Delete,
    Other,
}

pub struct Group {
    pub edits: Vec<Edit>,
    pub before: Sel,
    pub after: Sel,
    pub kind: Kind,
    pub time: f64,
    pub id: u64,
    cost: usize,
}

impl Group {
    fn cost_of(edits: &[Edit]) -> usize {
        edits.iter().map(Edit::cost).sum::<usize>() + 64
    }
}

pub struct History {
    undo: VecDeque<Group>,
    redo: Vec<Group>,
    next_id: u64,
    base_id: u64,
    cost: usize,
    pub cap: usize,
}

impl Default for History {
    fn default() -> Self {
        History { undo: VecDeque::new(), redo: Vec::new(), next_id: 1, base_id: 0, cost: 0, cap: CAP }
    }
}

impl History {
    pub fn state_id(&self) -> u64 {
        self.undo.back().map_or(self.base_id, |g| g.id)
    }

    pub fn clear(&mut self) {
        let id = self.state_id();
        self.undo.clear();
        self.redo.clear();
        self.cost = 0;
        self.base_id = id;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    #[cfg(test)]
    pub fn memory(&self) -> usize {
        self.cost
    }

    /// Record a new group (or extend the top one). Clears redo.
    #[allow(clippy::too_many_arguments)]
    pub fn record(&mut self, edits: Vec<Edit>, kind: Kind, time: f64, before: Sel, after: Sel, saved_id: u64) {
        if edits.is_empty() {
            return;
        }
        for g in self.redo.drain(..) {
            self.cost = self.cost.saturating_sub(g.cost);
        }
        let add = Group::cost_of(&edits);
        if let Some(top) = self.undo.back_mut() {
            let merge = kind != Kind::Other
                && top.kind == kind
                && time - top.time <= COALESCE_S
                && time >= top.time
                && top.after == before
                && top.id != saved_id;
            if merge {
                top.edits.extend(edits);
                top.after = after;
                top.time = time;
                top.cost += add;
                self.cost += add;
                self.trim();
                return;
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        self.undo.push_back(Group { edits, before, after, kind, time, id, cost: add });
        self.cost += add;
        self.trim();
    }

    fn trim(&mut self) {
        while self.cost > self.cap && self.undo.len() > 1 {
            if let Some(g) = self.undo.pop_front() {
                self.cost = self.cost.saturating_sub(g.cost);
                self.base_id = g.id;
            }
        }
    }

    pub fn pop_undo(&mut self) -> Option<Group> {
        let g = self.undo.pop_back()?;
        self.cost = self.cost.saturating_sub(g.cost);
        Some(g)
    }

    pub fn pop_redo(&mut self) -> Option<Group> {
        let g = self.redo.pop()?;
        self.cost = self.cost.saturating_sub(g.cost);
        Some(g)
    }

    /// Push the group produced by an undo (its edits now redo it).
    pub fn push_redo(&mut self, mut g: Group, edits: Vec<Edit>) {
        g.cost = Group::cost_of(&edits);
        g.edits = edits;
        self.cost += g.cost;
        self.redo.push(g);
    }

    /// Push the group produced by a redo back onto the undo stack (no coalescing, keeps redo).
    pub fn push_undo(&mut self, mut g: Group, edits: Vec<Edit>) {
        g.cost = Group::cost_of(&edits);
        g.edits = edits;
        g.time = f64::NEG_INFINITY; // never coalesce into a redone group
        self.cost += g.cost;
        self.undo.push_back(g);
        self.trim();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(c: u32) -> Edit {
        Edit::new((0, c), (0, c + 1), "")
    }

    #[test]
    fn coalesce_rules() {
        let mut h = History::default();
        let s = |c| Sel::caret((0, c));
        h.record(vec![e(0)], Kind::Type, 0.0, s(0), s(1), 0);
        h.record(vec![e(1)], Kind::Type, 0.3, s(1), s(2), 0);
        assert_eq!(h.undo.len(), 1);
        h.record(vec![e(2)], Kind::Type, 1.0, s(2), s(3), 0); // pause
        assert_eq!(h.undo.len(), 2);
        h.record(vec![e(3)], Kind::Type, 1.1, s(9), s(10), 0); // jump
        assert_eq!(h.undo.len(), 3);
        h.record(vec![e(4)], Kind::Delete, 1.2, s(10), s(9), 0); // other kind
        assert_eq!(h.undo.len(), 4);
        let saved = h.state_id();
        h.record(vec![e(5)], Kind::Delete, 1.3, s(9), s(8), saved); // saved point is never extended
        assert_eq!(h.undo.len(), 5);
        h.record(vec![e(6)], Kind::Other, 1.3, s(8), s(8), 0);
        assert_eq!(h.undo.len(), 6);
    }

    #[test]
    fn cap_drops_oldest() {
        let mut h = History { cap: 1000, ..Default::default() };
        for i in 0..100 {
            h.record(vec![Edit::new((0, 0), (0, 0), "x".repeat(100))], Kind::Other, f64::from(i), Sel::default(), Sel::default(), 0);
        }
        assert!(h.memory() <= 1000);
        assert!(!h.undo.is_empty());
        assert_ne!(h.base_id, 0);
        // One group over the cap is kept.
        h.record(vec![Edit::new((0, 0), (0, 0), "x".repeat(5000))], Kind::Other, 1000.0, Sel::default(), Sel::default(), 0);
        assert_eq!(h.undo.len(), 1);
    }
}
