//! Negative-cycle detection and enumeration on the token graph.
//!
//! Two tools with different jobs:
//!
//! - [`find_negative_cycle`] is Bellman-Ford in its queue-based (SPFA) form,
//!   run from a virtual source connected to every node. It proves the absence
//!   of arbitrage in `O(V·E)` and otherwise returns one negative cycle of any
//!   length. It cannot list all of them, and the one it returns is not
//!   necessarily short or the most profitable.
//! - [`enumerate_negative_cycles`] lists every simple negative cycle up to a
//!   hop limit. Real searchers cap cycles at two or three hops because every
//!   extra hop costs ~50-100k gas, so exhaustive enumeration over that small
//!   space is both cheap and complete.
//!
//! The scanner uses the first as a gate and the second to produce candidates.

use std::collections::VecDeque;

use crate::graph::TokenGraph;

/// Tolerance for float noise in weight comparisons.
const EPS: f64 = 1e-12;

/// A closed walk of edge indices: `edges[i].to == edges[i + 1].from`, and the
/// last edge returns to the first edge's `from`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Cycle {
    /// Edge indices in traversal order.
    pub edges: Vec<usize>,
}

impl Cycle {
    /// Number of hops.
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    /// Always false for a valid cycle; present for API symmetry with `len`.
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// The same cycle starting at hop `k`.
    #[must_use]
    pub fn rotated(&self, k: usize) -> Self {
        let mut edges = self.edges.clone();
        edges.rotate_left(k % self.edges.len());
        Self { edges }
    }
}

/// Bellman-Ford (SPFA) from a virtual source. Returns one negative cycle if
/// any exists.
pub fn find_negative_cycle(graph: &TokenGraph) -> Option<Cycle> {
    let n = graph.tokens.len();
    if n == 0 {
        return None;
    }
    // Virtual source: every node starts at distance 0 and in the queue.
    let mut dist = vec![0.0f64; n];
    let mut pred_edge = vec![usize::MAX; n];
    let mut hops = vec![0usize; n];
    let mut in_queue = vec![true; n];
    let mut queue: VecDeque<usize> = (0..n).collect();

    while let Some(u) = queue.pop_front() {
        in_queue[u] = false;
        for &e in &graph.outgoing[u] {
            let edge = graph.edges[e];
            let candidate = dist[u] + edge.weight;
            if candidate < dist[edge.to] - EPS {
                dist[edge.to] = candidate;
                pred_edge[edge.to] = e;
                hops[edge.to] = hops[u] + 1;
                // A shortest path with n edges must repeat a node: negative cycle.
                if hops[edge.to] >= n {
                    return Some(extract_cycle(graph, &pred_edge, edge.to));
                }
                if !in_queue[edge.to] {
                    in_queue[edge.to] = true;
                    queue.push_back(edge.to);
                }
            }
        }
    }
    None
}

/// Walks predecessors from `start` until a node repeats, then returns that loop.
fn extract_cycle(graph: &TokenGraph, pred_edge: &[usize], start: usize) -> Cycle {
    let n = graph.tokens.len();
    // Step back n times to be sure we are inside the cycle.
    let mut v = start;
    for _ in 0..n {
        v = graph.edges[pred_edge[v]].from;
    }
    let anchor = v;
    let mut edges = Vec::new();
    loop {
        let e = pred_edge[v];
        edges.push(e);
        v = graph.edges[e].from;
        if v == anchor {
            break;
        }
    }
    edges.reverse();
    Cycle { edges }
}

/// Every simple cycle of `2..=max_len` hops whose weight is negative.
///
/// Each cycle is reported once, rotated so it starts at its lowest-indexed
/// token; the scanner tries the other rotations itself. Two-hop cycles must use
/// two different pools (a round trip through one pool always loses the fee).
pub fn enumerate_negative_cycles(graph: &TokenGraph, max_len: usize) -> Vec<Cycle> {
    let mut found = Vec::new();
    let mut path = Vec::with_capacity(max_len);
    let mut on_path = vec![false; graph.tokens.len()];
    for start in 0..graph.tokens.len() {
        on_path[start] = true;
        dfs(
            graph,
            start,
            start,
            0.0,
            max_len,
            &mut path,
            &mut on_path,
            &mut found,
        );
        on_path[start] = false;
    }
    found
}

#[allow(clippy::too_many_arguments)]
fn dfs(
    graph: &TokenGraph,
    start: usize,
    at: usize,
    weight: f64,
    max_len: usize,
    path: &mut Vec<usize>,
    on_path: &mut [bool],
    found: &mut Vec<Cycle>,
) {
    for &e in &graph.outgoing[at] {
        let edge = graph.edges[e];
        let w = weight + edge.weight;
        if edge.to == start {
            let closes = !path.is_empty()
                && (path.len() > 1 || graph.edges[path[0]].pool != edge.pool)
                && w < -EPS;
            if closes {
                let mut edges = path.clone();
                edges.push(e);
                found.push(Cycle { edges });
            }
            continue;
        }
        // Only visit tokens above `start` so each cycle is found from its minimum.
        if edge.to < start || on_path[edge.to] || path.len() + 1 >= max_len {
            continue;
        }
        on_path[edge.to] = true;
        path.push(e);
        dfs(graph, start, edge.to, w, max_len, path, on_path, found);
        path.pop();
        on_path[edge.to] = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::{consistent_market, planted_market};

    #[test]
    fn consistent_prices_have_no_cycles() {
        let market = consistent_market(&[0, 3600, -12_000, 45_000], 0);
        let g = TokenGraph::build(&market);
        assert!(find_negative_cycle(&g).is_none());
        assert_eq!(enumerate_negative_cycles(&g, 3).len(), 0);
    }

    #[test]
    fn planted_mispricing_is_found_by_both() {
        let market = planted_market();
        let g = TokenGraph::build(&market);
        let spfa = find_negative_cycle(&g).expect("SPFA finds the planted cycle");
        assert!(g.weight(&spfa.edges) < 0.0);
        let all = enumerate_negative_cycles(&g, 3);
        assert_ne!(all.len(), 0);
        for c in &all {
            assert!(g.weight(&c.edges) < 0.0);
            // Closed walk.
            for i in 0..c.len() {
                assert_eq!(
                    g.edges[c.edges[i]].to,
                    g.edges[c.edges[(i + 1) % c.len()]].from
                );
            }
        }
    }
}
