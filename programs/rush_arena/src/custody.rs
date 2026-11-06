use super::*;
use anchor_spl::token_2022::spl_token_2022::{
    extension::{BaseStateWithExtensions, StateWithExtensions, ExtensionType, default_account_state::DefaultAccountState, transfer_hook},
    state::{Mint as SplMint, AccountState},
};

/// Registration is explicit operator allowlisting. Reject amount-changing and
/// unsupported execution extensions, including a hook activated after registration.
fn validate_mint(mint: &InterfaceAccount<Mint>, admission: bool) -> Result<()> {
    require!(mint.decimals <= 9, ArenaError::InvalidAsset);
    if *mint.to_account_info().owner == anchor_spl::token::ID { return Ok(()); }
    let info = mint.to_account_info();
    let data = info.try_borrow_data()?;
    let parsed = StateWithExtensions::<SplMint>::unpack(&data)?;
    for extension in parsed.get_extension_types()? {
        match extension {
            ExtensionType::MintCloseAuthority | ExtensionType::PermanentDelegate |
            ExtensionType::MetadataPointer | ExtensionType::TokenMetadata |
            ExtensionType::ConfidentialTransferMint => {},
            ExtensionType::DefaultAccountState => {
                // Default state applies to NEW accounts. Existing unfrozen owner/vault
                // accounts must remain withdrawable if the issuer changes this default.
                require!(!admission || parsed.get_extension::<DefaultAccountState>()?.state == AccountState::Initialized as u8, ArenaError::UnsupportedMint);
            },
            ExtensionType::TransferHook => {
                require!(transfer_hook::get_program_id(&parsed).is_none(), ArenaError::UnsupportedMint);
            },
            _ => return err!(ArenaError::UnsupportedMint),
        }
    }
    Ok(())
}

fn bind_balance(balance: &mut Account<Balance>, owner: Pubkey, asset: Pubkey, bump: u8) -> Result<()> {
    if balance.owner == Pubkey::default() {
        balance.owner = owner; balance.asset = asset; balance.bump = bump;
    }
    require!(balance.owner == owner && balance.asset == asset, ArenaError::InvalidAuthority);
    Ok(())
}

fn sol_reserve(asset: &Account<Asset>) -> Result<u64> {
    let account = asset.to_account_info();
    let rent = Rent::get()?.minimum_balance(account.data_len());
    account.lamports().checked_sub(rent).ok_or_else(|| error!(ArenaError::Insolvent))
}

