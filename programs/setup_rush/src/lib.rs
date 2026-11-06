//! Setup Rush on-chain Daily ("Clock In"), v3.
//!
//! Everyone plays the same real chart each day. A player `enter`s today's pool with a stake (SOL,
//! or a registered token such as test USDC or CASH through `enter_token`), plays one shot, and the
//! run is replay-verified off-chain (`verifyRun` in the game engine). The verifier co-signs
//! `post_score`, so only replay-verified P&L lands on chain. After the day ends the best score
//! `claim`s the pot. A day nobody posted a clean shot in refunds every entrant instead (`refund`, `refund_token`).
//!
//! Pools are keyed by day and asset mint (the default pubkey means SOL), entries by day, mint and
//! player, and a `Player` account keeps the clock-in streak and lifetime record. Token pools take any
//! mint the authority registered with `set_token` (`TokenConfig`, keyed by mint): classic SPL tokens
//! and Token-2022 both, through the token interface. `Config.token_mint` and `token_stake` remain for
//! the account layout; v3 no longer reads them.
#![allow(unexpected_cfgs)]
use anchor_lang::prelude::*;
use anchor_lang::system_program::{transfer, Transfer};
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

declare_id!("2jCQ5PCnNCPP4Kurf4Zz31YAzPzCk3zSfsDWd4eMZRgV");

pub const SECONDS_PER_DAY: i64 = 86_400;
/// A delegation lasts at most a week and covers at most this many entries.
pub const MAX_DELEGATION_SECONDS: i64 = 7 * 86_400;
pub const MAX_DELEGATED_ENTRIES: u16 = 50;
/// Runs that started before midnight UTC can still be posted for this long after it.
pub const POST_GRACE: i64 = 15 * 60;

#[program]
pub mod setup_rush {
    use super::*;

    pub fn init_config(ctx: Context<InitConfig>, verifier: Pubkey, stake_lamports: u64, token_mint: Pubkey, token_stake: u64) -> Result<()> {
        let c = &mut ctx.accounts.config;
        c.authority = ctx.accounts.authority.key();
        c.verifier = verifier;
        c.stake_lamports = stake_lamports;
        c.token_mint = token_mint;
        c.token_stake = token_stake;
        c.bump = ctx.bumps.config;
        Ok(())
    }

    pub fn set_config(ctx: Context<SetConfig>, verifier: Pubkey, stake_lamports: u64, token_mint: Pubkey, token_stake: u64) -> Result<()> {
        let c = &mut ctx.accounts.config;
        c.verifier = verifier;
        c.stake_lamports = stake_lamports;
        c.token_mint = token_mint;
        c.token_stake = token_stake;
        Ok(())
    }

    /// Register a token pool (classic SPL or Token-2022 mint) with its entry stake, or change it.
    /// A disabled token keeps its past pools claimable but takes no new entries.
    pub fn set_token(ctx: Context<SetToken>, stake: u64, enabled: bool) -> Result<()> {
        let t = &mut ctx.accounts.token_config;
        t.mint = ctx.accounts.mint.key();
        t.stake = stake;
        t.enabled = enabled;
        t.bump = ctx.bumps.token_config;
        emit!(TokenSet { mint: t.mint, stake, enabled });
        Ok(())
    }

