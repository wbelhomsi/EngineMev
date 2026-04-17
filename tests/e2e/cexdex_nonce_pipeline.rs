//! End-to-end test for the cexdex nonce-based fan-out pipeline.
//!
//! Wires: NoncePool (seeded via update_cached_hash) -> NonceInfo ->
//! build_signed_bundle_tx -> decoded VersionedTransaction.
//! Asserts on-the-wire structure: ix[0] advance_nonce, recent_blockhash
//! matches the nonce's cached hash, tip transfer is the last instruction.
//!
//! Run with: cargo test --features e2e --test e2e cexdex_nonce_pipeline

use solana_mev_bot::cexdex::{NonceInfo, NoncePool};
use solana_mev_bot::executor::relays::common::build_signed_bundle_tx;
use solana_sdk::hash::Hash;
use solana_sdk::message::VersionedMessage;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};
use solana_system_interface::instruction as system_instruction;

/// Full checkout -> build -> decode flow verifying the on-the-wire tx structure.
///
/// Asserts:
///   1. NoncePool.checkout() returns the seeded (pubkey, hash) pair.
///   2. build_signed_bundle_tx prepends AdvanceNonceAccount (System discriminator 4) at ix[0].
///   3. recent_blockhash in the compiled message matches the nonce's cached hash.
///   4. The last instruction is a System::Transfer (tip, discriminator 2).
#[test]
fn nonce_advance_at_ix0_and_blockhash_matches() {
    let signer = Keypair::new();
    let nonce_pk = Pubkey::new_unique();
    let tip_account = Pubkey::new_unique();
    let nonce_hash = Hash::new_unique();

    // Seed pool with a cached hash
    let pool = NoncePool::new(vec![nonce_pk]);
    pool.update_cached_hash(nonce_pk, nonce_hash);
    let (checked_out_pk, checked_out_hash) = pool.checkout().expect("checkout should succeed");
    assert_eq!(checked_out_pk, nonce_pk, "checked-out pubkey must match");
    assert_eq!(checked_out_hash, nonce_hash, "checked-out hash must match seeded value");

    // Base ix = a trivial transfer to simulate a swap leg
    let dummy_dest = Pubkey::new_unique();
    let base_ix = system_instruction::transfer(&signer.pubkey(), &dummy_dest, 100);

    let bytes = build_signed_bundle_tx(
        "test",
        &[base_ix],
        50_000,
        &tip_account,
        &signer,
        checked_out_hash,
        &[],
        Some(NonceInfo {
            account: nonce_pk,
            authority: signer.pubkey(),
        }),
    )
    .expect("build_signed_bundle_tx should succeed");

    let tx: solana_sdk::transaction::VersionedTransaction =
        bincode::deserialize(&bytes).expect("deserialize tx");

    match &tx.message {
        VersionedMessage::Legacy(m) => {
            // ix[0] should be AdvanceNonceAccount (System discriminator 4)
            let ix0 = &m.instructions[0];
            let disc = u32::from_le_bytes(ix0.data[0..4].try_into().unwrap());
            assert_eq!(disc, 4, "ix[0] must be System::AdvanceNonceAccount (discriminator 4)");

            // recent_blockhash must equal the nonce's cached hash
            assert_eq!(m.recent_blockhash, nonce_hash, "recent_blockhash must be the nonce hash");

            // Last instruction should be the tip transfer (System discriminator 2)
            let last = m.instructions.last().expect("no instructions");
            let last_disc = u32::from_le_bytes(last.data[0..4].try_into().unwrap());
            assert_eq!(last_disc, 2, "last ix must be System::Transfer (tip, discriminator 2)");
        }
        VersionedMessage::V0(m) => {
            let ix0 = &m.instructions[0];
            let disc = u32::from_le_bytes(ix0.data[0..4].try_into().unwrap());
            assert_eq!(disc, 4, "ix[0] must be System::AdvanceNonceAccount (discriminator 4)");

            assert_eq!(m.recent_blockhash, nonce_hash, "recent_blockhash must be the nonce hash");

            let last = m.instructions.last().expect("no instructions");
            let last_disc = u32::from_le_bytes(last.data[0..4].try_into().unwrap());
            assert_eq!(last_disc, 2, "last ix must be System::Transfer (tip, discriminator 2)");
        }
        _ => panic!("unexpected message version"),
    }
}

/// Verify NoncePool rejects checkout when no hashes have been seeded yet.
#[test]
fn nonce_pool_checkout_returns_none_before_warmup() {
    let pool = NoncePool::new(vec![Pubkey::new_unique(), Pubkey::new_unique()]);
    assert!(pool.checkout().is_none(), "checkout must return None before any hash is cached");
}

/// Verify mark_settled releases in-flight state and allows re-checkout.
#[test]
fn nonce_pool_mark_settled_releases_for_reuse() {
    let nonce_pk = Pubkey::new_unique();
    let nonce_hash = Hash::new_unique();
    let pool = NoncePool::new(vec![nonce_pk]);
    pool.update_cached_hash(nonce_pk, nonce_hash);

    let (pk1, h1) = pool.checkout().expect("first checkout");
    assert_eq!(pk1, nonce_pk);
    assert_eq!(h1, nonce_hash);

    // Settle then re-checkout: should get the same nonce back
    pool.mark_settled(pk1);
    let (pk2, h2) = pool.checkout().expect("second checkout after settle");
    assert_eq!(pk2, nonce_pk);
    assert_eq!(h2, nonce_hash);
}

/// When nonce is None, ix[0] is the first base instruction (System::Transfer,
/// discriminator 2), NOT AdvanceNonceAccount.
#[test]
fn build_signed_bundle_tx_no_nonce_omits_advance() {
    let signer = Keypair::new();
    let tip_account = Pubkey::new_unique();
    let blockhash = Hash::new_unique();
    let dummy_dest = Pubkey::new_unique();
    let base_ix = system_instruction::transfer(&signer.pubkey(), &dummy_dest, 100);

    let bytes = build_signed_bundle_tx(
        "test",
        &[base_ix],
        50_000,
        &tip_account,
        &signer,
        blockhash,
        &[],
        None,
    )
    .expect("build without nonce should succeed");

    let tx: solana_sdk::transaction::VersionedTransaction =
        bincode::deserialize(&bytes).expect("deserialize tx");

    let ix0_data = match &tx.message {
        VersionedMessage::Legacy(m) => m.instructions[0].data.clone(),
        VersionedMessage::V0(m) => m.instructions[0].data.clone(),
        _ => panic!("unexpected message version"),
    };
    let disc = u32::from_le_bytes(ix0_data[0..4].try_into().unwrap());
    assert_eq!(
        disc, 2,
        "without nonce, ix[0] must be the base System::Transfer (discriminator 2), not AdvanceNonce"
    );
}
