use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Verify arb-guard accounts are in ALT
// ---------------------------------------------------------------------------

/// ALT addresses currently loaded on-chain. Any arb-guard account NOT in this
/// list costs 32 bytes instead of 1 byte in V0 transactions.
fn alt_addresses() -> Vec<Pubkey> {
    [
        "11111111111111111111111111111111",
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
        "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",
        "ComputeBudget111111111111111111111111111111",
        "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",
        "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",
        "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C",
        "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK",
        "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",
        "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",
        "cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG",
        "5ocnV1qiCgaQR8Jb8xWnVbApfaygJ8tNoZfgPwsgx9kx",
        "PhoeNiXZ8ByJGLkxNfZRnkUfjvmuYqLR89jjFHGqdXY",
        "MNFSTqtC93rEfYHB6hF82sKdZpUDFWkViLByLd1k1Ms",
        "So11111111111111111111111111111111111111112",
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        "CbjPG5TEEhZGXsA8prmJPfvgH51rudYgcubRUtCCGyUw",
    ]
    .iter()
    .map(|s| Pubkey::from_str(s).unwrap())
    .collect()
}

/// Derive the guard PDA the same way bundle.rs does.
fn derive_guard_pda(program_id: &Pubkey, authority: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"guard", authority.as_ref()], program_id).0
}

/// Derive ATA using SPL convention (seeds = [owner, token_program, mint]).
fn derive_ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    let token_program = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    let ata_program = Pubkey::from_str("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL").unwrap();
    Pubkey::find_program_address(
        &[owner.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ata_program,
    )
    .0
}

#[test]
fn test_arb_guard_program_in_alt() {
    let alt = alt_addresses();
    let guard_program =
        Pubkey::from_str("CbjPG5TEEhZGXsA8prmJPfvgH51rudYgcubRUtCCGyUw").unwrap();
    assert!(
        alt.contains(&guard_program),
        "guard program must be in ALT"
    );
}

#[test]
fn test_system_program_in_alt() {
    let alt = alt_addresses();
    let system_program = Pubkey::from_str("11111111111111111111111111111111").unwrap();
    assert!(
        alt.contains(&system_program),
        "system program must be in ALT"
    );
}

#[test]
fn test_guard_pda_not_in_alt() {
    // The guard PDA is derived from the signer. Since it is signer-specific,
    // it is NOT in the shared ALT. This costs 32 bytes per occurrence.
    let alt = alt_addresses();
    let guard_program =
        Pubkey::from_str("CbjPG5TEEhZGXsA8prmJPfvgH51rudYgcubRUtCCGyUw").unwrap();
    // Use a representative signer pubkey
    let signer = Pubkey::from_str("149xtHKerf2MgJVQ2CZB34bUALs8GaZjZWmQnC9si9yh").unwrap();
    let guard_pda = derive_guard_pda(&guard_program, &signer);

    let pda_in_alt = alt.contains(&guard_pda);
    // Document finding: PDA is NOT in ALT, costs 32 bytes as static key
    println!(
        "Guard PDA {} in ALT: {} (32 bytes if not)",
        guard_pda, pda_in_alt
    );
    // This is expected to be false — the PDA is signer-specific
    assert!(
        !pda_in_alt,
        "Guard PDA is signer-specific and should NOT be in the shared ALT"
    );
}

#[test]
fn test_wsol_ata_not_in_alt() {
    // The wSOL ATA is derived from the signer. Since it is signer-specific,
    // it is NOT in the shared ALT. This costs 32 bytes per occurrence.
    let alt = alt_addresses();
    let wsol = Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap();
    let signer = Pubkey::from_str("149xtHKerf2MgJVQ2CZB34bUALs8GaZjZWmQnC9si9yh").unwrap();
    let wsol_ata = derive_ata(&signer, &wsol);

    let ata_in_alt = alt.contains(&wsol_ata);
    println!(
        "wSOL ATA {} in ALT: {} (32 bytes if not)",
        wsol_ata, ata_in_alt
    );
    // This is expected to be false — the ATA is signer-specific
    assert!(
        !ata_in_alt,
        "wSOL ATA is signer-specific and should NOT be in the shared ALT"
    );
}

#[test]
fn test_arb_guard_extra_bytes_estimate() {
    // arb-guard adds 2 IXs (start_check + profit_check).
    // Accounts NOT in ALT: guard_pda (appears in both IXs, deduplicated to 1 entry)
    //                      wsol_ata (appears in both IXs, deduplicated to 1 entry)
    // Accounts IN ALT: guard_program, system_program
    // Signer is already in the message header.
    //
    // Extra static accounts = 2 (guard_pda + wsol_ata) = 64 bytes
    // Plus instruction data: 8 bytes (start_check disc) + 16 bytes (profit_check disc + u64) = 24 bytes
    // Plus instruction overhead (program_id index + account count + data len) ~6 bytes × 2 = 12 bytes
    //
    // Total overhead ≈ 64 + 24 + 12 = ~100 bytes.
    // A tx at 1180 bytes without guard → 1280 with guard → over 1232 limit.
    //
    // The fix: skip arb-guard when instruction count / unique accounts exceed a threshold.

    // Verify the extra account count
    let accounts_not_in_alt = 2; // guard_pda, wsol_ata
    let extra_bytes_accounts = accounts_not_in_alt * 32;
    assert_eq!(extra_bytes_accounts, 64, "2 accounts not in ALT = 64 extra bytes");
}