    /// Stake SOL into today's pool. One entry per wallet per day per asset.
    pub fn enter(ctx: Context<Enter>, day: u32) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(day == (now / SECONDS_PER_DAY) as u32, RushError::WrongDay);
        let stake = ctx.accounts.config.stake_lamports;
        {
            let pool = &mut ctx.accounts.pool;
            if pool.day == 0 {
                pool.day = day;
                pool.mint = Pubkey::default();
                pool.bump = ctx.bumps.pool;
            }
            require!(!pool.claimed, RushError::PoolClosed);
        }
        if stake > 0 {
            transfer(
                CpiContext::new(
                    ctx.accounts.system_program.key(),
                    Transfer { from: ctx.accounts.player.to_account_info(), to: ctx.accounts.pool.to_account_info() },
                ),
                stake,
            )?;
        }
        let pool = &mut ctx.accounts.pool;
        pool.entries += 1;
        pool.pot += stake;
        fill_entry(&mut ctx.accounts.entry, ctx.accounts.player.key(), day, Pubkey::default(), now, ctx.bumps.entry);
        bump_player(&mut ctx.accounts.player_stats, ctx.accounts.player.key(), day, ctx.bumps.player_stats);
        emit!(Entered { day, mint: Pubkey::default(), player: ctx.accounts.player.key(), stake, entries: pool.entries, streak: ctx.accounts.player_stats.streak });
        Ok(())
    }

    /// Stake a registered token (test USDC or CASH) into today's pool for that mint.
    pub fn enter_token(ctx: Context<EnterToken>, day: u32) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(day == (now / SECONDS_PER_DAY) as u32, RushError::WrongDay);
        let mint = ctx.accounts.mint.key();
        require!(ctx.accounts.token_config.enabled, RushError::TokenOff);
        let stake = ctx.accounts.token_config.stake;
        {
            let pool = &mut ctx.accounts.pool;
            if pool.day == 0 {
                pool.day = day;
                pool.mint = mint;
                pool.bump = ctx.bumps.pool;
            }
            require!(!pool.claimed, RushError::PoolClosed);
        }
        if stake > 0 {
            token_interface::transfer_checked(
                CpiContext::new(
                    ctx.accounts.token_program.key(),
                    TransferChecked {
                        from: ctx.accounts.player_token.to_account_info(),
                        mint: ctx.accounts.mint.to_account_info(),
                        to: ctx.accounts.vault.to_account_info(),
                        authority: ctx.accounts.player.to_account_info(),
                    },
                ),
                stake,
                ctx.accounts.mint.decimals,
            )?;
        }
        let pool = &mut ctx.accounts.pool;
        pool.entries += 1;
        pool.pot += stake;
        fill_entry(&mut ctx.accounts.entry, ctx.accounts.player.key(), day, mint, now, ctx.bumps.entry);
        bump_player(&mut ctx.accounts.player_stats, ctx.accounts.player.key(), day, ctx.bumps.player_stats);
        emit!(Entered { day, mint, player: ctx.accounts.player.key(), stake, entries: pool.entries, streak: ctx.accounts.player_stats.streak });
        Ok(())
    }

    /// Post the one shot. `score` is the replay-verified P&L in cents; the verifier must co-sign.
    pub fn post_score(ctx: Context<PostScore>, day: u32, mint: Pubkey, score: i64, log_hash: [u8; 32]) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(now < (day as i64 + 1) * SECONDS_PER_DAY + POST_GRACE, RushError::DayOver);
        let e = &mut ctx.accounts.entry;
        require!(!e.posted, RushError::AlreadyPosted);
        require_keys_eq!(e.mint, mint, RushError::WrongMint);
        e.posted = true;
        e.score = score;
        e.log_hash = log_hash;
        e.posted_at = now;
        let p = &mut ctx.accounts.pool;
        require!(!p.claimed, RushError::PoolClosed);
        if p.posted == 0 || score > p.best_score {
            p.best_score = score;
            p.best_player = e.player;
        }
        p.posted += 1;
        let st = &mut ctx.accounts.player_stats;
        st.posts += 1;
        if st.posts == 1 || score > st.best_score {
            st.best_score = score;
        }
        emit!(ScorePosted { day, mint, player: e.player, score, best_score: p.best_score, best_player: p.best_player });
        Ok(())
    }

    /// After the day (plus grace), the best score takes the SOL pot.
    pub fn claim(ctx: Context<Claim>, day: u32) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(now >= (day as i64 + 1) * SECONDS_PER_DAY + POST_GRACE, RushError::DayNotOver);
        let amount = {
            let p = &mut ctx.accounts.pool;
            require!(!p.claimed, RushError::PoolClosed);
            require!(p.posted > 0, RushError::NoScores);
            require_keys_eq!(p.best_player, ctx.accounts.player.key(), RushError::NotWinner);
            require_keys_eq!(p.mint, Pubkey::default(), RushError::WrongMint);
            p.claimed = true;
            let a = p.pot;
            p.pot = 0;
            a
        };
        let pool_info = ctx.accounts.pool.to_account_info();
        let player_info = ctx.accounts.player.to_account_info();
        **pool_info.try_borrow_mut_lamports()? -= amount;
        **player_info.try_borrow_mut_lamports()? += amount;
        ctx.accounts.player_stats.wins += 1;
        emit!(Claimed { day, mint: Pubkey::default(), player: ctx.accounts.player.key(), amount });
        Ok(())
    }

    /// After the day (plus grace), the best score takes the token pot (classic SPL or Token-2022).
    pub fn claim_token(ctx: Context<ClaimToken>, day: u32) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(now >= (day as i64 + 1) * SECONDS_PER_DAY + POST_GRACE, RushError::DayNotOver);
        let mint = ctx.accounts.mint.key();
        let amount = {
            let p = &mut ctx.accounts.pool;
            require!(!p.claimed, RushError::PoolClosed);
            require!(p.posted > 0, RushError::NoScores);
            require_keys_eq!(p.best_player, ctx.accounts.player.key(), RushError::NotWinner);
            require_keys_eq!(p.mint, mint, RushError::WrongMint);
            p.claimed = true;
            let a = p.pot;
            p.pot = 0;
            a
        };
        let day_bytes = day.to_le_bytes();
        let mint_bytes = mint.to_bytes();
        let seeds: &[&[u8]] = &[b"pool", &day_bytes[..], &mint_bytes[..], &[ctx.accounts.pool.bump]];
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.vault.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.player_token.to_account_info(),
                    authority: ctx.accounts.pool.to_account_info(),
                },
                &[seeds],
            ),
            amount,
            ctx.accounts.mint.decimals,
        )?;
        ctx.accounts.player_stats.wins += 1;
        emit!(Claimed { day, mint, player: ctx.accounts.player.key(), amount });
        Ok(())
    }

    /// One wallet approval for a run of Daily entries: a browser key may enter today's pools, post the
    /// verified score and claim for this owner, `max_entries` times until `expires_at`. SOL entries and
    /// the rent the browser key pays come out of the lamports escrowed here; token entries move the
    /// owner's tokens through the SPL delegate approval the owner gives this account in the same
    /// transaction. Delegating again replaces the key and limits. Never required: the owner-signed
    /// instructions keep working.
    pub fn delegate(ctx: Context<Delegate>, signer: Pubkey, max_entries: u16, expires_at: i64, escrow_lamports: u64) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(signer != Pubkey::default() && signer != ctx.accounts.owner.key(), RushError::BadDelegation);
        require!(max_entries >= 1 && max_entries <= MAX_DELEGATED_ENTRIES, RushError::BadDelegation);
        require!(expires_at > now && expires_at <= now + MAX_DELEGATION_SECONDS, RushError::BadDelegation);
        {
            let d = &mut ctx.accounts.delegation;
            d.owner = ctx.accounts.owner.key();
            d.signer = signer;
            d.max_entries = max_entries;
            d.used = 0;
            d.expires_at = expires_at;
            d.bump = ctx.bumps.delegation;
        }
        if escrow_lamports > 0 {
            transfer(
                CpiContext::new(
                    ctx.accounts.system_program.key(),
                    Transfer { from: ctx.accounts.owner.to_account_info(), to: ctx.accounts.delegation.to_account_info() },
                ),
                escrow_lamports,
            )?;
        }
        emit!(Delegated { owner: ctx.accounts.owner.key(), signer, max_entries, expires_at });
        Ok(())
    }

    /// The owner takes the delegation back; the escrow and the account's rent return to the owner.
    pub fn revoke_delegation(_ctx: Context<RevokeDelegation>) -> Result<()> {
        Ok(())
    }

    /// A delegated SOL entry: the browser key signs and pays rent, the stake comes from the escrow.
    pub fn enter_delegated(ctx: Context<EnterDelegated>, day: u32) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(day == (now / SECONDS_PER_DAY) as u32, RushError::WrongDay);
        use_delegation(&mut ctx.accounts.delegation, now)?;
        let owner = ctx.accounts.delegation.owner;
        let stake = ctx.accounts.config.stake_lamports;
        {
            let pool = &mut ctx.accounts.pool;
            if pool.day == 0 {
                pool.day = day;
                pool.mint = Pubkey::default();
                pool.bump = ctx.bumps.pool;
            }
            require!(!pool.claimed, RushError::PoolClosed);
        }
        if stake > 0 {
            let from = ctx.accounts.delegation.to_account_info();
            let to = ctx.accounts.pool.to_account_info();
            let keep = Rent::get()?.minimum_balance(from.data_len());
            require!(from.lamports() >= keep + stake, RushError::EscrowEmpty);
            **from.try_borrow_mut_lamports()? -= stake;
            **to.try_borrow_mut_lamports()? += stake;
        }
        let pool = &mut ctx.accounts.pool;
        pool.entries += 1;
        pool.pot += stake;
        fill_entry(&mut ctx.accounts.entry, owner, day, Pubkey::default(), now, ctx.bumps.entry);
        bump_player(&mut ctx.accounts.player_stats, owner, day, ctx.bumps.player_stats);
        emit!(Entered { day, mint: Pubkey::default(), player: owner, stake, entries: pool.entries, streak: ctx.accounts.player_stats.streak });
        Ok(())
    }

    /// A delegated token entry: the browser key signs and pays rent; the stake leaves the owner's token
    /// account through the delegate approval held by the delegation PDA.
    pub fn enter_token_delegated(ctx: Context<EnterTokenDelegated>, day: u32) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(day == (now / SECONDS_PER_DAY) as u32, RushError::WrongDay);
        use_delegation(&mut ctx.accounts.delegation, now)?;
        let owner = ctx.accounts.delegation.owner;
        let mint = ctx.accounts.mint.key();
        require!(ctx.accounts.token_config.enabled, RushError::TokenOff);
        let stake = ctx.accounts.token_config.stake;
        {
            let pool = &mut ctx.accounts.pool;
            if pool.day == 0 {
                pool.day = day;
                pool.mint = mint;
                pool.bump = ctx.bumps.pool;
            }
            require!(!pool.claimed, RushError::PoolClosed);
        }
        if stake > 0 {
            let bump = ctx.accounts.delegation.bump;
            let seeds: &[&[u8]] = &[b"delegate", owner.as_ref(), &[bump]];
            token_interface::transfer_checked(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.key(),
                    TransferChecked {
                        from: ctx.accounts.owner_token.to_account_info(),
                        mint: ctx.accounts.mint.to_account_info(),
                        to: ctx.accounts.vault.to_account_info(),
                        authority: ctx.accounts.delegation.to_account_info(),
                    },
                    &[seeds],
                ),
                stake,
                ctx.accounts.mint.decimals,
            )?;
        }
        let pool = &mut ctx.accounts.pool;
        pool.entries += 1;
        pool.pot += stake;
        fill_entry(&mut ctx.accounts.entry, owner, day, mint, now, ctx.bumps.entry);
        bump_player(&mut ctx.accounts.player_stats, owner, day, ctx.bumps.player_stats);
        emit!(Entered { day, mint, player: owner, stake, entries: pool.entries, streak: ctx.accounts.player_stats.streak });
        Ok(())
    }

    /// post_score signed by the delegated browser key instead of the owner; the verifier still co-signs.
    /// An expired delegation may still post and claim: expiry only stops new entries.
    pub fn post_score_delegated(ctx: Context<PostScoreDelegated>, day: u32, mint: Pubkey, score: i64, log_hash: [u8; 32]) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(now < (day as i64 + 1) * SECONDS_PER_DAY + POST_GRACE, RushError::DayOver);
        let e = &mut ctx.accounts.entry;
        require!(!e.posted, RushError::AlreadyPosted);
        require_keys_eq!(e.mint, mint, RushError::WrongMint);
        e.posted = true;
        e.score = score;
        e.log_hash = log_hash;
        e.posted_at = now;
        let p = &mut ctx.accounts.pool;
        require!(!p.claimed, RushError::PoolClosed);
        if p.posted == 0 || score > p.best_score {
            p.best_score = score;
            p.best_player = e.player;
        }
        p.posted += 1;
        let st = &mut ctx.accounts.player_stats;
        st.posts += 1;
        if st.posts == 1 || score > st.best_score {
            st.best_score = score;
        }
        emit!(ScorePosted { day, mint, player: e.player, score, best_score: p.best_score, best_player: p.best_player });
        Ok(())
    }

    /// claim signed by the delegated browser key; the SOL pot goes to the owner, never elsewhere.
    pub fn claim_delegated(ctx: Context<ClaimDelegated>, day: u32) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(now >= (day as i64 + 1) * SECONDS_PER_DAY + POST_GRACE, RushError::DayNotOver);
        let owner = ctx.accounts.owner.key();
        let amount = {
            let p = &mut ctx.accounts.pool;
            require!(!p.claimed, RushError::PoolClosed);
            require!(p.posted > 0, RushError::NoScores);
            require_keys_eq!(p.best_player, owner, RushError::NotWinner);
            require_keys_eq!(p.mint, Pubkey::default(), RushError::WrongMint);
            p.claimed = true;
            let a = p.pot;
            p.pot = 0;
            a
        };
        let pool_info = ctx.accounts.pool.to_account_info();
        let owner_info = ctx.accounts.owner.to_account_info();
        **pool_info.try_borrow_mut_lamports()? -= amount;
        **owner_info.try_borrow_mut_lamports()? += amount;
        ctx.accounts.player_stats.wins += 1;
        emit!(Claimed { day, mint: Pubkey::default(), player: owner, amount });
        Ok(())
    }

    /// claim_token signed by the delegated browser key; the pot goes to the owner's token account.
    pub fn claim_token_delegated(ctx: Context<ClaimTokenDelegated>, day: u32) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(now >= (day as i64 + 1) * SECONDS_PER_DAY + POST_GRACE, RushError::DayNotOver);
        let owner = ctx.accounts.owner.key();
        let mint = ctx.accounts.mint.key();
        let amount = {
            let p = &mut ctx.accounts.pool;
            require!(!p.claimed, RushError::PoolClosed);
            require!(p.posted > 0, RushError::NoScores);
            require_keys_eq!(p.best_player, owner, RushError::NotWinner);
            require_keys_eq!(p.mint, mint, RushError::WrongMint);
            p.claimed = true;
            let a = p.pot;
            p.pot = 0;
            a
        };
        let day_bytes = day.to_le_bytes();
        let mint_bytes = mint.to_bytes();
        let seeds: &[&[u8]] = &[b"pool", &day_bytes[..], &mint_bytes[..], &[ctx.accounts.pool.bump]];
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.vault.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.owner_token.to_account_info(),
                    authority: ctx.accounts.pool.to_account_info(),
                },
                &[seeds],
            ),
            amount,
            ctx.accounts.mint.decimals,
        )?;
        ctx.accounts.player_stats.wins += 1;
        emit!(Claimed { day, mint, player: owner, amount });
        Ok(())
    }

    /// A day nobody posted a clean shot in pays nobody, so after the day (plus grace) each entrant takes an equal share
    /// of the SOL pot back (the last refund takes any remainder) and their entry closes. Anyone may send it (the
    /// player, their browser key, a crank): the stake and the entry's rent only ever go to the entrant.
    pub fn refund(ctx: Context<Refund>, day: u32) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(now >= (day as i64 + 1) * SECONDS_PER_DAY + POST_GRACE, RushError::DayNotOver);
        let amount = refund_share(&mut ctx.accounts.pool)?;
        let pool_info = ctx.accounts.pool.to_account_info();
        let player_info = ctx.accounts.player.to_account_info();
        **pool_info.try_borrow_mut_lamports()? -= amount;
        **player_info.try_borrow_mut_lamports()? += amount;
        emit!(Refunded { day, mint: Pubkey::default(), player: ctx.accounts.player.key(), amount });
        Ok(())
    }

    /// The token-pool refund: the share leaves the pool's vault for the entrant's associated token account.
    pub fn refund_token(ctx: Context<RefundToken>, day: u32) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(now >= (day as i64 + 1) * SECONDS_PER_DAY + POST_GRACE, RushError::DayNotOver);
        let mint = ctx.accounts.mint.key();
        let amount = refund_share(&mut ctx.accounts.pool)?;
        if amount > 0 {
            let day_bytes = day.to_le_bytes();
            let mint_bytes = mint.to_bytes();
            let seeds: &[&[u8]] = &[b"pool", &day_bytes[..], &mint_bytes[..], &[ctx.accounts.pool.bump]];
            token_interface::transfer_checked(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.key(),
                    TransferChecked {
                        from: ctx.accounts.vault.to_account_info(),
                        mint: ctx.accounts.mint.to_account_info(),
                        to: ctx.accounts.player_token.to_account_info(),
                        authority: ctx.accounts.pool.to_account_info(),
                    },
                    &[seeds],
                ),
                amount,
                ctx.accounts.mint.decimals,
            )?;
        }
        emit!(Refunded { day, mint, player: ctx.accounts.player.key(), amount });
        Ok(())
    }
}

