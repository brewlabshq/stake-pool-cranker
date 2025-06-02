#![allow(clippy::arithmetic_side_effects)]
mod client;
mod utils;
mod config;

use {
    crate::client::*, config::StakePoolConfig, 
    dotenv::dotenv, 
    solana_commitment_config::CommitmentConfig, 
    solana_hash::Hash, solana_instruction::Instruction, 
    solana_keypair::Keypair, solana_message::Message, 
    solana_native_token::{self, Sol}, solana_pubkey::Pubkey, 
    solana_rpc_client::rpc_client::RpcClient, 
    solana_signer::{signers::Signers, Signer}, 
    solana_transaction::Transaction, std::{str::FromStr}, 
    utils::compute_budget::ComputeBudgetInstruction

};

pub const STAKE_POOL_ADDRESS: &str = "DpooSqZRL3qCmiq82YyB4zWmLfH3iEqx2gy8f2B6zjru";

#[allow(dead_code)]
enum ComputeUnitLimit {
    Default,
    Static(u32),
    Simulated,
}

pub(crate) struct Config {
    stake_pool_program_id: Pubkey,
    rpc_client: RpcClient,
    fee_payer: Box<dyn Signer>,
    dry_run: bool,
    no_update: bool,
    compute_unit_price: Option<u64>,
    compute_unit_limit: ComputeUnitLimit,
}



#[tokio::main]
async fn main() -> Result<(), Error> {
    dotenv().ok();

    let rpc_url = StakePoolConfig::get_config().rpc_url;
    let fee_payer_pvt_key = StakePoolConfig::get_config().fee_payer_private_key;

    set_config_and_update(rpc_url, fee_payer_pvt_key).await;

    Ok(())
}

type CommandResult = Result<(), Error>;

async fn set_config_and_update(rpc_url: String, fee_payer_pvt_key: String) {

    tokio::spawn(async move {
        let fee_payer = Box::new(Keypair::from_base58_string(&fee_payer_pvt_key));
        let stake_pool_address = &Pubkey::from_str("").unwrap();
        
        let config = Config {
            rpc_client: RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed()),
            stake_pool_program_id: Pubkey::from_str("SPoo1Ku8WFXoNDMHPsrGSTSG1Y47rzgn41SLUNakuHy").unwrap(),
            fee_payer: fee_payer,
            dry_run: false,
            no_update: false,
            compute_unit_limit: ComputeUnitLimit::Default,
            compute_unit_price: None
        };
        
        println!("Executing the update");
        
        let _ = command_update(&config, stake_pool_address, true, false, false);
    });

}

fn get_latest_blockhash(client: &RpcClient) -> Result<Hash, Error> {
    Ok(client
        .get_latest_blockhash_with_commitment(CommitmentConfig::confirmed())?
        .0)
}

fn checked_transaction_with_signers<T: Signers>(
    config: &Config,
    instructions: &[Instruction],
    signers: &T,
) -> Result<Transaction, Error> {
    checked_transaction_with_signers_and_additional_fee(config, instructions, signers, 0)
}

fn check_fee_payer_balance(config: &Config, required_balance: u64) -> Result<(), Error> {
    let balance = config.rpc_client.get_balance(&config.fee_payer.pubkey())?;
    if balance < required_balance {
        Err(format!(
            "Fee payer, {}, has insufficient balance: {} required, {} available",
            config.fee_payer.pubkey(),
            Sol(required_balance),
            Sol(balance)
        )
        .into())
    } else {
        Ok(())
    }
}


