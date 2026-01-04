// Here we export some useful types and functions for interacting with the Anchor program.
import { AnchorProvider, Program } from '@coral-xyz/anchor'
import { Cluster, PublicKey } from '@solana/web3.js'
import StakingProgramIDL from '../target/idl/staking-program.json'
import type { StakingProgram } from '../target/types/staking-program'

// Re-export the generated IDL and type
export { StakingProgram, StakingProgramIDL }

// The programId is imported from the program IDL.
export const STAKING_PROGRAM_ID = new PublicKey(StakingProgramIDL.address)

// This is a helper function to get the StakingProgram Anchor program.
export function getStakingProgram(provider: AnchorProvider, address?: PublicKey): Program<StakingProgram> {
  return new Program({ ...StakingProgramIDL, address: address ? address.toBase58() : StakingProgramIDL.address } as StakingProgram, provider)
}

// This is a helper function to get the program ID for the StakingProgram program depending on the cluster.
export function getStakingProgramId(cluster: Cluster) {
  switch (cluster) {
    case 'devnet':
    case 'testnet':
      // This is the program ID for the StakingProgram program on devnet and testnet.
      return new PublicKey('StakeABCxyz123456789ABCDEFGHIJKLMNOPQR')
    case 'mainnet-beta':
    default:
      return STAKING_PROGRAM_ID
  }
}
