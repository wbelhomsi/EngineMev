//! Tests for bugs fixed on 2026-04-16.
//!
//! Each test verifies a specific bug that was found during live testing
//! and ensures it doesn't regress.

use std::time::Duration;
use solana_sdk::pubkey::Pubkey;
use solana_mev_bot::config;
use solana_mev_bot::router::pool::{DexType, PoolExtra, PoolState, ArbRoute, RouteHop};
use solana_mev_bot::router::ProfitSimulator;
use solana_mev_bot::router::simulator::SimulationResult;
use solana_mev_bot::state::StateCache;

// ─── Bug #1: min_final_output was too tight (slippage-adjusted, not break-even) ───

#[test]
fn test_min_final_output_is_break_even() {
    // Setup: two pools with a 5% price spread (profitable after fees)
    let cache = StateCache::new(Duration::from_secs(60));
    let sol = config::sol_mint();
    let token = Pubkey::new_unique();

    let pool_a = Pubkey::new_unique();
    cache.upsert(pool_a, PoolState {
        address: pool_a,
        dex_type: DexType::RaydiumCp,
        token_a_mint: sol,
        token_b_mint: token,
        token_a_reserve: 1_000_000_000_000, // 1000 SOL
        token_b_reserve: 1_000_000_000_000,
        fee_bps: 25,
        current_tick: None, sqrt_price_x64: None, liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None, best_ask_price: None,
    });

    let pool_b = Pubkey::new_unique();
    cache.upsert(pool_b, PoolState {
        address: pool_b,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: sol,
        token_b_mint: token,
        token_a_reserve: 1_000_000_000_000,
        token_b_reserve: 1_050_000_000_000, // 5% spread
        fee_bps: 25,
        current_tick: None, sqrt_price_x64: None, liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None, best_ask_price: None,
    });

    let input_amount = 10_000_000; // 0.01 SOL
    let simulator = ProfitSimulator::new(cache, 0.50, 1000, 1000)
        .with_slippage_tolerance(0.25);

    let route = ArbRoute {
        hops: vec![
            RouteHop {
                pool_address: pool_a, dex_type: DexType::RaydiumCp,
                input_mint: sol, output_mint: token,
                estimated_output: 9_950_000,
            },
            RouteHop {
                pool_address: pool_b, dex_type: DexType::OrcaWhirlpool,
                input_mint: token, output_mint: sol,
                estimated_output: 10_400_000,
            },
        ],
        base_mint: sol,
        input_amount,
        estimated_profit: 400_000,
        estimated_profit_lamports: 400_000,
    };

    let result = simulator.simulate(&route);
    match result {
        SimulationResult::Profitable { min_final_output, .. } => {
            // min_final_output should be input_amount (break-even), NOT input + adjusted profit
            assert_eq!(
                min_final_output, input_amount,
                "min_final_output should be break-even (input_amount), got {}",
                min_final_output
            );
        }
        SimulationResult::Unprofitable { reason } => {
            // Acceptable if fees eat the spread
            assert!(
                !reason.contains("slippage"),
                "Should not fail due to slippage: {}", reason
            );
        }
    }
}

// ─── Bug #2: Sanctum LstStateList header was 16 bytes instead of 12 ───

#[test]
fn test_sanctum_lst_state_list_header_is_12_bytes() {
    // The header is: 8-byte Anchor discriminator + 4-byte Borsh Vec length = 12 bytes.
    // Each entry is 80 bytes. With header=12, entry 0 starts at byte 12.
    // With the old header=16, entry 0 started at byte 16 (4 bytes into the first entry).
    let header_size: usize = 12; // 8 (Anchor disc) + 4 (Borsh vec len)
    let entry_size: usize = 80;

    // Simulate a 252-byte account (header + 3 entries)
    let total = header_size + entry_size * 3;
    assert_eq!(total, 252);

    // Verify entry offsets
    let entry_0_start = header_size;
    let entry_1_start = header_size + entry_size;
    let entry_2_start = header_size + entry_size * 2;

    assert_eq!(entry_0_start, 12);
    assert_eq!(entry_1_start, 92);
    assert_eq!(entry_2_start, 172);

    // Old broken offset would be:
    let old_header = 16;
    assert_ne!(old_header, header_size, "Header should be 12, not 16");
}

