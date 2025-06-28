#![allow(clippy::arithmetic_side_effects)]
mod client;
mod config;
mod utils;

use {
    crate::{
        client::*,
        utils::types::{
            AccountType, PodStakeStatus, PodU32, PodU64, ValidatorList, ValidatorListHeader,
            ValidatorStakeInfo,
        },
    },
    actix_cors::Cors,
    actix_web::{App, HttpResponse, HttpServer, get},
    config::StakePoolConfig,
    dotenv::dotenv,
    solana_commitment_config::CommitmentConfig,
    solana_epoch_info::EpochInfo,
    solana_hash::Hash,
    solana_instruction::Instruction,
    solana_keypair::Keypair,
    solana_message::Message,
    solana_native_token::{self, Sol},
    solana_pubkey::Pubkey,
    solana_rpc_client::rpc_client::RpcClient,
    solana_signer::{Signer, signers::Signers},
    solana_transaction::Transaction,
    spl_stake_pool::state::AccountType as SplAccountType,
    std::str::FromStr,
    tokio::time::interval,
    utils::compute_budget::ComputeBudgetInstruction,
};

#[allow(dead_code)]
enum ComputeUnitLimit {
    Default,
    Static(u32),
    Simulated,
}

pub(crate) struct Config {
    stake_pool_program_id: Pubkey,
    rpc_client: RpcClient,
    fee_payer: Box<dyn Signer + Send + Sync + 'static>,
    dry_run: bool,
    no_update: bool,
    compute_unit_price: Option<u64>,
    compute_unit_limit: ComputeUnitLimit,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> std::io::Result<()> {
    dotenv().ok();

    let config = StakePoolConfig::get_config();

    // Start the background task that runs every 5 minutes
    tokio::spawn(async {
        let mut ticker = interval(tokio::time::Duration::from_secs(300)); // 5 minutes
        loop {
            ticker.tick().await;
            set_config_and_update().await;
        }
    });

    println!("Stake pool starting on port: {}", config.port);

    HttpServer::new(|| {
        App::new()
            .wrap(
                Cors::default()
                    .allow_any_origin()
                    .allowed_methods(vec!["GET"]),
            )
            .service(get_validators)
    })
    .bind(("0.0.0.0", config.port))?
    .run()
    .await
}

type CommandResult = Result<(), Error>;

#[get("/validators")]
async fn get_validators() -> HttpResponse {
    let result = tokio::task::spawn_blocking(move || {
        let rpc_url = StakePoolConfig::get_config().rpc_url;
        let stake_pool_address = StakePoolConfig::get_config().stake_pool_address;

        let rpc_client = RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());
        let stake_pool_pubkey = Pubkey::from_str(&stake_pool_address).unwrap();

        let stake_pool = get_stake_pool(&rpc_client, &stake_pool_pubkey)
            .map_err(|_| "Failed to fetch stake pool")?;
        let validator_list = get_validator_list(&rpc_client, &stake_pool.validator_list)
            .map_err(|_| "Failed to fetch validator list")?;

        let serialized_validator_list = ValidatorList {
            header: ValidatorListHeader {
                account_type: match validator_list.header.account_type {
                    SplAccountType::StakePool => AccountType::StakePool,
                    SplAccountType::Uninitialized => AccountType::Uninitialized,
                    SplAccountType::ValidatorList => AccountType::ValidatorList,
                },
                max_validators: validator_list.header.max_validators,
            },
            validators: validator_list
                .validators
                .into_iter()
                .map(|x| ValidatorStakeInfo {
                    active_stake_lamports: PodU64(
                        u64::from_le_bytes(x.active_stake_lamports.0).to_le_bytes(),
                    ),
                    transient_stake_lamports: PodU64(
                        u64::from_le_bytes(x.transient_stake_lamports.0).to_le_bytes(),
                    ),
                    last_update_epoch: PodU64(
                        u64::from_le_bytes(x.last_update_epoch.0).to_le_bytes(),
                    ),
                    transient_seed_suffix: PodU64(
                        u64::from_le_bytes(x.transient_seed_suffix.0).to_le_bytes(),
                    ),
                    unused: PodU32(u32::from_le_bytes(x.unused.0).to_le_bytes()),
                    validator_seed_suffix: PodU32(
                        u32::from_le_bytes(x.validator_seed_suffix.0).to_le_bytes(),
                    ),
                    status: PodStakeStatus(unsafe {
                        std::ptr::read(&x.status as *const _ as *const u8)
                    }),
                    vote_account_address: x.vote_account_address,
                })
                .collect(),
        };

        Ok::<_, &str>(serialized_validator_list)
    })
    .await;