/// One entrant's share of a pot nobody won: an equal split of what is left, the last entrant taking the remainder.
fn refund_share(p: &mut Pool) -> Result<u64> {
    require!(p.posted == 0, RushError::PotHasScores);
    require!(!p.claimed, RushError::PoolClosed);
    require!(p.entries > 0, RushError::NothingToRefund);
    let amount = if p.entries == 1 { p.pot } else { p.pot / p.entries as u64 };
    p.pot -= amount;
    p.entries -= 1;
    Ok(amount)
}

/// One more delegated entry, while the delegation is live.
fn use_delegation(d: &mut Delegation, now: i64) -> Result<()> {
    require!(now < d.expires_at, RushError::DelegationExpired);
    require!(d.used < d.max_entries, RushError::DelegationUsedUp);
    d.used += 1;
    Ok(())
}

fn fill_entry(e: &mut Account<Entry>, player: Pubkey, day: u32, mint: Pubkey, now: i64, bump: u8) {
    e.player = player;
    e.day = day;
    e.mint = mint;
    e.posted = false;
    e.score = 0;
    e.log_hash = [0u8; 32];
    e.posted_at = 0;
    e.entered_at = now;
    e.bump = bump;
}

/// Clock-in streak: consecutive UTC days with an entry (any asset).
fn bump_player(st: &mut Account<Player>, player: Pubkey, day: u32, bump: u8) {
    if st.player == Pubkey::default() {
        st.player = player;
        st.bump = bump;
    }
    if st.last_day == day {
        return; // another asset the same day: no change
    }
    st.streak = if st.last_day + 1 == day { st.streak + 1 } else { 1 };
    if st.streak > st.best_streak {
        st.best_streak = st.streak;
    }
    st.last_day = day;
    st.days += 1;
}

