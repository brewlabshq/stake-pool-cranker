use {
    crate::utils::compute_budget::ComputeBudgetInstruction,
    anyhow::{Context, Result},
    solana_hash::Hash,
    solana_instruction::Instruction,
    solana_message::Message,
    solana_program::borsh1::try_from_slice_unchecked,
    solana_pubkey::Pubkey,
    solana_rpc_client::nonblocking::rpc_client::RpcClient,
    solana_transaction::Transaction,
    spl_stake_pool::state::{StakePool, ValidatorList},
};

pub async fn get_stake_pool(
    rpc_client: &RpcClient,
    stake_pool_address: &Pubkey,
) -> Result<StakePool> {
    let account_data = rpc_client
        .get_account_data(stake_pool_address)
        .await
        .context(format!(
            "Failed to get account data for stake pool address: {stake_pool_address}"
        ))?;
    let stake_pool = try_from_slice_unchecked::<StakePool>(account_data.as_slice())
        .map_err(|err| anyhow::anyhow!("Invalid stake pool {}: {}", stake_pool_address, err))?;
    Ok(stake_pool)
}

pub async fn get_validator_list(
    rpc_client: &RpcClient,
    validator_list_address: &Pubkey,
) -> Result<ValidatorList> {
    let account_data = rpc_client
        .get_account_data(validator_list_address)
        .await
        .context(format!(
            "Failed to get account data for validator list address: {validator_list_address}"
        ))?;
    let validator_list = try_from_slice_unchecked::<ValidatorList>(account_data.as_slice())
        .map_err(|err| {
            anyhow::anyhow!("Invalid validator list {}: {}", validator_list_address, err)
        })?;
    Ok(validator_list)
}

/// Helper function to add a compute unit limit instruction to a given set
/// of instructions by simulating, and then propagating the result via context.
pub(crate) async fn add_compute_unit_limit_from_simulation(
    rpc_client: &RpcClient,
    instructions: &mut Vec<Instruction>,
    payer: &Pubkey,
    blockhash: &Hash,
) -> Result<()> {
    // add a max compute unit limit instruction for the simulation
    const MAX_COMPUTE_UNIT_LIMIT: u32 = 1_400_000;
    instructions.push(ComputeBudgetInstruction::set_compute_unit_limit(
        MAX_COMPUTE_UNIT_LIMIT,
    ));

    let transaction = Transaction::new_unsigned(Message::new_with_blockhash(
        instructions,
        Some(payer),
        blockhash,
    ));

    let simulation_result = rpc_client
        .simulate_transaction(&transaction)
        .await
        .context("Failed to simulate transaction for compute unit limit")?
        .value;

    let units_consumed = simulation_result
        .units_consumed
        .ok_or_else(|| anyhow::anyhow!("No units consumed on simulation"))?;
    // Overwrite the compute unit limit instruction with the actual units consumed
    let compute_unit_limit = u32::try_from(units_consumed)?;
    instructions
        .last_mut()
        .expect("Compute budget instruction was added earlier")
        .data = ComputeBudgetInstruction::set_compute_unit_limit(compute_unit_limit).data;
    Ok(())
}
