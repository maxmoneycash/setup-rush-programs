use super::*;

pub const MAX_FEE_BPS: u16 = 500;
pub const OPEN: u8 = 0;
pub const FINAL: u8 = 1;
pub const CANCELLED: u8 = 2;

#[account]
#[derive(InitSpace, Default)]
pub struct Config {
    pub version: u8,
    pub authority: Pubkey,
    pub pending_authority: Pubkey,
    pub verifier: Pubkey,
    pub treasury: Pubkey,
    pub paused: bool,
    pub next_room: u64,
    pub bump: u8,
}

#[account]
#[derive(InitSpace, Default)]
pub struct Asset {
    pub mint: Pubkey,
    pub token_program: Pubkey,
    pub decimals: u8,
    pub enabled: bool,
    /// All owner credits, room pots, and unclaimed fees in this asset's base units.
    pub liability: u64,
    pub bump: u8,
}

#[account]
#[derive(InitSpace, Default)]
pub struct Balance {
    pub owner: Pubkey,
    pub asset: Pubkey,
    pub available: u64,
    pub epoch: u64,
    pub bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Default)]
pub struct SessionLimits {
    pub signer: Pubkey,
    pub expires_at: i64,
    pub per_round: u64,
    pub total_spend: u64,
    pub max_entries: u16,
    /// bit 0: manual; bit 1: automatic. No other actor classes are accepted.
    pub actor_mask: u8,
    pub max_fee_bps: u16,
}

#[account]
#[derive(InitSpace, Default)]
pub struct Session {
    pub balance: Pubkey,
    pub epoch: u64,
    pub limits: SessionLimits,
    pub spent: u64,
    pub entries: u16,
    pub bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Default)]
pub struct RoomTerms {
    pub stake: u64,
    pub fee_bps: u16,
    pub actor: u8,
    pub rules_hash: [u8; 32],
    pub tape_commitment: [u8; 32],
    pub join_end: i64,
    pub starts_at: i64,
    pub ends_at: i64,
    pub settle_by: i64,
    pub max_players: u16,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Default)]
pub struct JoinTerms {
    pub stake: u64,
    pub fee_bps: u16,
    pub actor: u8,
    pub rules_hash: [u8; 32],
    /// Reject a transaction signed before revocation, even if the same key is re-granted.
    pub session_epoch: u64,
    /// Canonical actor / strategy version / parameters selected for this entry.
    pub play_hash: [u8; 32],
}

#[account]
#[derive(InitSpace, Default)]
pub struct Room {
    pub id: u64,
    pub asset: Pubkey,
    pub verifier: Pubkey,
    pub treasury: Pubkey,
    pub rent_payer: Pubkey,
    pub terms: RoomTerms,
    pub status: u8,
    pub players: u16,
    pub posted: u16,
    pub claimed: u16,
    pub closed_entries: u16,
    pub pot: u64,
    pub best_score: i64,
    pub winners: u16,
    pub dust_owner: Pubkey,
    pub per_winner: u64,
    pub dust: u64,
    pub fee: u64,
    pub fee_claimed: bool,
    pub bump: u8,
}

#[account]
#[derive(InitSpace, Default)]
pub struct Entry {
    pub room: Pubkey,
    pub owner: Pubkey,
    pub rent_payer: Pubkey,
    pub stake: u64,
    pub play_hash: [u8; 32],
    pub score: i64,
    pub log_hash: [u8; 32],
    pub posted: bool,
    pub claimed: bool,
    pub bump: u8,
}

pub fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b).ok_or_else(|| error!(ArenaError::Arithmetic))
}
pub fn sub(a: u64, b: u64) -> Result<u64> {
    a.checked_sub(b).ok_or_else(|| error!(ArenaError::InsufficientBalance))
}
pub fn increment(a: u16) -> Result<u16> {
    a.checked_add(1).ok_or_else(|| error!(ArenaError::Arithmetic))
}