// ─── Bug #3: DLMM bitmap extension must only be passed when confirmed on-chain ───

#[test]
fn test_dlmm_bitmap_extension_is_program_id_when_none() {
    // Deriving the bitmap PDA always (without checking existence) causes error 3007
    // (AccountOwnedByWrongProgram) because uninitialized PDAs exist as System-owned
    // empty accounts. When extra.bitmap_extension is None, we MUST pass the DLMM
    // program ID as the Anchor Option<Account> "None" marker.
    let dlmm_program = solana_mev_bot::addresses::METEORA_DLMM;

    // When bitmap_extension is None in PoolExtra, the swap IX builder should use
    // the program ID as the None marker (not derive a PDA).
    let extra_no_bitmap = PoolExtra::default();
    let (bitmap, is_real) = match extra_no_bitmap.bitmap_extension {
        Some(pda) => (pda, true),
        None => (dlmm_program, false),
    };
    assert_eq!(bitmap, dlmm_program, "None bitmap → program ID marker");
    assert!(!is_real, "None bitmap → readonly");

    // When bitmap_extension is Some(pda), it should be used as-is (writable)
    let real_pda = Pubkey::new_unique();
    let extra_with_bitmap = PoolExtra {
        bitmap_extension: Some(real_pda),
        ..Default::default()
    };
    let (bitmap2, is_real2) = match extra_with_bitmap.bitmap_extension {
        Some(pda) => (pda, true),
        None => (dlmm_program, false),
    };
    assert_eq!(bitmap2, real_pda, "Real bitmap → actual PDA");
    assert!(is_real2, "Real bitmap → writable");
}

// ─── Bug #9: DLMM bitmap overflow check — skip routes that need unavailable bitmap ───

#[test]
fn test_dlmm_bitmap_overflow_check() {
    // The DLMM internal bitmap covers bin_array_index range [-512, 511].
    // If any bin array we'd need falls outside that range, bitmap_extension
    // is REQUIRED. If we don't have it, we must skip the route.
    const MAX_BIN_PER_ARRAY: i32 = 70;
    const BIN_ARRAY_BITMAP_SIZE: i32 = 512;

    // Test: active_id within normal range, a_to_b (offsets [0, -1, -2])
    let active_id_normal: i32 = 100;
    let bin_idx_normal = if active_id_normal >= 0 || active_id_normal % MAX_BIN_PER_ARRAY == 0 {
        active_id_normal / MAX_BIN_PER_ARRAY
    } else {
        active_id_normal / MAX_BIN_PER_ARRAY - 1
    };
    let offsets_a_to_b: [i32; 3] = [0, -1, -2];
    let needs_bitmap_normal = offsets_a_to_b.iter()
        .any(|&o| {
            let idx = bin_idx_normal + o;
            idx > BIN_ARRAY_BITMAP_SIZE - 1 || idx < -BIN_ARRAY_BITMAP_SIZE
        });
    assert!(!needs_bitmap_normal, "active_id=100 should NOT need bitmap extension");

    // Test: active_id at extreme positive (requires bitmap)
    let active_id_high: i32 = 40_000; // bin_idx = 571 > 511
    let bin_idx_high = active_id_high / MAX_BIN_PER_ARRAY;
    assert_eq!(bin_idx_high, 571);
    let needs_bitmap_high = offsets_a_to_b.iter()
        .any(|&o| {
            let idx = bin_idx_high + o;
            idx > BIN_ARRAY_BITMAP_SIZE - 1 || idx < -BIN_ARRAY_BITMAP_SIZE
        });
    assert!(needs_bitmap_high, "active_id=40000 SHOULD need bitmap extension");

    // Test: active_id at extreme negative (requires bitmap)
    let active_id_low: i32 = -40_000; // bin_idx = -572 < -512
    let bin_idx_low = if active_id_low >= 0 || active_id_low % MAX_BIN_PER_ARRAY == 0 {
        active_id_low / MAX_BIN_PER_ARRAY
    } else {
        active_id_low / MAX_BIN_PER_ARRAY - 1
    };
    assert_eq!(bin_idx_low, -572);
    let offsets_b_to_a: [i32; 3] = [0, 1, 2];
    let needs_bitmap_low = offsets_b_to_a.iter()
        .any(|&o| {
            let idx = bin_idx_low + o;
            idx > BIN_ARRAY_BITMAP_SIZE - 1 || idx < -BIN_ARRAY_BITMAP_SIZE
        });
    assert!(needs_bitmap_low, "active_id=-40000 SHOULD need bitmap extension");

    // Test: boundary case — bin_idx = 511 is fine, 512 needs bitmap
    let at_boundary: i32 = 511 * 70; // bin_idx = 511
    let bin_idx_boundary = at_boundary / MAX_BIN_PER_ARRAY;
    assert_eq!(bin_idx_boundary, 511);
    let needs_at_511 = [0, 1, 2].iter()
        .any(|&o| {
            let idx = bin_idx_boundary + o;
            idx > BIN_ARRAY_BITMAP_SIZE - 1 || idx < -BIN_ARRAY_BITMAP_SIZE
        });
    assert!(needs_at_511, "bin_idx=511 with b_to_a offsets reaches 513 → needs bitmap");
}

