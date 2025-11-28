//! Token graph with `-ln(rate)` edge weights.

use std::collections::HashMap;

use alloy_primitives::Address;

use crate::market::Market;

/// A directed swap through one pool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    /// Index of the input token.
    pub from: usize,
    /// Index of the output token.
    pub to: usize,
    /// Index of the pool in [`Market::pools`].
    pub pool: usize,
    /// `-ln(marginal rate after fees)`. Negative means the edge alone multiplies value.
    pub weight: f64,
}

/// Directed multigraph: one node per token, two edges per tradable pool.
#[derive(Debug, Clone)]
pub struct TokenGraph {
    /// Token address for each node.
    pub tokens: Vec<Address>,
    /// All edges.
    pub edges: Vec<Edge>,
    /// Outgoing edge indices per node.
    pub outgoing: Vec<Vec<usize>>,
    index: HashMap<Address, usize>,
}

impl TokenGraph {
    /// Builds the graph from current pool state.
    ///
    /// Pools that cannot trade at the margin (empty reserves, or a V3 pool with
    /// no liquidity at the current tick) contribute no edges.
    pub fn build(market: &Market) -> Self {
        let mut graph = Self {
            tokens: Vec::new(),
            edges: Vec::new(),
            outgoing: Vec::new(),
            index: HashMap::new(),
        };
        for (pool_idx, pool) in market.pools.iter().enumerate() {
            for (token_in, token_out) in [(pool.token0, pool.token1), (pool.token1, pool.token0)] {
                let Some(rate) = pool.marginal_rate(token_in) else {
                    continue;
                };
                if !(rate.is_finite() && rate > 0.0) {
                    continue;
                }
                let from = graph.node(token_in);
                let to = graph.node(token_out);
                graph.outgoing[from].push(graph.edges.len());
                graph.edges.push(Edge {
                    from,
                    to,
                    pool: pool_idx,
                    weight: -rate.ln(),
                });
            }
        }
        graph
    }

    fn node(&mut self, token: Address) -> usize {
        *self.index.entry(token).or_insert_with(|| {
            self.tokens.push(token);
            self.outgoing.push(Vec::new());
            self.tokens.len() - 1
        })
    }

    /// Node index of a token, if it appears in any tradable pool.
    pub fn node_of(&self, token: Address) -> Option<usize> {
        self.index.get(&token).copied()
    }

    /// Sum of edge weights; negative means the rates multiply to more than one.
    pub fn weight(&self, edges: &[usize]) -> f64 {
        edges.iter().map(|&e| self.edges[e].weight).sum()
    }
}