pub fn validate_schedule(t: &RoomTerms, now: i64) -> Result<()> {
    require!(t.stake > 0 && (2..=16).contains(&t.max_players), ArenaError::InvalidTerms);
    require!(t.fee_bps <= MAX_FEE_BPS && t.actor <= 1, ArenaError::InvalidTerms);
    require!(t.rules_hash != [0; 32] && t.tape_commitment != [0; 32], ArenaError::InvalidTerms);
    let join_wait = t.join_end.checked_sub(now).ok_or(ArenaError::InvalidTerms)?;
    let start_wait = t.starts_at.checked_sub(t.join_end).ok_or(ArenaError::InvalidTerms)?;
    let duration = t.ends_at.checked_sub(t.starts_at).ok_or(ArenaError::InvalidTerms)?;
    let settle_wait = t.settle_by.checked_sub(t.ends_at).ok_or(ArenaError::InvalidTerms)?;
    require!((5..=900).contains(&join_wait) && (0..=60).contains(&start_wait), ArenaError::InvalidTerms);
    require!((15..=900).contains(&duration) && (30..=86_400).contains(&settle_wait), ArenaError::InvalidTerms);
    require!(t.stake.checked_mul(u64::from(t.max_players)).is_some(), ArenaError::Arithmetic);
    Ok(())
}

pub fn validate_limits(l: &SessionLimits, now: i64, available: u64, owner: Pubkey) -> Result<()> {
    require!(l.signer != Pubkey::default() && l.signer != owner, ArenaError::InvalidAuthority);
    let duration = l.expires_at.checked_sub(now).ok_or(ArenaError::InvalidSession)?;
    require!((1..=86_400).contains(&duration), ArenaError::InvalidSession);
    require!(l.per_round > 0 && l.per_round <= l.total_spend && l.total_spend <= available, ArenaError::InvalidSession);
    require!(l.max_entries > 0 && l.actor_mask > 0 && l.actor_mask <= 3 && l.max_fee_bps <= MAX_FEE_BPS, ArenaError::InvalidSession);
    Ok(())
}

pub fn consume_session(s: &mut Session, balance: Pubkey, epoch: u64, signer: Pubkey, t: &RoomTerms, now: i64) -> Result<()> {
    require!(s.balance == balance && s.epoch == epoch && s.limits.signer == signer, ArenaError::InvalidSession);
    require!(now < s.limits.expires_at, ArenaError::SessionExpired);
    require!(t.actor <= 1 && (s.limits.actor_mask & (1 << t.actor)) != 0, ArenaError::SessionLimit);
    require!(t.stake <= s.limits.per_round && t.fee_bps <= s.limits.max_fee_bps, ArenaError::SessionLimit);
    let spent = add(s.spent, t.stake)?;
    let entries = increment(s.entries)?;
    require!(spent <= s.limits.total_spend && entries <= s.limits.max_entries, ArenaError::SessionLimit);
    s.spent = spent; s.entries = entries;
    Ok(())
}

/// Fee is floored once. Every base unit is assigned, including ties and division dust.
pub fn payouts(pot: u64, players: u16, winners: u16, fee_bps: u16) -> Result<(u64, u64, u64)> {
    require!(players >= 2 && winners > 0 && winners <= players && fee_bps <= MAX_FEE_BPS, ArenaError::InvalidTerms);
    let fee = if winners == players { 0 } else { ((u128::from(pot) * u128::from(fee_bps)) / 10_000) as u64 };
    let prize = sub(pot, fee)?;
    Ok((fee, prize / u64::from(winners), prize % u64::from(winners)))
}