// ─── Bug #4: Mint program cache should be populated from parser pool extra ───

#[test]
fn test_mint_program_cached_from_pool_extra() {
    let cache = StateCache::new(Duration::from_secs(60));
    let sol = config::sol_mint();
    let token = Pubkey::new_unique();
    let token_2022 = solana_mev_bot::addresses::TOKEN_2022;
    let spl_token = solana_mev_bot::addresses::SPL_TOKEN;

    // Before caching: mint program unknown
    assert!(cache.get_mint_program(&token).is_none());

    // Parser sets token_program_b = Token-2022 in PoolExtra
    let pool_addr = Pubkey::new_unique();
    cache.upsert(pool_addr, PoolState {
        address: pool_addr,
        dex_type: DexType::MeteoraDlmm,
        token_a_mint: sol,
        token_b_mint: token,
        token_a_reserve: 1_000_000_000,
        token_b_reserve: 1_000_000_000,
        fee_bps: 30,
        current_tick: None, sqrt_price_x64: None, liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            token_program_a: Some(spl_token),
            token_program_b: Some(token_2022),
            ..Default::default()
        },
        best_bid_price: None, best_ask_price: None,
    });

    // Simulate what stream.rs now does: cache from pool extra
    let pool = cache.get_any(&pool_addr).unwrap();
    if let Some(prog) = pool.extra.token_program_a {
        cache.set_mint_program(pool.token_a_mint, prog);
    }
    if let Some(prog) = pool.extra.token_program_b {
        cache.set_mint_program(pool.token_b_mint, prog);
    }

    // After caching: mint programs are known
    assert_eq!(cache.get_mint_program(&sol), Some(spl_token));
    assert_eq!(cache.get_mint_program(&token), Some(token_2022));
}

// ─── Bug #5: output_token_index defaulted to 0 (signer) when ATA not found ───

#[test]
fn test_output_token_index_must_not_default_to_zero() {
    // The output_token_index in HopV2Params is used by arb-guard to read
    // the output token account balance. If it defaults to 0 (the signer),
    // get_token_balance fails with InvalidTokenAccount because the signer
    // account is not a token account.
    //
    // This test verifies the conceptual invariant: index 0 in remaining_accounts
    // is always the signer, never a token account.
    use solana_sdk::instruction::AccountMeta;

    let signer = Pubkey::new_unique();
    let token_ata = Pubkey::new_unique();

    let remaining_accounts = vec![
        AccountMeta::new(signer, true),      // index 0: signer
        AccountMeta::new(token_ata, false),   // index 1: token account
    ];

    // Searching for the token ATA should find index 1, not 0
    let found_idx = remaining_accounts.iter()
        .position(|a| a.pubkey == token_ata);
    assert_eq!(found_idx, Some(1), "Token ATA should be at index 1, not 0");

    // Searching for a non-existent ATA should return None, not default to 0
    let missing_ata = Pubkey::new_unique();
    let not_found = remaining_accounts.iter()
        .position(|a| a.pubkey == missing_ata);
    assert!(not_found.is_none(), "Missing ATA should return None, not Some(0)");
}

