use crate::cache::CacheError;
use crate::fetcher::BlockId;
use crate::rpc::RpcError;
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

    #[error("invalid hex in raw bal response: {0}")]
    BalHex(#[from] hex::FromHexError),

    #[error("failed to rlp-decode raw bal: {0}")]
    BalRlp(#[from] alloy_rlp::Error),

    /// EIP-7928 assigns index 0 to pre-execution system calls, 1..=n to the n
    /// transactions in block order, and n+1 to post-execution system calls.
    /// Anything above n+1 cannot be attributed and means the client and the
    /// block header disagree about how many transactions the block holds.
    #[error("block access index {index} out of range for a block with {tx_count} transactions")]
    InvalidBlockAccessIndex { index: u64, tx_count: usize },

    #[error("unsupported client '{name}', expected one of: reth, nethermind")]
    UnsupportedClient { name: String },
}
