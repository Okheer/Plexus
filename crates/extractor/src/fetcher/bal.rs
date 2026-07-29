use alloy_eip7928::AccountChanges;

use crate::bal::{fetch_bal, BalError, ClientKind};
use crate::cache::config::CacheConfig;
use crate::cache::io::{read_json, write_json};
use crate::cache::CacheError;
use crate::fetcher::FetchError;
use crate::rpc::client::RpcClient;

use super::block::resolve_block_number;
use super::BlockId;

fn map_fetch_err(e: FetchError) -> BalError {
    match e {
        FetchError::Rpc(e) => BalError::Rpc(e),
        FetchError::Cache(e) => BalError::Cache(e),
        FetchError::BlockNotFound(id) => BalError::BlockNotFound(id),
        FetchError::MalformedResponse { field } => BalError::MalformedResponse { field },
    }
}

pub async fn fetch_bal_cached(
    client: &RpcClient,
    cache: &CacheConfig,
    chain_id: u64,
    kind: ClientKind,
    block_id: BlockId,
) -> Result<Vec<AccountChanges>, BalError> {
    let block_number = resolve_block_number(client, &block_id)
        .await
        .map_err(map_fetch_err)?;

    let path = cache.bal_path(chain_id, block_number);

    match read_json::<Vec<AccountChanges>>(&path) {
        Ok(bal) => return Ok(bal),
        Err(CacheError::NotFound(_)) => {}
        Err(CacheError::Malformed { .. }) => {}
        Err(e) => return Err(e.into()),
    }

    let bal = fetch_bal(client, kind, &BlockId::Number(block_number)).await?;

    write_json(&path, &bal)?;

    Ok(bal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::bal::Bal;
    use serde_json::Value;
    use tempfile::tempdir;
    use wiremock::matchers::method as http_method;
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn rpc_ok_body(req: &Request, result: Value) -> Value {
        let id = serde_json::from_slice::<Value>(&req.body)
            .ok()
            .and_then(|v| v.get("id").cloned())
            .unwrap_or(serde_json::json!(0));
        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
    }

    fn sample_reth_json() -> Value {
        serde_json::json!([
            {
                "address": "0x1111111111111111111111111111111111111111",
                "storageChanges": [
                    { "slot": "0x1", "changes": [ { "blockAccessIndex": "0x1", "newValue": "0x2a" } ] }
                ],
                "storageReads": ["0x7"],
                "balanceChanges": [ { "blockAccessIndex": "0x1", "postBalance": "0xde0b6b3a7640000" } ],
                "nonceChanges": [],
                "codeChanges": []
            }
        ])
    }

    fn sample_accounts() -> Vec<AccountChanges> {
        serde_json::from_value(sample_reth_json()).unwrap()
    }

    fn sample_nethermind_hex() -> Value {
        let bal = Bal::from(sample_accounts());
        Value::String(format!("0x{}", hex::encode(alloy_rlp::encode(&bal))))
    }

    async fn server_returning(result: Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(http_method("POST"))
            .respond_with(move |req: &Request| {
                ResponseTemplate::new(200).set_body_json(rpc_ok_body(req, result.clone()))
            })
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn cache_miss_fetches_via_rpc_and_writes_cache() {
        let server = server_returning(sample_reth_json()).await;
        let client = RpcClient::new(server.uri()).unwrap();
        let dir = tempdir().unwrap();
        let cache = CacheConfig::with_root(dir.path().to_path_buf());

        let bal = fetch_bal_cached(&client, &cache, 1, ClientKind::Reth, BlockId::Number(100))
            .await
            .unwrap();

        assert_eq!(bal.len(), 1);
        assert_eq!(bal[0].storage_changes[0].changes[0].block_access_index, 1);
        assert!(cache.bal_path(1, 100).exists());
    }

    #[tokio::test]
    async fn cache_hit_skips_rpc_entirely() {
        let server = server_returning(sample_reth_json()).await;
        let client = RpcClient::new(server.uri()).unwrap();
        let dir = tempdir().unwrap();
        let cache = CacheConfig::with_root(dir.path().to_path_buf());

        fetch_bal_cached(&client, &cache, 1, ClientKind::Reth, BlockId::Number(100))
            .await
            .unwrap();

        let bal = fetch_bal_cached(&client, &cache, 1, ClientKind::Reth, BlockId::Number(100))
            .await
            .unwrap();

        assert_eq!(bal.len(), 1);
    }

    #[tokio::test]
    async fn malformed_cache_file_triggers_refetch_and_overwrite() {
        let server = server_returning(sample_reth_json()).await;
        let client = RpcClient::new(server.uri()).unwrap();
        let dir = tempdir().unwrap();
        let cache = CacheConfig::with_root(dir.path().to_path_buf());

        let path = cache.bal_path(1, 100);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not valid json").unwrap();

        let bal = fetch_bal_cached(&client, &cache, 1, ClientKind::Reth, BlockId::Number(100))
            .await
            .unwrap();

        assert_eq!(bal.len(), 1);
        let reread: Vec<AccountChanges> = read_json(&path).unwrap();
        assert_eq!(reread, bal);
    }

    #[tokio::test]
    async fn nethermind_cache_miss_fetches_and_writes_cache() {
        let server = server_returning(sample_nethermind_hex()).await;
        let client = RpcClient::new(server.uri()).unwrap();
        let dir = tempdir().unwrap();
        let cache = CacheConfig::with_root(dir.path().to_path_buf());

        let bal = fetch_bal_cached(
            &client,
            &cache,
            1,
            ClientKind::Nethermind,
            BlockId::Number(100),
        )
        .await
        .unwrap();

        assert_eq!(bal.len(), 1);
        assert_eq!(bal[0].storage_changes[0].changes[0].block_access_index, 1);
        assert!(cache.bal_path(1, 100).exists());
    }

    #[tokio::test]
    async fn cache_is_client_agnostic_across_reth_and_nethermind() {
        let server = server_returning(sample_reth_json()).await;
        let client = RpcClient::new(server.uri()).unwrap();
        let dir = tempdir().unwrap();
        let cache = CacheConfig::with_root(dir.path().to_path_buf());

        let via_reth = fetch_bal_cached(&client, &cache, 1, ClientKind::Reth, BlockId::Number(100))
            .await
            .unwrap();

        let via_nethermind = fetch_bal_cached(
            &client,
            &cache,
            1,
            ClientKind::Nethermind,
            BlockId::Number(100),
        )
        .await
        .unwrap();

        assert_eq!(via_reth, via_nethermind);
    }
}
