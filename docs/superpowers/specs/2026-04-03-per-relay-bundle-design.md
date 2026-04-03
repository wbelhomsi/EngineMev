# Per-Relay Bundle Architecture

**Date:** 2026-04-03
**Status:** Approved

## Problem

Currently `bundle.rs` builds ONE transaction containing tips for ALL relays (Jito + Astralane), then sends the same tx to every relay. This causes:
1. **Transaction too large** — 20+ Sanctum accounts + ATA creates + 2 tip IXs exceed the 1232-byte limit
2. **Wrong tip accounting** — simulator sums all relay tips, rejecting profitable routes
3. **Wasted tip SOL** — Jito doesn't need an Astralane tip in its bundle (and vice versa)

## Design

### File Structure

```
src/executor/
├── mod.rs              # Re-exports BundleBuilder, RelayDispatcher
├── bundle.rs           # Builds base instructions (NO tips, NO signing)
├── relay_dispatcher.rs # Spawns all configured relays concurrently
└── relays/
    ├── mod.rs          # Relay trait + RelayResult + shared types
    ├── jito.rs         # Jito relay (own rate limit, own tip accounts, own submission)
    ├── astralane.rs    # Astralane relay (own rate limit, own tip accounts, keepalive)
    ├── nozomi.rs       # Nozomi relay
    ├── bloxroute.rs    # bloXroute relay
    └── zeroslot.rs     # ZeroSlot relay
```

### Data Flow

```
Opportunity detected in main.rs router loop
  → BundleBuilder::build_arb_instructions(route) → Vec<Instruction> + min_final_output
  → RelayDispatcher::dispatch(instructions, tip_lamports, signer, blockhash)
      → For each configured relay (concurrent, independent):
          tokio::spawn(relay.submit(instructions, tip_lamports, signer, blockhash))
            1. Check own rate limit → skip if too soon
            2. Clone base instructions
            3. Append tip IX to own tip account (rotated)
            4. Build Transaction, sign with signer
            5. Serialize to bytes, base64 encode
            6. HTTP POST to relay endpoint
            7. Log result (accepted/rejected/error)
```

### Relay Trait

```rust
#[async_trait::async_trait]
pub trait Relay: Send + Sync {
    /// Human-readable relay name for logging
    fn name(&self) -> &str;

    /// Whether this relay is configured (has endpoint URL)
    fn is_configured(&self) -> bool;

    /// Submit a bundle. Each relay independently:
    /// - Checks its own rate limit
    /// - Appends its own tip instruction
    /// - Signs and serializes the transaction
    /// - Sends via HTTP
    /// No relay waits for any other relay.
    async fn submit(
        &self,
        base_instructions: &[Instruction],
        tip_lamports: u64,
        signer: &Keypair,
        recent_blockhash: Hash,
    ) -> RelayResult;
}
```

### bundle.rs Changes

`build_arb_bundle` is replaced by `build_arb_instructions`:

```rust
pub fn build_arb_instructions(
    &self,
    route: &ArbRoute,
    min_final_output: u64,
) -> Result<Vec<Instruction>>
```

Returns: compute budget IXs + ATA create IXs + swap IXs. **No tips, no signing, no serialization.**

The caller (main.rs) computes `min_final_output` from the simulator result and passes it in.

### Tip Handling

- **Same tip amount** passed to all relays (`tip_lamports` from simulator)
- Each relay tips to **its own tip accounts**:
  - Jito: 8 rotating accounts (hardcoded)
  - Astralane: 17 rotating accounts (hardcoded)
  - Nozomi/bloXroute/ZeroSlot: each has its own tip accounts (or uses Jito-compatible accounts — verify per relay)
- Tip rotation index: each relay struct owns its own `AtomicUsize` counter
- Tip IX: `system_instruction::transfer(signer, tip_account, tip_lamports)`

### Simulator Tip Accounting

Simulator uses **one tip amount** (not sum of all relays), since each relay independently builds its own tx with one tip:

```rust
tip_lamports = (gross_profit * tip_fraction) as u64;
final_profit = gross_profit - tip_lamports;
// No relay_extra_tips — each tx only has one tip
```

Remove `relay_extra_tips()`, `astralane_tip_lamports()`, `with_relay_extra_tips()` from simulator.

### Per-Relay Details

