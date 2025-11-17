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
    actix_web::{App, HttpResponse, HttpServer, get, web},
    anyhow::{Context, Result},
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
    solana_rpc_client::nonblocking::rpc_client::RpcClient,
    solana_signer::{Signer, signers::Signers},
    solana_transaction::Transaction,
    spl_stake_pool::state::AccountType as SplAccountType,
    std::{str::FromStr, sync::Arc},
    tokio::time::{Duration, interval, sleep},
    tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt},
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
    tracing_subscriber::registry()
        .with(EnvFilter::from_env("RUST_LOG"))
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_thread_names(true)
                .with_line_number(true),
        )
        .init();

    let config = Arc::new(
        StakePoolConfig::get_config()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?,
    );
    let worker_config = config.clone();
    let port = config.port;
    tracing::info!("Stake pool starting on port: {}", port);

    tokio::spawn(async move {
        let mut ticker = interval(tokio::time::Duration::from_secs(30 * 60));
        loop {
            ticker.tick().await;
            if let Err(err) = set_config_and_update((*worker_config).clone()).await {
                tracing::error!("ConfigUpdate Worker:- Error: {:#?}", err);
            }
        }
    });

    HttpServer::new(move || {
        App::new()
            .wrap(
                Cors::default()
                    .allow_any_origin()
                    .allowed_methods(vec!["GET"]),
            )
            .app_data(web::Data::new(config.clone()))
            .service(get_validators)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}

#[get("/validators")]
async fn get_validators(config: web::Data<StakePoolConfig>) -> HttpResponse {
    let result = tokio::task::spawn(async move {
        if config.stake_pool_address.is_empty() {
            return Err("No stake pool addresses configured");
        }
        let rpc_client =
            RpcClient::new_with_commitment(config.rpc_url.clone(), CommitmentConfig::confirmed());
        let stake_pool_pubkey = Pubkey::from_str(&config.stake_pool_address[0])
            .map_err(|_| "Invalid stake pool address")?;

        let stake_pool = get_stake_pool(&rpc_client, &stake_pool_pubkey)
            .await
            .map_err(|_| "Failed to fetch stake pool")?;
        let validator_list = get_validator_list(&rpc_client, &stake_pool.validator_list)
            .await
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

async fn get_epoch_info(client: &RpcClient) -> Result<EpochInfo> {
    let epoch_info = client
        .get_epoch_info()
        .await
        .context("Failed to fetch epoch info from blockchain\n")?;

    Ok(epoch_info)
}

async fn set_config_and_update(config: StakePoolConfig) -> Result<()> {
    let fee_payer = Keypair::from_base58_string(&config.fee_payer_private_key);
    let channel_id = config.slack_channel_id;
    let stake_pool_addresses = config.stake_pool_address.clone();
    let rpc_client = RpcClient::new_with_commitment(config.rpc_url, CommitmentConfig::confirmed());

    let fee_payer_box: Box<dyn Signer + Send + Sync + 'static> = Box::new(fee_payer);

    let config = Config {
        rpc_client: rpc_client,
        stake_pool_program_id: Pubkey::from_str("SPoo1Ku8WFXoNDMHPsrGSTSG1Y47rzgn41SLUNakuHy")?,
        fee_payer: fee_payer_box,
        dry_run: false,
        no_update: false,
        compute_unit_limit: ComputeUnitLimit::Static(250_000),
        compute_unit_price: None,
    };

    tracing::info!("Thread is awake, checking if epoch changed...");

    for stake_pool_address_str in &stake_pool_addresses {
        let stake_pool_pubkey = Pubkey::from_str(stake_pool_address_str)?;

        let stake_pool = get_stake_pool(&config.rpc_client, &stake_pool_pubkey).await?;
        let epoch_info = match get_epoch_info(&config.rpc_client).await {
            Ok(info) => info,
            Err(err) => {
                tracing::error!("Failed with error: {:#?}", err);
                slack_notification::send::send_message(
                    &channel_id,
                    "Rpc is failing to get the latest epoch info. Retrying again in 5 minutes",
                )
                .await
                .context("Failed to send message on slack about rpc failure")?;
                return Ok(());
            }
        };

        if stake_pool.last_update_epoch == epoch_info.epoch {
            tracing::info!(
                "Epoch has not changed for stake pool {}, skipping the update...",
                stake_pool_address_str
            );
            continue;
        }

        tracing::info!(
            "Epoch changed, executing the update for stake pool {}...",
            stake_pool_address_str
        );

        slack_notification::send::send_message(
            &channel_id,
            &format!(
                "Epoch changed, executing update for stake pool {} for epoch {}",
                stake_pool_address_str, epoch_info.epoch
            ),
        )
        .await
        .context("Failed to send slack message about triggering rewards")?;

        if let Err(err) = command_update(&config, &stake_pool_pubkey, true, false, false).await {
            tracing::error!(
                "Failed to update stake pool {}. Failed with error: {:#?}",
                stake_pool_address_str,
                err
            );
            if let Err(err) = slack_notification::send::send_message(
                &channel_id,
                &format!(
                    "Failed to run command to update stake pool {}",
                    stake_pool_address_str
                ),
            )
            .await
            {
                tracing::error!(
                    "Failed to send slack message about command update.\nError {}:-",
                    err
                );
            }
        }
    }
    Ok(())
}

async fn get_latest_blockhash(client: &RpcClient) -> Result<Hash> {
    Ok(client
        .get_latest_blockhash_with_commitment(CommitmentConfig::confirmed())
        .await?
        .0)
}

async fn checked_transaction_with_signers<T: Signers>(
    config: &Config,
    instructions: &[Instruction],
    signers: &T,
) -> Result<Transaction> {
    let tx = checked_transaction_with_signers_and_additional_fee(config, instructions, signers, 0)
        .await?;
    Ok(tx)
}

async fn check_fee_payer_balance(config: &Config, required_balance: u64) -> Result<()> {
    let balance = config
        .rpc_client
        .get_balance(&config.fee_payer.pubkey())
        .await?;
    if balance < required_balance {
        Err(anyhow::anyhow!(
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

async fn checked_transaction_with_signers_and_additional_fee<T: Signers>(
    config: &Config,
    instructions: &[Instruction],
    signers: &T,
    additional_fee: u64,
) -> Result<Transaction> {
    let recent_blockhash = get_latest_blockhash(&config.rpc_client)
        .await
        .context("Failed to get latest blockhash")?;
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
            )
            .await?
        }
    }

    let message = Message::new_with_blockhash(
        &instructions,
        Some(&config.fee_payer.pubkey()),
        &recent_blockhash,
    );

    let required_fee = config
        .rpc_client
        .get_fee_for_message(&message)
        .await
        .context("Failed to fetch fee for transaction message")?;

    check_fee_payer_balance(config, additional_fee.saturating_add(required_fee)).await?;

    let transaction = Transaction::new(signers, message, recent_blockhash);

    Ok(transaction)
}

async fn send_transaction(config: &Config, transaction: Transaction) -> Result<()> {
    if config.dry_run {
        let result = config
            .rpc_client
            .simulate_transaction(&transaction)
            .await
            .map_err(|err| {
                tracing::error!("Simulation failed: {:?}", err);
                err
            })?;
        tracing::info!("Simulate result: {:?}", result);
    } else {
        let signature = config
            .rpc_client
            .send_and_confirm_transaction_with_spinner(&transaction)
            .await
            .with_context(|| "Failed to send and confirm transaction with spinner")?;
        tracing::info!("Signature: {}", signature);
    }
    Ok(())
}

async fn send_transaction_no_wait(config: &Config, transaction: Transaction) -> Result<()> {
    if config.dry_run {
        let result = config
            .rpc_client
            .simulate_transaction(&transaction)
            .await
            .map_err(|err| {
                tracing::error!("Simulation failed: {:?}", err);
                err
            })?;
        tracing::info!("Simulate result: {:?}", result);
    } else {
        let signature = config
            .rpc_client
            .send_transaction(&transaction)
            .await
            .with_context(|| "Failed to send transaction (no wait)")?;
        tracing::info!("Signature: {}", signature);
    }
    Ok(())
}

async fn command_update(
    config: &Config,
    stake_pool_address: &Pubkey,
    force: bool,
    no_merge: bool,
    stale_only: bool,
) -> Result<()> {
    if config.no_update {
        tracing::info!("Update requested, but --no-update flag specified, so doing nothing");
        return Ok(());
    }
    let stake_pool = get_stake_pool(&config.rpc_client, stake_pool_address).await?;
    let epoch_info = get_epoch_info(&config.rpc_client).await?;

    if stake_pool.last_update_epoch == epoch_info.epoch {
        if force {
            tracing::info!("Update not required, but --force flag specified, so doing it anyway");
        } else {
            tracing::info!("Update not required");
            return Ok(());
        }
    }

    let validator_list = get_validator_list(&config.rpc_client, &stake_pool.validator_list).await?;

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
            )
            .await?;
            send_transaction_no_wait(config, transaction).await?;
            // to prevent rpc timeout
            sleep(Duration::from_secs(30)).await;
        }

        // wait on the last one
        let transaction = checked_transaction_with_signers(
            config,
            &last_instruction,
            &[config.fee_payer.as_ref()],
        )
        .await?;
        send_transaction(config, transaction).await?;
    }
    let transaction = checked_transaction_with_signers(
        config,
        &final_instructions,
        &[config.fee_payer.as_ref()],
    )
    .await
    .with_context(
        || "Failed to create checked transaction with signers for final stake pool instructions",
    )?;
    send_transaction(config, transaction).await?;

    Ok(())
}
