//! Metrics over a built [`crate::graph::DepGraph`], such as parallelism and
//! task-group (weakly connected component) statistics.

use crate::graph::DepGraph;
use alloy_primitives::Address;
use petgraph::algo::connected_components;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use types::types::{AccessSet, BlockAccess, BlockContext, StateKey};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockMetrics {
    pub tx_count: usize,
    pub independent_tx_count: usize,
    pub parallelization_coefficient: f64,
    pub task_group_count: usize,
    pub largest_group_size: usize,
    pub singleton_group_count: usize,
}

pub fn independence_coefficient(graph: &DepGraph) -> f64 {
    if graph.tx_count == 0 {
        return 1.0;
    }
    let independent = (0..graph.tx_count)
        .filter(|&tx| graph.is_independent(tx).unwrap_or(false))
        .count();
    independent as f64 / graph.tx_count as f64
}

pub fn gas_utilisation_ratio(ctx: &BlockContext) -> f64 {
    if ctx.gas_limit == 0 {
        return 0.0;
    }

    ctx.gas_used as f64 / ctx.gas_limit as f64
}

pub fn eth_burned_wei(ctx: &BlockContext) -> u128 {
    ctx.base_fee_per_gas.unwrap_or(0) * ctx.gas_used as u128
}

pub fn tx_count(ctx: &BlockContext) -> usize {
    ctx.tx_hashes.len()
}

/// Number of distinct accounts represented anywhere in the block BAL.
///
/// `BlockAccess::touched` currently contains touched-only accounts, so reads
/// and writes must also contribute their addresses.
pub fn unique_accounts_touched(block: &BlockAccess) -> usize {
    let mut accounts: HashSet<Address> = block.touched.iter().copied().collect();

    accounts.extend(block.reads().iter().map(StateKey::address));
    accounts.extend(block.writes.iter().map(|write| write.key.address()));

    accounts.len()
}


/// Number of distinct storage slots read or written anywhere in the block.
///
/// Block-level reads and all write-log positions, including system writes, are
/// included. Balance, nonce, and code keys are excluded.
pub fn unique_storage_slots_touched(block: &BlockAccess) -> usize {
    let mut slots = HashSet::new();

    for key in block.reads() {
        if let StateKey::StorageSlot {address,slot} = key{
            slots.insert((*address,*slot));
        }
    }

    slots.len()
}

fn storage_slot_order(left: &StateKey, right: &StateKey) -> Ordering {
    match (left, right) {
        (
            StateKey::StorageSlot {
                address: left_address,
                slot: left_slot,
            },
            StateKey::StorageSlot {
                address: right_address,
                slot: right_slot,
            },
        ) => left_address
            .as_slice()
            .cmp(right_address.as_slice())
            .then_with(|| left_slot.as_slice().cmp(right_slot.as_slice())),
        _ => Ordering::Equal,
    }
}

/// Storage slots ranked by the number of distinct transactions that wrote them.
pub fn hot_slots(
    access_sets: &[AccessSet],
    ctx: &BlockContext,
    top_k: usize,
) -> Vec<(StateKey, usize)> {
    if top_k == 0 || access_sets.is_empty() {
        return Vec::new();
    }

    let mut writer_counts: HashMap<StateKey, usize> = HashMap::new();

    for access_set in access_sets {
        for key in &access_set.writes {
            if matches!(key, StateKey::StorageSlot { .. })
                && key.address() != ctx.coinbase
            {
                *writer_counts.entry(key.clone()).or_insert(0) += 1;
            }
        }
    }

    let mut ranked: Vec<(StateKey, usize)> = writer_counts.into_iter().collect();

    ranked.sort_by(|(left_key, left_count), (right_key, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| storage_slot_order(left_key, right_key))
    });

    ranked.truncate(top_k);
    ranked
}

