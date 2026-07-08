use crate::cache::{config::CacheConfig, io::read_json, CacheError};
use crate::rpc::client::RpcClient;
use crate::traces::error::TraceError;
use alloy_primitives::B256;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use types::types::BlockContext;

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

#[derive(Debug)]
pub struct TraceConfig {
    pub max_concurrent_tasks: usize,
}

impl Default for TraceConfig {
    fn default() -> Self {
        TraceConfig {
            max_concurrent_tasks: 50, //set to 50
        }
    }
}

impl TraceConfig {
    pub fn with_max_concurrent_tasks(mut self, n: usize) -> Self {
        self.max_concurrent_tasks = n;
        self
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
    max_concurrent_tasks: usize,
) -> (Vec<B256>, Vec<(B256, String)>) {
    let task_limit = Arc::new(Semaphore::new(max_concurrent_tasks));
    let mut set: JoinSet<(B256, Result<(), String>)> = JoinSet::new();

    let tracer_cfg = serde_json::json!({
        "tracer": "prestateTracer",
        "tracerConfig": {"diffMode": true}
    });

    for hash in to_fetch {
        let client = client.clone();
        let cache = cache.clone();

        let permit = task_limit.clone().acquire_owned().await.unwrap();
        let tracer_cfg = tracer_cfg.clone();

        set.spawn(async move {
            let _permit = permit;
            let hex_hash = format!("0x{}", hex::encode(hash));

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
        match result {
            Ok((hash, Ok(()))) => {
                tracing::info!(?hash, "trace fetched and cached");
                fetched.push(hash);
            }
            Ok((hash, Err(reason))) => {
                tracing::warn!(?hash, %reason, "trace fetch failed");
                failed.push((hash, reason));
            }
            Err(join_err) => {
                tracing::error!(
                    %join_err,
                    "a trace fetch task panicked or was cancelled"
                );
            }
        }
    }
    (fetched, failed)
}

/// Fetches and caches prestate traces for every txn in a block
///
/// Reads the cached block_header.json to get list of txn hashes
/// Already cached tx_{Hash},json files are skipped
/// Missing file are concuurently fetched by 'debug_traceTransaction'
pub async fn populate_traces(
    client: Arc<RpcClient>,
    cache: Arc<CacheConfig>,
    chain_id: u64,
    block_number: u64,
    config: TraceConfig,
) -> Result<TraceFetchSummary, TraceError> {
    // read cache file and load the txn from header
    let header_path = cache.block_header_path(chain_id, block_number);
    let block_ctx = read_json::<BlockContext>(&header_path).map_err(|e| match e {
        CacheError::NotFound(_) => TraceError::BlockHeaderNotCached {
            chain_id,
            block_number,
        },
        CacheError::Malformed { .. } => TraceError::BlockHeaderMalformed {
            block_number,
            source: e,
        },
        other => TraceError::Io(other),
    })?;

    tracing::info!(
        block_number,
        tx_count = block_ctx.tx_hashes.len(),
        "starting trace population"
    );

    // Segregate the fetched and missed file
    let (cache_hits, to_fetch) =
        partition_hashes(&cache, chain_id, block_number, block_ctx.tx_hashes);

    tracing::info!(
        hits = cache_hits.len(),
        miss = to_fetch.len(),
        "cache scan complete"
    );

    // fetch missing tx_hash
    let (fetched, failed) = fetch_and_write_traces(
        client,
        cache,
        chain_id,
        block_number,
        to_fetch,
        config.max_concurrent_tasks,
    )
    .await;

    let summary = TraceFetchSummary {
        block_number,
        cache_hits,
        fetched,
        failed,
    };

    tracing::info!(
        block_number,
        hits = summary.cache_hits.len(),
        fetched = summary.fetched.len(),
        failed = summary.failed.len(),
        "trace population complete"
    );
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::config::CacheConfig;
    use alloy_primitives::B256;
    use std::fs::{create_dir_all, File};
    use std::sync::atomic::{AtomicU32, Ordering};
    use tempfile::{tempdir, TempDir};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    // Define a struct that implements Respond
    struct FailFirstResponder {
        call_count: Arc<AtomicU32>,
    }

    impl Respond for FailFirstResponder {
        fn respond(&self, _req: &Request) -> ResponseTemplate {
            let n = self.call_count.fetch_add(1, Ordering::Relaxed);
            if n == 0 {
                // First call fails
                // error(404) is a permanent error thus it will not retry
                ResponseTemplate::new(404)
            } else {
                // All others succeed
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "result": { "pre": {}, "post": {} }
                }))
            }
        }
    }

