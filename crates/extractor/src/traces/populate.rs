use alloy_primitives::B256;
use crate::cache::{config::CacheConfig, io::read_json, CacheError};
use crate::rpc::client::RpcClient;

#[derive(Debug)]
pub struct TraceFetchSummary {
   pub block_number: u64,
   pub cache_hits: Vec<B256>,
   pub fetched: Vec<B256>,
   pub failed: Vec<(B256,String)>,
}

impl TraceFetchSummary{

    pub fn total(&self)-> usize{
     self.cache_hits.len() + self.fetched.len() + self.failed.len()
    }

    pub fn is_complete(&self)-> bool{
        self.failed.is_empty()
    }
}