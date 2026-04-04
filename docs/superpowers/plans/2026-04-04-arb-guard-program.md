# Arb Guard On-Chain Program Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deploy an Anchor program that wraps arb transactions with on-chain profit verification, reverting the entire TX if no profit is detected.

**Architecture:** Two-instruction Anchor program (start_check + profit_check) using a PDA to store pre-swap balance. Integrated into EngineMev's bundle builder as optional guard IXs. Graceful degradation when not configured.

**Tech Stack:** Anchor 0.32.1, anchor-lang, anchor-spl, Solana CLI, TypeScript (tests)

---

### Task 1: Install Anchor CLI and initialize project

**Files:**
- Create: `programs/arb-guard/Cargo.toml`
- Create: `programs/arb-guard/src/lib.rs`
- Create: `Anchor.toml`

- [ ] **Step 1: Install Anchor CLI 0.32.1**

```bash
cargo install --git https://github.com/coral-xyz/anchor --tag v0.32.1 anchor-cli --locked
anchor --version
# Expected: anchor-cli 0.32.1
```

If this fails (common with version conflicts), try:
```bash
cargo install --git https://github.com/coral-xyz/anchor avm --locked
avm install 0.32.1
avm use 0.32.1
anchor --version
```

- [ ] **Step 2: Initialize Anchor workspace**

Do NOT use `anchor init` (it creates a full new project). Instead, create the files manually to integrate with our existing Cargo workspace:

Create `Anchor.toml` at project root:
```toml
[features]
seeds = false
skip-lint = false

[programs.localnet]
arb_guard = "Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS"

[programs.mainnet]
arb_guard = "Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS"

[registry]
url = "https://api.apr.dev"

[provider]
cluster = "Localnet"
wallet = "~/.config/solana/id.json"

[scripts]
test = "npx ts-mocha -p ./tsconfig.json -t 1000000 tests/**/*.ts"
```

Note: `Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS` is a placeholder — Anchor generates the real program ID on first build.

Create `programs/arb-guard/Cargo.toml`:
```toml
[package]
name = "arb-guard"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "lib"]
name = "arb_guard"

[features]
no-entrypoint = []
no-idl = []
no-log-ix-name = []
cpi = ["no-entrypoint"]
default = []

[dependencies]
anchor-lang = "0.32.1"
anchor-spl = "0.32.1"
```

- [ ] **Step 3: Write the program**

Create `programs/arb-guard/src/lib.rs`:

```rust
use anchor_lang::prelude::*;
use anchor_spl::token::TokenAccount;

declare_id!("Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS");

#[program]
pub mod arb_guard {
    use super::*;

    /// Record the token account balance before swaps begin.
    /// Called as the FIRST instruction in an arb transaction.
    pub fn start_check(ctx: Context<StartCheck>) -> Result<()> {
        let guard = &mut ctx.accounts.guard_state;
        guard.start_balance = ctx.accounts.token_account.amount;
        guard.authority = ctx.accounts.authority.key();
        Ok(())
    }

    /// Verify profit after swaps complete.
    /// Called as the LAST instruction (before tip) in an arb transaction.
    /// Reverts the entire TX if balance hasn't increased by min_profit.
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

#[account]
pub struct GuardState {
    pub start_balance: u64,
    pub authority: Pubkey,
}

#[error_code]
pub enum ArbGuardError {
    #[msg("No profit detected — reverting to protect funds")]
    NoProfitDetected,
}
```

- [ ] **Step 4: Build the program**

```bash
cd /home/lunatic/Projects/EngineMev
anchor build --program-name arb_guard
```

This generates:
- `target/deploy/arb_guard.so` — the BPF binary
- `target/idl/arb_guard.json` — the IDL
- `target/types/arb_guard.ts` — TypeScript types
- Updates `declare_id!` in lib.rs with the actual program keypair

After build, note the program ID from the output and update `Anchor.toml` and `lib.rs`.

- [ ] **Step 5: Commit**

```bash
git add programs/ Anchor.toml
git commit -m "feat: arb-guard Anchor program — start_check + profit_check

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin main
```

---

### Task 2: Write Anchor tests

**Files:**
- Create: `tests/arb-guard/arb-guard.ts`
- Create: `tsconfig.json` (if not exists)
- Create: `package.json` (for test deps)

- [ ] **Step 1: Initialize Node.js test environment**

```bash
cd /home/lunatic/Projects/EngineMev
npm init -y
npm install --save-dev @coral-xyz/anchor @solana/web3.js chai mocha ts-mocha typescript @types/chai @types/mocha
```

Create `tsconfig.json`:
```json
{
  "compilerOptions": {
    "types": ["mocha", "chai"],
    "typeRoots": ["./node_modules/@types"],
    "lib": ["es2015"],
    "module": "commonjs",
    "target": "es6",
    "esModuleInterop": true,
    "resolveJsonModule": true
  }
}
```