#[derive(Accounts)]
pub struct InitConfig<'info> {
    #[account(init, payer = authority, space = 8 + Config::INIT_SPACE, seeds = [b"config".as_ref()], bump)]
    pub config: Account<'info, Config>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetConfig<'info> {
    #[account(mut, seeds = [b"config".as_ref()], bump = config.bump, has_one = authority)]
    pub config: Account<'info, Config>,
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct SetToken<'info> {
    #[account(seeds = [b"config".as_ref()], bump = config.bump, has_one = authority)]
    pub config: Account<'info, Config>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(init_if_needed, payer = authority, space = 8 + TokenConfig::INIT_SPACE, seeds = [b"token".as_ref(), mint.key().as_ref()], bump)]
    pub token_config: Account<'info, TokenConfig>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(day: u32)]
pub struct Enter<'info> {
    #[account(seeds = [b"config".as_ref()], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(init_if_needed, payer = player, space = 8 + Pool::INIT_SPACE, seeds = [b"pool".as_ref(), &day.to_le_bytes()[..], Pubkey::default().as_ref()], bump)]
    pub pool: Account<'info, Pool>,
    #[account(init, payer = player, space = 8 + Entry::INIT_SPACE, seeds = [b"entry".as_ref(), &day.to_le_bytes()[..], Pubkey::default().as_ref(), player.key().as_ref()], bump)]
    pub entry: Account<'info, Entry>,
    #[account(init_if_needed, payer = player, space = 8 + Player::INIT_SPACE, seeds = [b"player".as_ref(), player.key().as_ref()], bump)]
    pub player_stats: Account<'info, Player>,
    #[account(mut)]
    pub player: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(day: u32)]
