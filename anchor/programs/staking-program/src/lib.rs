#![allow(clippy::result_large_err)]

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("HcYkXa8AFyNEuigA3gsCbLVUNT5cVB6QM7ykTqjAsNJX");

#[program]
pub mod staking_program {
    use super::*;

    /// Initialize a new staking pool
    /// - pool_authority: The authority that controls the pool
    /// - stake_token_mint: Token A that users will stake
    /// - reward_token_mint: Token B that users will receive as rewards
    /// - reward_rate: Rewards per second per staked token (scaled by 1e9)
    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        reward_rate: u64,
        min_stake_duration: i64,
    ) -> Result<()> {
        let pool = &mut ctx.accounts.pool;
        pool.authority = ctx.accounts.authority.key();
        pool.stake_token_mint = ctx.accounts.stake_token_mint.key();
        pool.reward_token_mint = ctx.accounts.reward_token_mint.key();
        pool.reward_rate = reward_rate;
        pool.min_stake_duration = min_stake_duration;
        pool.total_staked = 0;
        pool.bump = ctx.bumps.pool;
        
        msg!("Staking pool initialized with reward rate: {} per second", reward_rate);
        Ok(())
    }

    /// Stake tokens into the pool
    pub fn stake(ctx: Context<StakeTokens>, amount: u64) -> Result<()> {
        require!(amount > 0, StakingError::InvalidAmount);

        let clock = Clock::get()?;
        let user_stake = &mut ctx.accounts.user_stake;
        let is_new = user_stake.amount == 0;

        // If user has existing stake, claim pending rewards first
        if user_stake.amount > 0 {
            let rewards = calculate_rewards(
                user_stake.amount,
                ctx.accounts.pool.reward_rate,
                user_stake.last_stake_time,
                clock.unix_timestamp,
            )?;
            user_stake.pending_rewards = user_stake.pending_rewards.checked_add(rewards)
                .ok_or(StakingError::Overflow)?;
        }

        // Transfer stake tokens from user to pool vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_stake_token.to_account_info(),
            to: ctx.accounts.pool_stake_vault.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        // Update user stake account
        if is_new {
            user_stake.user = ctx.accounts.user.key();
            user_stake.pool = ctx.accounts.pool.key();
            user_stake.bump = ctx.bumps.user_stake;
        }
        user_stake.amount = user_stake.amount.checked_add(amount)
            .ok_or(StakingError::Overflow)?;
        user_stake.last_stake_time = clock.unix_timestamp;

        // Update pool total
        let pool = &mut ctx.accounts.pool;
        pool.total_staked = pool.total_staked.checked_add(amount)
            .ok_or(StakingError::Overflow)?;

        msg!("Staked {} tokens. Total staked: {}", amount, user_stake.amount);
        Ok(())
    }

    /// Unstake tokens from the pool
    pub fn unstake(ctx: Context<Unstake>, amount: u64) -> Result<()> {
        require!(amount > 0, StakingError::InvalidAmount);
        
        let user_stake = &mut ctx.accounts.user_stake;
        require!(user_stake.amount >= amount, StakingError::InsufficientStake);

        let clock = Clock::get()?;
        let elapsed = clock.unix_timestamp - user_stake.last_stake_time;
        require!(
            elapsed >= ctx.accounts.pool.min_stake_duration,
            StakingError::StakeDurationNotMet
        );

        // Calculate and add pending rewards
        let rewards = calculate_rewards(
            user_stake.amount,
            ctx.accounts.pool.reward_rate,
            user_stake.last_stake_time,
            clock.unix_timestamp,
        )?;
        user_stake.pending_rewards = user_stake.pending_rewards.checked_add(rewards)
            .ok_or(StakingError::Overflow)?;

        // Transfer stake tokens back to user
        let authority = ctx.accounts.pool.authority;
        let seeds = &[
            b"pool",
            authority.as_ref(),
            &[ctx.accounts.pool.bump],
        ];
        let signer = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.pool_stake_vault.to_account_info(),
            to: ctx.accounts.user_stake_token.to_account_info(),
            authority: ctx.accounts.pool.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer);
        token::transfer(cpi_ctx, amount)?;

        // Update user stake
        user_stake.amount = user_stake.amount.checked_sub(amount)
            .ok_or(StakingError::Underflow)?;
        user_stake.last_stake_time = clock.unix_timestamp;

        // Update pool total
        let pool = &mut ctx.accounts.pool;
        pool.total_staked = pool.total_staked.checked_sub(amount)
            .ok_or(StakingError::Underflow)?;

        msg!("Unstaked {} tokens. Remaining: {}", amount, user_stake.amount);
        Ok(())
    }

    /// Claim accumulated reward tokens
    pub fn claim_rewards(ctx: Context<ClaimRewards>) -> Result<()> {
        let user_stake = &mut ctx.accounts.user_stake;
        
        let clock = Clock::get()?;
        
        // Calculate current rewards
        let current_rewards = calculate_rewards(
            user_stake.amount,
            ctx.accounts.pool.reward_rate,
            user_stake.last_stake_time,
            clock.unix_timestamp,
        )?;
        
        let total_rewards = user_stake.pending_rewards.checked_add(current_rewards)
            .ok_or(StakingError::Overflow)?;
        
        require!(total_rewards > 0, StakingError::NoRewardsToClaim);

        // Transfer reward tokens to user
        let authority = ctx.accounts.pool.authority;
        let seeds = &[
            b"pool",
            authority.as_ref(),
            &[ctx.accounts.pool.bump],
        ];
        let signer = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.pool_reward_vault.to_account_info(),
            to: ctx.accounts.user_reward_token.to_account_info(),
            authority: ctx.accounts.pool.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer);
        token::transfer(cpi_ctx, total_rewards)?;

        // Reset rewards and update timestamp
        user_stake.pending_rewards = 0;
        user_stake.last_stake_time = clock.unix_timestamp;

        msg!("Claimed {} reward tokens", total_rewards);
        Ok(())
    }

    /// Fund the reward vault (admin function)
    pub fn fund_rewards(ctx: Context<FundRewards>, amount: u64) -> Result<()> {
        require!(amount > 0, StakingError::InvalidAmount);

        let cpi_accounts = Transfer {
            from: ctx.accounts.funder_token_account.to_account_info(),
            to: ctx.accounts.pool_reward_vault.to_account_info(),
            authority: ctx.accounts.funder.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        msg!("Funded reward vault with {} tokens", amount);
        Ok(())
    }
}