fn checked_transaction_with_signers_and_additional_fee<T: Signers>(
    config: &Config,
    instructions: &[Instruction],
    signers: &T,
    additional_fee: u64,
) -> Result<Transaction, Error> {
    let recent_blockhash = get_latest_blockhash(&config.rpc_client)?;
    let mut instructions = instructions.to_vec();
    if let Some(compute_unit_price) = config.compute_unit_price {
        instructions.push(ComputeBudgetInstruction::set_compute_unit_price(
            compute_unit_price,
        ));
    }
    match config.compute_unit_limit {
        ComputeUnitLimit::Default => {}
        ComputeUnitLimit::Static(compute_unit_limit) => {
            instructions.push(ComputeBudgetInstruction::set_compute_unit_limit(
                compute_unit_limit,
            ));
        }
        ComputeUnitLimit::Simulated => {
            add_compute_unit_limit_from_simulation(
                &config.rpc_client,
                &mut instructions,
                &config.fee_payer.pubkey(),
                &recent_blockhash,
            )?;
        }
    }
    let message = Message::new_with_blockhash(
        &instructions,
        Some(&config.fee_payer.pubkey()),
        &recent_blockhash,
    );
    check_fee_payer_balance(
        config,
        additional_fee.saturating_add(config.rpc_client.get_fee_for_message(&message)?),
    )?;
    let transaction = Transaction::new(signers, message, recent_blockhash);
    Ok(transaction)
}

fn send_transaction(
    config: &Config,
    transaction: Transaction,
) -> solana_client::client_error::Result<()> {
    if config.dry_run {
        let result = config.rpc_client.simulate_transaction(&transaction)?;
        println!("Simulate result: {:?}", result);
    } else {
        let signature = config
            .rpc_client
            .send_and_confirm_transaction_with_spinner(&transaction)?;
        println!("Signature: {}", signature);
    }
    Ok(())
}

fn send_transaction_no_wait(
    config: &Config,
    transaction: Transaction,
) -> solana_client::client_error::Result<()> {
    if config.dry_run {
        let result = config.rpc_client.simulate_transaction(&transaction)?;
        println!("Simulate result: {:?}", result);
    } else {
        let signature = config.rpc_client.send_transaction(&transaction)?;
        println!("Signature: {}", signature);
    }
    Ok(())
}


fn command_update(
    config: &Config,
    stake_pool_address: &Pubkey,
    force: bool,
    no_merge: bool,
    stale_only: bool,
) -> CommandResult {
    if config.no_update {
        println!("Update requested, but --no-update flag specified, so doing nothing");
        return Ok(());
    }
    let stake_pool = get_stake_pool(&config.rpc_client, stake_pool_address)?;
    let epoch_info = config.rpc_client.get_epoch_info()?;

    if stake_pool.last_update_epoch == epoch_info.epoch {
        if force {
            println!("Update not required, but --force flag specified, so doing it anyway");
        } else {
            println!("Update not required");
            return Ok(());
        }
    }

    let validator_list = get_validator_list(&config.rpc_client, &stake_pool.validator_list)?;

    let (mut update_list_instructions, final_instructions) = if stale_only {
        spl_stake_pool::instruction::update_stale_stake_pool(
            &config.stake_pool_program_id,
            &stake_pool,
            &validator_list,
            stake_pool_address,
            no_merge,
            epoch_info.epoch,
        )
    } else {
        spl_stake_pool::instruction::update_stake_pool(
            &config.stake_pool_program_id,
            &stake_pool,
            &validator_list,
            stake_pool_address,
            no_merge,
        )
    };

    let update_list_instructions_len = update_list_instructions.len();
    if update_list_instructions_len > 0 {
        let last_instruction = update_list_instructions.split_off(update_list_instructions_len - 1);
        // send the first ones without waiting
        for instruction in update_list_instructions {
            let transaction = checked_transaction_with_signers(
                config,
                &[instruction],
                &[config.fee_payer.as_ref()],
            )?;
            send_transaction_no_wait(config, transaction)?;
        }

        // wait on the last one
        let transaction = checked_transaction_with_signers(
            config,
            &last_instruction,
            &[config.fee_payer.as_ref()],
        )?;
        send_transaction(config, transaction)?;
    }
    let transaction = checked_transaction_with_signers(
        config,
        &final_instructions,
        &[config.fee_payer.as_ref()],
    )?;
    send_transaction(config, transaction)?;

    Ok(())
}