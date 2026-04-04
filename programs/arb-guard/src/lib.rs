use anchor_lang::prelude::*;
use anchor_spl::token::TokenAccount;

declare_id!("CbjPG5TEEhZGXsA8prmJPfvgH51rudYgcubRUtCCGyUw");

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
