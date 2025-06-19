use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

#[derive(Default, Serialize, Deserialize)]
pub enum AccountType {
    /// If the account has not been initialized, the enum will be 0
    #[default]
    Uninitialized,
    /// Stake pool
    StakePool,
    /// Validator stake list
    ValidatorList,
}

#[derive(Serialize, Deserialize)]
pub struct PodU64(pub [u8; 8]);
#[derive(Serialize, Deserialize)]
pub struct PodU32(pub [u8; 4]);
#[derive(Serialize, Deserialize)]
pub struct PodStakeStatus(pub u8);

#[derive(Serialize, Deserialize)]
pub struct ValidatorListHeader {
    /// Account type, must be `ValidatorList` currently
    pub account_type: AccountType,

    /// Maximum allowable number of validators
    pub max_validators: u32,
}

#[derive(Serialize, Deserialize)]
pub struct ValidatorList {
    pub header: ValidatorListHeader,
    pub validators: Vec<ValidatorStakeInfo>,
}

#[derive(Serialize, Deserialize)]
pub struct ValidatorStakeInfo {
    /// Amount of lamports on the validator stake account, including rent
    ///
    /// Note that if `last_update_epoch` does not match the current epoch then
    /// this field may not be accurate
    pub active_stake_lamports: PodU64,

    /// Amount of transient stake delegated to this validator
    ///
    /// Note that if `last_update_epoch` does not match the current epoch then
    /// this field may not be accurate
    pub transient_stake_lamports: PodU64,

    /// Last epoch the active and transient stake lamports fields were updated
    pub last_update_epoch: PodU64,

    /// Transient account seed suffix, used to derive the transient stake
    /// account address
    pub transient_seed_suffix: PodU64,

    /// Unused space, initially meant to specify the end of seed suffixes
    pub unused: PodU32,

    /// Validator account seed suffix
    pub validator_seed_suffix: PodU32, // really `Option<NonZeroU32>` so 0 is `None`

    /// Status of the validator stake account
    pub status: PodStakeStatus,

    /// Validator vote account address
    pub vote_account_address: Pubkey,
}
