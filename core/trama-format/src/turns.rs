// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Sequences of edges that may not be traversed in order.
//!
//! A turn restriction is a fact about a *run* of edges rather than about any one of them: come in
//! along this street, and that exit is shut. The commonest run is two edges long, which is the
//! turn at a single junction. Some are longer — a no-U-turn across a dual carriageway comes in on
//! one carriageway, crosses the link between them, and may not come back down the other — and
//! those are the same statement with more edges in the middle, not a different kind of thing.
//!
//! Nothing here knows what a street is. It knows that certain successions of edges are refused,
//! which is a fact about a graph, and it is shared between `trama-routing` and `trama-trace`
//! because a search that copied it would be a search that could disagree about which movements
//! exist over one file.
//!
//! # Why an automaton
//!
//! A search cannot ask "were the last four edges this run?" without carrying the last four edges,
//! and the number of ways to arrive somewhere along four edges is not bounded by anything useful.
//! So the walk carries a `Progress` instead: how far along the forbidden runs it currently is,
//! which is one number no matter how many runs there are or how long they get. That is Aho-Corasick,
//! and the fail links are what make overlapping runs work — having walked `a, b`, a search is
//! partway along `a, b, c` *and* partway along `b, d`, and must stay partway along both.
//!
//! A file with no sequences produces an automaton with one state, which every step returns to. The
//! common case therefore pays nothing, and the caller needs no branch to get that.

use std::collections::{BTreeMap, VecDeque};

use serde_json::Value;

use crate::{Graph, edge_properties};

/// How far along the forbidden runs a walk has got. `Turns::START` is having walked nothing.
pub type Progress = u32;

#[derive(Clone)]
struct State {
    /// Where each edge leads from here, for the edges that continue some run.
    next: BTreeMap<usize, Progress>,
    /// The longest proper suffix of the run walked so far that is itself a run prefix.
    fail: Progress,
    /// A forbidden run ends here, so arriving is the movement that is refused.
    refuses: bool,
}

/// The runs of edges a walk may not make, as an automaton over edge indices.
#[derive(Clone)]
pub struct Turns {
    states: Vec<State>,
    /// The runs the automaton was built from, kept so they can be restated — reversed, for a
    /// search running against the arrows. The automaton itself cannot be read backwards.
    runs: Vec<Vec<usize>>,
}

impl Default for Turns {
    fn default() -> Self {
        Turns::new()
    }
}

impl Turns {
    /// Having walked nothing. Every search starts here and returns here whenever a step matches no
    /// run at all, which is almost every step.
    pub const START: Progress = 0;

    /// Forbidding nothing.
    pub fn new() -> Turns {
        Turns { states: vec![State { next: BTreeMap::new(), fail: Turns::START, refuses: false }], runs: Vec::new() }
    }

    /// Whether anything is forbidden, so a caller can skip work it would otherwise do for nothing.
    pub fn is_empty(&self) -> bool {
        self.states.len() == 1
    }

    /// The automaton for a set of runs, each a succession of edge indices that may not be walked.
    ///
    /// A run shorter than two edges is dropped rather than honoured: one edge alone is not a
    /// movement between edges, and treating it as one would silently turn a malformed restriction
    /// into an impassable street.
    pub fn from_sequences(sequences: impl IntoIterator<Item = Vec<usize>>) -> Turns {
        let mut turns = Turns::new();
        for run in sequences {
            if run.len() < 2 {
                continue;
            }
            let mut at = Turns::START;
            let kept = run.clone();
            for edge in run {
                at = match turns.states[at as usize].next.get(&edge) {
                    Some(next) => *next,
                    None => {
                        let next = turns.states.len() as Progress;
                        turns.states.push(State { next: BTreeMap::new(), fail: Turns::START, refuses: false });
                        turns.states[at as usize].next.insert(edge, next);
                        next
                    }
                };
            }
            turns.states[at as usize].refuses = true;
            turns.runs.push(kept);
        }
        turns.link();
        turns
    }

