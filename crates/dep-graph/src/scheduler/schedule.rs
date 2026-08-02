//! Scheduling logic: critical-path computation and the Greedy / OLS
//! parallel-execution simulations.

use std::collections::{HashMap, VecDeque};

use petgraph::graph::NodeIndex;
use petgraph::Direction;

use crate::graph::DepGraph;
use crate::scheduler::error::ScheduleError;

/// Gas cost associated with a single transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxGas {
    pub tx_index: usize,
    pub gas_used: u64,
}

/// Which scheduling strategy produced a given result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleStrategy {
    /// BFS level-by-level, round-robin assignment to cores.
    Greedy,
    /// Level-by-level, largest-gas-first onto the least-loaded core.
    Ols,
}

/// Result of simulating a scheduling strategy over a dependency graph.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleResult {
    pub strategy: ScheduleStrategy,
    pub core_count: usize,
    pub simulated_steps: usize,
    pub critical_path_gas: u64,
    pub sequential_gas: u64,
    pub speedup: f64,
}

/// Build a `tx_index -> gas_used` lookup, validating indices against the graph.
fn gas_lookup(graph: &DepGraph, gas: &[TxGas]) -> Result<HashMap<usize, u64>, ScheduleError> {
    let mut map = HashMap::with_capacity(gas.len());
    for tx in gas {
        if tx.tx_index >= graph.tx_count {
            return Err(ScheduleError::TxIndexOutOfBounds {
                tx_index: tx.tx_index,
                tx_count: graph.tx_count,
            });
        }
        map.insert(tx.tx_index, tx.gas_used);
    }
    Ok(map)
}

/// Gas for a node, defaulting to 0 when the tx has no entry in `gas`.
fn node_gas(graph: &DepGraph, node: NodeIndex, gas: &HashMap<usize, u64>) -> u64 {
    let tx_index = graph.graph[node];
    *gas.get(&tx_index).unwrap_or(&0)
}

/// Gas-weighted longest dependency chain (the latency floor).
///
/// Uses a topological order, then DP: `finish[n] = gas[n] + max(finish[preds])`.
pub fn critical_path(graph: &DepGraph, gas: &[TxGas]) -> Result<u64, ScheduleError> {
    let gas_map = gas_lookup(graph, gas)?;

    let topo = petgraph::algo::toposort(&graph.graph, None).map_err(|cycle| {
        let tx_index = graph.graph[cycle.node_id()];
        ScheduleError::CyclicGraph { tx_index }
    })?;

    let mut finish: HashMap<NodeIndex, u64> = HashMap::with_capacity(topo.len());
    let mut best = 0u64;

    for node in topo {
        let mut pred_max = 0u64;
        for pred in graph.graph.neighbors_directed(node, Direction::Incoming) {
            pred_max = pred_max.max(*finish.get(&pred).unwrap_or(&0));
        }
        let f = node_gas(graph, node, &gas_map) + pred_max;
        finish.insert(node, f);
        best = best.max(f);
    }

    Ok(best)
}

/// Compute BFS "levels" (waves) via Kahn's algorithm. Each level is the set of
/// nodes that become ready once the previous level completes. Shared by both
/// scheduling strategies so their level structure is always identical.
fn compute_levels(graph: &DepGraph) -> Result<Vec<Vec<NodeIndex>>, ScheduleError> {
    let g = &graph.graph;

    let mut in_degree: HashMap<NodeIndex, usize> = HashMap::with_capacity(g.node_count());
    for node in g.node_indices() {
        in_degree.insert(
            node,
            g.neighbors_directed(node, Direction::Incoming).count(),
        );
    }

    let mut ready: VecDeque<NodeIndex> = g.node_indices().filter(|n| in_degree[n] == 0).collect();

    let mut levels: Vec<Vec<NodeIndex>> = Vec::new();
    let mut processed = 0usize;

    while !ready.is_empty() {
        let level: Vec<NodeIndex> = ready.drain(..).collect();
        processed += level.len();

        let mut next: VecDeque<NodeIndex> = VecDeque::new();
        for &node in &level {
            for child in g.neighbors_directed(node, Direction::Outgoing) {
                let d = in_degree.get_mut(&child).expect("child in in_degree map");
                *d -= 1;
                if *d == 0 {
                    next.push_back(child);
                }
            }
        }
        levels.push(level);
        ready = next;
    }

    // If not every node was processed, a cycle exists.
    if processed != g.node_count() {
        let stuck = g
            .node_indices()
            .find(|n| in_degree[n] > 0)
            .expect("a stuck node must exist when processed < node_count");
        return Err(ScheduleError::CyclicGraph { tx_index: g[stuck] });
    }

    Ok(levels)
}