- [ ] **Step 2: Write test file**

Create `tests/arb-guard/arb-guard.ts`:

```typescript
import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { ArbGuard } from "../../target/types/arb_guard";
import {
  createMint,
  createAccount,
  mintTo,
  transfer,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { assert } from "chai";

describe("arb-guard", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.ArbGuard as Program<ArbGuard>;
  const authority = provider.wallet.publicKey;

  let mint: anchor.web3.PublicKey;
  let tokenAccount: anchor.web3.PublicKey;
  let guardState: anchor.web3.PublicKey;

  before(async () => {
    // Create a test token and fund it
    mint = await createMint(provider.connection, provider.wallet.payer, authority, null, 9);
    tokenAccount = await createAccount(provider.connection, provider.wallet.payer, mint, authority);
    await mintTo(provider.connection, provider.wallet.payer, mint, tokenAccount, authority, 1_000_000_000);

    // Derive guard PDA
    [guardState] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("guard"), authority.toBuffer()],
      program.programId
    );
  });

  it("start_check records balance", async () => {
    await program.methods
      .startCheck()
      .accounts({
        authority,
        guardState,
        tokenAccount,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    const state = await program.account.guardState.fetch(guardState);
    assert.equal(state.startBalance.toNumber(), 1_000_000_000);
    assert.ok(state.authority.equals(authority));
  });

  it("profit_check succeeds when balance increased", async () => {
    // Mint more tokens (simulating swap profit)
    await mintTo(provider.connection, provider.wallet.payer, mint, tokenAccount, authority, 100_000);

    await program.methods
      .profitCheck(new anchor.BN(0))
      .accounts({ authority, guardState, tokenAccount })
      .rpc();
    // Should not throw
  });

  it("profit_check reverts when balance decreased", async () => {
    // Record current balance
    await program.methods
      .startCheck()
      .accounts({
        authority,
        guardState,
        tokenAccount,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    // Transfer tokens out (simulating loss)
    const recipient = await createAccount(provider.connection, provider.wallet.payer, mint, anchor.web3.Keypair.generate().publicKey);
    await transfer(provider.connection, provider.wallet.payer, tokenAccount, recipient, authority, 500_000_000);

    // profit_check should fail
    try {
      await program.methods
        .profitCheck(new anchor.BN(0))
        .accounts({ authority, guardState, tokenAccount })
        .rpc();
      assert.fail("Should have reverted");
    } catch (err) {
      assert.include(err.message, "NoProfitDetected");
    }
  });

  it("profit_check enforces min_profit", async () => {
    // Reset: mint tokens back up
    await mintTo(provider.connection, provider.wallet.payer, mint, tokenAccount, authority, 500_000_000);

    // Record balance
    await program.methods
      .startCheck()
      .accounts({
        authority,
        guardState,
        tokenAccount,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    // Add small profit (100 lamports)
    await mintTo(provider.connection, provider.wallet.payer, mint, tokenAccount, authority, 100);

    // min_profit=50 should succeed
    await program.methods
      .profitCheck(new anchor.BN(50))
      .accounts({ authority, guardState, tokenAccount })
      .rpc();

    // Reset and try min_profit=200 (more than we added)
    await program.methods
      .startCheck()
      .accounts({
        authority,
        guardState,
        tokenAccount,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    await mintTo(provider.connection, provider.wallet.payer, mint, tokenAccount, authority, 100);

    try {
      await program.methods
        .profitCheck(new anchor.BN(200))
        .accounts({ authority, guardState, tokenAccount })
        .rpc();
      assert.fail("Should have reverted — profit below min");
    } catch (err) {
      assert.include(err.message, "NoProfitDetected");
    }
  });
});
```

- [ ] **Step 3: Run tests**

```bash
anchor test
```

Expected: All 4 tests pass on localnet.

- [ ] **Step 4: Commit**

```bash
git add tests/arb-guard/ tsconfig.json package.json package-lock.json
git commit -m "test: arb-guard Anchor tests — profit/loss/min_profit verification

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin main
```

---

### Task 3: Integrate guard IXs into EngineMev bundle builder

**Files:**
- Modify: `src/executor/bundle.rs`
- Modify: `src/config.rs`

- [ ] **Step 1: Add ARB_GUARD_PROGRAM_ID to config**

In `src/config.rs`, add to `BotConfig`:
```rust
pub arb_guard_program_id: Option<Pubkey>,
```

Parse from env in `from_env()`:
```rust
arb_guard_program_id: std::env::var("ARB_GUARD_PROGRAM_ID")
    .ok()
    .and_then(|s| Pubkey::from_str(&s).ok()),
```

- [ ] **Step 2: Add guard IX builders to bundle.rs**

Add two functions that build the start_check and profit_check instructions:

```rust
/// Build start_check instruction for arb-guard program.
/// Records the wSOL ATA balance before swaps.
fn build_guard_start_check_ix(
    program_id: &Pubkey,
    authority: &Pubkey,
    token_account: &Pubkey,
) -> Instruction {
    // Anchor discriminator: sha256("global:start_check")[..8]
    let disc = anchor_discriminator("start_check");
    let guard_state = derive_guard_pda(program_id, authority);
    
    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*authority, true),          // authority (signer, mut for init_if_needed)
            AccountMeta::new(guard_state, false),         // guard_state PDA (mut)
            AccountMeta::new_readonly(*token_account, false), // token_account
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false), // system_program
        ],
        data: disc.to_vec(),
    }
}

/// Build profit_check instruction for arb-guard program.
/// Verifies balance increased by at least min_profit.
fn build_guard_profit_check_ix(
    program_id: &Pubkey,
    authority: &Pubkey,
    token_account: &Pubkey,
    min_profit: u64,
) -> Instruction {
    let disc = anchor_discriminator("profit_check");
    let guard_state = derive_guard_pda(program_id, authority);
    
    let mut data = disc.to_vec();
    data.extend_from_slice(&min_profit.to_le_bytes());
    
    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new_readonly(*authority, true),  // authority (signer)
            AccountMeta::new_readonly(guard_state, false), // guard_state PDA
            AccountMeta::new_readonly(*token_account, false), // token_account
        ],
        data,
    }
}

fn derive_guard_pda(program_id: &Pubkey, authority: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"guard", authority.as_ref()],
        program_id,
    ).0
}

fn anchor_discriminator(name: &str) -> [u8; 8] {
    use std::io::Write;
    let mut hasher = solana_sdk::hash::Hasher::default();
    hasher.hash(format!("global:{}", name).as_bytes());
    let hash = hasher.result();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&hash.as_ref()[..8]);
    disc
}
```

- [ ] **Step 3: Wrap build_arb_instructions with guard IXs**

In `build_arb_instructions()`, at the beginning (after the method opens), check if the guard is configured. If so, prepend start_check and append profit_check:

Before the compute budget instructions:
```rust
// Optional: arb-guard start_check (records pre-swap balance)
if let Some(ref guard_program) = self.config.arb_guard_program_id {
    let wsol_ata = derive_ata(&signer_pubkey, &wsol);
    instructions.push(build_guard_start_check_ix(guard_program, &signer_pubkey, &wsol_ata));
}
```

After the wSOL unwrap (CloseAccount), before returning:
```rust
// Optional: arb-guard profit_check (reverts if no profit)
if let Some(ref guard_program) = self.config.arb_guard_program_id {
    let wsol_ata = derive_ata(&signer_pubkey, &wsol);
    let min_profit = std::env::var("MIN_ON_CHAIN_PROFIT")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(0u64);
    instructions.push(build_guard_profit_check_ix(guard_program, &signer_pubkey, &wsol_ata, min_profit));
}
```

Note: The `BundleBuilder` needs access to `config`. Either pass `config.arb_guard_program_id` to the builder or make it available via the state cache. The simplest approach: pass it as an `Option<Pubkey>` field on `BundleBuilder`.

- [ ] **Step 4: Verify compilation + tests**

```bash
cargo check && cargo test --test unit
```

- [ ] **Step 5: Commit**

```bash
git add src/executor/bundle.rs src/config.rs
git commit -m "feat: integrate arb-guard IXs into bundle builder

Prepends start_check and appends profit_check when ARB_GUARD_PROGRAM_ID
is configured. Graceful degradation when not set.

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin main
```

---

### Task 4: Deploy to mainnet and verify

- [ ] **Step 1: Build release**

```bash
anchor build --program-name arb_guard
```

- [ ] **Step 2: Deploy to mainnet**

```bash
# Set provider to mainnet
solana config set --url mainnet-beta
anchor deploy --program-name arb_guard --provider.cluster mainnet
```

Note the deployed program ID. Update `Anchor.toml` and `lib.rs` with the real ID.

Cost: ~2 SOL for program rent.

- [ ] **Step 3: Add program ID to .env**

```bash
echo "ARB_GUARD_PROGRAM_ID=<deployed_address>" >> .env
```

- [ ] **Step 4: Run live test**

```bash
MIN_PROFIT_LAMPORTS=1000 SKIP_SIMULATOR=true timeout 30 cargo run --release --bin solana-mev-bot 2>&1 | grep "guard\|start_check\|profit_check\|accepted\|OPPORTUNITY" | head -10
```

Expected: Guard IXs added to bundles. Bundles still accepted by relays. First `start_check` call creates the guard PDA via `init_if_needed`.

- [ ] **Step 5: Commit final verification**

```bash
git add -A
git commit -m "deploy: arb-guard live on mainnet — profit protection active

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin main
```
