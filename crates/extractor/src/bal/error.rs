use crate::cache::CacheError;
use crate::fetcher::BlockId;
use crate::rpc::RpcError;
use alloy_primitives::{Address, U256};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum BalError {
    #[error("rpc error: {0}")]
    Rpc(#[from] RpcError),

    #[error("cache error: {0}")]
    Cache(#[from] CacheError),

    #[error("block not found: {0:?}")]
    BlockNotFound(BlockId),

    #[error("malformed bal response: missing or invalid field '{field}'")]
    MalformedResponse { field: &'static str },

    /// EIP-7928 assigns index 0 to pre-execution system calls, 1..=n to the n
    /// transactions in block order, and n+1 to post-execution system calls.
    /// Anything above n+1 cannot be attributed and means the client and the
    /// block header disagree about how many transactions the block holds.
    #[error("block access index {index} out of range for a block with {tx_count} transactions")]
    InvalidBlockAccessIndex { index: u64, tx_count: usize },

    #[error("unsupported client '{name}', expected one of: reth, nethermind")]
    UnsupportedClient { name: String },

    #[error("malformed bal: slot {slot} on address {address} appears in both storage_reads and storage_changes")]
    DisjointnessViolation { address: Address, slot: U256 },

    #[error("index {index} not fitting in a uint32 as per EIP")]
    IndexTooLarge { index: u64 },

    #[error(
        "BAL with item count being {item_count} is too large, max items - {max_items}, gas limit being {gas_limit}"
    )]
    BlockAccessListTooLarge {
        item_count: u64,
        max_items: u64,
        gas_limit: u64,
    },
}
