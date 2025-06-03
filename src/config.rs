use std::env;

use thiserror::Error;

#[derive(Default, Debug)]
pub struct StakePoolConfig {
    pub rpc_url: String,
    pub fee_payer_private_key: String,
    pub stake_pool_address: String
}

#[derive(Error, Debug)]
enum ConfigError {
    #[error("Error: Invalid RPC Url")]
    InavlidRpcURL,
    #[error("Error: Invalid Fee Payer")]
    InvalidFeePayerPrivateKey,
    #[error("Error: Invalid Stake Pool Address")]
    InvalidStakePoolAddress
}

impl StakePoolConfig {
    pub fn get_config() -> Self {
        let rpc_url = env::var("RPC_URL").unwrap_or_else(|_| ConfigError::InavlidRpcURL.to_string());

        let fee_payer_private_key = env::var("FEE_PAYER_PRIVATE_KEY").unwrap_or_else(|_| ConfigError::InvalidFeePayerPrivateKey.to_string());

        let stake_pool_address = env::var("STAKE_POOL_ADDRESS").unwrap_or_else(|_| ConfigError::InvalidStakePoolAddress.to_string());

        Self { rpc_url, fee_payer_private_key, stake_pool_address }
    }
}