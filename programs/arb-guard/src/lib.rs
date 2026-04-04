use anchor_lang::prelude::*;
use anchor_lang::solana_program;
use anchor_spl::token::TokenAccount;

declare_id!("CbjPG5TEEhZGXsA8prmJPfvgH51rudYgcubRUtCCGyUw");

const SPL_TOKEN_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    6, 221, 246, 225, 215, 101, 161, 147, 217, 203, 225, 70, 206, 235, 121, 172,
    28, 180, 133, 237, 95, 91, 55, 145, 58, 140, 245, 133, 126, 255, 0, 169,
]);

const ORCA_WHIRLPOOL_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    14, 3, 104, 95, 142, 144, 144, 83, 228, 88, 18, 28, 102, 245, 167, 106,
    237, 199, 112, 106, 161, 28, 130, 248, 170, 149, 42, 143, 43, 120, 121, 169,
]);

const ORCA_SWAP_V2_DISCRIMINATOR: [u8; 8] = [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62];

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct ArbParams {
    pub amount_in: u64,
    pub min_amount_out: u64,
    pub hops: Vec<HopParams>,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct HopParams {
    pub dex_type: u8, // 0 = OrcaWhirlpool
    pub a_to_b: bool,
}

fn get_token_balance(account_info: &AccountInfo) -> Result<u64> {
    let data = account_info.try_borrow_data()?;
    if data.len() < 72 {
        return err!(ArbGuardError::InvalidTokenAccount);
    }
    Ok(u64::from_le_bytes(data[64..72].try_into().unwrap()))
}

#[program]
pub mod arb_guard {
    use super::*;

    /// Record the token account balance before swaps begin.
    /// Called as the FIRST instruction in an arb transaction.
    /// Sets the guard lock to prevent re-entry within the same TX.
    pub fn start_check(ctx: Context<StartCheck>) -> Result<()> {
        let guard = &mut ctx.accounts.guard_state;
        // Prevent re-entry: if already locked, someone is trying to reset the guard mid-TX
        require!(!guard.locked, ArbGuardError::GuardAlreadyActive);
        guard.start_balance = ctx.accounts.token_account.amount;
        guard.authority = ctx.accounts.authority.key();
        guard.token_account = ctx.accounts.token_account.key();
        guard.locked = true;
        Ok(())
    }

    /// Verify profit after swaps complete.
    /// Called as the LAST instruction (before tip) in an arb transaction.
    /// Reverts the entire TX if balance hasn't increased by min_profit.
    /// Unlocks the guard for the next transaction.
    pub fn profit_check(ctx: Context<ProfitCheck>, min_profit: u64) -> Result<()> {
        let guard = &mut ctx.accounts.guard_state;
        require!(guard.locked, ArbGuardError::GuardNotActive);

        ctx.accounts.token_account.reload()?;
        let end_balance = ctx.accounts.token_account.amount;
        let threshold = guard.start_balance
            .checked_add(min_profit)
            .ok_or(ArbGuardError::OverflowError)?;

        require!(
            end_balance >= threshold,
            ArbGuardError::NoProfitDetected
        );

        // Unlock for next transaction
        guard.locked = false;
        Ok(())
    }

    /// Execute a multi-hop arbitrage via CPI into DEX programs.
    /// All accounts are passed via remaining_accounts to keep the interface flexible.
    /// Layout: [signer, token_program, memo_program, input_token_account, input_mint, orca_program,
    ///          ...per-hop accounts (9 each): whirlpool, vault_a, vault_b, tick0, tick1, tick2, oracle, output_ata, output_mint]
    pub fn execute_arb<'info>(
        ctx: Context<'info, ExecuteArb>,
        params: ArbParams,
    ) -> Result<()> {
        let remaining = &ctx.remaining_accounts;
        let mut idx: usize = 0;

        // Parse fixed accounts (6)
        let signer = &remaining[idx]; idx += 1;
        require!(signer.is_signer, ArbGuardError::SignerRequired);

        let token_program = &remaining[idx]; idx += 1;
        require!(
            *token_program.key == SPL_TOKEN_PROGRAM_ID,
            ArbGuardError::InvalidTokenProgram
        );

        let memo_program = &remaining[idx]; idx += 1;
        let input_token_account = &remaining[idx]; idx += 1;
        let input_mint = &remaining[idx]; idx += 1;

        let orca_program = &remaining[idx]; idx += 1;
        require!(
            *orca_program.key == ORCA_WHIRLPOOL_PROGRAM_ID,
            ArbGuardError::InvalidDexProgram
        );

        // Execute hops
        let mut interim_amount_in = params.amount_in;
        let mut current_input_account = input_token_account;
        let mut current_input_mint = input_mint;

        for (_hop_idx, hop) in params.hops.iter().enumerate() {
            require!(hop.dex_type == 0, ArbGuardError::UnsupportedDex);

            // Parse per-hop accounts (9)
            let whirlpool = &remaining[idx]; idx += 1;
            let token_vault_a = &remaining[idx]; idx += 1;
            let token_vault_b = &remaining[idx]; idx += 1;
            let tick_array_0 = &remaining[idx]; idx += 1;
            let tick_array_1 = &remaining[idx]; idx += 1;
            let tick_array_2 = &remaining[idx]; idx += 1;
            let oracle = &remaining[idx]; idx += 1;
            let output_token_account = &remaining[idx]; idx += 1;
            let output_mint = &remaining[idx]; idx += 1;

            // Pre-swap balance
            let pre_balance = get_token_balance(output_token_account)?;

            // sqrt_price_limit based on direction
            let sqrt_price_limit: u128 = if hop.a_to_b {
                4295048016u128
            } else {
                79226673515401279992447579055u128
            };

            // Build swap_v2 instruction data
            let mut ix_data = Vec::with_capacity(43);
            ix_data.extend_from_slice(&ORCA_SWAP_V2_DISCRIMINATOR);
            ix_data.extend_from_slice(&interim_amount_in.to_le_bytes());
            ix_data.extend_from_slice(&0u64.to_le_bytes()); // other_amount_threshold=0
            ix_data.extend_from_slice(&sqrt_price_limit.to_le_bytes());
            ix_data.push(1u8); // amount_specified_is_input = true
            ix_data.push(if hop.a_to_b { 1u8 } else { 0u8 });
            ix_data.push(0u8); // remaining_accounts_info = None

            // Map input/output to a/b based on direction
            let (mint_a, mint_b, owner_a, owner_b) = if hop.a_to_b {
                (current_input_mint, output_mint, current_input_account, output_token_account)
            } else {
                (output_mint, current_input_mint, output_token_account, current_input_account)
            };

            // Build 15-account CPI
            let account_infos = vec![
                token_program.clone(),
                token_program.clone(),
                memo_program.clone(),
                signer.clone(),
                whirlpool.clone(),
                mint_a.clone(),
                mint_b.clone(),
                owner_a.clone(),
                token_vault_a.clone(),
                owner_b.clone(),
                token_vault_b.clone(),
                tick_array_0.clone(),
                tick_array_1.clone(),
                tick_array_2.clone(),
                oracle.clone(),
            ];

            let account_metas: Vec<solana_program::instruction::AccountMeta> = account_infos
                .iter()
                .map(|a| {
                    if a.is_writable {
                        solana_program::instruction::AccountMeta::new(*a.key, a.is_signer)
                    } else {
                        solana_program::instruction::AccountMeta::new_readonly(*a.key, a.is_signer)
                    }
                })
                .collect();

            let ix = solana_program::instruction::Instruction {
                program_id: *orca_program.key,
                accounts: account_metas,
                data: ix_data,
            };

            solana_program::program::invoke(&ix, &account_infos)?;

            // Post-swap balance diff
            let post_balance = get_token_balance(output_token_account)?;
            require!(post_balance > pre_balance, ArbGuardError::SwapOutputZero);
            interim_amount_in = post_balance - pre_balance;

            // Chain: this hop's output becomes next hop's input
            current_input_account = output_token_account;
            current_input_mint = output_mint;
        }

        // Final output check
        require!(
            interim_amount_in >= params.min_amount_out,
            ArbGuardError::InsufficientOutput
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
        space = 8 + 8 + 32 + 32 + 1, // disc + u64 + Pubkey + Pubkey + bool = 81
        seeds = [b"guard", authority.key().as_ref()],
        bump,
    )]
    pub guard_state: Account<'info, GuardState>,
    /// Token account must be owned by the authority (prevents passing someone else's account)
    #[account(
        constraint = token_account.owner == authority.key()
            @ ArbGuardError::TokenAccountOwnerMismatch
    )]
    pub token_account: Account<'info, TokenAccount>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ProfitCheck<'info> {
    pub authority: Signer<'info>,
    #[account(
        mut, // mutable to update locked flag
        seeds = [b"guard", authority.key().as_ref()],
        bump,
        has_one = authority,
    )]
    pub guard_state: Account<'info, GuardState>,
    /// Must be the same token account used in start_check (pinned via guard_state.token_account)
    #[account(
        constraint = token_account.key() == guard_state.token_account
            @ ArbGuardError::TokenAccountMismatch,
        constraint = token_account.owner == authority.key()
            @ ArbGuardError::TokenAccountOwnerMismatch
    )]
    pub token_account: Account<'info, TokenAccount>,
}

