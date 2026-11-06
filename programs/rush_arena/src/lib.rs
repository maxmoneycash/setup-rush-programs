//! Asset-denominated escrow for server-executed PvP rounds. P&L is a score, never money creation.
//! The verifier is trusted for scores; custody limits, deadlines and refunds are enforced here.
#![allow(unexpected_cfgs)]
use anchor_lang::prelude::*;
use anchor_lang::system_program::{transfer, Transfer};
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

pub mod state;
pub mod custody;
pub mod rounds;
use custody::*;
use rounds::*;
use state::*;

declare_id!("5ZkKsZVj1ZxtiMnydGn82ojs4rSZ3ARpWLfP7w9AFYo9");

#[program]
pub mod rush_arena {
    use super::*;
    pub fn initialize(ctx: Context<Initialize>, verifier: Pubkey, treasury: Pubkey) -> Result<()> {
        require!(verifier != Pubkey::default() && treasury != Pubkey::default(), ArenaError::InvalidAuthority);
        let c = &mut ctx.accounts.config;
        c.version = 1; c.authority = ctx.accounts.authority.key(); c.verifier = verifier;
        c.treasury = treasury; c.paused = false; c.next_room = 0; c.bump = ctx.bumps.config;
        Ok(())
    }
    pub fn configure(ctx: Context<Configure>, verifier: Pubkey, treasury: Pubkey, paused: bool) -> Result<()> {
        require!(verifier != Pubkey::default() && treasury != Pubkey::default(), ArenaError::InvalidAuthority);
        let c = &mut ctx.accounts.config;
        c.verifier = verifier; c.treasury = treasury; c.paused = paused;
        Ok(())
    }
    /// A zero pending address cancels an outstanding proposal; it never removes the admin.
    pub fn propose_authority(ctx: Context<Configure>, successor: Pubkey) -> Result<()> {
        ctx.accounts.config.pending_authority = successor; Ok(())
    }
    pub fn accept_authority(ctx: Context<AcceptAuthority>) -> Result<()> {
        ctx.accounts.config.authority = ctx.accounts.successor.key();
        ctx.accounts.config.pending_authority = Pubkey::default(); Ok(())
    }
    pub fn register_sol(ctx: Context<RegisterSol>) -> Result<()> { custody::register_sol(ctx) }
    pub fn register_token(ctx: Context<RegisterToken>) -> Result<()> { custody::register_token(ctx) }
    pub fn enable_asset(ctx: Context<EnableAsset>, enabled: bool) -> Result<()> { ctx.accounts.asset.enabled = enabled; Ok(()) }
    pub fn deposit_sol(ctx: Context<DepositSol>, amount: u64) -> Result<()> { custody::deposit_sol(ctx, amount) }
    pub fn withdraw_sol(ctx: Context<WithdrawSol>, amount: u64) -> Result<()> { custody::withdraw_sol(ctx, amount) }
    pub fn deposit_token(ctx: Context<DepositToken>, amount: u64) -> Result<()> { custody::deposit_token(ctx, amount) }
    pub fn withdraw_token(ctx: Context<WithdrawToken>, amount: u64) -> Result<()> { custody::withdraw_token(ctx, amount) }
    pub fn grant_session(ctx: Context<GrantSession>, expected_epoch: u64, limits: SessionLimits) -> Result<()> { custody::grant_session(ctx, expected_epoch, limits) }
    pub fn revoke_session(ctx: Context<RevokeSession>) -> Result<()> {
        ctx.accounts.balance.epoch = add(ctx.accounts.balance.epoch, 1)?; Ok(())
    }
    pub fn create_room(ctx: Context<CreateRoom>, id: u64, terms: RoomTerms) -> Result<()> { rounds::create_room(ctx, id, terms) }
    pub fn join_room(ctx: Context<JoinRoom>, expected: JoinTerms) -> Result<()> { rounds::join_room(ctx, expected) }
    pub fn post_result(ctx: Context<PostResult>, score: i64, log_hash: [u8;32]) -> Result<()> { rounds::post_result(ctx, score, log_hash) }
    pub fn finalize(ctx: Context<ChangeRoom>) -> Result<()> { rounds::finalize(ctx) }
    pub fn cancel(ctx: Context<ChangeRoom>) -> Result<()> { rounds::cancel(ctx) }
    pub fn claim_entry(ctx: Context<ClaimEntry>) -> Result<()> { rounds::claim_entry(ctx) }
    pub fn claim_fee(ctx: Context<ClaimFee>) -> Result<()> { rounds::claim_fee(ctx) }
    pub fn close_entry(ctx: Context<CloseEntry>) -> Result<()> {
        ctx.accounts.room.closed_entries = increment(ctx.accounts.room.closed_entries)?; Ok(())
    }
    pub fn close_room(_ctx: Context<CloseRoom>) -> Result<()> { Ok(()) }
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(init, payer=authority, space=8+Config::INIT_SPACE, seeds=[b"arena"], bump)]
    pub config: Account<'info, Config>,
    #[account(constraint=program.programdata_address()? == Some(program_data.key()) @ ArenaError::InvalidAuthority)]
    pub program: Program<'info, crate::program::RushArena>,
    #[account(constraint=program_data.upgrade_authority_address == Some(authority.key()) @ ArenaError::InvalidAuthority)]
    pub program_data: Account<'info, ProgramData>,
    #[account(mut)] pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}
#[derive(Accounts)]
pub struct Configure<'info> {
    #[account(mut, seeds=[b"arena"], bump=config.bump, has_one=authority)]
    pub config: Account<'info, Config>,
    pub authority: Signer<'info>,
}
#[derive(Accounts)]
pub struct AcceptAuthority<'info> {
    #[account(mut, seeds=[b"arena"], bump=config.bump, constraint=config.pending_authority == successor.key() @ ArenaError::InvalidAuthority)]
    pub config: Account<'info, Config>,
    pub successor: Signer<'info>,
}
