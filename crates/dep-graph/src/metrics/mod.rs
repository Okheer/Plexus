//! Metrics over a built [`crate::graph::DepGraph`], such as parallelism and
//! critical-path statistics.

use crate::graph::DepGraph;

/// Fraction of transactions in the block with **no** dependency edges at all
/// (neither incoming nor outgoing).
///
/// * `1.0` — every transaction is independent; the whole block can execute in
///   parallel.
/// * `0.0` — every transaction participates in at least one conflict.
///
/// An empty block is trivially fully parallel, so it returns `1.0`.
pub fn independence_coefficient(graph: &DepGraph) -> f64 {
    if graph.tx_count == 0 {
        return 1.0;
    }
    let independent = (0..graph.tx_count)
        .filter(|&tx| graph.is_independent(tx).unwrap_or(false))
        .count();
    independent as f64 / graph.tx_count as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::types::ConflictType;

    #[test]
    fn empty_graph_is_fully_independent() {
        let g = DepGraph::new(0, 1);
        assert_eq!(independence_coefficient(&g), 1.0);
    }

    #[test]
    fn edgeless_graph_is_fully_independent() {
        let g = DepGraph::new(4, 1);
        assert_eq!(independence_coefficient(&g), 1.0);
    }

    #[test]
    fn single_edge_marks_both_endpoints_dependent() {
        let mut g = DepGraph::new(4, 1);
        let a = g.node_for_tx(0).unwrap();
        let b = g.node_for_tx(1).unwrap();
        g.graph.add_edge(a, b, ConflictType::WriteAfterWrite);
        assert_eq!(independence_coefficient(&g), 0.5);
    }
}
