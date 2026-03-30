use solana_sdk::pubkey::Pubkey;
use tracing::trace;

use crate::config::programs;
use crate::router::pool::{DetectedSwap, DexType};

/// Decodes raw transaction bytes to extract swap information.
///
/// This is intentionally kept minimal and fast — we only need to
/// identify the DEX program, the pool, and the token pair. Full
/// instruction decoding happens later during simulation.
pub struct SwapDecoder;

impl SwapDecoder {
    pub fn new() -> Self {
        Self
    }

    /// Attempt to decode a swap from raw transaction bytes.
    ///
    /// Returns None if the transaction doesn't contain a recognizable swap.
    pub fn decode_swap(&self, tx_data: &[u8]) -> Option<DetectedSwap> {
        // Deserialize the transaction to access instructions
        let tx: solana_sdk::transaction::Transaction =
            bincode::deserialize(tx_data).ok()?;

        let message = &tx.message;

        // Walk each instruction, check if it targets a known DEX program
        for ix in &message.instructions {
            let program_idx = ix.program_id_index as usize;
            if program_idx >= message.account_keys.len() {
                continue;
            }

            let program_id = &message.account_keys[program_idx];
            let dex_type = Self::identify_dex(program_id)?;

            // Decode pool address and token mints from instruction accounts.
            // Account layout varies by DEX — each has its own convention.
            let swap_info = match dex_type {
                DexType::RaydiumAmm => self.decode_raydium_amm(message, ix),
                DexType::RaydiumClmm => self.decode_raydium_clmm(message, ix),
                DexType::OrcaWhirlpool => self.decode_orca_whirlpool(message, ix),
                DexType::MeteoraDlmm => self.decode_meteora_dlmm(message, ix),
            };

            if let Some(info) = swap_info {
                return Some(DetectedSwap {
                    signature: bs58::encode(&tx.signatures[0]).into_string(),
                    dex_type,
                    pool_address: info.pool,
                    input_mint: info.input_mint,
                    output_mint: info.output_mint,
                    amount: info.amount,
                    observed_slot: 0, // filled by caller
                });
            }
        }

        None
    }

    /// Match a program ID to a known DEX type.
    fn identify_dex(program_id: &Pubkey) -> Option<DexType> {
        if *program_id == programs::raydium_amm() {
            Some(DexType::RaydiumAmm)
        } else if *program_id == programs::raydium_clmm() {
            Some(DexType::RaydiumClmm)
        } else if *program_id == programs::orca_whirlpool() {
            Some(DexType::OrcaWhirlpool)
        } else if *program_id == programs::meteora_dlmm() {
            Some(DexType::MeteoraDlmm)
        } else {
            None
        }
    }

    /// Decode Raydium AMM swap instruction.
    ///
    /// Raydium AMM swap account layout (swap instruction = discriminator 9):
    /// [0] token_program
    /// [1] amm_id (pool)
    /// [2] amm_authority
    /// [3] amm_open_orders
    /// [4] amm_target_orders (or pool_coin_token_account)
    /// [5] pool_coin_token_account
    /// [6] pool_pc_token_account
    /// [7] serum_program
    /// [8] serum_market
    /// [9] serum_bids
    /// [10] serum_asks
    /// [11] serum_event_queue
    /// [12] serum_coin_vault
    /// [13] serum_pc_vault
    /// [14] serum_vault_signer
    /// [15] user_source_token_account
    /// [16] user_dest_token_account
    /// [17] user_owner
    fn decode_raydium_amm(
        &self,
        message: &solana_sdk::message::Message,
        ix: &solana_sdk::instruction::CompiledInstruction,
    ) -> Option<SwapInfo> {
        if ix.accounts.len() < 18 {
            return None;
        }

        // Check instruction discriminator (swap = 9)
        if ix.data.first() != Some(&9) {
            return None;
        }

        let pool = message.account_keys.get(ix.accounts[1] as usize)?;

        // Decode amount from instruction data (u64 at offset 1)
        let amount = if ix.data.len() >= 9 {
            Some(u64::from_le_bytes(
                ix.data[1..9].try_into().ok()?,
            ))
        } else {
            None
        };

        Some(SwapInfo {
            pool: *pool,
            input_mint: Pubkey::default(), // resolved from account data
            output_mint: Pubkey::default(),
            amount,
        })
    }

    /// Decode Raydium CLMM swap instruction.
    fn decode_raydium_clmm(
        &self,
        message: &solana_sdk::message::Message,
        ix: &solana_sdk::instruction::CompiledInstruction,
    ) -> Option<SwapInfo> {
        // CLMM swap requires at least ~13 accounts
        if ix.accounts.len() < 13 {
            return None;
        }

        // Anchor discriminator for swap: first 8 bytes
        // We check length to ensure it's a valid instruction
        if ix.data.len() < 8 {
            return None;
        }

        let pool = message.account_keys.get(ix.accounts[2] as usize)?;

        Some(SwapInfo {
            pool: *pool,
            input_mint: Pubkey::default(),
            output_mint: Pubkey::default(),
            amount: None, // decoded from anchor data
        })
    }

    /// Decode Orca Whirlpool swap instruction.
    ///
    /// Whirlpool swap account layout:
    /// [0] token_program
    /// [1] token_authority
    /// [2] whirlpool (pool)
    /// [3] token_owner_account_a
    /// [4] token_vault_a
    /// [5] token_owner_account_b
    /// [6] token_vault_b
    /// [7] tick_array_0
    /// [8] tick_array_1
    /// [9] tick_array_2
    /// [10] oracle
    fn decode_orca_whirlpool(
        &self,
        message: &solana_sdk::message::Message,
        ix: &solana_sdk::instruction::CompiledInstruction,
    ) -> Option<SwapInfo> {
        if ix.accounts.len() < 11 {
            return None;
        }

        if ix.data.len() < 8 {
            return None;
        }

        let pool = message.account_keys.get(ix.accounts[2] as usize)?;

        Some(SwapInfo {
            pool: *pool,
            input_mint: Pubkey::default(),
            output_mint: Pubkey::default(),
            amount: None,
        })
    }

    /// Decode Meteora DLMM swap instruction.
    fn decode_meteora_dlmm(
        &self,
        message: &solana_sdk::message::Message,
        ix: &solana_sdk::instruction::CompiledInstruction,
    ) -> Option<SwapInfo> {
        if ix.accounts.len() < 10 {
            return None;
        }

        if ix.data.len() < 8 {
            return None;
        }

        // DLMM pool is typically at account index 0 or 1
        let pool = message.account_keys.get(ix.accounts[0] as usize)?;

        Some(SwapInfo {
            pool: *pool,
            input_mint: Pubkey::default(),
            output_mint: Pubkey::default(),
            amount: None,
        })
    }
}

/// Intermediate decoded swap info before creating DetectedSwap.
struct SwapInfo {
    pool: Pubkey,
    input_mint: Pubkey,
    output_mint: Pubkey,
    amount: Option<u64>,
}
