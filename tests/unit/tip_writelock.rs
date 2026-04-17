use solana_sdk::{
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    message::{v0, VersionedMessage},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::VersionedTransaction,
};
use solana_message::AddressLookupTableAccount;
use solana_system_interface::instruction as system_instruction;

/// The 8 hardcoded Jito tip accounts (must match jito.rs).
const JITO_TIP_ACCOUNTS: &[&str] = &[
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTqvQss8hp11i4bPKELzFLDELBGnNYpzHCDf",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSLzzWw1pa8U5j7cUi2",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
];

// ---------------------------------------------------------------------------
// Test 1: system_instruction::transfer marks tip account as writable
// ---------------------------------------------------------------------------

#[test]
fn test_tip_transfer_marks_tip_account_writable() {
    let from = Pubkey::new_unique();
    let to = Pubkey::new_unique();
    let ix = system_instruction::transfer(&from, &to, 1000);

    let to_meta = ix.accounts.iter().find(|a| a.pubkey == to).unwrap();
    assert!(
        to_meta.is_writable,
        "Tip account must be writable for Jito acceptance"
    );
    // from (payer) must also be writable and signer
    let from_meta = ix.accounts.iter().find(|a| a.pubkey == from).unwrap();
    assert!(from_meta.is_writable);
    assert!(from_meta.is_signer);
}

// ---------------------------------------------------------------------------
// Test 2: V0 message preserves tip writability when tip is NOT in ALT
// ---------------------------------------------------------------------------

#[test]
fn test_v0_message_preserves_tip_writability_with_alt() {
    let signer = Keypair::new();
    let tip_account = Pubkey::new_unique(); // NOT in ALT
    let some_program = Pubkey::new_unique();
    let some_writable = Pubkey::new_unique();

    // A dummy swap instruction with accounts that will use the ALT
    let swap_ix = Instruction {
        program_id: some_program,
        accounts: vec![
            AccountMeta::new(signer.pubkey(), true),
            AccountMeta::new(some_writable, false),
        ],
        data: vec![1, 2, 3],
    };
    let tip_ix = system_instruction::transfer(&signer.pubkey(), &tip_account, 1000);

    let instructions = vec![swap_ix, tip_ix];

    // ALT contains only the program and some_writable — NOT the tip account
    let alt = AddressLookupTableAccount {
        key: Pubkey::new_unique(),
        addresses: vec![some_program, some_writable],
    };

    let blockhash = Hash::new_unique();
    let v0_msg = v0::Message::try_compile(
        &signer.pubkey(),
        &instructions,
        &[alt],
        blockhash,
    )
    .expect("V0 compile should succeed");

    // The tip account must be a static key (not in ALT) and writable
    let msg = VersionedMessage::V0(v0_msg.clone());

    // Static writable non-signer accounts are in positions:
    //   [num_required_signatures .. account_keys.len() - num_readonly_unsigned]
    // But the simplest check: is_maybe_writable should return true for tip
    let tip_idx = v0_msg
        .account_keys
        .iter()
        .position(|k| *k == tip_account)
        .expect("Tip account must be in static account keys (not ALT-compressed)");

    // In V0 messages, static key writability follows the same header rules as legacy:
    // Positions 0..num_required_signatures are signers
    //   First (num_required_signatures - num_readonly_signed) are writable signers
    //   Last num_readonly_signed are readonly signers
    // Positions num_required_signatures..account_keys.len() are non-signers
    //   First (non_signers - num_readonly_unsigned) are writable non-signers
    //   Last num_readonly_unsigned are readonly non-signers
    let header = &v0_msg.header;
    let num_signers = header.num_required_signatures as usize;
    let num_readonly_unsigned = header.num_readonly_unsigned_accounts as usize;
    let total_static = v0_msg.account_keys.len();

    // tip_idx should be in the writable non-signer section
    assert!(
        tip_idx >= num_signers,
        "Tip account should be a non-signer (idx={}, num_signers={})",
        tip_idx,
        num_signers
    );
    assert!(
        tip_idx < total_static - num_readonly_unsigned,
        "Tip account must be in writable section (idx={}, readonly_start={})",
        tip_idx,
        total_static - num_readonly_unsigned
    );

    // Also verify the tx fits in a packet
    let tx = VersionedTransaction::try_new(msg, &[&signer]).unwrap();
    let serialized = bincode::serialize(&tx).unwrap();
    assert!(
        serialized.len() <= 1232,
        "V0 tx with ALT should fit in 1232 bytes, got {}",
        serialized.len()
    );
}