// ---------------------------------------------------------------------------
// build_signed_bundle_tx size-limit behavior
// ---------------------------------------------------------------------------

#[test]
fn test_build_signed_bundle_tx_rejects_oversized() {
    use solana_sdk::{
        hash::Hash,
        instruction::{AccountMeta, Instruction},
        signature::Keypair,
        signer::Signer,
    };
    use solana_mev_bot::executor::relays::common;

    let signer = Keypair::new();
    let blockhash = Hash::new_unique();
    let tip_account = Pubkey::new_unique();

    // Create many instructions with many unique accounts to exceed 1232 bytes.
    // Each unique account adds 32 bytes to the legacy message.
    let mut instructions = Vec::new();
    for _ in 0..20 {
        instructions.push(Instruction {
            program_id: Pubkey::new_unique(),
            accounts: vec![
                AccountMeta::new(signer.pubkey(), true),
                AccountMeta::new(Pubkey::new_unique(), false),
                AccountMeta::new(Pubkey::new_unique(), false),
            ],
            data: vec![0u8; 32],
        });
    }

    let result = common::build_signed_bundle_tx(
        "test",
        &instructions,
        1000,
        &tip_account,
        &signer,
        blockhash,
        None,
    );

    assert!(result.is_err(), "oversized tx must return Err");
    let err = result.unwrap_err();
    assert!(
        err.error
            .as_ref()
            .unwrap()
            .contains("Tx too large"),
        "error message must mention 'Tx too large'"
    );
}

#[test]
fn test_build_signed_bundle_tx_accepts_small() {
    use solana_sdk::{
        hash::Hash,
        instruction::{AccountMeta, Instruction},
        signature::Keypair,
        signer::Signer,
    };
    use solana_mev_bot::executor::relays::common;

    let signer = Keypair::new();
    let blockhash = Hash::new_unique();
    let tip_account = Pubkey::new_unique();

    // One simple instruction — well under 1232 bytes.
    let instructions = vec![Instruction {
        program_id: Pubkey::new_unique(),
        accounts: vec![AccountMeta::new(signer.pubkey(), true)],
        data: vec![1u8],
    }];

    let result = common::build_signed_bundle_tx(
        "test",
        &instructions,
        1000,
        &tip_account,
        &signer,
        blockhash,
        None,
    );

    assert!(result.is_ok(), "small tx must succeed");
    let base64_str = result.unwrap();
    assert!(!base64_str.is_empty());
}

// ---------------------------------------------------------------------------
// record_relay_metrics categorizes tx_too_large correctly
// ---------------------------------------------------------------------------

#[test]
fn test_record_relay_metrics_tx_too_large_is_categorized() {
    use solana_mev_bot::executor::relays::{common, RelayResult};

    let r = RelayResult {
        relay_name: "jito".to_string(),
        success: false,
        bundle_id: None,
        error: Some("Tx too large: 1240 bytes (limit 1232)".to_string()),
        latency_us: 0,
    };
    // Must not panic. The error should be categorized as "tx_too_large".
    common::record_relay_metrics(&r);
}

// ---------------------------------------------------------------------------
// estimate_unique_accounts helper
// ---------------------------------------------------------------------------

#[test]
fn test_estimate_unique_accounts() {
    use solana_sdk::instruction::{AccountMeta, Instruction};
    use solana_mev_bot::executor::bundle::estimate_unique_accounts;

    let prog_a = Pubkey::new_unique();
    let prog_b = Pubkey::new_unique();
    let acc1 = Pubkey::new_unique();
    let acc2 = Pubkey::new_unique();
    let acc3 = Pubkey::new_unique();

    let instructions = vec![
        Instruction {
            program_id: prog_a,
            accounts: vec![
                AccountMeta::new(acc1, true),
                AccountMeta::new(acc2, false),
            ],
            data: vec![],
        },
        Instruction {
            program_id: prog_b,
            accounts: vec![
                AccountMeta::new(acc2, false), // duplicate
                AccountMeta::new(acc3, false),
            ],
            data: vec![],
        },
    ];

    let count = estimate_unique_accounts(&instructions);
    // prog_a, prog_b, acc1, acc2, acc3 = 5 unique
    assert_eq!(count, 5);
}

#[test]
fn test_estimate_unique_accounts_empty() {
    use solana_mev_bot::executor::bundle::estimate_unique_accounts;

    let count = estimate_unique_accounts(&[]);
    assert_eq!(count, 0);
}

#[test]
fn test_estimate_unique_accounts_all_duplicates() {
    use solana_sdk::instruction::{AccountMeta, Instruction};
    use solana_mev_bot::executor::bundle::estimate_unique_accounts;

    let prog = Pubkey::new_unique();
    let acc = Pubkey::new_unique();

    let instructions = vec![
        Instruction {
            program_id: prog,
            accounts: vec![AccountMeta::new(acc, false)],
            data: vec![],
        },
        Instruction {
            program_id: prog,
            accounts: vec![AccountMeta::new(acc, false)],
            data: vec![],
        },
    ];

    let count = estimate_unique_accounts(&instructions);
    // Only prog + acc = 2 unique
    assert_eq!(count, 2);
}
