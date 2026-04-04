use anchor_lang::prelude::*;
use anchor_spl::token::TokenAccount;

declare_id!("CbjPG5TEEhZGXsA8prmJPfvgH51rudYgcubRUtCCGyUw");

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
}
