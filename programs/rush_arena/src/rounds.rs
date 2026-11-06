use super::*;

pub fn create_room(ctx: Context<CreateRoom>, id: u64, terms: RoomTerms) -> Result<()> {
    validate_schedule(&terms, Clock::get()?.unix_timestamp)?;
    require!(id == ctx.accounts.config.next_room, ArenaError::InvalidTerms);
    ctx.accounts.config.next_room = add(id, 1)?;
    let r = &mut ctx.accounts.room;
    r.id = id; r.asset = ctx.accounts.asset.key(); r.verifier = ctx.accounts.config.verifier;
    r.treasury = ctx.accounts.config.treasury; r.rent_payer = ctx.accounts.payer.key();
    r.terms = terms; r.status = OPEN; r.best_score = i64::MIN; r.bump = ctx.bumps.room;
    Ok(())
}

pub fn join_room(ctx: Context<JoinRoom>, expected: JoinTerms) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let room = &mut ctx.accounts.room;
    require!(room.status == OPEN, ArenaError::RoomClosed);
    require!(now < room.terms.join_end, ArenaError::EntryClosed);
    require!(room.players < room.terms.max_players, ArenaError::RoomFull);
    let terms = &room.terms;
    require!(expected.stake == terms.stake && expected.fee_bps == terms.fee_bps && expected.actor == terms.actor && expected.rules_hash == terms.rules_hash, ArenaError::TermsChanged);
    let balance = &mut ctx.accounts.balance;
    require!(expected.session_epoch == balance.epoch, ArenaError::InvalidSession);
    require!(expected.play_hash != [0;32], ArenaError::InvalidTerms);
    if ctx.accounts.signer.key() != balance.owner {
        let session = ctx.accounts.session.as_mut().ok_or_else(|| error!(ArenaError::InvalidSession))?;
        let balance_key = balance.key();
        let expected_key = Pubkey::find_program_address(&[b"session", balance_key.as_ref()], &crate::ID).0;
        require!(session.key() == expected_key, ArenaError::InvalidSession);
        consume_session(session, balance_key, balance.epoch, ctx.accounts.signer.key(), terms, now)?;
    }
    balance.available = sub(balance.available, terms.stake)?;
    room.pot = add(room.pot, terms.stake)?; room.players = increment(room.players)?;
    let e = &mut ctx.accounts.entry;
    e.room = room.key(); e.owner = balance.owner; e.rent_payer = ctx.accounts.payer.key();
    e.stake = room.terms.stake; e.play_hash = expected.play_hash; e.bump = ctx.bumps.entry;
    Ok(())
}

pub fn post_result(ctx: Context<PostResult>, score: i64, log_hash: [u8;32]) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let r = &mut ctx.accounts.room; let e = &mut ctx.accounts.entry;
    require!(r.status == OPEN, ArenaError::RoomClosed);
    require!(r.players >= 2 && now >= r.terms.ends_at && now < r.terms.settle_by, ArenaError::ResultWindow);
    require!(!e.posted, ArenaError::AlreadyPosted);
    require!(log_hash != [0;32], ArenaError::InvalidTerms);
    e.score = score; e.log_hash = log_hash; e.posted = true;
    if r.posted == 0 || score > r.best_score {
        r.best_score = score; r.winners = 1; r.dust_owner = e.owner;
    } else if score == r.best_score {
        r.winners = increment(r.winners)?;
        if e.owner.to_bytes() < r.dust_owner.to_bytes() { r.dust_owner = e.owner; }
    }
    r.posted = increment(r.posted)?;
    emit!(ResultRecorded { room: r.key(), room_id: r.id, owner: e.owner, asset: r.asset, play_hash: e.play_hash, score, log_hash });
    Ok(())
}

pub fn finalize(ctx: Context<ChangeRoom>) -> Result<()> {
    let r = &mut ctx.accounts.room;
    let now = Clock::get()?.unix_timestamp;
    require!(r.status == OPEN, ArenaError::RoomClosed);
    require!(r.players >= 2 && r.posted == r.players && now >= r.terms.ends_at && now < r.terms.settle_by, ArenaError::NotReady);
    let (fee, per_winner, dust) = payouts(r.pot, r.players, r.winners, r.terms.fee_bps)?;
    r.fee = fee; r.per_winner = per_winner; r.dust = dust;
    r.fee_claimed = fee == 0; r.status = FINAL;
    record_resolution(r);
    Ok(())
}