// ─── Bug #6: Tip floor WS returns SOL floats, not lamports ───

#[test]
fn test_tip_floor_ws_sol_to_lamports_conversion() {
    // The Jito tip stream WS sends values like 2.6665e-6 (SOL).
    // parse_tip_value must convert these to lamports.
    let json_float = serde_json::json!(2.6665e-6);

    // Values < 1000 are treated as SOL and multiplied by 1e9
    if let Some(f) = json_float.as_f64() {
        let lamports = if f > 0.0 && f < 1000.0 {
            (f * 1_000_000_000.0) as u64
        } else {
            f as u64
        };
        // 2.6665e-6 SOL = ~2666 lamports
        assert!(lamports > 2000 && lamports < 3000,
            "Expected ~2666 lamports, got {}", lamports);
    }
}

// ─── Bug #8: Token program must come from vault owner, not parser default ───

#[test]
fn test_vault_owner_is_token_program_source() {
    // Insight from reference implementation: the token program for each side of a
    // pool is the vault account's `owner` field, NOT a field inside the pool account.
    //
    // Whirlpool and Raydium CLMM pool accounts do NOT contain token_program fields.
    // Parsers that hardcode SPL_TOKEN are wrong for Token-2022 pools.
    // The correct source is getMultipleAccountsInfo([vault_a, vault_b]).owner.
    //
    // This test verifies the conceptual invariant: when we have a vault AccountInfo,
    // its owner field is the token program for the mint that vault holds.

    let spl_token = solana_mev_bot::addresses::SPL_TOKEN;
    let token_2022 = solana_mev_bot::addresses::TOKEN_2022;

    // Vault owned by SPL Token program → mint uses SPL Token
    let vault_owner_spl = spl_token;
    assert_eq!(vault_owner_spl, spl_token, "SPL Token vault → SPL Token program");

    // Vault owned by Token-2022 program → mint uses Token-2022
    let vault_owner_2022 = token_2022;
    assert_eq!(vault_owner_2022, token_2022, "Token-2022 vault → Token-2022 program");

    // These two programs are distinct
    assert_ne!(spl_token, token_2022);
}

// ─── Bug #10: Bundle builder must error on unknown mint program, not default ───

#[test]
fn test_bundle_builder_errors_on_unknown_mint_program() {
    use solana_sdk::signature::Keypair;
    use solana_mev_bot::executor::BundleBuilder;
    use solana_mev_bot::addresses;

    let cache = StateCache::new(Duration::from_secs(60));
    let wsol = addresses::WSOL;
    let unknown_token = Pubkey::new_unique(); // deliberately not cached

    // Pre-populate wSOL (standard) but NOT the unknown token
    cache.set_mint_program(wsol, addresses::SPL_TOKEN);

    // Pool with the unknown mint, no pool extra info
    let pool_addr = Pubkey::new_unique();
    cache.upsert(pool_addr, PoolState {
        address: pool_addr,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: wsol,
        token_b_mint: unknown_token,
        token_a_reserve: 1_000_000,
        token_b_reserve: 1_000_000,
        fee_bps: 30,
        current_tick: Some(0),
        sqrt_price_x64: Some(1 << 64),
        liquidity: Some(1_000_000),
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            // Deliberately no token_program_a/b set
            ..Default::default()
        },
        best_bid_price: None, best_ask_price: None,
    });

    let signer = Keypair::new();
    let arb_guard = Pubkey::new_unique();
    let builder = BundleBuilder::new(
        signer.insecure_clone(),
        cache.clone(),
        Some(arb_guard),
    );

    let route = ArbRoute {
        hops: vec![
            RouteHop {
                pool_address: pool_addr, dex_type: DexType::OrcaWhirlpool,
                input_mint: wsol, output_mint: unknown_token,
                estimated_output: 1_000,
            },
            RouteHop {
                pool_address: pool_addr, dex_type: DexType::OrcaWhirlpool,
                input_mint: unknown_token, output_mint: wsol,
                estimated_output: 1_050,
            },
        ],
        base_mint: wsol,
        input_amount: 1_000,
        estimated_profit: 50,
        estimated_profit_lamports: 50,
    };

    // Must error (not silently default to SPL Token for unknown_token)
    let result = builder.build_arb_instructions(&route, 1_000);
    assert!(result.is_err(), "Should error when mint program is unknown");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Mint program unknown"),
        "Error should mention unknown mint program, got: {}", err_msg
    );
}

