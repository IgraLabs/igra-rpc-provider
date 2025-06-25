use borsh::{BorshDeserialize, BorshSerialize};
use kaspa_addresses::Address;
use kaspa_bip32::DerivationPath;
use kaspa_consensus_core::sign::Signed;
use std::collections::HashSet;
use std::error::Error;
use thiserror::Error;

pub type WalletResult<T> = Result<T, KaspaWalletError>;

/// A transaction that can be signed by the wallet daemon
/// This matches the structure from kaswallet/common/src/model.rs
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct WalletSignableTransaction {
    pub transaction: Signed,
    pub derivation_paths: HashSet<DerivationPath>,
    pub address_by_input_index: Vec<WalletAddress>,
    pub address_by_output_index: Vec<Address>,
}

#[derive(Debug, Error, Clone)]
pub enum KaspaWalletError {
    #[error("{0}")]
    UserInputError(String),
    #[error("{0}")]
    InternalServerError(String),
}

pub trait ResultExt<T> {
    fn to_wallet_result_internal(self) -> WalletResult<T>;
    fn to_wallet_result_user_input(self) -> WalletResult<T>;
}

impl<T, E> ResultExt<T> for Result<T, E>
where
    E: Error + Send + Sync,
{
    fn to_wallet_result_internal(self) -> WalletResult<T> {
        self.map_err(|e| KaspaWalletError::InternalServerError(e.to_string()))
    }

    fn to_wallet_result_user_input(self) -> WalletResult<T> {
        self.map_err(|e| KaspaWalletError::UserInputError(e.to_string()))
    }
}

/// Wallet address structure matching kaswallet
#[derive(Clone, Debug, Hash, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct WalletAddress {
    pub index: u32,
    pub cosigner_index: u16,
    pub keychain: Keychain,
}

/// Keychain enum matching kaswallet
#[derive(Clone, Debug, Hash, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
pub enum Keychain {
    External = 0,
    Internal = 1,
}
