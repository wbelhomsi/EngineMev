# Surfpool E2E Tests Design

**Date:** 2026-04-04
**Status:** Approved

## Goal

End-to-end tests using Surfpool (mainnet fork) that verify each DEX swap instruction executes correctly on-chain, and that the full 2-hop arb pipeline works.

## Architecture

### Test Infrastructure

**`tests/e2e_surfpool/harness.rs`** — Surfpool lifecycle manager
- Spawns `surfpool start --rpc-url $RPC_URL --ci --airdrop <signer> --no-deploy` as subprocess
- Polls `getHealth` every 500ms, timeout 30s
- Provides `send_tx(instructions, signer, blockhash)` → sends via `sendTransaction(skipPreflight=true)`, returns (signature, logs, error)
- Provides `get_token_balance(owner, mint)` → fetches SPL token balance
- Provides `get_sol_balance(pubkey)` → fetches native SOL balance
- `Drop` impl kills the subprocess
- Uses port 18900 (not 8899) to avoid conflicts with any running Surfpool instance

**`tests/e2e_surfpool/common.rs`** — Shared test helpers
- `test_keypair()` → deterministic Keypair for tests
- `build_swap_tx(pool, input_mint, output_mint, amount, signer, blockhash, state_cache)` → builds compute budget + ATA creates + wSOL wrap + swap IX + wSOL unwrap
- Hardcoded known pool addresses per DEX type (from mainnet)

### Per-DEX Swap Tests

Each test: find known pool → build single swap → send to Surfpool → verify success.

| Test | DEX | Pool | Input | Output |
|------|-----|------|-------|--------|
| `test_orca_whirlpool_swap` | Orca | SOL/USDC Whirlpool | SOL | USDC |
| `test_raydium_cp_swap` | Raydium CP | SOL/USDC | SOL | USDC |
| `test_raydium_clmm_swap` | Raydium CLMM | SOL/USDC | SOL | USDC |
| `test_meteora_dlmm_swap` | DLMM | SOL/USDC (with bitmap) | SOL | USDC |
| `test_meteora_damm_v2_swap` | DAMM v2 | any SOL pair | SOL | token |
| `test_sanctum_swap` | Sanctum | jitoSOL virtual | SOL | jitoSOL |
| `test_phoenix_swap` | Phoenix | SOL/USDC market | SOL | USDC |
| `test_manifest_swap` | Manifest | SOL/USDC market | SOL | USDC |

### Pipeline Tests

| Test | Description |
|------|-------------|
| `test_2hop_arb_roundtrip` | SOL→USDC on DEX A → USDC→SOL on DEX B. Verify SOL balance. |
| `test_wsol_wrap_unwrap` | wSOL wrap + swap + CloseAccount → native SOL returned |
| `test_token2022_ata` | Swap involving Token-2022 mint → ATA created with correct program |

### Feature Gate

```toml
[features]
e2e_surfpool = []
```

Run: `RPC_URL=https://... cargo test --features e2e_surfpool --test e2e_surfpool`

### Known Pool Addresses

Hardcoded mainnet pool addresses that Surfpool will lazy-clone. These must be active pools with liquidity. Selected pools should trade SOL or wSOL to avoid needing pre-funded token accounts.

### Surfpool Configuration

```bash
surfpool start \
  --rpc-url $RPC_URL \
  --ci \
  --port 18900 \
  --ws-port 18901 \
  --airdrop <test_signer> \
  --airdrop-amount 100000000000 \
  --no-deploy
```

100 SOL airdrop, CI mode (no TUI), custom port to avoid conflicts.

### Error Reporting

On test failure, print full program logs from the transaction. This is the most valuable debugging output (how we found Token-2022, bitmap, and wSOL issues).

### Dependencies

- `surfpool` CLI (installed via `curl -sL https://run.surfpool.run/ | bash`)
- `RPC_URL` environment variable pointing to a mainnet RPC
- `reqwest` for HTTP calls to Surfpool RPC
- `solana-sdk` for transaction building and signing