#[derive(Accounts)]
pub struct ExecuteArb {}

#[account]
pub struct GuardState {
    pub start_balance: u64,     // 8 bytes
    pub authority: Pubkey,      // 32 bytes
    pub token_account: Pubkey,  // 32 bytes — pinned between start_check and profit_check
    pub locked: bool,           // 1 byte — prevents re-entry
}

#[error_code]
pub enum ArbGuardError {
    #[msg("No profit detected — reverting to protect funds")]
    NoProfitDetected,
    #[msg("Guard is already active — cannot call start_check twice")]
    GuardAlreadyActive,
    #[msg("Guard is not active — call start_check first")]
    GuardNotActive,
    #[msg("Token account owner does not match authority")]
    TokenAccountOwnerMismatch,
    #[msg("Token account does not match the one used in start_check")]
    TokenAccountMismatch,
    #[msg("Overflow in profit calculation")]
    OverflowError,
    #[msg("Signer required as first account")]
    SignerRequired,
    #[msg("Invalid SPL Token program")]
    InvalidTokenProgram,
    #[msg("Invalid DEX program ID")]
    InvalidDexProgram,
    #[msg("Unsupported DEX type")]
    UnsupportedDex,
    #[msg("Swap produced zero output")]
    SwapOutputZero,
    #[msg("Final output below minimum")]
    InsufficientOutput,
    #[msg("Invalid token account data")]
    InvalidTokenAccount,
}
