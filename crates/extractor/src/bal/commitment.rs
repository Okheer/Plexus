//! EIP-7928 block access list commitment verification.
//!
//! Every block header commits to its access list through `blockAccessListHash`,
//! defined as the Keccak-256 of the RLP-encoded BAL. Checking a fetched BAL
//! against that commitment before using it turns a bad fetch, a corrupt cache
//! entry, or a client bug into an immediate [`BalError::BalHashMismatch`]
//! instead of silently wrong dependency edges further downstream.
//!
//! # Two functions, two strengths
//!
//! The clients hand us different things, which buys different amounts of trust:
//!
//! - [`verify_raw_bal_commitment`] hashes the exact bytes the node sent. This is
//!   a true integrity check over the wire encoding, and is what Nethermind's
//!   `debug_getRawBlockAccessList` path can use because it carries raw RLP.
//! - [`verify_bal_commitment`] hashes a *re-encoding* of the decoded BAL, which
//!   is all that's possible for Reth's JSON path and for anything read back out
//!   of the `bal.json` cache, since neither keeps the original bytes.
//!
//! The re-encoding check catches dropped fields, reordered accounts, corrupted
//! values, and decoder bugs — the failures that actually matter here. What it
//! cannot catch is a non-canonical encoding that decodes to the same value,
//! because re-encoding normalizes exactly that difference away. Don't read it as
//! a byte-for-byte guarantee about what the node sent.
//!
//! `raw_and_decoded_hashes_agree` in this module's tests is what justifies
//! treating the two as interchangeable for canonically-encoded input.

use alloy_eip7928::{compute_block_access_list_hash, AccountChanges};
use alloy_primitives::{keccak256, B256};

use crate::bal::error::BalError;

/// Computes the `blockAccessListHash` a decoded BAL commits to.
///
/// This RLP-encodes `bal` and hashes the result, so it reproduces the header's
/// commitment only for a canonically-encoded access list — see the module docs.
/// An empty BAL hashes to the spec's `EMPTY_BLOCK_ACCESS_LIST_HASH` sentinel
/// (`0x1dcc4de8…`) without any special-casing, because RLP-encoding an empty
/// list yields `0xc0`.
pub fn bal_commitment_hash(bal: &[AccountChanges]) -> B256 {
    compute_block_access_list_hash(bal)
}

/// Checks a decoded BAL against the block's committed `blockAccessListHash`.
///
/// # Errors
///
/// Returns [`BalError::BalHashMismatch`] carrying both hashes if the BAL does
/// not match `expected`.
pub fn verify_bal_commitment(bal: &[AccountChanges], expected: B256) -> Result<(), BalError> {
    ensure_hash(bal_commitment_hash(bal), expected)
}

/// Checks raw RLP bytes, as received from the node, against the block's
/// committed `blockAccessListHash`.
///
/// Prefer this over [`verify_bal_commitment`] wherever the original bytes are
/// still in hand: it hashes what the node actually sent rather than a
/// re-encoding of it. `raw` is the decoded byte string, not a hex string.
///
/// # Errors
///
/// Returns [`BalError::BalHashMismatch`] carrying both hashes if the bytes do
/// not match `expected`.
pub fn verify_raw_bal_commitment(raw: &[u8], expected: B256) -> Result<(), BalError> {
    ensure_hash(keccak256(raw), expected)
}

fn ensure_hash(computed: B256, expected: B256) -> Result<(), BalError> {
    if computed == expected {
        Ok(())
    } else {
        Err(BalError::BalHashMismatch { computed, expected })
    }
}

#[cfg(test)]
mod tests {
    use alloy_eip7928::{bal::Bal, EMPTY_BLOCK_ACCESS_LIST_HASH};
    use alloy_primitives::{b256, U256};

    use super::*;