/// Shared driver: simulate `levels` under a per-level core-assignment policy.
fn simulate(
    graph: &DepGraph,
    gas: &[TxGas],
    cores: usize,
    strategy: ScheduleStrategy,
) -> Result<ScheduleResult, ScheduleError> {
    if cores == 0 {
        return Err(ScheduleError::ZeroCores);
    }

    let gas_map = gas_lookup(graph, gas)?;
    let levels = compute_levels(graph)?;

    let sequential_gas: u64 = graph
        .graph
        .node_indices()
        .map(|n| node_gas(graph, n, &gas_map))
        .sum();

    let mut parallel_time: u64 = 0;

    for level in &levels {
        // Per-level per-core accumulated load.
        let mut loads = vec![0u64; cores];

        match strategy {
            ScheduleStrategy::Greedy => {
                // Round-robin: tx at position i -> core i % cores.
                for (i, &node) in level.iter().enumerate() {
                    loads[i % cores] += node_gas(graph, node, &gas_map);
                }
            }
            ScheduleStrategy::Ols => {
                // Largest-gas-first onto the least-loaded core.
                let mut items: Vec<u64> = level
                    .iter()
                    .map(|&n| node_gas(graph, n, &gas_map))
                    .collect();
                items.sort_unstable_by(|a, b| b.cmp(a));
                for g in items {
                    let min_core = loads
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, &load)| load)
                        .map(|(idx, _)| idx)
                        .expect("cores >= 1");
                    loads[min_core] += g;
                }
            }
        }

        // The level finishes when its busiest core finishes.
        parallel_time += loads.into_iter().max().unwrap_or(0);
    }

    let critical_path_gas = critical_path(graph, gas)?;

    let speedup = if parallel_time == 0 {
        0.0
    } else {
        sequential_gas as f64 / parallel_time as f64
    };

    Ok(ScheduleResult {
        strategy,
        core_count: cores,
        simulated_steps: levels.len(),
        critical_path_gas,
        sequential_gas,
        speedup,
    })
}

/// Greedy strategy: BFS levels, round-robin assignment across `cores`.
pub fn greedy_schedule(
    graph: &DepGraph,
    gas: &[TxGas],
    cores: usize,
) -> Result<ScheduleResult, ScheduleError> {
    simulate(graph, gas, cores, ScheduleStrategy::Greedy)
}