    // Setting environment once
    fn setup_env() -> (TempDir, u64, u64) {
        let temp_dir = tempdir().unwrap();
        let chain_id = 1;
        let block_number = 100;

        (temp_dir, chain_id, block_number)
    }

    #[test]
    fn test_partial_cache_fetches_only_missing() {
        let (temp_dir, chain_id, block_number) = setup_env();
        let cache = CacheConfig::with_root(temp_dir.path().to_path_buf());

        let hash_cached_1 = B256::from([0x11; 32]);
        let hash_cached_2 = B256::from([0x22; 32]);
        let hash_missing = B256::from([0x33; 32]);

        let all_hashes = vec![hash_cached_1, hash_cached_2, hash_missing];

        //manually flush tx_json file
        create_dir_all(cache.block_dir(chain_id, block_number)).unwrap();
        File::create(cache.tx_path(chain_id, block_number, &hash_cached_1)).unwrap();
        File::create(cache.tx_path(chain_id, block_number, &hash_cached_2)).unwrap();

        let (hits, missed) = partition_hashes(&cache, chain_id, block_number, all_hashes);

        assert_eq!(hits.len(), 2);
        assert!(hits.contains(&hash_cached_1));
        assert!(hits.contains(&hash_cached_2));

        assert_eq!(missed[0], hash_missing);
    }

    #[tokio::test]
    async fn test_missing_block_header_returns_trace_error() {
        //temp file
        let (temp_dir, chain_id, block_number) = setup_env();

        // Added dummy URL as the test never expects to connect to it
        let client = Arc::new(RpcClient::new("http://localhost".to_string()).unwrap());
        let cache = Arc::new(CacheConfig::with_root(temp_dir.path().to_path_buf()));

        //Call populate_trace with a chain_id and block_number that has no block_header.json in that folder.
        let result = populate_traces(
            client,
            cache,
            chain_id,
            block_number,
            TraceConfig::default(),
        )
        .await;
        //Assert that the result is Err(TraceError::BlockHeaderNotCached { .. }).
        assert!(matches!(
            result,
            Err(TraceError::BlockHeaderNotCached { .. })
        ));
    }

    #[tokio::test]
    async fn test_no_cache_fetches_all_and_writes_to_disk() {
        //setting up mock server
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": { "pre": {}, "post": {} }
            })))
            .mount(&mock_server)
            .await;

        let (temp_dir, chain_id, block_number) = setup_env();

        let hash_cached_1 = B256::from([0x11; 32]);
        let hash_cached_2 = B256::from([0x22; 32]);
        let to_fetch = vec![hash_cached_1, hash_cached_2];

        let client = Arc::new(RpcClient::new(mock_server.uri()).unwrap());
        let cache = Arc::new(CacheConfig::with_root(temp_dir.path().to_path_buf()));

        let (_fetched, _failed) =
            fetch_and_write_traces(client, cache.clone(), chain_id, block_number, to_fetch, 50)
                .await;

        //checking if the cache are being writte on disc
        assert!(cache
            .tx_path(chain_id, block_number, &hash_cached_1)
            .exists());
        assert!(cache
            .tx_path(chain_id, block_number, &hash_cached_2)
            .exists());
    }

    #[tokio::test]
    async fn rpc_failure_on_one_hash_does_not_fail_others() {
        let mock_server = MockServer::start().await;
        let call_count = Arc::new(AtomicU32::new(0));
        Mock::given(method("POST"))
            .respond_with(FailFirstResponder { call_count })
            .mount(&mock_server)
            .await;

        let temp_dir = tempdir().unwrap();
        let cache = Arc::new(CacheConfig::with_root(temp_dir.path().to_path_buf()));
        let client = Arc::new(RpcClient::new(mock_server.uri()).unwrap());

        let chain_id = 1u64;
        let block_number = 100u64;

        let hash_a = B256::from([0xAA; 32]);
        let hash_b = B256::from([0xBB; 32]);
        let hash_c = B256::from([0xCC; 32]);

        let (fetched, failed) = fetch_and_write_traces(
            client,
            cache.clone(),
            1,
            100,
            vec![hash_a, hash_b, hash_c],
            50,
        )
        .await;
        // One failed, two succeeded — batch was NOT aborted
        assert_eq!(fetched.len(), 2);
        assert_eq!(failed.len(), 1);

        let files_on_disk = [hash_a, hash_b, hash_c]
            .iter()
            .filter(|h| cache.tx_path(chain_id, block_number, h).exists())
            .count();
        assert_eq!(
            files_on_disk, 2,
            "expected 2 files on disk, found {}",
            files_on_disk
        );
    }
}