// ---------------------------------------------------------------------------
// Test 3: Every real Jito tip account parses to a valid Pubkey
// ---------------------------------------------------------------------------

#[test]
fn test_jito_tip_accounts_parse() {
    for (i, addr) in JITO_TIP_ACCOUNTS.iter().enumerate() {
        let pk: Pubkey = addr
            .parse()
            .unwrap_or_else(|e| panic!("Jito tip account {} ('{}') failed to parse: {}", i, addr, e));
        // Must not be the default/zero pubkey
        assert_ne!(pk, Pubkey::default(), "Tip account {} is zero pubkey", i);
    }
}

// ---------------------------------------------------------------------------
// Test 4: build_signed_bundle_tx produces a tx where tip account is writable
// ---------------------------------------------------------------------------

#[test]
fn test_build_signed_bundle_tx_tip_writable() {
    use solana_mev_bot::executor::relays::common;

    let signer = Keypair::new();
    let tip_account: Pubkey = JITO_TIP_ACCOUNTS[0].parse().unwrap();
    let some_program = Pubkey::new_unique();

    let swap_ix = Instruction {
        program_id: some_program,
        accounts: vec![AccountMeta::new(signer.pubkey(), true)],
        data: vec![0xAA],
    };

    let blockhash = Hash::new_unique();

    // Without ALT (legacy path)
    let serialized = common::build_signed_bundle_tx(
        "jito",
        std::slice::from_ref(&swap_ix),
        10_000,
        &tip_account,
        &signer,
        blockhash,
        &[],
        None,
    )
    .expect("build_signed_bundle_tx should succeed");

    let tx: VersionedTransaction =
        bincode::deserialize(&serialized).expect("bincode deserialize");

    let tip_found = match &tx.message {
        VersionedMessage::Legacy(msg) => {
            // In legacy messages, writable non-signers are after the signers
            // but before readonly non-signers.
            let header = &msg.header;
            let num_ro_unsigned = header.num_readonly_unsigned_accounts as usize;
            let total = msg.account_keys.len();
            let writable_end = total - num_ro_unsigned;

            msg.account_keys
                .iter()
                .enumerate()
                .any(|(idx, k)| {
                    *k == tip_account && idx < writable_end
                })
        }
        VersionedMessage::V0(msg) => {
            let header = &msg.header;
            let num_ro_unsigned = header.num_readonly_unsigned_accounts as usize;
            let total = msg.account_keys.len();
            let writable_end = total - num_ro_unsigned;

            msg.account_keys
                .iter()
                .enumerate()
                .any(|(idx, k)| {
                    *k == tip_account && idx < writable_end
                })
        }
        _ => false,
    };
    assert!(
        tip_found,
        "Tip account must appear as writable in the compiled transaction"
    );
}

// ---------------------------------------------------------------------------
// Test 5: build_signed_bundle_tx with ALT still keeps tip writable
// ---------------------------------------------------------------------------