pub fn cancel(ctx: Context<ChangeRoom>) -> Result<()> {
    let r = &mut ctx.accounts.room;
    let now = Clock::get()?.unix_timestamp;
    require!(r.status == OPEN, ArenaError::RoomClosed);
    require!((r.players < 2 && now >= r.terms.join_end) || now >= r.terms.settle_by, ArenaError::NotRefundable);
    r.status = CANCELLED; r.fee = 0; r.fee_claimed = true;
    record_resolution(r);
    Ok(())
}

pub fn claim_entry(ctx: Context<ClaimEntry>) -> Result<()> {
    let r = &mut ctx.accounts.room; let e = &mut ctx.accounts.entry;
    require!(r.status == FINAL || r.status == CANCELLED, ArenaError::NotReady);
    require!(!e.claimed, ArenaError::AlreadyClaimed);
    let amount = if r.status == CANCELLED { e.stake }
        else if e.posted && e.score == r.best_score { add(r.per_winner, if e.owner == r.dust_owner { r.dust } else { 0 })? }
        else { 0 };
    r.pot = sub(r.pot, amount)?; r.claimed = increment(r.claimed)?;
    ctx.accounts.balance.available = add(ctx.accounts.balance.available, amount)?;
    e.claimed = true;
    emit!(EntryCredited { room: r.key(), room_id: r.id, owner: e.owner, asset: r.asset, amount, refunded: r.status == CANCELLED,
        stake: e.stake, play_hash: e.play_hash, rules_hash: r.terms.rules_hash, tape_commitment: r.terms.tape_commitment,
        scored: e.posted, score: e.score, log_hash: e.log_hash });
    Ok(())
}

pub fn claim_fee(ctx: Context<ClaimFee>) -> Result<()> {
    let r = &mut ctx.accounts.room;
    require!(r.status == FINAL, ArenaError::NotReady);
    require!(!r.fee_claimed, ArenaError::AlreadyClaimed);
    let b = &mut ctx.accounts.balance;
    if b.owner == Pubkey::default() { b.owner = r.treasury; b.asset = r.asset; b.bump = ctx.bumps.balance; }
    require!(b.owner == r.treasury && b.asset == r.asset, ArenaError::InvalidAuthority);
    b.available = add(b.available, r.fee)?;
    r.pot = sub(r.pot, r.fee)?; r.fee_claimed = true;
    Ok(())
}

#[event]
pub struct EntryCredited {
    pub room: Pubkey,
    pub room_id: u64,
    pub owner: Pubkey,
    pub asset: Pubkey,
    pub amount: u64,
    pub refunded: bool,
    pub stake: u64,
    pub play_hash: [u8;32],
    pub rules_hash: [u8;32],
    pub tape_commitment: [u8;32],
    pub scored: bool,
    pub score: i64,
    pub log_hash: [u8;32],
}
#[event]
pub struct ResultRecorded {
    pub room: Pubkey,
    pub room_id: u64,
    pub owner: Pubkey,
    pub asset: Pubkey,
    pub play_hash: [u8;32],
    pub score: i64,
    pub log_hash: [u8;32],
}
#[event]
pub struct RoomResolved {
    pub room: Pubkey,
    pub room_id: u64,
    pub asset: Pubkey,
    pub verifier: Pubkey,
    pub treasury: Pubkey,
    pub terms: RoomTerms,
    pub refunded: bool,
    pub players: u16,
    pub pot: u64,
    pub best_score: i64,
    pub winners: u16,
    pub dust_owner: Pubkey,
    pub per_winner: u64,
    pub dust: u64,
    pub fee: u64,
}
fn record_resolution(r: &Account<Room>) {
    emit!(RoomResolved { room: r.key(), room_id: r.id, asset: r.asset, verifier: r.verifier, treasury: r.treasury,
        terms: r.terms.clone(), refunded: r.status == CANCELLED, players: r.players, pot: r.pot, best_score: r.best_score,
        winners: r.winners, dust_owner: r.dust_owner, per_winner: r.per_winner, dust: r.dust, fee: r.fee });
}

#[derive(Accounts)]
#[instruction(id: u64)]
pub struct CreateRoom<'info> {
    #[account(mut, seeds=[b"arena"], bump=config.bump, constraint=!config.paused @ ArenaError::Paused, constraint=authority.key() == config.verifier || authority.key() == config.authority @ ArenaError::InvalidAuthority)] pub config: Account<'info, Config>,
    #[account(seeds=[b"asset", asset.mint.as_ref()], bump=asset.bump, constraint=asset.enabled @ ArenaError::AssetDisabled)] pub asset: Account<'info, Asset>,
    #[account(init, payer=payer, space=8+Room::INIT_SPACE, seeds=[b"room", id.to_le_bytes().as_ref()], bump)] pub room: Account<'info, Room>,
    pub authority: Signer<'info>,
    #[account(mut)] pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}