    /// Two accounts across two transactions plus a system write at index 0 —
    /// the shape a real Glamsterdam block produces.
    fn sample_bal() -> Vec<AccountChanges> {
        serde_json::from_value(serde_json::json!([
            {
                "address": "0x000f3df6d732807ef1319fb7b8bb8522d0beac02",
                "storageChanges": [
                    { "slot": "0x2a", "changes": [ { "blockAccessIndex": "0x0", "newValue": "0x99" } ] }
                ],
                "storageReads": [],
                "balanceChanges": [],
                "nonceChanges": [],
                "codeChanges": []
            },
            {
                "address": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "storageChanges": [
                    { "slot": "0x1", "changes": [
                        { "blockAccessIndex": "0x1", "newValue": "0x64" },
                        { "blockAccessIndex": "0x2", "newValue": "0xc8" }
                    ] }
                ],
                "storageReads": ["0x5"],
                "balanceChanges": [ { "blockAccessIndex": "0x1", "postBalance": "0xde0b6b3a7640000" } ],
                "nonceChanges": [ { "blockAccessIndex": "0x1", "newNonce": "0x1" } ],
                "codeChanges": []
            }
        ]))
        .unwrap()
    }

    /// The raw RLP a node would serve for a given BAL.
    fn raw_rlp(bal: &[AccountChanges]) -> Vec<u8> {
        alloy_rlp::encode(Bal::from(bal.to_vec()))
    }

    // ── the matching case ────────────────────────────────────────────────────

    #[test]
    fn matching_bal_passes_verification() {
        let bal = sample_bal();
        let committed = bal_commitment_hash(&bal);

        assert!(verify_bal_commitment(&bal, committed).is_ok());
    }

    #[test]
    fn matching_raw_bytes_pass_verification() {
        let bal = sample_bal();
        let raw = raw_rlp(&bal);
        let committed = keccak256(&raw);

        assert!(verify_raw_bal_commitment(&raw, committed).is_ok());
    }

    // This is what lets the Reth (JSON, re-encoded) and Nethermind (raw bytes)
    // paths be checked against the same header commitment. Without it, the
    // decoded-BAL check would only be self-consistent, not tied to the wire form.
    #[test]
    fn raw_and_decoded_hashes_agree() {
        let bal = sample_bal();

        assert_eq!(bal_commitment_hash(&bal), keccak256(raw_rlp(&bal)));
    }

    // The commitment reproduced by hand in issue #17: keccak-256 of the raw RLP
    // equals the header's blockAccessListHash. Same check, now in code.
    #[test]
    fn raw_bytes_verify_against_the_decoded_commitment() {
        let bal = sample_bal();
        let header_commitment = bal_commitment_hash(&bal);

        assert!(verify_raw_bal_commitment(&raw_rlp(&bal), header_commitment).is_ok());
    }

    // ── the corrupted / mismatched case ──────────────────────────────────────

    #[test]
    fn corrupted_raw_bytes_are_caught() {
        let bal = sample_bal();
        let committed = keccak256(raw_rlp(&bal));

        let mut corrupted = raw_rlp(&bal);
        let last = corrupted.len() - 1;
        corrupted[last] ^= 0xff;

        let err = verify_raw_bal_commitment(&corrupted, committed).unwrap_err();

        assert!(matches!(err, BalError::BalHashMismatch { .. }));
    }

    #[test]
    fn truncated_raw_bytes_are_caught() {
        let bal = sample_bal();
        let raw = raw_rlp(&bal);
        let committed = keccak256(&raw);

        let err = verify_raw_bal_commitment(&raw[..raw.len() - 1], committed).unwrap_err();

        assert!(matches!(err, BalError::BalHashMismatch { .. }));
    }

    // A BAL that still decodes cleanly but carries a wrong value is the failure
    // this whole check exists for — nothing else downstream would notice.
    #[test]
    fn mutated_storage_value_is_caught() {
        let bal = sample_bal();
        let committed = bal_commitment_hash(&bal);

        let mut tampered = bal.clone();
        tampered[1].storage_changes[0].changes[0].new_value = U256::from(0xdead_u64);

        let err = verify_bal_commitment(&tampered, committed).unwrap_err();

        assert!(matches!(err, BalError::BalHashMismatch { .. }));
    }