pub fn register_sol(ctx: Context<RegisterSol>) -> Result<()> {
    let a = &mut ctx.accounts.asset;
    a.mint = Pubkey::default(); a.token_program = System::id(); a.decimals = 9;
    a.enabled = true; a.liability = 0; a.bump = ctx.bumps.asset;
    Ok(())
}
pub fn register_token(ctx: Context<RegisterToken>) -> Result<()> {
    validate_mint(&ctx.accounts.mint, true)?;
    let a = &mut ctx.accounts.asset;
    a.mint = ctx.accounts.mint.key(); a.token_program = ctx.accounts.token_program.key();
    a.decimals = ctx.accounts.mint.decimals; a.enabled = true; a.liability = 0; a.bump = ctx.bumps.asset;
    Ok(())
}
pub fn deposit_sol(ctx: Context<DepositSol>, amount: u64) -> Result<()> {
    require!(amount > 0, ArenaError::InvalidAmount);
    require!(sol_reserve(&ctx.accounts.asset)? >= ctx.accounts.asset.liability, ArenaError::Insolvent);
    bind_balance(&mut ctx.accounts.balance, ctx.accounts.owner.key(), ctx.accounts.asset.key(), ctx.bumps.balance)?;
    transfer(CpiContext::new(ctx.accounts.system_program.key(), Transfer {
        from: ctx.accounts.owner.to_account_info(), to: ctx.accounts.asset.to_account_info(),
    }), amount)?;
    ctx.accounts.balance.available = add(ctx.accounts.balance.available, amount)?;
    ctx.accounts.asset.liability = add(ctx.accounts.asset.liability, amount)?;
    Ok(())
}
pub fn withdraw_sol(ctx: Context<WithdrawSol>, amount: u64) -> Result<()> {
    require!(amount > 0, ArenaError::InvalidAmount);
    require!(sol_reserve(&ctx.accounts.asset)? >= ctx.accounts.asset.liability, ArenaError::Insolvent);
    ctx.accounts.balance.available = sub(ctx.accounts.balance.available, amount)?;
    ctx.accounts.asset.liability = sub(ctx.accounts.asset.liability, amount)?;
    let from = ctx.accounts.asset.to_account_info(); let to = ctx.accounts.owner.to_account_info();
    let remaining = sub(from.lamports(), amount)?; let received = add(to.lamports(), amount)?;
    **from.try_borrow_mut_lamports()? = remaining;
    **to.try_borrow_mut_lamports()? = received;
    Ok(())
}
pub fn deposit_token(ctx: Context<DepositToken>, amount: u64) -> Result<()> {
    require!(amount > 0, ArenaError::InvalidAmount);
    validate_mint(&ctx.accounts.mint, true)?;
    require!(ctx.accounts.vault.amount >= ctx.accounts.asset.liability, ArenaError::Insolvent);
    bind_balance(&mut ctx.accounts.balance, ctx.accounts.owner.key(), ctx.accounts.asset.key(), ctx.bumps.balance)?;
    let before = ctx.accounts.vault.amount;
    token_interface::transfer_checked(CpiContext::new(ctx.accounts.token_program.key(), TransferChecked {
        from: ctx.accounts.source.to_account_info(), mint: ctx.accounts.mint.to_account_info(),
        to: ctx.accounts.vault.to_account_info(), authority: ctx.accounts.owner.to_account_info(),
    }), amount, ctx.accounts.asset.decimals)?;
    ctx.accounts.vault.reload()?;
    require!(ctx.accounts.vault.amount == add(before, amount)?, ArenaError::TransferMismatch);
    ctx.accounts.balance.available = add(ctx.accounts.balance.available, amount)?;
    ctx.accounts.asset.liability = add(ctx.accounts.asset.liability, amount)?;
    Ok(())
}
pub fn withdraw_token(ctx: Context<WithdrawToken>, amount: u64) -> Result<()> {
    require!(amount > 0, ArenaError::InvalidAmount);
    validate_mint(&ctx.accounts.mint, false)?;
    require!(ctx.accounts.vault.amount >= ctx.accounts.asset.liability, ArenaError::Insolvent);
    let before = ctx.accounts.destination.amount;
    ctx.accounts.balance.available = sub(ctx.accounts.balance.available, amount)?;
    ctx.accounts.asset.liability = sub(ctx.accounts.asset.liability, amount)?;
    let mint = ctx.accounts.asset.mint; let bump = [ctx.accounts.asset.bump];
    let seeds: &[&[u8]] = &[b"asset", mint.as_ref(), &bump];
    token_interface::transfer_checked(CpiContext::new_with_signer(ctx.accounts.token_program.key(), TransferChecked {
        from: ctx.accounts.vault.to_account_info(), mint: ctx.accounts.mint.to_account_info(),
        to: ctx.accounts.destination.to_account_info(), authority: ctx.accounts.asset.to_account_info(),
    }, &[seeds]), amount, ctx.accounts.asset.decimals)?;
    ctx.accounts.destination.reload()?;
    require!(ctx.accounts.destination.amount == add(before, amount)?, ArenaError::TransferMismatch);
    Ok(())
}

pub fn grant_session(ctx: Context<GrantSession>, expected_epoch: u64, limits: SessionLimits) -> Result<()> {
    // The owner signs the current version. A later revoke from ANY device
    // invalidates an earlier grant, including one withheld by its sponsor.
    require!(ctx.accounts.balance.epoch == expected_epoch, ArenaError::InvalidSession);
    validate_limits(&limits, Clock::get()?.unix_timestamp, ctx.accounts.balance.available, ctx.accounts.owner.key())?;
    let epoch = add(ctx.accounts.balance.epoch, 1)?;
    ctx.accounts.balance.epoch = epoch;
    let s = &mut ctx.accounts.session;
    s.balance = ctx.accounts.balance.key(); s.epoch = epoch; s.limits = limits;
    s.spent = 0; s.entries = 0; s.bump = ctx.bumps.session;
    Ok(())
}

#[derive(Accounts)]
pub struct RegisterSol<'info> {
    #[account(seeds=[b"arena"], bump=config.bump, has_one=authority)] pub config: Account<'info, Config>,
    #[account(init, payer=authority, space=8+Asset::INIT_SPACE, seeds=[b"asset", Pubkey::default().as_ref()], bump)] pub asset: Account<'info, Asset>,
    #[account(mut)] pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}
#[derive(Accounts)]
pub struct RegisterToken<'info> {
    #[account(seeds=[b"arena"], bump=config.bump, has_one=authority)] pub config: Account<'info, Config>,
    #[account(init, payer=authority, space=8+Asset::INIT_SPACE, seeds=[b"asset", mint.key().as_ref()], bump)] pub asset: Account<'info, Asset>,
    #[account(mint::token_program=token_program)] pub mint: InterfaceAccount<'info, Mint>,
    #[account(init_if_needed, payer=authority, associated_token::mint=mint, associated_token::authority=asset, associated_token::token_program=token_program)] pub vault: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)] pub authority: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}
