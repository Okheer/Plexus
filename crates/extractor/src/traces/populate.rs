use crate::cache::{config::CacheConfig, io::read_json, CacheError};
use crate::rpc::client::RpcClient;
use alloy_primitives::B256;
use std::sync::Arc;
use tokio::task::JoinSet;

#[derive(Debug)]
pub struct TraceFetchSummary {
    pub block_number: u64,
    pub cache_hits: Vec<B256>,
    pub fetched: Vec<B256>,
    pub failed: Vec<(B256, String)>,
}

impl TraceFetchSummary {
    pub fn total(&self) -> usize {
        self.cache_hits.len() + self.fetched.len() + self.failed.len()
    }

    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

fn partition_hashes(
    cache: &CacheConfig,
    chain_id: u64,
    block_number: u64,
    tx_hashes: Vec<B256>,
) -> (Vec<B256>, Vec<B256>) {
    let mut hits = Vec::new();
    let mut miss = Vec::new();

    for hash in tx_hashes {
        let path = cache.tx_path(chain_id, block_number, &hash);

        if path.exists() {
            hits.push(hash);
        } else {
            miss.push(hash);
        }
    }

    (hits, miss)
}

/// Concurrently fetches traces for missing txn then atomically write them to disk
/// Returns a tuple containing the successful fetched and hashes and any failures
async fn fetch_and_write_traces(
    client: Arc<RpcClient>,
    cache: Arc<CacheConfig>,
    chain_id: u64,
    block_number: u64,
    to_fetch: Vec<B256>,
) -> (Vec<B256>, Vec<(B256, String)>) {
    let mut set: JoinSet<(B256, Result<(), String>)> = JoinSet::new();

    for hash in to_fetch {
        let client = client.clone();
        let cache = cache.clone();

        set.spawn(async move {
            let hex_hash = format!("0x{}", hex::encode(hash));
            let tracer_cfg = serde_json::json!({
                "tracer": "prestateTracer",
                "tracerConfig": {"diffMode": true}
            });

            let trace_result: Result<serde_json::Value, _> = client
                .request("debug_traceTransaction", (hex_hash, tracer_cfg))
                .await;

            let trace = match trace_result {
                Ok(t) => t,
                Err(e) => return (hash, Err(format!("rpc: {e}"))),
            };

            let path = cache.tx_path(chain_id, block_number, &hash);
            match crate::cache::io::write_json(&path, &trace) {
                Ok(()) => (hash, Ok(())),
                Err(e) => (hash, Err(format!("write: {e}"))),
            }
        });
    }

    let mut fetched = Vec::new();
    let mut failed = Vec::new();
    while let Some(result) = set.join_next().await {
        match result.expect("trace task panicked") {
            (hash, Ok(())) => {
                tracing::info!(?hash, "trace fetched and cached");
                fetched.push(hash);
            }
            (hash, Err(reason)) => {
                tracing::warn!(?hash, %reason, "trace fetch failed");
                failed.push((hash, reason));
            }
        }
    }
    (fetched, failed)
}