// Helper function to calculate rewards
fn calculate_rewards(
    staked_amount: u64,
    reward_rate: u64,
    last_stake_time: i64,
    current_time: i64,
) -> Result<u64> {
    let time_elapsed = current_time.checked_sub(last_stake_time)
        .ok_or(StakingError::Underflow)? as u64;
    
    let rewards = (staked_amount as u128)
        .checked_mul(reward_rate as u128)
        .ok_or(StakingError::Overflow)?
        .checked_mul(time_elapsed as u128)
        .ok_or(StakingError::Overflow)?
        .checked_div(1_000_000_000)
        .ok_or(StakingError::DivisionByZero)? as u64;
    
    Ok(rewards)
}

// Account structures

#[derive(Accounts)]
pub struct InitializePool<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        init,
        payer = authority,
        space = 8 + StakingPool::INIT_SPACE,
        seeds = [b"pool", authority.key().as_ref()],
        bump
    )]
    pub pool: Account<'info, StakingPool>,

    pub stake_token_mint: Account<'info, Mint>,
    pub reward_token_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = authority,
        token::mint = stake_token_mint,
        token::authority = pool,
        seeds = [b"stake_vault", pool.key().as_ref()],
        bump
    )]
    pub pool_stake_vault: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = authority,
        token::mint = reward_token_mint,
        token::authority = pool,
        seeds = [b"reward_vault", pool.key().as_ref()],
        bump
    )]
    pub pool_reward_vault: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct StakeTokens<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub pool: Account<'info, StakingPool>,

    #[account(
        init_if_needed,
        payer = user,
        space = 8 + UserStake::INIT_SPACE,
        seeds = [b"user_stake", pool.key().as_ref(), user.key().as_ref()],
        bump
    )]
    pub user_stake: Account<'info, UserStake>,

    #[account(
        mut,
        token::mint = pool.stake_token_mint,
        token::authority = user
    )]
    pub user_stake_token: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"stake_vault", pool.key().as_ref()],
        bump
    )]
    pub pool_stake_vault: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct Unstake<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub pool: Account<'info, StakingPool>,

    #[account(
        mut,
        seeds = [b"user_stake", pool.key().as_ref(), user.key().as_ref()],
        bump = user_stake.bump,
        constraint = user_stake.user == user.key()
    )]
    pub user_stake: Account<'info, UserStake>,

    #[account(
        mut,
        constraint = user_stake_token.owner == user.key(),
        constraint = user_stake_token.mint == pool.stake_token_mint
    )]
    pub user_stake_token: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"stake_vault", pool.key().as_ref()],
        bump
    )]
    pub pool_stake_vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ClaimRewards<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub pool: Account<'info, StakingPool>,

    #[account(
        mut,
        seeds = [b"user_stake", pool.key().as_ref(), user.key().as_ref()],
        bump = user_stake.bump,
        constraint = user_stake.user == user.key()
    )]
    pub user_stake: Account<'info, UserStake>,

    #[account(
        mut,
        constraint = user_reward_token.owner == user.key(),
        constraint = user_reward_token.mint == pool.reward_token_mint
    )]
    pub user_reward_token: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"reward_vault", pool.key().as_ref()],
        bump
    )]
    pub pool_reward_vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct FundRewards<'info> {
    #[account(mut)]
    pub funder: Signer<'info>,

    #[account(mut)]
    pub pool: Account<'info, StakingPool>,

    #[account(
        mut,
        constraint = funder_token_account.owner == funder.key(),
        constraint = funder_token_account.mint == pool.reward_token_mint
    )]
    pub funder_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"reward_vault", pool.key().as_ref()],
        bump
    )]
    pub pool_reward_vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

// Data accounts

#[account]
#[derive(InitSpace)]
pub struct StakingPool {
    pub authority: Pubkey,
    pub stake_token_mint: Pubkey,
    pub reward_token_mint: Pubkey,
    pub reward_rate: u64,           // Rewards per second per token (scaled by 1e9)
    pub min_stake_duration: i64,    // Minimum time before unstaking allowed (seconds)
    pub total_staked: u64,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct UserStake {
    pub user: Pubkey,
    pub pool: Pubkey,
    pub amount: u64,
    pub last_stake_time: i64,
    pub pending_rewards: u64,
    pub bump: u8,
}

// Error codes

#[error_code]
pub enum StakingError {
    #[msg("Amount must be greater than zero")]
    InvalidAmount,
    #[msg("Insufficient stake amount")]
    InsufficientStake,
    #[msg("Minimum stake duration not met")]
    StakeDurationNotMet,
    #[msg("No rewards to claim")]
    NoRewardsToClaim,
    #[msg("Arithmetic overflow")]
    Overflow,
    #[msg("Arithmetic underflow")]
    Underflow,
    #[msg("Division by zero")]
    DivisionByZero,
}
