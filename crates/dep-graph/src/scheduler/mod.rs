pub mod error;

use crate::graph::DepGraph;
use error::ScheduleError;
use petgraph::graph::NodeIndex;
use petgraph::Direction;
use std::collections::HashMap;

/// Which scheduling strategy produced a [`ScheduleResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleStrategy {
    Greedy,
    Ols,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxGas {
    pub tx_index: usize,
    pub gas_used: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleResult {
    pub strategy: ScheduleStrategy,
    pub core_count: usize,
    pub simulated_steps: usize,
    pub critical_path_gas: u64,
    pub sequential_gas: u64,
    pub speedup: f64,
}

/// # Errors
///
/// * [`ScheduleError::CyclicGraph`] if the graph is not a DAG.
/// * [`ScheduleError::TxIndexOutOfBounds`] if a gas entry points outside the
///   graph.
pub fn critical_path(graph: &DepGraph, gas: &[TxGas]) -> Result<u64, ScheduleError> {
    let gas_by_tx = gas_lookup(graph, gas)?;

    let order = petgraph::algo::toposort(&graph.graph, None).map_err(|cycle| {
        ScheduleError::CyclicGraph {
            tx_index: graph.graph[cycle.node_id()],
        }
    })?;

    let mut finish = vec![0u64; graph.tx_count];
    for node in order {
        let tx = graph.graph[node];
        let mut best_pred = 0u64;
        for pred in graph.graph.neighbors_directed(node, Direction::Incoming) {
            let pred_tx = graph.graph[pred];
            best_pred = best_pred.max(finish[pred_tx]);
        }
        finish[tx] = gas_by_tx[tx] + best_pred;
    }

    Ok(finish.into_iter().max().unwrap_or(0))
}

/// # Errors
///
/// See [`critical_path`], plus [`ScheduleError::ZeroCores`] when `cores == 0`.
pub fn greedy_schedule(
    graph: &DepGraph,
    gas: &[TxGas],
    cores: usize,
) -> Result<ScheduleResult, ScheduleError> {
    simulate(graph, gas, cores, ScheduleStrategy::Greedy)
}

/// # Errors
///
/// See [`critical_path`], plus [`ScheduleError::ZeroCores`] when `cores == 0`.
pub fn ols_schedule(
    graph: &DepGraph,
    gas: &[TxGas],
    cores: usize,
) -> Result<ScheduleResult, ScheduleError> {
    simulate(graph, gas, cores, ScheduleStrategy::Ols)
}

fn simulate(
    graph: &DepGraph,
    gas: &[TxGas],
    cores: usize,
    strategy: ScheduleStrategy,
) -> Result<ScheduleResult, ScheduleError> {
    if cores == 0 {
        return Err(ScheduleError::ZeroCores);
    }

    let gas_by_tx = gas_lookup(graph, gas)?;
    let critical_path_gas = critical_path(graph, gas)?;
    let sequential_gas: u64 = gas_by_tx.iter().sum();

    let levels = compute_levels(graph);
    let mut parallel_time: u64 = 0;
    for level in &levels {
        let mut core_load = vec![0u64; cores];

        match strategy {
            ScheduleStrategy::Greedy => {
                for (i, &tx) in level.iter().enumerate() {
                    core_load[i % cores] += gas_by_tx[tx];
                }
            }
            ScheduleStrategy::Ols => {
                let mut ordered: Vec<usize> = level.clone();
                ordered.sort_by_key(|&tx| std::cmp::Reverse(gas_by_tx[tx]));
                for &tx in &ordered {
                    let least = least_loaded_core(&core_load);
                    core_load[least] += gas_by_tx[tx];
                }
            }
        }

        parallel_time += core_load.into_iter().max().unwrap_or(0);
    }

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

fn compute_levels(graph: &DepGraph) -> Vec<Vec<usize>> {
    let g = &graph.graph;

    let mut indeg: HashMap<NodeIndex, usize> = HashMap::with_capacity(graph.tx_count);
    for node in g.node_indices() {
        let d = g.neighbors_directed(node, Direction::Incoming).count();
        indeg.insert(node, d);
    }

    // First wave: everything already unblocked.
    let mut current: Vec<NodeIndex> = g
        .node_indices()
        .filter(|node| indeg[node] == 0)
        .collect();

    let mut levels: Vec<Vec<usize>> = Vec::new();
    while !current.is_empty() {
        // Deterministic order within a level.
        current.sort_by_key(|&node| g[node]);

        let mut next: Vec<NodeIndex> = Vec::new();
        for &node in &current {
            for child in g.neighbors_directed(node, Direction::Outgoing) {
                let remaining = indeg
                    .get_mut(&child)
                    .expect("every node was inserted into indeg");
                *remaining -= 1;
                if *remaining == 0 {
                    next.push(child);
                }
            }
        }

        levels.push(current.iter().map(|&node| g[node]).collect());
        current = next;
    }

    levels
}

fn least_loaded_core(core_load: &[u64]) -> usize {
    core_load
        .iter()
        .enumerate()
        .min_by_key(|(_, &load)| load)
        .map(|(idx, _)| idx)
        .expect("core_load is non-empty")
}

/// # Errors
///
/// [`ScheduleError::TxIndexOutOfBounds`] if any entry points outside the graph.
/// Transactions without a gas entry default to `0`.
fn gas_lookup(graph: &DepGraph, gas: &[TxGas]) -> Result<Vec<u64>, ScheduleError> {
    let mut table = vec![0u64; graph.tx_count];
    for entry in gas {
        if entry.tx_index >= graph.tx_count {
            return Err(ScheduleError::TxIndexOutOfBounds {
                tx_index: entry.tx_index,
                tx_count: graph.tx_count,
            });
        }
        table[entry.tx_index] = entry.gas_used;
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::types::ConflictType;

    /// Helper: attach a `gas_used` to every tx index `0..n`.
    fn uniform_gas(n: usize, gas_used: u64) -> Vec<TxGas> {
        (0..n)
            .map(|tx_index| TxGas { tx_index, gas_used })
            .collect()
    }

    /// Helper: add a dependency edge `from -> to` ("`to` depends on `from`").
    fn add_dep(graph: &mut DepGraph, from: usize, to: usize) {
        let a = graph.node_for_tx(from).unwrap();
        let b = graph.node_for_tx(to).unwrap();
        graph.graph.add_edge(a, b, ConflictType::WriteAfterWrite);
    }

    /// Float comparison — never use `==` on `f64`.
    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn linear_chain_of_5_has_critical_path_equal_to_total_and_speedup_one() {
        // 0 -> 1 -> 2 -> 3 -> 4, each tx costs 10 gas.
        let mut g = DepGraph::new(5, 1);
        for i in 0..4 {
            add_dep(&mut g, i, i + 1);
        }
        let gas = uniform_gas(5, 10);

        assert_eq!(critical_path(&g, &gas).unwrap(), 50);

        // A chain is fully serial: greedy speedup is 1.0 for *any* core count.
        for cores in [1usize, 2, 4, 8, 100] {
            let greedy = greedy_schedule(&g, &gas, cores).unwrap();
            assert_eq!(greedy.simulated_steps, 5);
            assert_eq!(greedy.critical_path_gas, 50);
            assert_eq!(greedy.sequential_gas, 50);
            assert!(
                approx(greedy.speedup, 1.0),
                "cores={cores} speedup={}",
                greedy.speedup
            );
        }
    }

    #[test]
    fn ten_independent_equal_gas_txs_on_2_cores_give_speedup_2() {
        // No edges at all: everything is in level 0.
        let g = DepGraph::new(10, 1);
        let gas = uniform_gas(10, 10);

        let greedy = greedy_schedule(&g, &gas, 2).unwrap();
        let ols = ols_schedule(&g, &gas, 2).unwrap();

        assert_eq!(greedy.simulated_steps, 1);
        assert_eq!(ols.simulated_steps, 1);
        assert!(approx(greedy.speedup, 2.0), "greedy={}", greedy.speedup);
        assert!(approx(ols.speedup, 2.0), "ols={}", ols.speedup);
    }

    #[test]
    fn ols_speedup_is_never_worse_than_greedy_across_50_graphs() {
        // 50 deterministically-generated DAGs with varied shapes, gas, and core
        // counts. Edges only ever go from a lower tx index to a higher one,
        // which guarantees acyclicity by construction.
        for seed in 0u64..50 {
            let n = 4 + (seed as usize % 9); // 4..=12 transactions
            let cores = 1 + (seed as usize % 4); // 1..=4 cores
            let mut g = DepGraph::new(n, seed);

            // Pseudo-random but fixed edge/gas generation.
            let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
            let mut next = || {
                state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                state >> 33
            };

            let mut gas = Vec::with_capacity(n);
            for tx_index in 0..n {
                gas.push(TxGas {
                    tx_index,
                    gas_used: 1 + next() % 100,
                });
                // Maybe add a dependency on an earlier transaction.
                if tx_index > 0 && next() % 2 == 0 {
                    let from = (next() as usize) % tx_index;
                    add_dep(&mut g, from, tx_index);
                }
            }

            let greedy = greedy_schedule(&g, &gas, cores).unwrap();
            let ols = ols_schedule(&g, &gas, cores).unwrap();

            assert!(
                ols.speedup >= greedy.speedup - 1e-9,
                "seed={seed} n={n} cores={cores}: ols={} < greedy={}",
                ols.speedup,
                greedy.speedup
            );
        }
    }

    #[test]
    fn critical_path_never_exceeds_sequential_gas() {
        for seed in 0u64..50 {
            let n = 3 + (seed as usize % 8);
            let mut g = DepGraph::new(n, seed);
            let mut gas = Vec::with_capacity(n);
            for tx_index in 0..n {
                gas.push(TxGas {
                    tx_index,
                    gas_used: 1 + (seed + tx_index as u64) % 50,
                });
                if tx_index > 0 {
                    add_dep(&mut g, tx_index - 1, tx_index);
                }
            }
            let sequential: u64 = gas.iter().map(|t| t.gas_used).sum();
            assert!(critical_path(&g, &gas).unwrap() <= sequential);
        }
    }

    #[test]
    fn zero_cores_is_rejected() {
        let g = DepGraph::new(3, 1);
        let gas = uniform_gas(3, 10);
        assert_eq!(
            greedy_schedule(&g, &gas, 0).unwrap_err(),
            ScheduleError::ZeroCores
        );
        assert_eq!(
            ols_schedule(&g, &gas, 0).unwrap_err(),
            ScheduleError::ZeroCores
        );
    }

    #[test]
    fn cycle_is_reported() {
        // 0 -> 1 -> 0 is a cycle.
        let mut g = DepGraph::new(2, 1);
        add_dep(&mut g, 0, 1);
        add_dep(&mut g, 1, 0);
        let gas = uniform_gas(2, 10);
        assert!(matches!(
            critical_path(&g, &gas),
            Err(ScheduleError::CyclicGraph { .. })
        ));
    }

    #[test]
    fn out_of_bounds_gas_entry_is_rejected() {
        let g = DepGraph::new(2, 1);
        let gas = vec![TxGas {
            tx_index: 5,
            gas_used: 1,
        }];
        assert_eq!(
            critical_path(&g, &gas).unwrap_err(),
            ScheduleError::TxIndexOutOfBounds {
                tx_index: 5,
                tx_count: 2,
            }
        );
    }

    #[test]
    fn diamond_dependency_critical_path_takes_the_heavier_branch() {
        let mut g = DepGraph::new(4, 1);
        add_dep(&mut g, 0, 1);
        add_dep(&mut g, 0, 2);
        add_dep(&mut g, 1, 3);
        add_dep(&mut g, 2, 3);
        let gas = vec![
            TxGas { tx_index: 0, gas_used: 10 },
            TxGas { tx_index: 1, gas_used: 5 },
            TxGas { tx_index: 2, gas_used: 20 },
            TxGas { tx_index: 3, gas_used: 10 },
        ];
        assert_eq!(critical_path(&g, &gas).unwrap(), 40);
    }
}