    match result {
        Ok(Ok(validators)) => HttpResponse::Ok().json(validators),
        Ok(Err(msg)) => HttpResponse::InternalServerError().body(msg),
        Err(_) => HttpResponse::InternalServerError().body("Internal panic occurred"),
    }
}

async fn get_epoch_info_with_backoff(
    client: &RpcClient,
    retries: u8,
) -> Result<EpochInfo, Box<dyn std::error::Error + Send + Sync>> {
    let delay = 500; //500ms
    let mut attempts = 0;

    while attempts < retries {
        match client.get_epoch_info() {
            Ok(info) => return Ok(info),
            Err(err) => {
                eprintln!(
                    "Attempt {} failed to get current epoch. Failed with reason: {:?}",
                    attempts + 1,
                    err
                );
                let jitter = rand::random_range(0..100); //upto 100ms
                tokio::time::sleep(tokio::time::Duration::from_millis(delay + jitter)).await;
                attempts += 1;
            }
        }
    }

    Err("Exceeded max retries for get_epoch_info".into())
}

async fn set_config_and_update() {
    let stake_pool_address = StakePoolConfig::get_config().stake_pool_address;
    let fee_payer_pvt_key = StakePoolConfig::get_config().fee_payer_private_key;
    let fee_payer = Keypair::from_base58_string(&fee_payer_pvt_key);

    let stake_pool_pubkey = Pubkey::from_str(&stake_pool_address).unwrap();

    let channel_id = StakePoolConfig::get_config().slack_channel_id;

    let rpc_url = StakePoolConfig::get_config().rpc_url;
    let rpc_client = RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());

    let fee_payer_box: Box<dyn Signer + Send + Sync + 'static> = Box::new(fee_payer);

    let config = Config {
        rpc_client: rpc_client,
        stake_pool_program_id: Pubkey::from_str("SPoo1Ku8WFXoNDMHPsrGSTSG1Y47rzgn41SLUNakuHy")
            .unwrap(),
        fee_payer: fee_payer_box,
        dry_run: false,
        no_update: false,
        compute_unit_limit: ComputeUnitLimit::Default,
        compute_unit_price: None,
    };

    println!("thread is awake, checking if epoch changed...");

    let stake_pool = match get_stake_pool(&config.rpc_client, &stake_pool_pubkey) {
        Ok(r) => r,
        Err(e) => {
            println!("Error: fail to get stake pool");
            return;
        }
    };

    let epoch_info = match get_epoch_info_with_backoff(&config.rpc_client, 5).await {
        Ok(info) => {
            println!("Epoch info is: {:?}", info);
            info
        }
        Err(err) => {
            println!("Failed with error: {:#?}", err);
            match slack_notification::send::send_message(
                &channel_id,
                "Rpc is failing to get the latest epoch info. Retrying again in 5 minutes",
            )
            .await
            {
                Ok(_) => {}
                Err(err) => {
                    eprintln!(
                        "Failed to send message on slack about rpc failure. Failed with reason: {:#?}",
                        err
                    );
                }
            };
            return;
        }
    };

    if stake_pool.last_update_epoch == epoch_info.epoch {
        println!("Epoch has not changed, skipping the update...");
        return;
    }

    println!("Epoch changed, executing the update...");

    if let Ok(response) = slack_notification::send::send_message(
        &channel_id,
        &format!(
            "Epoch changed, executing update for Dynosol for epoch {}",
            epoch_info.epoch
        ),
    )
    .await
    {
        println!("Slack api response: {:#?}", response); //sample message
    } else {
        eprintln!("Failed to send slack message about triggering rewards");
    }

    match command_update(&config, &stake_pool_pubkey, true, false, false).await {
        Ok(_) => {}
        Err(err) => {
            eprintln!("Failed to update DynoSol. Failed with error: {:#?}", err);
            if let Ok(response) = slack_notification::send::send_message(
                &channel_id,
                "Failed to run command to update Dyno Sol",
            )
            .await
            {
                println!("Slack api response: {:#?}", response);
            } else {
                eprintln!("Failed to send slack message about command update");
            }
        }
    }
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

async fn command_update(
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