pub struct EnterToken<'info> {
    // Boxed: with twelve accounts, two of them token-interface accounts, the validation frame overflowed the
    // 4 KB SBF stack by 8 bytes (the build warns), which showed up on devnet as a null read.
    #[account(seeds = [b"config".as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    /// The mint's registration: its stake and whether it takes entries.
    #[account(seeds = [b"token".as_ref(), mint.key().as_ref()], bump = token_config.bump, has_one = mint @ RushError::WrongMint)]
    pub token_config: Box<Account<'info, TokenConfig>>,
    #[account(init_if_needed, payer = player, space = 8 + Pool::INIT_SPACE, seeds = [b"pool".as_ref(), &day.to_le_bytes()[..], mint.key().as_ref()], bump)]
    pub pool: Box<Account<'info, Pool>>,
    /// The pool's token vault, an associated token account owned by the pool PDA.
    #[account(init_if_needed, payer = player, associated_token::mint = mint, associated_token::authority = pool, associated_token::token_program = token_program)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, constraint = player_token.mint == mint.key() @ RushError::WrongMint, constraint = player_token.owner == player.key() @ RushError::WrongMint)]
    pub player_token: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(init, payer = player, space = 8 + Entry::INIT_SPACE, seeds = [b"entry".as_ref(), &day.to_le_bytes()[..], mint.key().as_ref(), player.key().as_ref()], bump)]
    pub entry: Box<Account<'info, Entry>>,
    #[account(init_if_needed, payer = player, space = 8 + Player::INIT_SPACE, seeds = [b"player".as_ref(), player.key().as_ref()], bump)]
    pub player_stats: Box<Account<'info, Player>>,
    #[account(mut)]
    pub player: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(day: u32, mint: Pubkey)]
