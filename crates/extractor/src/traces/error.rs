use crate::cache::CacheError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TraceError {
    #[error("block header not cached for {block_number} on {chain_id}")]
    BlockHeaderNotCached { chain_id: u64, block_number: u64 },

    #[error("block header is cached but file is broken for {block_number}: {source}")]
    BlockHeaderMalformed {
        block_number: u64,
        #[source]
        source: CacheError,
    },

    #[error("cache I/O error")]
    Io(#[from] CacheError),
}
