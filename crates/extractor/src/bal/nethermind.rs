use alloy_eip7928::bal::DecodedBal;
use alloy_eip7928::AccountChanges;
use alloy_primitives::Bytes;

use crate::bal::error::BalError;
use crate::fetcher::BlockId;
use crate::rpc::client::RpcClient;

pub async fn fetch_nethermind_bal(
    client: &RpcClient,
    block_id: &BlockId,
) -> Result<Vec<AccountChanges>, BalError> {
    let raw: Option<String> = client
        .request("debug_getRawBlockAccessList", (block_id.as_rpc_param(),))
        .await?;

    let raw = raw.ok_or_else(|| BalError::BlockNotFound(block_id.clone()))?;

    decode_raw_bal(&raw)
}

pub fn decode_raw_bal(raw_hex: &str) -> Result<Vec<AccountChanges>, BalError> {
    let stripped = raw_hex.strip_prefix("0x").unwrap_or(raw_hex);
    let bytes = hex::decode(stripped)?;
    let decoded = DecodedBal::from_rlp_bytes(Bytes::from(bytes))?;
    Ok(decoded.split().0.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::bal::Bal;
    use serde_json::Value;
    use wiremock::matchers::method as http_method;
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn rpc_ok_body(req: &Request, result: Value) -> Value {
        let id = serde_json::from_slice::<Value>(&req.body)
            .ok()
            .and_then(|v| v.get("id").cloned())
            .unwrap_or(serde_json::json!(0));
        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
    }

    fn sample_accounts() -> Vec<AccountChanges> {
        serde_json::from_value(serde_json::json!([
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
        ]))
        .unwrap()
    }

    fn rlp_hex(accounts: &[AccountChanges]) -> String {
        let bal = Bal::from(accounts.to_vec());
        format!("0x{}", hex::encode(alloy_rlp::encode(&bal)))
    }

    async fn server_returning(result: Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(http_method("POST"))
            .respond_with(move |req: &Request| {
                ResponseTemplate::new(200).set_body_json(rpc_ok_body(req, result.clone()))
            })
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn fetches_and_decodes_a_nethermind_bal() {
        let server = server_returning(Value::String(rlp_hex(&sample_accounts()))).await;
        let client = RpcClient::new(server.uri()).unwrap();

        let bal = fetch_nethermind_bal(&client, &BlockId::Number(100))
            .await
            .unwrap();

        assert_eq!(bal.len(), 1);
        assert_eq!(bal[0].storage_changes[0].changes[0].block_access_index, 1);
        assert_eq!(bal[0].storage_reads.len(), 1);
    }

    #[tokio::test]
    async fn empty_bal_decodes_to_an_empty_list() {
        let server = server_returning(Value::String(rlp_hex(&[]))).await;
        let client = RpcClient::new(server.uri()).unwrap();

        let bal = fetch_nethermind_bal(&client, &BlockId::Number(100))
            .await
            .unwrap();

        assert!(bal.is_empty());
    }

    #[tokio::test]
    async fn null_result_is_block_not_found() {
        let server = server_returning(Value::Null).await;
        let client = RpcClient::new(server.uri()).unwrap();

        let err = fetch_nethermind_bal(&client, &BlockId::Number(100))
            .await
            .unwrap_err();

        assert!(matches!(err, BalError::BlockNotFound(BlockId::Number(100))));
    }

    #[tokio::test]
    async fn invalid_hex_is_reported() {
        let server = server_returning(Value::String("0xnothex".into())).await;
        let client = RpcClient::new(server.uri()).unwrap();

        let err = fetch_nethermind_bal(&client, &BlockId::Number(100))
            .await
            .unwrap_err();

        assert!(matches!(err, BalError::BalHex(_)));
    }

    #[tokio::test]
    async fn invalid_rlp_is_reported() {
        // 0x80 is an empty string, not the list the BAL wrapper expects
        let server = server_returning(Value::String("0x80".into())).await;
        let client = RpcClient::new(server.uri()).unwrap();

        let err = fetch_nethermind_bal(&client, &BlockId::Number(100))
            .await
            .unwrap_err();

        assert!(matches!(err, BalError::BalRlp(_)));
    }

    #[tokio::test]
    async fn tag_is_passed_through_as_an_rpc_param() {
        let server = server_returning(Value::String(rlp_hex(&sample_accounts()))).await;
        let client = RpcClient::new(server.uri()).unwrap();

        fetch_nethermind_bal(&client, &BlockId::Tag("latest".into()))
            .await
            .unwrap();

        let body =
            serde_json::from_slice::<Value>(&server.received_requests().await.unwrap()[0].body)
                .unwrap();
        assert_eq!(body["method"], "debug_getRawBlockAccessList");
        assert_eq!(body["params"][0], "latest");
    }
}