pub struct PostScore<'info> {
    #[account(seeds = [b"config".as_ref()], bump = config.bump, has_one = verifier)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [b"pool".as_ref(), &day.to_le_bytes()[..], mint.as_ref()], bump = pool.bump)]
    pub pool: Account<'info, Pool>,
    #[account(mut, seeds = [b"entry".as_ref(), &day.to_le_bytes()[..], mint.as_ref(), player.key().as_ref()], bump = entry.bump, has_one = player)]
    pub entry: Account<'info, Entry>,
    #[account(mut, seeds = [b"player".as_ref(), player.key().as_ref()], bump = player_stats.bump)]
    pub player_stats: Account<'info, Player>,
    pub player: Signer<'info>,
    /// The off-chain replay verifier. Its signature says the score was rebuilt from the run log.
    pub verifier: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(day: u32)]
pub struct Claim<'info> {
    #[account(mut, seeds = [b"pool".as_ref(), &day.to_le_bytes()[..], Pubkey::default().as_ref()], bump = pool.bump)]
    pub pool: Account<'info, Pool>,
    #[account(mut, seeds = [b"player".as_ref(), player.key().as_ref()], bump = player_stats.bump)]
    pub player_stats: Account<'info, Player>,
    #[account(mut)]
    pub player: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(day: u32)]
pub struct ClaimToken<'info> {
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut, seeds = [b"pool".as_ref(), &day.to_le_bytes()[..], mint.key().as_ref()], bump = pool.bump)]
    pub pool: Account<'info, Pool>,
    #[account(mut, associated_token::mint = mint, associated_token::authority = pool, associated_token::token_program = token_program)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(init_if_needed, payer = player, associated_token::mint = mint, associated_token::authority = player, associated_token::token_program = token_program)]
    pub player_token: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, seeds = [b"player".as_ref(), player.key().as_ref()], bump = player_stats.bump)]
    pub player_stats: Account<'info, Player>,
    #[account(mut)]
    pub player: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Delegate<'info> {
    #[account(init_if_needed, payer = owner, space = 8 + Delegation::INIT_SPACE, seeds = [b"delegate".as_ref(), owner.key().as_ref()], bump)]
    pub delegation: Account<'info, Delegation>,
    #[account(mut)]
    pub owner: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RevokeDelegation<'info> {
    #[account(mut, seeds = [b"delegate".as_ref(), owner.key().as_ref()], bump = delegation.bump, has_one = owner, close = owner)]
    pub delegation: Account<'info, Delegation>,
    #[account(mut)]
    pub owner: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(day: u32)]
pub struct EnterDelegated<'info> {
    #[account(seeds = [b"config".as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [b"delegate".as_ref(), delegation.owner.as_ref()], bump = delegation.bump, has_one = signer @ RushError::WrongSigner)]
    pub delegation: Box<Account<'info, Delegation>>,
    #[account(init_if_needed, payer = signer, space = 8 + Pool::INIT_SPACE, seeds = [b"pool".as_ref(), &day.to_le_bytes()[..], Pubkey::default().as_ref()], bump)]
    pub pool: Box<Account<'info, Pool>>,
    #[account(init, payer = signer, space = 8 + Entry::INIT_SPACE, seeds = [b"entry".as_ref(), &day.to_le_bytes()[..], Pubkey::default().as_ref(), delegation.owner.as_ref()], bump)]
    pub entry: Box<Account<'info, Entry>>,
    #[account(init_if_needed, payer = signer, space = 8 + Player::INIT_SPACE, seeds = [b"player".as_ref(), delegation.owner.as_ref()], bump)]
    pub player_stats: Box<Account<'info, Player>>,
    #[account(mut)]
    pub signer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(day: u32)]