#[test]
fn test_build_signed_bundle_tx_with_alt_tip_writable() {
    use solana_mev_bot::executor::relays::common;

    let signer = Keypair::new();
    let tip_account: Pubkey = JITO_TIP_ACCOUNTS[3].parse().unwrap();
    let some_program = Pubkey::new_unique();
    let vault_a = Pubkey::new_unique();
    let vault_b = Pubkey::new_unique();

    // Simulate a swap IX with several accounts that live in the ALT
    let swap_ix = Instruction {
        program_id: some_program,
        accounts: vec![
            AccountMeta::new(signer.pubkey(), true),
            AccountMeta::new(vault_a, false),
            AccountMeta::new(vault_b, false),
            AccountMeta::new_readonly(some_program, false),
        ],
        data: vec![0xBB, 0xCC],
    };

    // ALT contains swap-related accounts but NOT the tip account
    let alt = AddressLookupTableAccount {
        key: Pubkey::new_unique(),
        addresses: vec![some_program, vault_a, vault_b],
    };

    let blockhash = Hash::new_unique();

    let serialized = common::build_signed_bundle_tx(
        "jito",
        &[swap_ix],
        50_000,
        &tip_account,
        &signer,
        blockhash,
        &[&alt],
        None,
    )
    .expect("build_signed_bundle_tx with ALT should succeed");

    let tx: VersionedTransaction =
        bincode::deserialize(&serialized).expect("bincode deserialize");

    // Must be V0 since we provided an ALT
    let v0_msg = match &tx.message {
        VersionedMessage::V0(msg) => msg,
        other => panic!("Expected V0 message, got {:?}", other),
    };

    // Tip account must be in static keys (not in ALT lookup tables)
    let tip_idx = v0_msg
        .account_keys
        .iter()
        .position(|k| *k == tip_account)
        .expect("Tip account must be a static key in V0 message");

    // Verify it's in the writable section
    let header = &v0_msg.header;
    let num_ro_unsigned = header.num_readonly_unsigned_accounts as usize;
    let total = v0_msg.account_keys.len();
    let writable_end = total - num_ro_unsigned;

    assert!(
        tip_idx < writable_end,
        "Tip account at static index {} must be writable (writable_end={})",
        tip_idx,
        writable_end
    );

    // Also verify the system program is present (needed for transfer)
    let system_program_id = solana_system_interface::program::id();
    let has_system_program = v0_msg
        .account_keys
        .contains(&system_program_id)
        || v0_msg.address_table_lookups.iter().any(|lookup| {
            // System program could be in ALT readonly section
            lookup.account_key == alt.key
                && (lookup.readonly_indexes.iter().any(|&idx| {
                    (idx as usize) < alt.addresses.len()
                        && alt.addresses[idx as usize] == system_program_id
                }) || lookup.writable_indexes.iter().any(|&idx| {
                    (idx as usize) < alt.addresses.len()
                        && alt.addresses[idx as usize] == system_program_id
                }))
        });
    assert!(
        has_system_program,
        "System program must be present in V0 message (static or ALT)"
    );
}

// ---------------------------------------------------------------------------
// Test 6: Tip account in ALT readonly section must still be writable
//
// This is the CRITICAL edge case: if someone accidentally adds a Jito tip
// account to the ALT (even as readonly), the V0 compiler should still
// mark it writable because the instruction requires it.
// ---------------------------------------------------------------------------

