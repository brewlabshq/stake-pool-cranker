use std::env;

use anyhow::{Context, Result};

#[allow(dead_code)]
#[derive(Default, Debug, Clone)]
pub struct StakePoolConfig {
    pub port: u16,
    pub rpc_url: String,
    pub fee_payer_private_key: String,
    pub stake_pool_address: Vec<String>,
    pub slack_token: String,
    pub slack_channel_id: String,
}

impl StakePoolConfig {
    pub fn get_config() -> Result<Self> {
        let port = match env::var("PORT") {
            Ok(port) => port.parse::<u16>()?,
            Err(_) => 8000,
        };

        let rpc_url = env::var("RPC_URL").context("RPC_URL is not set")?;

        let fee_payer_private_key =
            env::var("FEE_PAYER_PRIVATE_KEY").context("FEE_PAYER_PRIVATE_KEY is not set")?;

        let stake_pool_address_str =
            env::var("STAKE_POOL_ADDRESS").context("STAKE_POOL_ADDRESS is not set")?;
        let stake_pool_address: Vec<String> = stake_pool_address_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let slack_token = env::var("SLACK_TOKEN").context("SLACK_TOKEN is not set")?;

        let slack_channel_id =
            env::var("SLACK_CHANNEL_ID").context("SLACK_CHANNEL_ID is not set")?;

        Ok(Self {
            port,
            rpc_url,
            fee_payer_private_key,
            stake_pool_address,
            slack_token,
            slack_channel_id,
        })
    }
}