pub struct EnterTokenDelegated<'info> {
    #[account(seeds = [b"config".as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(seeds = [b"token".as_ref(), mint.key().as_ref()], bump = token_config.bump, has_one = mint @ RushError::WrongMint)]
    pub token_config: Box<Account<'info, TokenConfig>>,
    #[account(mut, seeds = [b"delegate".as_ref(), delegation.owner.as_ref()], bump = delegation.bump, has_one = signer @ RushError::WrongSigner)]
    pub delegation: Box<Account<'info, Delegation>>,
    #[account(init_if_needed, payer = signer, space = 8 + Pool::INIT_SPACE, seeds = [b"pool".as_ref(), &day.to_le_bytes()[..], mint.key().as_ref()], bump)]
    pub pool: Box<Account<'info, Pool>>,
    #[account(init_if_needed, payer = signer, associated_token::mint = mint, associated_token::authority = pool, associated_token::token_program = token_program)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// The owner's token account; the SPL delegate it approved is this delegation PDA.
    #[account(mut, constraint = owner_token.mint == mint.key() @ RushError::WrongMint, constraint = owner_token.owner == delegation.owner @ RushError::WrongMint)]
    pub owner_token: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(init, payer = signer, space = 8 + Entry::INIT_SPACE, seeds = [b"entry".as_ref(), &day.to_le_bytes()[..], mint.key().as_ref(), delegation.owner.as_ref()], bump)]
    pub entry: Box<Account<'info, Entry>>,
    #[account(init_if_needed, payer = signer, space = 8 + Player::INIT_SPACE, seeds = [b"player".as_ref(), delegation.owner.as_ref()], bump)]
    pub player_stats: Box<Account<'info, Player>>,
    #[account(mut)]
    pub signer: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(day: u32, mint: Pubkey)]
pub struct PostScoreDelegated<'info> {
    #[account(seeds = [b"config".as_ref()], bump = config.bump, has_one = verifier)]
    pub config: Box<Account<'info, Config>>,
    #[account(seeds = [b"delegate".as_ref(), delegation.owner.as_ref()], bump = delegation.bump, has_one = signer @ RushError::WrongSigner)]
    pub delegation: Box<Account<'info, Delegation>>,
    #[account(mut, seeds = [b"pool".as_ref(), &day.to_le_bytes()[..], mint.as_ref()], bump = pool.bump)]
    pub pool: Box<Account<'info, Pool>>,
    #[account(mut, seeds = [b"entry".as_ref(), &day.to_le_bytes()[..], mint.as_ref(), delegation.owner.as_ref()], bump = entry.bump, constraint = entry.player == delegation.owner @ RushError::WrongSigner)]
    pub entry: Box<Account<'info, Entry>>,
    #[account(mut, seeds = [b"player".as_ref(), delegation.owner.as_ref()], bump = player_stats.bump)]
    pub player_stats: Box<Account<'info, Player>>,
    pub signer: Signer<'info>,
    /// The off-chain replay verifier, as for post_score.
    pub verifier: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(day: u32)]
pub struct ClaimDelegated<'info> {
    #[account(seeds = [b"delegate".as_ref(), owner.key().as_ref()], bump = delegation.bump, has_one = owner, has_one = signer @ RushError::WrongSigner)]
    pub delegation: Box<Account<'info, Delegation>>,
    #[account(mut, seeds = [b"pool".as_ref(), &day.to_le_bytes()[..], Pubkey::default().as_ref()], bump = pool.bump)]
    pub pool: Box<Account<'info, Pool>>,
    #[account(mut, seeds = [b"player".as_ref(), owner.key().as_ref()], bump = player_stats.bump)]
    pub player_stats: Box<Account<'info, Player>>,
    /// CHECK: the delegation's owner (has_one above); the pot is paid here and nowhere else.
    #[account(mut)]
    pub owner: UncheckedAccount<'info>,
    pub signer: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(day: u32)]
pub struct ClaimTokenDelegated<'info> {
    #[account(seeds = [b"delegate".as_ref(), owner.key().as_ref()], bump = delegation.bump, has_one = owner, has_one = signer @ RushError::WrongSigner)]
    pub delegation: Box<Account<'info, Delegation>>,
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut, seeds = [b"pool".as_ref(), &day.to_le_bytes()[..], mint.key().as_ref()], bump = pool.bump)]
    pub pool: Box<Account<'info, Pool>>,
    #[account(mut, associated_token::mint = mint, associated_token::authority = pool, associated_token::token_program = token_program)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(init_if_needed, payer = signer, associated_token::mint = mint, associated_token::authority = owner, associated_token::token_program = token_program)]
    pub owner_token: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, seeds = [b"player".as_ref(), owner.key().as_ref()], bump = player_stats.bump)]
    pub player_stats: Box<Account<'info, Player>>,
    /// CHECK: the delegation's owner (has_one above); its associated token account receives the pot.
    pub owner: UncheckedAccount<'info>,
    #[account(mut)]
    pub signer: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(day: u32)]