#[test]
fn test_tip_in_alt_readonly_still_writable() {
    let signer = Keypair::new();
    let tip_account: Pubkey = JITO_TIP_ACCOUNTS[0].parse().unwrap();
    let some_program = Pubkey::new_unique();

    let swap_ix = Instruction {
        program_id: some_program,
        accounts: vec![AccountMeta::new(signer.pubkey(), true)],
        data: vec![0xDD],
    };
    let tip_ix = system_instruction::transfer(&signer.pubkey(), &tip_account, 5000);

    let instructions = vec![swap_ix, tip_ix];

    // ALT contains the tip account — this could happen if someone puts
    // Jito tip accounts in the ALT for other reasons
    let alt = AddressLookupTableAccount {
        key: Pubkey::new_unique(),
        addresses: vec![some_program, tip_account],
    };

    let blockhash = Hash::new_unique();
    let v0_msg = v0::Message::try_compile(
        &signer.pubkey(),
        &instructions,
        std::slice::from_ref(&alt),
        blockhash,
    )
    .expect("V0 compile should succeed");

    // The tip account might be resolved via ALT. If so, it must be in
    // the writable_indexes, NOT readonly_indexes.
    let tip_in_static = v0_msg.account_keys.contains(&tip_account);

    if tip_in_static {
        // Static key — check writability via header
        let tip_idx = v0_msg
            .account_keys
            .iter()
            .position(|k| *k == tip_account)
            .unwrap();
        let header = &v0_msg.header;
        let num_ro_unsigned = header.num_readonly_unsigned_accounts as usize;
        let total = v0_msg.account_keys.len();
        let writable_end = total - num_ro_unsigned;
        assert!(
            tip_idx < writable_end,
            "Tip in static keys must be writable (idx={}, writable_end={})",
            tip_idx,
            writable_end
        );
    } else {
        // Resolved via ALT — must be in writable_indexes
        let mut found_writable = false;
        for lookup in &v0_msg.address_table_lookups {
            if lookup.account_key == alt.key {
                // Find which ALT index corresponds to tip_account
                let alt_idx = alt
                    .addresses
                    .iter()
                    .position(|a| *a == tip_account)
                    .unwrap() as u8;
                if lookup.writable_indexes.contains(&alt_idx) {
                    found_writable = true;
                }
                assert!(
                    !lookup.readonly_indexes.contains(&alt_idx),
                    "Tip account must NOT be in ALT readonly_indexes"
                );
            }
        }
        assert!(
            found_writable,
            "Tip account resolved via ALT must be in writable_indexes"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 7: Base58 encoding round-trip preserves tip account writability
//
// This tests the actual fix: previously build_signed_bundle_tx returned
// base64, but Jito sendBundle expects base58. Sending base64 data where
// base58 is expected causes Jito to decode garbage and fail to find
// tip accounts in the writable keys.
// ---------------------------------------------------------------------------

#[test]
fn test_base58_roundtrip_preserves_tip_writability() {
    use solana_mev_bot::executor::relays::common;

    let signer = Keypair::new();
    let tip_account: Pubkey = JITO_TIP_ACCOUNTS[2].parse().unwrap();
    let some_program = Pubkey::new_unique();

    let swap_ix = Instruction {
        program_id: some_program,
        accounts: vec![AccountMeta::new(signer.pubkey(), true)],
        data: vec![0xEE],
    };

    let blockhash = Hash::new_unique();

    let serialized = common::build_signed_bundle_tx(
        "jito",
        &[swap_ix],
        25_000,
        &tip_account,
        &signer,
        blockhash,
        &[],
        None,
    )
    .expect("build_signed_bundle_tx should succeed");

    // Encode as base58 (what Jito expects)
    let bs58_encoded = common::encode_base58(&serialized);

    // Decode back from base58
    let decoded = bs58::decode(&bs58_encoded)
        .into_vec()
        .expect("base58 decode should succeed");

    assert_eq!(
        serialized, decoded,
        "Base58 round-trip must preserve exact bytes"
    );

    // Verify the decoded transaction still has the tip account writable
    let tx: VersionedTransaction =
        bincode::deserialize(&decoded).expect("bincode deserialize");

    let tip_found_writable = match &tx.message {
        VersionedMessage::Legacy(msg) => {
            let header = &msg.header;
            let num_ro_unsigned = header.num_readonly_unsigned_accounts as usize;
            let total = msg.account_keys.len();
            let writable_end = total - num_ro_unsigned;
            msg.account_keys
                .iter()
                .enumerate()
                .any(|(idx, k)| *k == tip_account && idx < writable_end)
        }
        VersionedMessage::V0(msg) => {
            let header = &msg.header;
            let num_ro_unsigned = header.num_readonly_unsigned_accounts as usize;
            let total = msg.account_keys.len();
            let writable_end = total - num_ro_unsigned;
            msg.account_keys
                .iter()
                .enumerate()
                .any(|(idx, k)| *k == tip_account && idx < writable_end)
        }
        _ => false,
    };

    assert!(
        tip_found_writable,
        "Tip account must be writable after base58 round-trip"
    );
}

// ---------------------------------------------------------------------------
// Test 8: Base64 vs base58 encoding produces different strings
//
// Sanity check: if base64 and base58 produced the same output, the encoding
// bug wouldn't matter. They must differ.
// ---------------------------------------------------------------------------

#[test]
fn test_base64_and_base58_differ() {
    use solana_mev_bot::executor::relays::common;

    let data = vec![0u8, 1, 2, 3, 255, 254, 253];
    let b58 = common::encode_base58(&data);
    let b64 = common::encode_base64(&data);

    assert_ne!(
        b58, b64,
        "Base58 and base64 encodings must differ for non-trivial data"
    );

    // Verify both decode back to the original
    let decoded_58 = bs58::decode(&b58).into_vec().unwrap();
    let decoded_64 = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &b64,
    )
    .unwrap();

    assert_eq!(data, decoded_58);
    assert_eq!(data, decoded_64);
}
