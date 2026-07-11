use std::path::PathBuf;
use alloy_primitives::B256;
pub enum HeaderStatus {
    Ok,
    Missing(PathBuf),
    Malformed { path: PathBuf, reason: String },
}
pub struct CacheVerificationReport {
    pub chain_id: u64,
    pub block_number: u64,
    pub header: HeaderStatus,
    pub verified_tx_files: Vec<B256>,
    pub missing_tx_files: Vec<B256>,
    pub corrupted_tx_files: Vec<(B256, PathBuf, String)>,
}

impl CacheVerificationReport {
    pub fn is_complete(&self) -> bool {
        matches!(self.header, HeaderStatus::Ok)
            && self.missing_tx_files.is_empty()
            && self.corrupted_tx_files.is_empty()
    }
}

