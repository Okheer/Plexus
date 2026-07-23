//! Metrics over a built [`crate::graph::DepGraph`], such as parallelism and
//! task-group (weakly connected component) statistics.

use crate::graph::DepGraph;

/// Aggregate, block-level statistics derived from a [`DepGraph`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockMetrics {
    pub tx_count: usize,
    pub independent_tx_count: usize,
    pub parallelization_coefficient: f64,
    pub task_group_count: usize,
    pub largest_group_size: usize,
    pub singleton_group_count: usize,
}

/// Fraction of transactions with no dependency edges at all.
pub fn independence_coefficient(graph: &DepGraph) -> f64 {
    if graph.tx_count == 0 {
        return 1.0;
    }
    let independent = (0..graph.tx_count)
        .filter(|&tx| graph.is_independent(tx).unwrap_or(false))
        .count();
    independent as f64 / graph.tx_count as f64
}

/// Compute the full set of [`BlockMetrics`] in a single pass.
pub fn compute_metrics(graph: &DepGraph) -> BlockMetrics {
    let tx_count = graph.tx_count;

    let independent_tx_count = (0..tx_count)
        .filter(|&tx| graph.is_independent(tx).unwrap_or(false))
        .count();

    let parallelization_coefficient = if tx_count == 0 {
        1.0
    } else {
        independent_tx_count as f64 / tx_count as f64
    };

    let node_count = graph.graph.node_count();
    let mut dsu = DisjointSet::new(node_count);
    for edge in graph.graph.edge_indices() {
        if let Some((source, target)) = graph.graph.edge_endpoints(edge) {
            dsu.union(source.index(), target.index());
        }
    }

    let mut group_sizes = vec![0usize; node_count];
    for node in 0..node_count {
        let root = dsu.find(node);
        group_sizes[root] += 1;
    }

    let task_group_count = group_sizes.iter().filter(|&&size| size > 0).count();
    let largest_group_size = group_sizes.iter().copied().max().unwrap_or(0);
    let singleton_group_count = group_sizes.iter().filter(|&&size| size == 1).count();

    BlockMetrics {
        tx_count,
        independent_tx_count,
        parallelization_coefficient,
        task_group_count,
        largest_group_size,
        singleton_group_count,
    }
}

/// Minimal union–find with path compression and union by size.
struct DisjointSet {
    parent: Vec<usize>,
    size: Vec<usize>,
}

impl DisjointSet {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            size: vec![1; n],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        let mut root = x;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut node = x;
        while self.parent[node] != root {
            let next = self.parent[node];
            self.parent[node] = root;
            node = next;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        let (big, small) = if self.size[ra] >= self.size[rb] {
            (ra, rb)
        } else {
            (rb, ra)
        };
        self.parent[small] = big;
        self.size[big] += self.size[small];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::types::ConflictType;

    fn add_edge(g: &mut DepGraph, from: usize, to: usize) {
        let a = g.node_for_tx(from).unwrap();
        let b = g.node_for_tx(to).unwrap();
        g.graph.add_edge(a, b, ConflictType::WriteAfterWrite);
    }

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
        add_edge(&mut g, 0, 1);
        assert_eq!(independence_coefficient(&g), 0.5);
    }

    #[test]
    fn ten_independent_txs() {
        let g = DepGraph::new(10, 1);
        let m = compute_metrics(&g);
        assert_eq!(m.tx_count, 10);
        assert_eq!(m.independent_tx_count, 10);
        assert_eq!(m.parallelization_coefficient, 1.0);
        assert_eq!(m.task_group_count, 10);
        assert_eq!(m.largest_group_size, 1);
        assert_eq!(m.singleton_group_count, 10);
    }

    #[test]
    fn chain_of_five_plus_five_independent() {
        let mut g = DepGraph::new(10, 1);
        for i in 0..4 {
            add_edge(&mut g, i, i + 1);
        }
        let m = compute_metrics(&g);
        assert_eq!(m.tx_count, 10);
        assert_eq!(m.independent_tx_count, 5);
        assert_eq!(m.parallelization_coefficient, 0.5);
        assert_eq!(m.task_group_count, 6);
        assert_eq!(m.largest_group_size, 5);
        assert_eq!(m.singleton_group_count, 5);
    }

    #[test]
    fn single_chain_of_ten() {
        let mut g = DepGraph::new(10, 1);
        for i in 0..9 {
            add_edge(&mut g, i, i + 1);
        }
        let m = compute_metrics(&g);
        assert_eq!(m.tx_count, 10);
        assert_eq!(m.independent_tx_count, 0);
        assert_eq!(m.parallelization_coefficient, 0.0);
        assert_eq!(m.task_group_count, 1);
        assert_eq!(m.largest_group_size, 10);
        assert_eq!(m.singleton_group_count, 0);
    }

    #[test]
    fn property_bounds_hold_over_random_graphs() {
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        for _ in 0..100 {
            let tx_count = (next() % 20 + 1) as usize;
            let mut g = DepGraph::new(tx_count, 1);

            let edge_attempts = next() % (tx_count as u64 * 2 + 1);
            for _ in 0..edge_attempts {
                let a = (next() as usize) % tx_count;
                let b = (next() as usize) % tx_count;
                if a != b {
                    add_edge(&mut g, a, b);
                }
            }

            let m = compute_metrics(&g);

            assert!((0.0..=1.0).contains(&m.parallelization_coefficient));
            assert!(m.task_group_count >= 1 && m.task_group_count <= tx_count);
            assert!(m.largest_group_size >= 1 && m.largest_group_size <= tx_count);
            assert!(m.independent_tx_count <= tx_count);
        }
    }
    #[test]
    fn empty_graph_compute_metrics() {
        let g = DepGraph::new(0, 1);
        let m = compute_metrics(&g);
        assert_eq!(m.tx_count, 0);
        assert_eq!(m.independent_tx_count, 0);
        assert_eq!(m.parallelization_coefficient, 1.0);
        assert_eq!(m.task_group_count, 0);
        assert_eq!(m.largest_group_size, 0);
        assert_eq!(m.singleton_group_count, 0);
    }
}