    #[test]
    fn dropped_account_is_caught() {
        let bal = sample_bal();
        let committed = bal_commitment_hash(&bal);

        let err = verify_bal_commitment(&bal[..1], committed).unwrap_err();

        assert!(matches!(err, BalError::BalHashMismatch { .. }));
    }

    // A BAL for the wrong block decodes perfectly and is still the wrong BAL.
    #[test]
    fn bal_from_a_different_block_is_caught() {
        let other_block_commitment = bal_commitment_hash(&[]);

        let err = verify_bal_commitment(&sample_bal(), other_block_commitment).unwrap_err();

        assert!(matches!(err, BalError::BalHashMismatch { .. }));
    }

    #[test]
    fn mismatch_error_reports_both_hashes() {
        let bal = sample_bal();
        let expected = B256::from([0x11; 32]);

        let err = verify_bal_commitment(&bal, expected).unwrap_err();

        match err {
            BalError::BalHashMismatch {
                computed,
                expected: got,
            } => {
                assert_eq!(got, expected);
                assert_eq!(computed, bal_commitment_hash(&bal));
                assert_ne!(computed, got);
            }
            other => panic!("expected BalHashMismatch, got {other:?}"),
        }
    }

    // The message is the operator-facing signal, so both hashes must be in it.
    #[test]
    fn mismatch_error_message_names_both_hashes() {
        let bal = sample_bal();
        let expected = B256::from([0x11; 32]);

        let msg = verify_bal_commitment(&bal, expected)
            .unwrap_err()
            .to_string();

        assert!(msg.contains(&bal_commitment_hash(&bal).to_string()));
        assert!(msg.contains(&expected.to_string()));
    }

    // ── the empty-BAL case from #17 ──────────────────────────────────────────

    /// The value #17 read off Erigon's genesis block on a real devnet node.
    ///
    /// Pinned as a literal rather than reused from `EMPTY_BLOCK_ACCESS_LIST_HASH`
    /// on purpose: comparing our computation against alloy's constant only proves
    /// the two agree with each other. This ties both of them to what a node
    /// actually reported, so the check still holds if that constant ever moves.
    const OBSERVED_EMPTY_BAL_HASH: B256 =
        b256!("0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347");

    #[test]
    fn empty_bal_hashes_to_the_sentinel_observed_on_a_real_node() {
        assert_eq!(bal_commitment_hash(&[]), OBSERVED_EMPTY_BAL_HASH);
    }

    #[test]
    fn alloy_sentinel_matches_the_one_observed_on_a_real_node() {
        assert_eq!(EMPTY_BLOCK_ACCESS_LIST_HASH, OBSERVED_EMPTY_BAL_HASH);
    }

    #[test]
    fn empty_bal_verifies_against_the_sentinel() {
        assert!(verify_bal_commitment(&[], EMPTY_BLOCK_ACCESS_LIST_HASH).is_ok());
    }

    // The empty case must go through the same path as any other BAL: RLP-encoding
    // an empty list gives 0xc0, and keccak(0xc0) is the sentinel.
    #[test]
    fn empty_raw_rlp_verifies_against_the_sentinel() {
        let raw = raw_rlp(&[]);

        assert_eq!(raw, vec![0xc0]);
        assert!(verify_raw_bal_commitment(&raw, EMPTY_BLOCK_ACCESS_LIST_HASH).is_ok());
    }

    // A block that committed to the empty sentinel but served a populated BAL is
    // a genuine disagreement, not an "empty means skip the check" shortcut.
    #[test]
    fn non_empty_bal_against_the_empty_sentinel_is_caught() {
        let err = verify_bal_commitment(&sample_bal(), EMPTY_BLOCK_ACCESS_LIST_HASH).unwrap_err();

        assert!(matches!(err, BalError::BalHashMismatch { .. }));
    }
}
