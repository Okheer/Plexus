use std::str::FromStr;
use alloy_primitives::{Address, B256};
use serde_json::Value;
use thiserror::Error;

use crate::cache::config::CacheConfig;
use crate::cache::io::{read_json,write_json};
use crate::cache::CacheError;
use crate::rpc::client::RpcClient;
use crate::rpc::RpcError;
use types::types::BlockContext;

#[derive(Debug,Clone)]
pub enum BlockId {
    Number(u64),
    Tag(String), // whether it is the latest, final , safe or pending
}

impl BlockId {
    fn as_rpc_param(&self) -> String {
        match self {
            BlockId::Number(n) => format!("0x{:x}",n),
            BlockId::Tag(t) => t.clone(),
        }
    }
}

#[derive(Error,Debug)]
pub enum FetchError {
    #[error("rpc error: {0}")]
    Rpc(#[from] RpcError),

    #[error("cache error : {0}")]
    Cache(#[from] CacheError),

    #[error("block not found :{0:?}")]
    BlockNotFound(BlockId),

    #[error("malformed rpc respone for block : missing or invalid field '{field}'")]
    MalformedResponse { field: &'static str},
}

type Result<T> = std::result::Result<T , FetchError>;

async fn raw_get_block_by_number(client: &RpcClient, block_id: &BlockId) -> Result<Value> {
    let raw: Option<Value> = client.request("eth_getBlockByNumber", (block_id.as_rpc_param(), false)).await?;

    raw.ok_or_else(|| FetchError::BlockNotFound(block_id.clone()))
}

// tag (latest , final etc ) to a concrete block number.
// if block_id is already a number then it will directly return with no rpc call 

async fn resolve_block_number(client: &RpcClient,block_id: &BlockId) -> Result<u64> {
    match block_id {
        BlockId::Number(n) => Ok(*n),
        BlockId::Tag(_) => {
            let raw = raw_get_block_by_number(client,block_id).await?;
            extract_u64(&raw,"number")
        }
    }
}

fn extract_u64(raw: &Value, field: &'static str) -> Result<u64> {
    let s = raw
        .get(field)
        .and_then(Value::as_str)
        .ok_or(FetchError::MalformedResponse { field })?;
    parse_hex_u64(s).map_err(|_| FetchError::MalformedResponse { field})
}

fn extract_b256(raw: &Value , field: &'static str ) -> Result<B256> {
    let s = raw
        .get(field)
        .and_then(Value::as_str)
        .ok_or(FetchError::MalformedResponse {field})?;
    B256::from_str(s).map_err(|_| FetchError::MalformedResponse { field}) 
}

fn extract_address(raw: &Value, field: &'static str ) -> Result<Address> {
    let s = raw 
        .get(field)
        .and_then(Value::as_str)
        .ok_or(FetchError::MalformedResponse {field})?;
    Address::from_str(s).map_err(|_| FetchError::MalformedResponse {field})
}

fn parse_hex_u64(s: &str) -> std::result::Result<u64, std::num::ParseIntError> {
    u64::from_str_radix(s.trim_start_matches("0x"),16)
}

fn parse_hex_u128(s: &str) -> Result<u128> {
    u128::from_str_radix(s.trim_start_matches("0x"),16)
        .map_err(|_| FetchError::MalformedResponse { field: "baseFeePerGas"})
}

fn parse_block_context(raw: &Value, chain_id: u64) -> Result<BlockContext> {
    let number = extract_u64(raw, "number")?;
    let hash = extract_b256(raw, "hash")?;
    let parent_hash = extract_b256(raw, "parentHash")?;
    let coinbase = extract_address(raw, "miner")?;
    let timestamp = extract_u64(raw, "timestamp")?;
    let gas_limit = extract_u64(raw, "gasLimit")?;
    let gas_used = extract_u64(raw, "gasUsed")?;

    // generally base fee is absent on EIP-1559 blocks 
    let base_fee_per_gas = raw
        .get("baseFeePerGas")
        .and_then(Value::as_str)
        .map(|s| parse_hex_u128(s))
        .transpose()?;

    let tx_hashes = raw
        .get("transactions")
        .and_then(Value::as_array)
        .ok_or(FetchError::MalformedResponse { field: "transactions" })?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or(FetchError::MalformedResponse { field: "transactions[]" })
                .and_then(|s| {
                    B256::from_str(s).map_err(|_| FetchError::MalformedResponse {
                        field: "transactions[]",
                    })
                })
        })
        .collect::<Result<Vec<B256>>>()?;

    Ok(BlockContext {
        number,
        hash,
        parent_hash,
        coinbase,
        chain_id,
        timestamp,
        base_fee_per_gas,
        gas_limit,
        gas_used,
        tx_hashes,
    })
}


pub async fn fetch_block_metadata(
    client: &RpcClient,
    cache: &CacheConfig,
    chain_id: u64,
    block_id: BlockId,
) -> Result<BlockContext> {
    todo!()
}