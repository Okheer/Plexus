use std::str::FromStr;
use alloy_primitives::{Address, B256};
use serde_json::Value;
use thiserror::Error;

use crate::cache::config::CacheConfig;
use crate::cache::io::{read_json,write_json};
use crate::cache::CacheError;
use crate::rpc::{RpcClient,RpcError};
use types::BlockContext;

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
    Rpc(#[From] RpcError),

    #[error("cache error : {0}")]
    Cache(#[From] CacheError),

    #[error("block not found :{0}")]
    BlockNotFound(BlockId),

    #[error("malformed rpc respone for block : missing or invalid field '{field}'")]
    MalformedRespone { field: &'static str},
}

type Result<T> = std::Result::Result<T , FetchError>;