    /// The fail links, by breadth so that every shorter suffix is already linked when it is needed.
    ///
    /// `refuses` is inherited along them, which is the step that makes a run ending inside another
    /// still refuse: walking `x, a, b` where `a, b` is forbidden must be refused even though the
    /// search was following `x, a, b, c` at the time.
    fn link(&mut self) {
        let mut queue = VecDeque::new();
        for depth_one in self.states[Turns::START as usize].next.values() {
            queue.push_back(*depth_one);
        }
        while let Some(at) = queue.pop_front() {
            let steps: Vec<(usize, Progress)> =
                self.states[at as usize].next.iter().map(|(edge, next)| (*edge, *next)).collect();
            for (edge, next) in steps {
                // The fail link of a child: follow the parent's fail links until one of them can
                // take this edge, and land there. The root can always be landed on.
                let mut fallback = self.states[at as usize].fail;
                let target = loop {
                    if let Some(found) = self.states[fallback as usize].next.get(&edge).filter(|found| **found != next)
                    {
                        break *found;
                    }
                    if fallback == Turns::START {
                        break Turns::START;
                    }
                    fallback = self.states[fallback as usize].fail;
                };
                self.states[next as usize].fail = target;
                self.states[next as usize].refuses |= self.states[target as usize].refuses;
                queue.push_back(next);
            }
        }
    }

    /// Where the walk stands after crossing `edge`, or `None` if crossing it is the forbidden step.
    pub fn advance(&self, at: Progress, edge: usize) -> Option<Progress> {
        let mut from = at;
        loop {
            if let Some(next) = self.states[from as usize].next.get(&edge) {
                return if self.states[*next as usize].refuses { None } else { Some(*next) };
            }
            if from == Turns::START {
                // This edge begins no run, so the walk is back to having matched nothing.
                return Some(Turns::START);
            }
            from = self.states[from as usize].fail;
        }
    }

    /// The same runs read the other way round: having crossed the last edge, the first is what
    /// may not be taken.
    ///
    /// A search running against the arrows meets every run from its far end, which is the same
    /// movement seen from the other side rather than a different one. Reversing the runs and
    /// rebuilding is the whole of it — an automaton cannot be walked backwards, which is why the
    /// runs are kept.
    pub fn reversed(&self) -> Turns {
        Turns::from_sequences(self.runs.iter().map(|run| run.iter().rev().copied().collect::<Vec<usize>>()))
    }

    /// How many states the automaton holds, which is what a search must multiply its arcs by.
    pub fn states(&self) -> usize {
        self.states.len()
    }

