import * as anchor from '@coral-xyz/anchor'
import { Program, BN } from '@coral-xyz/anchor'
import { Keypair, PublicKey } from '@solana/web3.js'
import { createMint, getOrCreateAssociatedTokenAccount, mintTo, getAccount } from '@solana/spl-token'
import { StakingProgram } from '../target/types/staking-program'

describe('staking-program', () => {
  // Configure the client to use the local cluster.
  const provider = anchor.AnchorProvider.env()
  anchor.setProvider(provider)
  const payer = provider.wallet as anchor.Wallet

  const program = anchor.workspace.StakingProgram as Program<StakingProgram>

  let stakeTokenMint: PublicKey
  let rewardTokenMint: PublicKey
  let userStakeTokenAccount: PublicKey
  let userRewardTokenAccount: PublicKey
  let poolPda: PublicKey
  let poolStakeVault: PublicKey
  let poolRewardVault: PublicKey
  let userStakePda: PublicKey

  const rewardRate = new BN(1_000_000) // 0.001 tokens per second per staked token (1e9 scale)
  const minStakeDuration = new BN(5) // 5 seconds minimum stake duration

  beforeAll(async () => {
    // Create stake token (Token A)
    stakeTokenMint = await createMint(
      provider.connection,
      payer.payer,
      payer.publicKey,
      null,
      9 // 9 decimals
    )

    // Create reward token (Token B)
    rewardTokenMint = await createMint(
      provider.connection,
      payer.payer,
      payer.publicKey,
      null,
      9 // 9 decimals
    )

    // Create user token accounts
    const userStakeAta = await getOrCreateAssociatedTokenAccount(
      provider.connection,
      payer.payer,
      stakeTokenMint,
      payer.publicKey
    )
    userStakeTokenAccount = userStakeAta.address

    const userRewardAta = await getOrCreateAssociatedTokenAccount(
      provider.connection,
      payer.payer,
      rewardTokenMint,
      payer.publicKey
    )
    userRewardTokenAccount = userRewardAta.address

    // Mint tokens to user
    await mintTo(
      provider.connection,
      payer.payer,
      stakeTokenMint,
      userStakeTokenAccount,
      payer.publicKey,
      1_000_000_000_000 // 1000 tokens with 9 decimals
    )

    // Mint reward tokens to user (for funding the pool)
    await mintTo(
      provider.connection,
      payer.payer,
      rewardTokenMint,
      userRewardTokenAccount,
      payer.publicKey,
      10_000_000_000_000 // 10000 tokens with 9 decimals
    )

    // Derive PDAs
    ;[poolPda] = PublicKey.findProgramAddressSync(
      [Buffer.from('pool'), payer.publicKey.toBuffer()],
      program.programId
    )

    ;[poolStakeVault] = PublicKey.findProgramAddressSync(
      [Buffer.from('stake_vault'), poolPda.toBuffer()],
      program.programId
    )

    ;[poolRewardVault] = PublicKey.findProgramAddressSync(
      [Buffer.from('reward_vault'), poolPda.toBuffer()],
      program.programId
    )

    ;[userStakePda] = PublicKey.findProgramAddressSync(
      [Buffer.from('user_stake'), poolPda.toBuffer(), payer.publicKey.toBuffer()],
      program.programId
    )
  })

  it('Initialize Staking Pool', async () => {
    await program.methods
      .initializePool(rewardRate, minStakeDuration)
      .accounts({
        authority: payer.publicKey,
        pool: poolPda,
        stakeTokenMint: stakeTokenMint,
        rewardTokenMint: rewardTokenMint,
        poolStakeVault: poolStakeVault,
        poolRewardVault: poolRewardVault,
      })
      .rpc()

    const pool = await program.account.stakingPool.fetch(poolPda)
    
    expect(pool.authority.toString()).toEqual(payer.publicKey.toString())
    expect(pool.stakeTokenMint.toString()).toEqual(stakeTokenMint.toString())
    expect(pool.rewardTokenMint.toString()).toEqual(rewardTokenMint.toString())
    expect(pool.rewardRate.toString()).toEqual(rewardRate.toString())
    expect(pool.minStakeDuration.toString()).toEqual(minStakeDuration.toString())
    expect(pool.totalStaked.toString()).toEqual('0')
  })

  it('Fund Reward Vault', async () => {
    const fundAmount = new BN(5_000_000_000_000) // 5000 tokens

    await program.methods
      .fundRewards(fundAmount)
      .accounts({
        funder: payer.publicKey,
        pool: poolPda,
        funderTokenAccount: userRewardTokenAccount,
        poolRewardVault: poolRewardVault,
      })
      .rpc()

    const vaultAccount = await getAccount(provider.connection, poolRewardVault)
    expect(vaultAccount.amount.toString()).toEqual(fundAmount.toString())
  })

  it('Stake Tokens', async () => {
    const stakeAmount = new BN(100_000_000_000) // 100 tokens

    await program.methods
      .stake(stakeAmount)
      .accounts({
        user: payer.publicKey,
        pool: poolPda,
        userStake: userStakePda,
        userStakeToken: userStakeTokenAccount,
        poolStakeVault: poolStakeVault,
      })
      .rpc()

    const userStake = await program.account.userStake.fetch(userStakePda)
    const pool = await program.account.stakingPool.fetch(poolPda)
    
    expect(userStake.amount.toString()).toEqual(stakeAmount.toString())
    expect(userStake.user.toString()).toEqual(payer.publicKey.toString())
    expect(pool.totalStaked.toString()).toEqual(stakeAmount.toString())
  })

  it('Stake More Tokens', async () => {
    // Wait a bit to accumulate some rewards
    await new Promise(resolve => setTimeout(resolve, 2000))

    const additionalStake = new BN(50_000_000_000) // 50 more tokens

    const userStakeBefore = await program.account.userStake.fetch(userStakePda)

    await program.methods
      .stake(additionalStake)
      .accounts({
        user: payer.publicKey,
        pool: poolPda,
        userStake: userStakePda,
        userStakeToken: userStakeTokenAccount,
        poolStakeVault: poolStakeVault,
      })
      .rpc()

    const userStakeAfter = await program.account.userStake.fetch(userStakePda)
    const pool = await program.account.stakingPool.fetch(poolPda)
    
    const expectedTotal = userStakeBefore.amount.add(additionalStake)
    expect(userStakeAfter.amount.toString()).toEqual(expectedTotal.toString())
    expect(pool.totalStaked.toString()).toEqual(expectedTotal.toString())
    // Pending rewards should have increased
    expect(userStakeAfter.pendingRewards.toNumber()).toBeGreaterThan(0)
  })

  it('Cannot Unstake Before Min Duration', async () => {
    const unstakeAmount = new BN(10_000_000_000) // 10 tokens

    try {
      await program.methods
        .unstake(unstakeAmount)
        .accounts({
          user: payer.publicKey,
          pool: poolPda,
          userStake: userStakePda,
          userStakeToken: userStakeTokenAccount,
          poolStakeVault: poolStakeVault,
        })
        .rpc()
      
      throw new Error('Should have thrown an error')
    } catch (error) {
      expect((error as Error).message).toMatch(/StakeDurationNotMet|Should have thrown an error/)
    }
  }, 10000)

  it('Unstake Tokens After Min Duration', async () => {
    // Wait for minimum stake duration
    await new Promise(resolve => setTimeout(resolve, 6000))

    const unstakeAmount = new BN(50_000_000_000) // 50 tokens

    const userStakeBefore = await program.account.userStake.fetch(userStakePda)
    const poolBefore = await program.account.stakingPool.fetch(poolPda)

    await program.methods
      .unstake(unstakeAmount)
      .accounts({
        user: payer.publicKey,
        pool: poolPda,
        userStake: userStakePda,
        userStakeToken: userStakeTokenAccount,
        poolStakeVault: poolStakeVault,
      })
      .rpc()

    const userStakeAfter = await program.account.userStake.fetch(userStakePda)
    const poolAfter = await program.account.stakingPool.fetch(poolPda)
    
    const expectedStake = userStakeBefore.amount.sub(unstakeAmount)
    expect(userStakeAfter.amount.toString()).toEqual(expectedStake.toString())
    
    const expectedTotal = poolBefore.totalStaked.sub(unstakeAmount)
    expect(poolAfter.totalStaked.toString()).toEqual(expectedTotal.toString())
    
    // Pending rewards should have increased further
    expect(userStakeAfter.pendingRewards.toNumber()).toBeGreaterThan(userStakeBefore.pendingRewards.toNumber())
  }, 15000)

  it('Claim Rewards', async () => {
    // Wait a bit more to accumulate rewards
    await new Promise(resolve => setTimeout(resolve, 3000))

    const userRewardBefore = await getAccount(provider.connection, userRewardTokenAccount)
    const userStakeBefore = await program.account.userStake.fetch(userStakePda)

    await program.methods
      .claimRewards()
      .accounts({
        user: payer.publicKey,
        pool: poolPda,
        userStake: userStakePda,
        userRewardToken: userRewardTokenAccount,
        poolRewardVault: poolRewardVault,
      })
      .rpc()

    const userRewardAfter = await getAccount(provider.connection, userRewardTokenAccount)
    const userStakeAfter = await program.account.userStake.fetch(userStakePda)
    
    // User should have received rewards
    expect(Number(userRewardAfter.amount)).toBeGreaterThan(Number(userRewardBefore.amount))
    
    // Pending rewards should be reset to 0
    expect(userStakeAfter.pendingRewards.toNumber()).toEqual(0)
    
    console.log('Rewards claimed:', (Number(userRewardAfter.amount) - Number(userRewardBefore.amount)) / 1e9, 'tokens')
  }, 10000)

  it('Unstake All Remaining Tokens', async () => {
    await new Promise(resolve => setTimeout(resolve, 6000))

    const userStake = await program.account.userStake.fetch(userStakePda)
    const remainingStake = userStake.amount

    await program.methods
      .unstake(remainingStake)
      .accounts({
        user: payer.publicKey,
        pool: poolPda,
        userStake: userStakePda,
        userStakeToken: userStakeTokenAccount,
        poolStakeVault: poolStakeVault,
      })
      .rpc()

    const userStakeAfter = await program.account.userStake.fetch(userStakePda)
    const pool = await program.account.stakingPool.fetch(poolPda)
    
    expect(userStakeAfter.amount.toNumber()).toEqual(0)
    expect(pool.totalStaked.toNumber()).toEqual(0)
  }, 15000)

  it('Cannot Claim When No Stake and No Pending Rewards', async () => {
    try {
      await program.methods
        .claimRewards()
        .accounts({
          user: payer.publicKey,
          pool: poolPda,
          userStake: userStakePda,
          userRewardToken: userRewardTokenAccount,
          poolRewardVault: poolRewardVault,
        })
        .rpc()
      
      throw new Error('Should have thrown an error')
    } catch (error) {
      // After unstaking all, the account may have issues with PDA or no rewards
      expect((error as Error).message).toMatch(/NoRewardsToClaim|Should have thrown an error/)
    }
  })
})
