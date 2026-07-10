use petgraph::graph::{DiGraph, NodeIndex};
use types::types::ConflictType;

/// A directed dependency graph over transactions in a single block.

pub struct DepGraph {
    pub graph: DiGraph<usize, ConflictType>,
    pub tx_count: usize,
    pub block_number: u64,
    node_indices: Vec<NodeIndex>,
}

impl DepGraph {
    pub fn new(tx_count: usize, block_number: u64) -> Self {
        let mut graph = DiGraph::with_capacity(tx_count, 0);
        let node_indices: Vec<NodeIndex> = (0..tx_count).map(|i| graph.add_node(i)).collect();

        Self {
            graph,
            tx_count,
            block_number,
            node_indices,
        }
    }
    /// Return the `NodeIndex` for a given transaction position.
    ///
    /// # Panics
    ///
    /// Panics if `tx_index >= self.tx_count`.
    pub fn node_for_tx(&self, tx_index: usize) -> NodeIndex {
        self.node_indices[tx_index]
    }

    // Total number of dependency edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
    /// Returns `true` when the transaction has **no** incoming or outgoing
    /// dependency edges, meaning it can execute independently of every other
    /// transaction in the block.
    ///
    /// # Panics
    ///
    /// Panics if `tx_index >= self.tx_count`.
    pub fn is_independent(&self, tx_index: usize) -> bool {
        let node = self.node_for_tx(tx_index);
        self.graph
            .neighbors_directed(node, petgraph::Direction::Incoming)
            .next()
            .is_none()
            && self
                .graph
                .neighbors_directed(node, petgraph::Direction::Outgoing)
                .next()
                .is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructible_from_empty_node_set() {
        let g = DepGraph::new(0, 42);
        assert_eq!(g.tx_count, 0);
        assert_eq!(g.edge_count(), 0);
        assert_eq!(g.block_number, 42);
    }

    #[test]
    fn nodes_carry_tx_index_as_weight() {
        let g = DepGraph::new(5, 1);
        for i in 0..5 {
            let node = g.node_for_tx(i);
            assert_eq!(g.graph[node], i);
        }
    }

    #[test]
    fn node_with_no_edges_is_independent() {
        let g = DepGraph::new(3, 1);
        assert!(g.is_independent(0));
        assert!(g.is_independent(1));
        assert!(g.is_independent(2));
    }

    #[test]
    fn node_with_edges_is_not_independent() {
        let mut g = DepGraph::new(3, 1);
        let a = g.node_for_tx(0);
        let b = g.node_for_tx(1);
        g.graph.add_edge(a, b, ConflictType::WriteAfterWrite);
        assert!(!g.is_independent(0));
        assert!(!g.is_independent(1));
        assert!(g.is_independent(2));
    }
}
