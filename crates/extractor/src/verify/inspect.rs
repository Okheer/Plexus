use crate::cache::{config::CacheConfig, error::CacheError, io::read_json};
use alloy_primitives::B256;
use std::path::PathBuf;
use types::types::BlockContext;

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

pub fn verify_block_cache(
    cache: &CacheConfig,
    chain_id: u64,
    block_number: u64,
) -> CacheVerificationReport {
    let header_path = cache.block_header_path(chain_id, block_number);

    let mut verified = Vec::new();
    let mut missing = Vec::new();
    let mut corrupted = Vec::new();

    let block_ctx = match read_json::<BlockContext>(&header_path) {
        Ok(ctx) => ctx,
        Err(CacheError::NotFound(p)) => {
            return CacheVerificationReport {
                chain_id: chain_id,
                block_number: block_number,
                header: HeaderStatus::Missing(p),
                verified_tx_files: verified,
                missing_tx_files: missing,
                corrupted_tx_files: corrupted,
            };
        }
        Err(CacheError::Malformed { path, source }) => {
            return CacheVerificationReport {
                chain_id: chain_id,
                block_number: block_number,
                header: HeaderStatus::Malformed {
                    path,
                    reason: source.to_string(),
                },
                verified_tx_files: verified,
                missing_tx_files: missing,
                corrupted_tx_files: corrupted,
            };
        }
        Err(CacheError::Io { path, source }) => {
            // treat as Malformed/unreadable too — surface the OS error, don't panic
            return CacheVerificationReport {
                chain_id: chain_id,
                block_number: block_number,
                header: HeaderStatus::Malformed {
                    path,
                    reason: source.to_string(),
                },
                verified_tx_files: verified,
                missing_tx_files: missing,
                corrupted_tx_files: corrupted,
            };
        }
    };

    for hash in &block_ctx.tx_hashes {
        let path = cache.tx_path(chain_id, block_number, hash);
        match read_json::<serde_json::Value>(&path) {
            Ok(_) => verified.push(*hash),
            Err(CacheError::NotFound(_)) => missing.push(*hash),
            Err(CacheError::Malformed { path, source }) => {
                corrupted.push((*hash, path, source.to_string()))
            }
            Err(CacheError::Io { path, source }) => {
                corrupted.push((*hash, path, source.to_string()))
            }
        }
    }

    CacheVerificationReport {
        chain_id,
        block_number,
        header: HeaderStatus::Ok,
        verified_tx_files: verified,
        missing_tx_files: missing,
        corrupted_tx_files: corrupted,
    }
}
