//! Parser for Solana durable nonce accounts (80 bytes, bincode-encoded
//! `Versions<State>` from `solana-nonce`). Used by the cexdex narrow
//! Geyser subscription to keep a live cache of each managed nonce's
//! current blockhash without extra RPC.
//!
//! Verified layout (2026-04-17) against a live initialized nonce:
//!   [0..4]   Versions tag (u32 LE, 0=Legacy, 1=Current)
//!   [4..8]   State tag    (u32 LE, 0=Uninitialized, 1=Initialized)
//!   [8..40]  authority    (Pubkey)
//!   [40..72] durable_nonce (Hash)
//!   [72..80] fee_calculator.lamports_per_signature (u64 LE)

use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;

/// Returned tuple is `(authority, current_nonce_hash)`.
/// Returns `None` for uninitialized accounts or data shorter than 72 bytes.
/// Callers must verify `authority == expected_searcher_pubkey` before trusting
/// the hash — defense in depth even though Geyser only delivers subscribed
/// accounts.
pub fn parse_nonce(data: &[u8]) -> Option<(Pubkey, Hash)> {
    if data.len() < 72 {
        return None;
    }
    // Accept both Versions::Legacy (0) and Versions::Current (1) — same Data layout.
    let state_tag = u32::from_le_bytes(data[4..8].try_into().ok()?);
    if state_tag != 1 {
        return None; // not Initialized
    }
    let authority = Pubkey::new_from_array(data[8..40].try_into().ok()?);
    let hash = Hash::new_from_array(data[40..72].try_into().ok()?);
    Some((authority, hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    /// Hex dump captured 2026-04-17 from nonce account
    /// 6vNq2tbRXPWAWnBU4wAvPGK6AgifGoa38NaYfFE2ovNG on mainnet.
    /// Versions=1, State=1, authority=6T3hyzTz..., nonce=DHHF5jgZ..., fee=5000.
    fn initialized_bytes() -> Vec<u8> {
        let hex = "\
            01000000 01000000 \
            50f69895 2c072e31 51b37770 8133e411 3e74b24e f989222e 0fbb230a dc875539 \
            b677e6de f2287cf8 2942d712 b6a91382 656b5f6f 00d1061b 984dbdf6 e1715b00 \
            88130000 00000000";
        hex.split_whitespace()
            .flat_map(|chunk| {
                (0..chunk.len())
                    .step_by(2)
                    .map(move |i| u8::from_str_radix(&chunk[i..i + 2], 16).unwrap())
            })
            .collect()
    }

    #[test]
    fn parses_initialized_nonce() {
        let data = initialized_bytes();
        assert_eq!(data.len(), 80);
        let (authority, hash) = parse_nonce(&data).expect("should parse");
        assert_eq!(
            authority,
            Pubkey::from_str("6T3hyzTz59ZCj18P9LQ6VKEVA2x7xT5jEPV7394b3Hxt").unwrap()
        );
        assert_eq!(
            hash,
            Hash::from_str("DHHF5jgZ76oxcLvZ3bV1Y4wmSsFFvbW6myRsc1fhQWCP").unwrap()
        );
    }

    #[test]
    fn rejects_short_data() {
        assert!(parse_nonce(&[0u8; 71]).is_none());
    }

    #[test]
    fn rejects_uninitialized() {
        let mut data = initialized_bytes();
        data[4] = 0; // flip State tag to Uninitialized
        assert!(parse_nonce(&data).is_none());
    }

    #[test]
    fn accepts_legacy_versions_tag() {
        // Versions::Legacy (tag=0) has identical Data layout.
        let mut data = initialized_bytes();
        data[0] = 0; // flip Versions tag to Legacy
        let (authority, _hash) = parse_nonce(&data).expect("should parse");
        assert_eq!(
            authority,
            Pubkey::from_str("6T3hyzTz59ZCj18P9LQ6VKEVA2x7xT5jEPV7394b3Hxt").unwrap()
        );
    }
}