/// OLS strategy: BFS levels, largest-gas-first onto the least-loaded core.
pub fn ols_schedule(
    graph: &DepGraph,
    gas: &[TxGas],
    cores: usize,
) -> Result<ScheduleResult, ScheduleError> {
    simulate(graph, gas, cores, ScheduleStrategy::Ols)
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::types::ConflictType;

    fn gas_of(n: usize, g: u64) -> TxGas {
        TxGas {
            tx_index: n,
            gas_used: g,
        }
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// Add a dependency edge `from -> to` (to depends on from).
    fn add_dep(graph: &mut DepGraph, from: usize, to: usize) {
        let a = graph.node_for_tx(from).unwrap();
        let b = graph.node_for_tx(to).unwrap();
        graph.graph.add_edge(a, b, ConflictType::WriteAfterWrite);
    }

    #[test]
    fn linear_chain_speedup_is_one() {
        // 0 -> 1 -> 2 -> 3 -> 4, each 100 gas.
        let mut graph = DepGraph::new(5, 1);
        for i in 0..4 {
            add_dep(&mut graph, i, i + 1);
        }
        let gas: Vec<TxGas> = (0..5).map(|i| gas_of(i, 100)).collect();

        let cp = critical_path(&graph, &gas).unwrap();
        assert_eq!(cp, 500, "critical path of a chain is the total gas");

        for cores in [1usize, 2, 4, 8] {
            let r = greedy_schedule(&graph, &gas, cores).unwrap();
            assert!(
                approx(r.speedup, 1.0),
                "chain has no parallelism at {cores} cores, got {}",
                r.speedup
            );
        }
    }

    #[test]
    fn independent_txs_speedup_is_two_on_two_cores() {
        // 10 independent txs, equal gas, 2 cores.
        let graph = DepGraph::new(10, 1);
        let gas: Vec<TxGas> = (0..10).map(|i| gas_of(i, 100)).collect();

        let greedy = greedy_schedule(&graph, &gas, 2).unwrap();
        let ols = ols_schedule(&graph, &gas, 2).unwrap();

        assert!(approx(greedy.speedup, 2.0), "greedy: {}", greedy.speedup);
        assert!(approx(ols.speedup, 2.0), "ols: {}", ols.speedup);
    }

    #[test]
    fn critical_path_never_exceeds_sequential_gas() {
        let mut graph = DepGraph::new(6, 1);
        add_dep(&mut graph, 0, 2);
        add_dep(&mut graph, 1, 2);
        add_dep(&mut graph, 2, 3);
        add_dep(&mut graph, 3, 5);
        add_dep(&mut graph, 4, 5);
        let gas: Vec<TxGas> = (0..6).map(|i| gas_of(i, 50 + i as u64 * 10)).collect();

        let r = greedy_schedule(&graph, &gas, 4).unwrap();
        assert!(r.critical_path_gas <= r.sequential_gas);
    }

    #[test]
    fn diamond_critical_path_takes_heavier_branch() {
        // 0 -> 1 -> 3 and 0 -> 2 -> 3. Branch 2 is heavier.
        let mut graph = DepGraph::new(4, 1);
        add_dep(&mut graph, 0, 1);
        add_dep(&mut graph, 0, 2);
        add_dep(&mut graph, 1, 3);
        add_dep(&mut graph, 2, 3);
        let gas = vec![gas_of(0, 10), gas_of(1, 20), gas_of(2, 100), gas_of(3, 5)];
        // 0 -> 2 -> 3 = 10 + 100 + 5 = 115.
        assert_eq!(critical_path(&graph, &gas).unwrap(), 115);
    }

    #[test]
    fn ols_at_least_greedy_over_many_graphs() {
        // 50 deterministic pseudo-random DAGs; OLS speedup must never be worse.
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        for case in 0..50 {
            let n = 4 + (case % 9); // 4..=12 txs
            let mut graph = DepGraph::new(n, 1);

            // Only add forward edges (i -> j, i < j) to guarantee acyclicity.
            for i in 0..n {
                for j in (i + 1)..n {
                    if next() % 3 == 0 {
                        add_dep(&mut graph, i, j);
                    }
                }
            }

            let gas: Vec<TxGas> = (0..n).map(|i| gas_of(i, 1 + (next() % 500))).collect();

            let cores = 1 + (case % 4); // 1..=4 cores

            let greedy = greedy_schedule(&graph, &gas, cores).unwrap();
            let ols = ols_schedule(&graph, &gas, cores).unwrap();

            assert!(
                ols.speedup >= greedy.speedup - 1e-9,
                "case {case}: OLS {} < Greedy {} ({n} txs, {cores} cores)",
                ols.speedup,
                greedy.speedup
            );
        }
    }

    #[test]
    fn zero_cores_is_an_error() {
        let graph = DepGraph::new(3, 1);
        let gas: Vec<TxGas> = (0..3).map(|i| gas_of(i, 10)).collect();
        assert!(matches!(
            greedy_schedule(&graph, &gas, 0),
            Err(ScheduleError::ZeroCores)
        ));
    }

    #[test]
    fn out_of_bounds_tx_index_is_an_error() {
        let graph = DepGraph::new(2, 1);
        let gas = vec![gas_of(0, 10), gas_of(5, 10)];
        assert!(matches!(
            critical_path(&graph, &gas),
            Err(ScheduleError::TxIndexOutOfBounds {
                tx_index: 5,
                tx_count: 2
            })
        ));
    }
}