pub struct Refund<'info> {
    #[account(mut, seeds = [b"pool".as_ref(), &day.to_le_bytes()[..], Pubkey::default().as_ref()], bump = pool.bump)]
    pub pool: Box<Account<'info, Pool>>,
    #[account(mut, seeds = [b"entry".as_ref(), &day.to_le_bytes()[..], Pubkey::default().as_ref(), player.key().as_ref()], bump = entry.bump, has_one = player, close = player)]
    pub entry: Box<Account<'info, Entry>>,
    /// CHECK: the entrant (has_one on the entry); the share and the entry's rent go here and nowhere else.
    #[account(mut)]
    pub player: UncheckedAccount<'info>,
    pub payer: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(day: u32)]
pub struct RefundToken<'info> {
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut, seeds = [b"pool".as_ref(), &day.to_le_bytes()[..], mint.key().as_ref()], bump = pool.bump)]
    pub pool: Box<Account<'info, Pool>>,
    #[account(mut, associated_token::mint = mint, associated_token::authority = pool, associated_token::token_program = token_program)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, seeds = [b"entry".as_ref(), &day.to_le_bytes()[..], mint.key().as_ref(), player.key().as_ref()], bump = entry.bump, has_one = player, close = player)]
    pub entry: Box<Account<'info, Entry>>,
    #[account(init_if_needed, payer = payer, associated_token::mint = mint, associated_token::authority = player, associated_token::token_program = token_program)]
    pub player_token: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the entrant (has_one on the entry); its associated token account receives the share.
    #[account(mut)]
    pub player: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[account]
#[derive(InitSpace)]
pub struct Config {
    pub authority: Pubkey,
    pub verifier: Pubkey,
    pub stake_lamports: u64,
    pub token_mint: Pubkey,
    pub token_stake: u64,
    pub bump: u8,
}

/// One wallet's standing approval: a browser key, how many entries it may make and until when. The
/// account's lamports above rent are the SOL escrow for delegated SOL entries.
#[account]
#[derive(InitSpace)]
pub struct Delegation {
    pub owner: Pubkey,
    pub signer: Pubkey,
    pub max_entries: u16,
    pub used: u16,
    pub expires_at: i64,
    pub bump: u8,
}

/// One registered token pool: the mint, its entry stake in the mint's base units, and whether it is open.
#[account]
#[derive(InitSpace)]
pub struct TokenConfig {
    pub mint: Pubkey,
    pub stake: u64,
    pub enabled: bool,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Pool {
    pub day: u32,
    pub mint: Pubkey,
    pub entries: u32,
    pub posted: u32,
    pub pot: u64,
    pub best_score: i64,
    pub best_player: Pubkey,
    pub claimed: bool,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Entry {
    pub player: Pubkey,
    pub day: u32,
    pub mint: Pubkey,
    pub posted: bool,
    pub score: i64,
    pub log_hash: [u8; 32],
    pub posted_at: i64,
    pub entered_at: i64,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Player {
    pub player: Pubkey,
    pub streak: u16,
    pub best_streak: u16,
    pub last_day: u32,
    pub days: u32,
    pub posts: u32,
    pub wins: u32,
    pub best_score: i64,
    pub bump: u8,
}

#[event]
pub struct Entered { pub day: u32, pub mint: Pubkey, pub player: Pubkey, pub stake: u64, pub entries: u32, pub streak: u16 }
#[event]
pub struct ScorePosted { pub day: u32, pub mint: Pubkey, pub player: Pubkey, pub score: i64, pub best_score: i64, pub best_player: Pubkey }
#[event]
pub struct Claimed { pub day: u32, pub mint: Pubkey, pub player: Pubkey, pub amount: u64 }
#[event]
pub struct TokenSet { pub mint: Pubkey, pub stake: u64, pub enabled: bool }
#[event]
pub struct Delegated { pub owner: Pubkey, pub signer: Pubkey, pub max_entries: u16, pub expires_at: i64 }
#[event]
pub struct Refunded { pub day: u32, pub mint: Pubkey, pub player: Pubkey, pub amount: u64 }

#[error_code]
pub enum RushError {
    #[msg("You can only enter today's pool")]
    WrongDay,
    #[msg("This pool is closed")]
    PoolClosed,
    #[msg("Posting for this day has ended")]
    DayOver,
    #[msg("This entry already posted its one shot")]
    AlreadyPosted,
    #[msg("The day is not over yet")]
    DayNotOver,
    #[msg("No scores were posted")]
    NoScores,
    #[msg("Only the best score can claim")]
    NotWinner,
    #[msg("Wrong token mint for this pool")]
    WrongMint,
    #[msg("This token pool is not taking entries")]
    TokenOff,
    #[msg("Delegation limits are out of range")]
    BadDelegation,
    #[msg("This delegation has expired")]
    DelegationExpired,
    #[msg("This delegation has used all its entries")]
    DelegationUsedUp,
    #[msg("Not the delegated signer")]
    WrongSigner,
    #[msg("The delegation's SOL escrow can't cover this entry")]
    EscrowEmpty,
    #[msg("Someone posted a score: the best one takes this pot")]
    PotHasScores,
    #[msg("Nothing left to refund in this pool")]
    NothingToRefund,
}