#[error_code]
pub enum ArenaError {
    #[msg("Arithmetic limit exceeded")] Arithmetic,
    #[msg("Authority does not match")] InvalidAuthority,
    #[msg("New activity is paused")] Paused,
    #[msg("Asset is disabled")] AssetDisabled,
    #[msg("Asset or token program does not match")] InvalidAsset,
    #[msg("This mint extension is unsupported")] UnsupportedMint,
    #[msg("Vault no longer covers all balances and pots")] Insolvent,
    #[msg("Amount must be positive")] InvalidAmount,
    #[msg("Available balance is too low")] InsufficientBalance,
    #[msg("Token transfer did not deliver the exact amount")] TransferMismatch,
    #[msg("Session permission does not match")] InvalidSession,
    #[msg("Session has expired")] SessionExpired,
    #[msg("Session spending or entry limit reached")] SessionLimit,
    #[msg("Room terms or schedule are invalid")] InvalidTerms,
    #[msg("Room terms differ from the approved entry")] TermsChanged,
    #[msg("Room is no longer open")] RoomClosed,
    #[msg("Room is full")] RoomFull,
    #[msg("Entry deadline has passed")] EntryClosed,
    #[msg("Result cannot be submitted at this time")] ResultWindow,
    #[msg("This result was already posted")] AlreadyPosted,
    #[msg("Round is not ready for settlement")] NotReady,
    #[msg("Refund deadline has not arrived")] NotRefundable,
    #[msg("This entry or fee was already claimed")] AlreadyClaimed,
    #[msg("Entries or fees still need to be claimed")] ClaimsOutstanding,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_integer_payouts_conserve_pot() {
        for players in 2..=16 {
            for winners in 1..=players {
                for stake in [1_u64, 2, 13, 999_999, u64::MAX / 16] {
                    for fee_bps in [0, 1, 99, 500] {
                        let pot = stake * u64::from(players);
                        let (fee, each, dust) = payouts(pot, players, winners, fee_bps).unwrap();
                        assert_eq!(fee + each * u64::from(winners) + dust, pot);
                        assert!(dust < u64::from(winners));
                        if players == winners { assert_eq!((fee, each, dust), (0, stake, 0)); }
                    }
                }
            }
        }
    }
    #[test]
    fn session_is_cumulative_revocable_expiring_and_actor_specific() {
        let owner = Pubkey::new_unique(); let signer = Pubkey::new_unique(); let balance = Pubkey::new_unique();
        let limits = SessionLimits { signer, expires_at: 200, per_round: 10, total_spend: 20, max_entries: 2, actor_mask: 2, max_fee_bps: 100 };
        validate_limits(&limits, 100, 20, owner).unwrap();
        let mut s = Session { balance, epoch: 7, limits, ..Session::default() };
        let t = RoomTerms { stake: 10, fee_bps: 100, actor: 1, ..RoomTerms::default() };
        assert!(consume_session(&mut s, balance, 8, signer, &t, 100).is_err());
        assert!(consume_session(&mut s, balance, 7, signer, &t, 200).is_err());
        assert!(consume_session(&mut s, balance, 7, signer, &RoomTerms { actor: 0, ..t.clone() }, 100).is_err());
        consume_session(&mut s, balance, 7, signer, &t, 100).unwrap();
        consume_session(&mut s, balance, 7, signer, &t, 100).unwrap();
        assert!(consume_session(&mut s, balance, 7, signer, &t, 100).is_err());
        assert_eq!((s.spent, s.entries), (20, 2));
    }
    #[test]
    fn schedules_reject_overflow_and_invalid_windows() {
        let t = RoomTerms { stake: 1, max_players: 2, rules_hash: [1;32], tape_commitment: [2;32], join_end: 105, starts_at: 105, ends_at: 120, settle_by: 150, ..RoomTerms::default() };
        validate_schedule(&t, 100).unwrap();
        assert!(validate_schedule(&t, 101).is_err());
        assert!(validate_schedule(&RoomTerms { stake: u64::MAX, ..t.clone() }, 100).is_err());
        assert!(validate_schedule(&RoomTerms { starts_at: i64::MIN, ..t.clone() }, 100).is_err());
        assert!(validate_schedule(&RoomTerms { settle_by: i64::MAX, ..t }, 100).is_err());
    }
}
