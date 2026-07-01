use std::str::FromStr;
use alloy_primitives::{Address, B256};
use serde_json::Value;
use thiserror::Error;

use crate::cache::config::CacheConfig;
use crate::cache::io::{read_json,write_json};
use crate::cache::CacheError;
use crate::rpc::{RpcClient,RpcError};
use types::BlockContext;