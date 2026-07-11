use thiserror::Error;

/// Errors that can occur when operating on a `DepGraph`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DepGraphError {
    #[error("transaction index {tx_index} is out of bounds (tx_count = {tx_count})")]
    TxIndexOutOfBounds { tx_index: usize, tx_count: usize },
}