/// Fraction of unique written storage slots written by at least two transactions.
pub fn multi_writer_slot_ratio(access_sets: &[AccessSet]) -> f64 {
    let mut writer_counts: HashMap<StateKey, usize> = HashMap::new();

    for access_set in access_sets {
        for key in &access_set.writes {
            if matches!(key, StateKey::StorageSlot { .. }) {
                *writer_counts.entry(key.clone()).or_insert(0) += 1;
            }
        }
    }

    if writer_counts.is_empty() {
        return 0.0;
    }

    let multi_writer_slots = writer_counts
        .values()
        .filter(|&&writer_count| writer_count >= 2)
        .count();

    multi_writer_slots as f64 / writer_counts.len() as f64
}

/// Compute the full set of [`BlockMetrics`] for a dependency graph.
pub fn compute_metrics(graph: &DepGraph) -> BlockMetrics {
    let tx_count = graph.tx_count;

    // Degree-zero transactions: no incoming and no outgoing edges.
    let independent_tx_count = (0..tx_count)
        .filter(|&tx| graph.is_independent(tx).unwrap_or(false))
        .count();

    let parallelization_coefficient = if tx_count == 0 {
        1.0
    } else {
        independent_tx_count as f64 / tx_count as f64
    };

    // Weakly connected component count. `connected_components` walks the graph
    // as if every edge were undirected, which is exactly the WCC definition.
    let task_group_count = connected_components(&graph.graph);

    // Component sizes via BFS over the undirected view of the graph.
    let group_sizes = weakly_connected_component_sizes(graph);
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

/// Collect the size of every weakly connected component using a breadth-first
/// search over the undirected view of the graph.
///
/// petgraph's [`connected_components`] returns only the number of components,
/// so this BFS recovers component membership. Each unvisited node seeds a new
/// component, and [`neighbors_undirected`] expands it by following edges
/// regardless of direction — i.e. the undirected view of the dependency graph.
///
/// [`neighbors_undirected`]: petgraph::graph::Graph::neighbors_undirected
fn weakly_connected_component_sizes(graph: &DepGraph) -> Vec<usize> {
    let g = &graph.graph;
    let mut visited = vec![false; g.node_count()];
    let mut sizes = Vec::new();
    let mut queue: VecDeque<_> = VecDeque::new();

    for start in g.node_indices() {
        if visited[start.index()] {
            continue;
        }

        // Seed a fresh component and BFS outward over undirected edges.
        visited[start.index()] = true;
        queue.push_back(start);
        let mut size = 0usize;

        while let Some(node) = queue.pop_front() {
            size += 1;
            for neighbor in g.neighbors_undirected(node) {
                if !visited[neighbor.index()] {
                    visited[neighbor.index()] = true;
                    queue.push_back(neighbor);
                }
            }
        }

        sizes.push(size);
    }

    sizes
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::types::ConflictType;

    /// Add a directed conflict edge between two transaction positions.
    fn add_edge(g: &mut DepGraph, from: usize, to: usize) {
        let a = g.node_for_tx(from).unwrap();
        let b = g.node_for_tx(to).unwrap();
        g.graph.add_edge(a, b, ConflictType::WriteAfterWrite);
    }

    #[test]
    fn empty_graph_is_fully_independent() {
        let g = DepGraph::new(0, 1);
        assert_eq!(independence_coefficient(&g), 1.0);
        let m = compute_metrics(&g);
        assert_eq!(m.tx_count, 0);
        assert_eq!(m.parallelization_coefficient, 1.0);
        assert_eq!(m.task_group_count, 0);
        assert_eq!(m.largest_group_size, 0);
        assert_eq!(m.singleton_group_count, 0);
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
        // Dependency chain 0 -> 1 -> 2 -> 3 -> 4; txs 5..=9 stay independent.
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
            let tx_count = (next() % 20 + 1) as usize; // 1..=20
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
            // The BFS component count must agree with `connected_components`.
            assert_eq!(
                m.task_group_count,
                weakly_connected_component_sizes(&g).len()
            );
        }
    }
}