#[derive(Accounts)]
pub struct EnableAsset<'info> {
    #[account(seeds=[b"arena"], bump=config.bump, has_one=authority)] pub config: Account<'info, Config>,
    #[account(mut, seeds=[b"asset", asset.mint.as_ref()], bump=asset.bump)] pub asset: Account<'info, Asset>,
    pub authority: Signer<'info>,
}
#[derive(Accounts)]
pub struct DepositSol<'info> {
    #[account(seeds=[b"arena"], bump=config.bump, constraint=!config.paused @ ArenaError::Paused)] pub config: Account<'info, Config>,
    #[account(mut, seeds=[b"asset", Pubkey::default().as_ref()], bump=asset.bump, constraint=asset.mint == Pubkey::default() @ ArenaError::InvalidAsset, constraint=asset.enabled @ ArenaError::AssetDisabled)] pub asset: Account<'info, Asset>,
    #[account(init_if_needed, payer=payer, space=8+Balance::INIT_SPACE, seeds=[b"balance", asset.key().as_ref(), owner.key().as_ref()], bump)] pub balance: Account<'info, Balance>,
    #[account(mut)] pub owner: Signer<'info>,
    #[account(mut)] pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}
#[derive(Accounts)]
pub struct WithdrawSol<'info> {
    #[account(mut, seeds=[b"asset", Pubkey::default().as_ref()], bump=asset.bump, constraint=asset.mint == Pubkey::default() @ ArenaError::InvalidAsset)] pub asset: Account<'info, Asset>,
    #[account(mut, seeds=[b"balance", asset.key().as_ref(), owner.key().as_ref()], bump=balance.bump, has_one=owner, has_one=asset)] pub balance: Account<'info, Balance>,
    #[account(mut)] pub owner: Signer<'info>,
}
#[derive(Accounts)]
pub struct DepositToken<'info> {
    #[account(seeds=[b"arena"], bump=config.bump, constraint=!config.paused @ ArenaError::Paused)] pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds=[b"asset", mint.key().as_ref()], bump=asset.bump, has_one=mint, has_one=token_program, constraint=asset.enabled @ ArenaError::AssetDisabled)] pub asset: Box<Account<'info, Asset>>,
    #[account(init_if_needed, payer=payer, space=8+Balance::INIT_SPACE, seeds=[b"balance", asset.key().as_ref(), owner.key().as_ref()], bump)] pub balance: Box<Account<'info, Balance>>,
    #[account(mint::token_program=token_program, constraint=mint.decimals == asset.decimals @ ArenaError::InvalidAsset)] pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut, associated_token::mint=mint, associated_token::authority=asset, associated_token::token_program=token_program)] pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, token::mint=mint, token::authority=owner, token::token_program=token_program)] pub source: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)] pub owner: Signer<'info>,
    #[account(mut)] pub payer: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}
#[derive(Accounts)]
pub struct WithdrawToken<'info> {
    #[account(mut, seeds=[b"asset", mint.key().as_ref()], bump=asset.bump, has_one=mint, has_one=token_program)] pub asset: Account<'info, Asset>,
    #[account(mut, seeds=[b"balance", asset.key().as_ref(), owner.key().as_ref()], bump=balance.bump, has_one=owner, has_one=asset)] pub balance: Account<'info, Balance>,
    #[account(mint::token_program=token_program, constraint=mint.decimals == asset.decimals @ ArenaError::InvalidAsset)] pub mint: InterfaceAccount<'info, Mint>,
    #[account(mut, associated_token::mint=mint, associated_token::authority=asset, associated_token::token_program=token_program)] pub vault: InterfaceAccount<'info, TokenAccount>,
    #[account(init_if_needed, payer=payer, associated_token::mint=mint, associated_token::authority=owner, associated_token::token_program=token_program)] pub destination: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)] pub owner: Signer<'info>,
    #[account(mut)] pub payer: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}
#[derive(Accounts)]
pub struct GrantSession<'info> {
    #[account(mut, seeds=[b"balance", balance.asset.as_ref(), owner.key().as_ref()], bump=balance.bump, has_one=owner)] pub balance: Account<'info, Balance>,
    #[account(init_if_needed, payer=payer, space=8+Session::INIT_SPACE, seeds=[b"session", balance.key().as_ref()], bump)] pub session: Account<'info, Session>,
    #[account(mut)] pub owner: Signer<'info>,
    #[account(mut)] pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}
#[derive(Accounts)]
pub struct RevokeSession<'info> {
    #[account(mut, seeds=[b"balance", balance.asset.as_ref(), owner.key().as_ref()], bump=balance.bump, has_one=owner)] pub balance: Account<'info, Balance>,
    pub owner: Signer<'info>,
}