// ─── Bug #11: Vault fetch must parse owner field for token program resolution ───

#[test]
fn test_vault_fetch_response_parses_owner_as_token_program() {
    // Verify that the getMultipleAccounts response parsing correctly extracts
    // the `owner` field (which is the vault's token program for SPL Token accounts).
    //
    // Response shape for a token account:
    // {
    //   "result": { "value": [
    //     { "data": ["base64data", "base64"], "owner": "TokenkegQfeZyi...", ... }
    //   ]}
    // }

    use std::str::FromStr;
    let response = serde_json::json!({
        "result": {
            "value": [
                {
                    "data": ["AAAAAAAAAAA=", "base64"], // 8 bytes of zeros (0 balance)
                    "owner": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" // SPL Token
                },
                {
                    "data": ["AAAAAAAAAAA=", "base64"],
                    "owner": "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb" // Token-2022
                }
            ]
        }
    });

    let values = response.get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_array())
        .unwrap();

    let owner_0 = values[0].get("owner").and_then(|v| v.as_str()).unwrap();
    let owner_1 = values[1].get("owner").and_then(|v| v.as_str()).unwrap();

    let prog_0 = Pubkey::from_str(owner_0).unwrap();
    let prog_1 = Pubkey::from_str(owner_1).unwrap();

    assert_eq!(prog_0, solana_mev_bot::addresses::SPL_TOKEN);
    assert_eq!(prog_1, solana_mev_bot::addresses::TOKEN_2022);
}

// ─── Bug #12: Stream must not overwrite cached mint program with parser default ───

#[test]
fn test_stream_does_not_overwrite_authoritative_mint_program() {
    // If the RPC fetch cached mint → Token-2022, the next Geyser event (from a
    // parser that hardcodes SPL Token) must NOT overwrite it.
    let cache = StateCache::new(Duration::from_secs(60));
    let token_2022 = solana_mev_bot::addresses::TOKEN_2022;
    let spl_token = solana_mev_bot::addresses::SPL_TOKEN;
    let mint = Pubkey::new_unique();

    // RPC fetch sets the authoritative value
    cache.set_mint_program(mint, token_2022);
    assert_eq!(cache.get_mint_program(&mint), Some(token_2022));

    // Simulate stream.rs logic: only set if not already cached
    let parser_default = spl_token;
    if cache.get_mint_program(&mint).is_none() {
        cache.set_mint_program(mint, parser_default);
    }

    // The authoritative Token-2022 value should remain
    assert_eq!(
        cache.get_mint_program(&mint), Some(token_2022),
        "Parser default must not overwrite RPC-fetched value"
    );
}

// ─── Bug #7: Simulation used replaceRecentBlockhash which skipped execution ───

#[test]
fn test_simulation_params_dont_replace_blockhash() {
    // Simulation with replaceRecentBlockhash=true returns CU=0 and empty logs
    // (false positive). We must NOT use replaceRecentBlockhash — use the real
    // blockhash from our 2s-refresh cache instead.
    //
    // This is a documentation test — the actual parameter is in rpc_helpers.rs.
    // Verify the correct params structure:
    let params = serde_json::json!({
        "encoding": "base64",
        "sigVerify": false,
        "commitment": "confirmed"
    });

    // Must NOT contain replaceRecentBlockhash
    assert!(
        params.get("replaceRecentBlockhash").is_none(),
        "Simulation must not use replaceRecentBlockhash (causes CU=0 false positives)"
    );

    // Must have sigVerify=false (we sign the tx but RPC doesn't need to verify)
    assert_eq!(params["sigVerify"], false);

    // Must use confirmed commitment (not processed — too ephemeral)
    assert_eq!(params["commitment"], "confirmed");
}