#[derive(Accounts)]
pub struct JoinRoom<'info> {
    #[account(seeds=[b"arena"], bump=config.bump, constraint=!config.paused @ ArenaError::Paused)] pub config: Box<Account<'info, Config>>,
    #[account(seeds=[b"asset", asset.mint.as_ref()], bump=asset.bump, constraint=asset.enabled @ ArenaError::AssetDisabled)] pub asset: Box<Account<'info, Asset>>,
    #[account(mut, seeds=[b"room", &room.id.to_le_bytes()], bump=room.bump, has_one=asset)] pub room: Box<Account<'info, Room>>,
    #[account(mut, seeds=[b"balance", asset.key().as_ref(), balance.owner.as_ref()], bump=balance.bump, has_one=asset)] pub balance: Box<Account<'info, Balance>>,
    #[account(init, payer=payer, space=8+Entry::INIT_SPACE, seeds=[b"entry", room.key().as_ref(), balance.owner.as_ref()], bump)] pub entry: Box<Account<'info, Entry>>,
    #[account(mut)] pub session: Option<Account<'info, Session>>,
    pub signer: Signer<'info>,
    #[account(mut)] pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}
#[derive(Accounts)]
pub struct PostResult<'info> {
    #[account(mut, seeds=[b"room", &room.id.to_le_bytes()], bump=room.bump, has_one=verifier)] pub room: Account<'info, Room>,
    #[account(mut, seeds=[b"entry", room.key().as_ref(), entry.owner.as_ref()], bump=entry.bump, has_one=room)] pub entry: Account<'info, Entry>,
    pub verifier: Signer<'info>,
}
#[derive(Accounts)]
pub struct ChangeRoom<'info> {
    #[account(mut, seeds=[b"room", &room.id.to_le_bytes()], bump=room.bump)] pub room: Account<'info, Room>,
}
#[derive(Accounts)]
pub struct ClaimEntry<'info> {
    #[account(mut, seeds=[b"room", &room.id.to_le_bytes()], bump=room.bump)] pub room: Account<'info, Room>,
    #[account(mut, seeds=[b"entry", room.key().as_ref(), entry.owner.as_ref()], bump=entry.bump, has_one=room)] pub entry: Account<'info, Entry>,
    #[account(mut, seeds=[b"balance", room.asset.as_ref(), entry.owner.as_ref()], bump=balance.bump, constraint=balance.owner == entry.owner && balance.asset == room.asset @ ArenaError::InvalidAuthority)] pub balance: Account<'info, Balance>,
}
#[derive(Accounts)]
pub struct ClaimFee<'info> {
    #[account(mut, seeds=[b"room", &room.id.to_le_bytes()], bump=room.bump)] pub room: Account<'info, Room>,
    #[account(init_if_needed, payer=payer, space=8+Balance::INIT_SPACE, seeds=[b"balance", room.asset.as_ref(), room.treasury.as_ref()], bump)] pub balance: Account<'info, Balance>,
    #[account(mut)] pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}
#[derive(Accounts)]
pub struct CloseEntry<'info> {
    #[account(mut, seeds=[b"room", &room.id.to_le_bytes()], bump=room.bump, constraint=room.status != OPEN @ ArenaError::NotReady)] pub room: Account<'info, Room>,
    #[account(mut, close=rent_payer, seeds=[b"entry", room.key().as_ref(), entry.owner.as_ref()], bump=entry.bump, has_one=room, has_one=rent_payer, constraint=entry.claimed @ ArenaError::ClaimsOutstanding)] pub entry: Account<'info, Entry>,
    /// CHECK: Original rent payer is bound by the entry; this only returns its rent.
    #[account(mut)] pub rent_payer: UncheckedAccount<'info>,
}
#[derive(Accounts)]
pub struct CloseRoom<'info> {
    #[account(mut, close=rent_payer, seeds=[b"room", &room.id.to_le_bytes()], bump=room.bump, has_one=rent_payer, constraint=room.status != OPEN @ ArenaError::NotReady, constraint=room.claimed == room.players && room.closed_entries == room.players && room.fee_claimed && room.pot == 0 @ ArenaError::ClaimsOutstanding)] pub room: Account<'info, Room>,
    /// CHECK: Original room rent payer is bound above; no escrow lives in this account.
    #[account(mut)] pub rent_payer: UncheckedAccount<'info>,
}
