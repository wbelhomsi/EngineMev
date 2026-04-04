# On-Chain Profit Guard (arb-guard) — Phase A

**Date:** 2026-04-04
**Status:** Approved

## Problem

Our swap transactions can execute on-chain even when the arb opportunity has expired (stale state between simulation and landing). The off-chain simulator catches most unprofitable routes, but state can change between simulation and on-chain execution. We need an on-chain safety net.

## Solution

An Anchor 0.32.1 program with 2 instructions that wrap our existing swap IXs:

```
TX: [start_check] → [ATA creates] → [wSOL wrap] → [swap 1] → [swap 2] → [wSOL unwrap] → [profit_check]
```

If `profit_check` finds the token balance hasn't increased by `min_profit`, the instruction errors → entire TX reverts → no SOL lost (except base tx fee ~5400 lamports).

## Program: arb-guard

### GuardState PDA

```rust
#[account]
pub struct GuardState {
    pub start_balance: u64,   // 8 bytes
    pub authority: Pubkey,    // 32 bytes
}
// Seeds: [b"guard", authority.key().as_ref()]
// Size: 8 (discriminator) + 8 + 32 = 48 bytes
// Rent: ~0.001 SOL (one-time, reusable)
```

### Instruction 1: start_check

Records the wSOL ATA balance before swaps begin.

```rust
pub fn start_check(ctx: Context<StartCheck>) -> Result<()> {
    let guard = &mut ctx.accounts.guard_state;
    guard.start_balance = ctx.accounts.token_account.amount;
    guard.authority = ctx.accounts.authority.key();
    Ok(())
}

#[derive(Accounts)]
pub struct StartCheck<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        init_if_needed,
        payer = authority,
        space = 8 + 8 + 32,
        seeds = [b"guard", authority.key().as_ref()],
        bump,
    )]
    pub guard_state: Account<'info, GuardState>,
    pub token_account: Account<'info, TokenAccount>,
    pub system_program: Program<'info, System>,
}
```

### Instruction 2: profit_check

Reads balance after swaps, asserts profit.

```rust
pub fn profit_check(ctx: Context<ProfitCheck>, min_profit: u64) -> Result<()> {
    ctx.accounts.token_account.reload()?;
    let end_balance = ctx.accounts.token_account.amount;
    let start_balance = ctx.accounts.guard_state.start_balance;
    require!(
        end_balance >= start_balance.checked_add(min_profit).unwrap_or(u64::MAX),
        ArbGuardError::NoProfitDetected
    );
    Ok(())
}

#[derive(Accounts)]
pub struct ProfitCheck<'info> {
    pub authority: Signer<'info>,
    #[account(
        seeds = [b"guard", authority.key().as_ref()],
        bump,
        has_one = authority,
    )]
    pub guard_state: Account<'info, GuardState>,
    pub token_account: Account<'info, TokenAccount>,
}
```

### Error

```rust
#[error_code]
pub enum ArbGuardError {
    #[msg("No profit detected — reverting to protect funds")]
    NoProfitDetected,
}
```

## Project Structure

```
programs/arb-guard/
├── Cargo.toml          # anchor-lang 0.32.1, anchor-spl
├── src/
│   └── lib.rs          # ~80 lines
Anchor.toml              # workspace config, program ID
```

## Integration with EngineMev

### bundle.rs changes

In `build_arb_instructions()`, when `ARB_GUARD_PROGRAM_ID` is configured:

1. **Prepend** `start_check` IX before compute budget IXs:
   - Accounts: authority (signer), guard_state PDA, wSOL ATA, system_program
   - Data: Anchor discriminator for `start_check`

2. **Append** `profit_check` IX after wSOL unwrap (CloseAccount):
   - Accounts: authority (signer), guard_state PDA, wSOL ATA
   - Data: Anchor discriminator for `profit_check` + min_profit (u64 LE)

3. `min_profit = 0` by default (just prevent loss). Configurable via `MIN_ON_CHAIN_PROFIT` env var.

### config.rs changes

- `ARB_GUARD_PROGRAM_ID` — optional env var. If not set, guard IXs are not added (graceful degradation).

### Relay changes

None — the guard IXs are part of `base_instructions`, each relay adds its own tip as before.

## Deployment

1. Install Anchor CLI: `cargo install --git https://github.com/coral-xyz/anchor --tag v0.32.1 anchor-cli`
2. `cd programs/arb-guard && anchor build`
3. `anchor deploy --provider.cluster mainnet` (~2 SOL program rent)
4. Copy program ID to `.env`: `ARB_GUARD_PROGRAM_ID=<address>`
5. Initialize guard PDA: first `start_check` call creates it via `init_if_needed`

## CU Budget

- `start_check`: ~5,000 CU (read token balance, write 48-byte PDA)
- `profit_check`: ~8,000 CU (reload token account, read PDA, compare)
- Total overhead: **~13,000 CU** on top of existing swap CU
- Fits within current 400K budget with room to spare

## Testing

### Anchor Tests (TypeScript)

- Deploy to localnet via `anchor test`
- Test 1: start_check → simulate profit (transfer tokens in) → profit_check → SUCCESS
- Test 2: start_check → simulate loss (transfer tokens out) → profit_check → REVERT with NoProfitDetected
- Test 3: start_check → no change → profit_check with min_profit=1 → REVERT
- Test 4: start_check → profit of 100 → profit_check with min_profit=50 → SUCCESS

### Surfpool E2E

- Full arb TX with guard wrapping Orca swap
- Verify guard doesn't interfere with successful swaps
- Verify guard reverts on unprofitable conditions

## Cost

- Program deployment: ~2 SOL rent (recoverable if program closed)
- Guard PDA: ~0.001 SOL rent (one-time, reusable)
- Per-TX overhead: 0 SOL (just CU)

## Phase B (Future)

Replace the swap IXs between guards with a single-instruction CPI executor that:
- CPIs into DEX A, reads actual output via `.reload()`
- CPIs into DEX B with real output amount
- Eliminates the stale `estimated_output` problem
- The guards become redundant (profit check is built into the executor)
