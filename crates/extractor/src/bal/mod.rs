//! Block access list (EIP-7928) fetching and normalization.
//!
//! Response shapes differ per client, but all of them decode into the
//! `alloy_eip7928` types, so only the fetch path is client-specific.

pub mod client_kind;
pub mod error;

pub use client_kind::ClientKind;
pub use error::BalError;

#[cfg(test)]
mod tests {
    use alloy_eip7928::{bal::Bal, AccountChanges, EMPTY_BLOCK_ACCESS_LIST_HASH};

    #[test]
    fn reth_response_shape_decodes_into_alloy_types() {
        let raw = serde_json::json!([
            {
                "address": "0x1111111111111111111111111111111111111111",
                "storageChanges": [
                    { "slot": "0x1", "changes": [ { "blockAccessIndex": "0x1", "newValue": "0x2a" } ] }
                ],
                "storageReads": ["0x7"],
                "balanceChanges": [ { "blockAccessIndex": "0x1", "postBalance": "0xde0b6b3a7640000" } ],
                "nonceChanges": [ { "blockAccessIndex": "0x1", "newNonce": "0x5" } ],
                "codeChanges": []
            }
        ]);

        let bal: Vec<AccountChanges> = serde_json::from_value(raw).unwrap();

        let account = &bal[0];
        assert_eq!(account.storage_changes[0].changes[0].block_access_index, 1);
        assert_eq!(account.storage_reads.len(), 1);
        assert_eq!(account.nonce_changes[0].new_nonce, 5);
    }

    // `alloy-eips` pulls in `alloy-eip7928` without its `rlp` feature, so the
    // hashing and RLP decoding this module needs only exist because of the
    // direct dependency. This fails to compile if that dependency is dropped
    // back to what `alloy` alone provides.
    #[test]
    fn rlp_feature_is_enabled_on_alloy_eip7928() {
        assert_eq!(Bal::default().compute_hash(), EMPTY_BLOCK_ACCESS_LIST_HASH);
    }
}