**Jito** (`jito.rs`):
- Endpoint: `JITO_RELAY_URL` env var
- Auth: `x-jito-auth` header from `JITO_AUTH_UUID` env var (optional)
- Format: JSON-RPC `sendBundle` with `[base64_txs, {"encoding": "base64"}]`
- Rate limit: configurable via `JITO_TPS` (default 5.0 with UUID)
- Tip accounts: 8 hardcoded, rotated per submission

**Astralane** (`astralane.rs`):
- Endpoint: `ASTRALANE_RELAY_URL` env var
- Auth: query param `?api-key=` + header `api_key` from `ASTRALANE_API_KEY`
- Format: JSON-RPC `sendBundle` with `revertProtection: true`
- Rate limit: configurable via `ASTRALANE_TPS` (default 40.0)
- Tip accounts: 17 hardcoded, rotated per submission
- Keepalive: `getHealth` every 30s (spawned at construction)

**Nozomi** (`nozomi.rs`):
- Endpoint: `NOZOMI_RELAY_URL` env var
- Auth: none
- Format: Jito-compatible JSON-RPC `sendBundle`
- Rate limit: configurable via `NOZOMI_TPS` (default 5.0)
- Tip accounts: Jito-compatible (same 8 accounts)

**bloXroute** (`bloxroute.rs`):
- Endpoint: `BLOXROUTE_RELAY_URL` env var
- Auth: `Authorization` header from `BLOXROUTE_AUTH_HEADER`
- Format: REST POST `{"transaction": [base64_txs], "useBundle": true}`
- Rate limit: configurable via `BLOXROUTE_TPS` (default 5.0)
- Tip accounts: Jito-compatible (same 8 accounts)

**ZeroSlot** (`zeroslot.rs`):
- Endpoint: `ZEROSLOT_RELAY_URL` env var
- Auth: none
- Format: Jito-compatible JSON-RPC `sendBundle`
- Rate limit: configurable via `ZEROSLOT_TPS` (default 5.0)
- Tip accounts: Jito-compatible (same 8 accounts)

### RelayDispatcher

```rust
pub struct RelayDispatcher {
    relays: Vec<Arc<dyn Relay>>,
    signer: Arc<Keypair>,
}

impl RelayDispatcher {
    /// Fire all configured relays concurrently. No relay waits for another.
    pub fn dispatch(
        &self,
        base_instructions: &[Instruction],
        tip_lamports: u64,
        recent_blockhash: Hash,
        rt: &tokio::runtime::Handle,
    ) {
        for relay in &self.relays {
            if !relay.is_configured() { continue; }
            let relay = relay.clone();
            let ixs = base_instructions.to_vec();
            let signer = self.signer.clone();
            let tip = tip_lamports;
            let bh = recent_blockhash;
            rt.spawn(async move {
                let result = relay.submit(&ixs, tip, &signer, bh).await;
                // Log result
            });
        }
    }
}
```

### What Gets Deleted

- `src/executor/relay.rs` (669 lines) — entirely replaced by `relays/` directory + `relay_dispatcher.rs`
- Tip instructions from `bundle.rs` `build_arb_transaction_with_tip` — tips move to per-relay
- `relay_extra_tips()` / `astralane_tip_lamports()` / `DEFAULT_ASTRALANE_TIP_LAMPORTS` from bundle.rs
- `ProfitSimulator::with_relay_extra_tips()` and `relay_extra_tips` field from simulator.rs
- `MultiRelay` struct

### main.rs Changes

```rust
// Before (broken):
match bundle_builder.build_arb_bundle(&route, total_tip_lamports, blockhash) {
    Ok(bundle_txs) => {
        rt.spawn(async move {
            relay.submit_bundle(&bundle_txs, tip).await;
        });
    }
}

// After:
match bundle_builder.build_arb_instructions(&route, min_final_output) {
    Ok(instructions) => {
        relay_dispatcher.dispatch(&instructions, tip_lamports, blockhash, &rt);
        bundles_submitted += 1;
    }
}
```

### Testing

- Unit test per relay: verify tip account rotation, rate limiting, request format
- Unit test bundle.rs: verify instructions contain no tip IXs
- Integration test: verify dispatcher fires all relays concurrently
- Live test: `SIMULATE_BUNDLES=true` should show SIM SUCCESS (unchanged)