    /// The runs written in a `PROP` column of stable ids, translated to this reader's indices.
    ///
    /// A cell holds space-separated runs, each run's edges joined by `>`, and each run is the
    /// continuation of the edge whose row it sits on: `"77 91>34"` on edge `e` forbids `e` then
    /// `77`, and `e` then `91` then `34`. A lone id is therefore the ordinary turn restriction and
    /// reads exactly as it did before runs existed, which is what keeps already-published files
    /// readable.
    ///
    /// Ids rather than indices because an id is what the file is addressed by and what survives a
    /// recompilation; an index is an artefact of this reader's own ordering. An id naming no edge
    /// in this container drops the run that contains it rather than the whole cell: a restriction
    /// can point at a street outside the extract, and a run with a missing middle is not a run
    /// this graph can walk anyway.
    pub fn read(container: &[u8], graph: &Graph, key: Option<&str>) -> Result<Turns, String> {
        let Some(key) = key else {
            return Ok(Turns::new());
        };
        let rows = edge_properties(container)?;
        let by_id: BTreeMap<u64, usize> =
            graph.edges.iter().enumerate().map(|(index, edge)| (edge.id, index)).collect();
        let mut sequences = Vec::new();
        for (index, edge) in graph.edges.iter().enumerate() {
            let Some(cell) = rows.get(edge.property_row as usize).and_then(|row| row.get(key)).and_then(Value::as_str)
            else {
                continue;
            };
            for listed in cell.split_whitespace() {
                let mut run = vec![index];
                let mut whole = true;
                for id in listed.split('>') {
                    match id.parse().ok().and_then(|id: u64| by_id.get(&id)) {
                        Some(found) => run.push(*found),
                        None => {
                            whole = false;
                            break;
                        }
                    }
                }
                if whole {
                    sequences.push(run);
                }
            }
        }
        Ok(Turns::from_sequences(sequences))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walk a run of edges from the start, returning the edge it was refused at.
    fn refused_at(turns: &Turns, walk: &[usize]) -> Option<usize> {
        let mut at = Turns::START;
        for (position, edge) in walk.iter().enumerate() {
            match turns.advance(at, *edge) {
                Some(next) => at = next,
                None => return Some(position),
            }
        }
        None
    }

    #[test]
    fn forbidding_nothing_refuses_nothing_and_costs_one_state() {
        let turns = Turns::new();

        assert!(turns.is_empty());
        assert_eq!(turns.states(), 1, "a search multiplies its arcs by this");
        assert_eq!(refused_at(&turns, &[1, 2, 3, 4]), None);
    }

    #[test]
    fn the_ordinary_turn_is_a_run_of_two() {
        let turns = Turns::from_sequences([vec![1, 2]]);

        assert_eq!(refused_at(&turns, &[1, 2]), Some(1), "1 then 2 is the forbidden movement");
        assert_eq!(refused_at(&turns, &[2, 1]), None, "the other way round is a different turn");
        assert_eq!(refused_at(&turns, &[1, 3, 2]), None, "and going elsewhere in between makes it another one");
    }

    /// The dual-carriageway no-U-turn: down one side, across the link, and not back up the other.
    #[test]
    fn a_longer_run_is_refused_only_at_its_last_edge() {
        let turns = Turns::from_sequences([vec![1, 2, 3]]);

        assert_eq!(refused_at(&turns, &[1, 2, 3]), Some(2), "refused on the last edge, not the first");
        assert_eq!(refused_at(&turns, &[1, 2]), None, "crossing the link is allowed; coming back is not");
        assert_eq!(refused_at(&turns, &[1, 2, 4]), None, "any other exit from the link is fine");
        assert_eq!(refused_at(&turns, &[9, 1, 2, 3]), Some(3), "the run is refused wherever it is reached from");
    }

    /// Why the fail links exist: two runs that share edges must both stay live.
    #[test]
    fn overlapping_runs_are_all_matched_at_once() {
        let turns = Turns::from_sequences([vec![1, 2, 3], vec![2, 4]]);

        // Walking 1 then 2 is partway along both runs. It has to be, or one of them is lost.
        assert_eq!(refused_at(&turns, &[1, 2, 3]), Some(2), "the long run");
        assert_eq!(refused_at(&turns, &[1, 2, 4]), Some(2), "and the short one it contains");
        assert_eq!(refused_at(&turns, &[1, 2, 5]), None, "while anything else still leaves the link");
    }

    /// A run ending inside a longer one is refused on its own account.
    #[test]
    fn a_run_that_ends_inside_another_still_refuses() {
        let turns = Turns::from_sequences([vec![1, 2, 3, 4], vec![2, 3]]);

        assert_eq!(refused_at(&turns, &[1, 2, 3]), Some(2), "the short run fires while the long one is mid-walk");
    }

    #[test]
    fn a_run_shorter_than_a_movement_is_dropped_rather_than_closing_a_street() {
        let turns = Turns::from_sequences([vec![7], Vec::new()]);

        assert!(turns.is_empty(), "one edge alone is not a movement between edges");
        assert_eq!(refused_at(&turns, &[7, 7, 7]), None);
    }

    #[test]
    fn a_repeated_edge_in_a_run_is_walked_as_written() {
        // A run can legitimately return to an edge: out along a slip road and back down it.
        let turns = Turns::from_sequences([vec![1, 2, 1]]);

        assert_eq!(refused_at(&turns, &[1, 2, 1]), Some(2));
        assert_eq!(refused_at(&turns, &[1, 2]), None);
    }
}
