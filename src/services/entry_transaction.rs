//! Entry Transaction Service
//!
//! Handles the logic for creating and processing Entry Transactions that bridge
//! KAS from L1 (KASPA) to L2 (IGRA Execution Layer).

use crate::{
    clients::wallet_caller::{TransactionParams, WalletCaller, WalletCallerError},
    config::{lane::LaneMode, AppConfig},
    services::{
        lane::LaneEnforcement,
        mining::TransactionMiner,
        transaction::{serialize_payload, VERSION},
    },
    types::rpc::{IgraPayload, TxTypeId},
};
use kaspa_addresses::Address;
use kaspa_consensus_core::constants::SOMPI_PER_KASPA;
use kaspa_wallet_core::utils::try_kaspa_str_to_sompi;
use thiserror::Error;
use tracing::{debug, info, warn};

// Constants
pub const L2_DATA_SIZE: usize = 28;
pub const ETHEREUM_ADDRESS_SIZE: usize = 20;
pub const MAX_KAS_EMISSION: u64 = 28_600_000_000; // Total KAS emission rounded up
pub const MAX_REASONABLE_SOMPI: u64 = MAX_KAS_EMISSION * SOMPI_PER_KASPA;
pub const MIN_ENTRY_AMOUNT_SOMPI: u64 = SOMPI_PER_KASPA; // 1 KAS minimum

/// Errors specific to Entry Transaction processing
#[derive(Debug, Error)]
pub enum EntryTransactionError {
    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Wallet error: {0}")]
    Wallet(Box<WalletCallerError>),

    #[error("Serialization error: {0}")]
    Serialization(String),
}

impl From<WalletCallerError> for EntryTransactionError {
    fn from(err: WalletCallerError) -> Self {
        EntryTransactionError::Wallet(Box::new(err))
    }
}

/// Validated arguments for Entry Transaction processing
#[derive(Debug, Clone)]
pub struct EntryTransactionRequest {
    pub recipient: Address,
    pub amount_sompi: u64,
    pub l2_address: [u8; ETHEREUM_ADDRESS_SIZE],
}

/// L2Data for Entry transactions containing address and amount
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L2Data {
    data: [u8; L2_DATA_SIZE],
}

impl L2Data {
    /// Creates new L2Data from Ethereum address and amount
    pub fn new(ethereum_address: [u8; ETHEREUM_ADDRESS_SIZE], amount_sompi: u64) -> Self {
        let mut data = [0u8; L2_DATA_SIZE];

        // First 20 bytes: Ethereum address
        data[0..ETHEREUM_ADDRESS_SIZE].copy_from_slice(&ethereum_address);

        // Last 8 bytes: Amount in little-endian
        data[ETHEREUM_ADDRESS_SIZE..].copy_from_slice(&amount_sompi.to_le_bytes());

        debug!(
            "L2Data created: address=0x{}, amount={} SOMPI",
            hex::encode(ethereum_address),
            amount_sompi
        );

        Self { data }
    }

    pub fn as_bytes(&self) -> &[u8; L2_DATA_SIZE] {
        &self.data
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.data.to_vec()
    }

    pub fn ethereum_address(&self) -> [u8; ETHEREUM_ADDRESS_SIZE] {
        let mut addr = [0u8; ETHEREUM_ADDRESS_SIZE];
        addr.copy_from_slice(&self.data[0..ETHEREUM_ADDRESS_SIZE]);
        addr
    }

    pub fn amount_sompi(&self) -> u64 {
        let mut amount_bytes = [0u8; 8];
        amount_bytes.copy_from_slice(&self.data[ETHEREUM_ADDRESS_SIZE..]);
        u64::from_le_bytes(amount_bytes)
    }
}

/// Service for processing Entry Transactions
pub struct EntryTransactionService {
    wallet_caller: WalletCaller,
    transaction_miner: TransactionMiner,
    retry_config: crate::config::RetryConfig,
}

impl EntryTransactionService {
    /// Creates a new service with loaded configuration and initialized clients
    pub async fn new() -> Result<Self, EntryTransactionError> {
        let config = AppConfig::load().map_err(|e| EntryTransactionError::Config(e.to_string()))?;

        // Entry txs carry a real IGRA payload (L2 address + amount), so the
        // same KIP-21 invariants the RPC `eth_sendRawTransaction` path
        // enforces must apply here. The CLI honours the operator-level
        // `LANE_ENFORCEMENT_DISABLED` opt-out only — the only paths that
        // bypass enforcement are pre-stage UTXO consolidation txs (filtered
        // inside `WalletCaller::complete_transaction_flow`).
        let lane_enforcement = match config
            .lane
            .resolve()
            .map_err(EntryTransactionError::Config)?
        {
            LaneMode::Enforced(id) => Some(
                LaneEnforcement::new(id, config.mining.tx_id_prefix.clone())
                    .map_err(EntryTransactionError::Config)?,
            ),
            LaneMode::Disabled => {
                warn!(
                    target: "lane_enforcement",
                    "EntryTransactionService: KIP-21 lane enforcement DISABLED via \
                     LANE_ENFORCEMENT_DISABLED=true — DO NOT use in production"
                );
                None
            }
        };

        let wallet_caller = WalletCaller::new(config.wallet.clone(), lane_enforcement).await?;
        let transaction_miner = TransactionMiner::new(config.mining.clone());
        let retry_config = config.retry.clone();

        info!("EntryTransactionService initialized successfully");

        Ok(Self {
            wallet_caller,
            transaction_miner,
            retry_config,
        })
    }

    /// Processes an entry transaction with the given request
    pub async fn process_transaction(
        &self,
        request: &EntryTransactionRequest,
    ) -> Result<String, EntryTransactionError> {
        // Create L2Data and payload
        let l2_data = L2Data::new(request.l2_address, request.amount_sompi);
        let payload = self.create_payload(&l2_data);

        // Serialize payload
        let serialized = serialize_payload(&payload)
            .map_err(|e| EntryTransactionError::Serialization(e.to_string()))?;

        // Create transaction parameters for Entry transaction
        let transaction_params = TransactionParams::entry_transaction(
            request.recipient.to_string(),
            request.amount_sompi,
            serialized,
        );

        // Mine and send transaction with retry support
        let result = self
            .wallet_caller
            .mine_and_send_transaction_with_retry(
                transaction_params,
                &self.transaction_miner,
                &self.retry_config,
            )
            .await?;

        info!("Entry transaction processed successfully: {}", result);
        Ok(result)
    }

    fn create_payload(&self, l2_data: &L2Data) -> IgraPayload {
        IgraPayload {
            version: VERSION,
            tx_type_id: TxTypeId::Entry,
            l2_data: l2_data.to_vec(),
            nonce: [0u8; 4],
        }
    }
}

/// Validation utilities for Entry Transaction inputs
pub mod validation {
    use super::*;

    /// Converts KAS amount string to SOMPI using kaspa-wallet-core
    pub fn validate_and_convert_amount(amount_str: &str) -> Result<u64, EntryTransactionError> {
        let trimmed = amount_str.trim();

        if trimmed.is_empty() {
            return Err(EntryTransactionError::Validation(
                "Amount cannot be empty".to_string(),
            ));
        }

        // Use kaspa-wallet-core's safe conversion function
        let amount_sompi = try_kaspa_str_to_sompi(trimmed)
            .map_err(|e| {
                EntryTransactionError::Validation(format!(
                    "Invalid amount '{trimmed}': {e}. Expected a valid KAS amount (e.g., 1.5)"
                ))
            })?
            .ok_or_else(|| {
                EntryTransactionError::Validation(format!(
                    "Invalid amount '{trimmed}'. Expected a valid KAS amount (e.g., 1.5)"
                ))
            })?;

        if amount_sompi < MIN_ENTRY_AMOUNT_SOMPI {
            return Err(EntryTransactionError::Validation(
                "Amount must be at least 1 KAS".to_string(),
            ));
        }

        if amount_sompi > MAX_REASONABLE_SOMPI {
            return Err(EntryTransactionError::Validation(format!(
                "Amount {amount_sompi} SOMPI exceeds maximum possible KAS emission ({MAX_KAS_EMISSION} KAS)"
            )));
        }

        Ok(amount_sompi)
    }

    /// Validates a Kaspa address string and returns the parsed Address
    pub fn validate_kaspa_address(address_str: &str) -> Result<Address, EntryTransactionError> {
        let trimmed = address_str.trim();

        if trimmed.is_empty() {
            return Err(EntryTransactionError::Validation(
                "Kaspa address cannot be empty".to_string(),
            ));
        }

        // TODO: Replace `catch_unwind` once `kaspa-addresses::Address::constructor` returns a `Result`.
        // The current implementation panics on invalid input, which is not idiomatic.
        // See issue: https://github.com/IgraLabs/rusty-kaspa/issues/XX
        std::panic::catch_unwind(|| Address::constructor(trimmed)).map_err(|_| {
            EntryTransactionError::Validation(format!(
                "Invalid Kaspa address '{trimmed}'. Expected format: kaspa:qpam..."
            ))
        })
    }

    /// Validates and parses an Ethereum address string
    pub fn validate_and_parse_ethereum_address(
        address_str: &str,
    ) -> Result<[u8; ETHEREUM_ADDRESS_SIZE], EntryTransactionError> {
        let trimmed = address_str.trim();

        if trimmed.is_empty() {
            return Err(EntryTransactionError::Validation(
                "Ethereum address cannot be empty".to_string(),
            ));
        }

        let hex_str = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);

        if hex_str.len() != 40 {
            return Err(EntryTransactionError::Validation(format!(
                "Ethereum address must be exactly 40 hex characters, got {}",
                hex_str.len()
            )));
        }

        let mut address_bytes = [0u8; ETHEREUM_ADDRESS_SIZE];
        hex::decode_to_slice(hex_str, &mut address_bytes).map_err(|_| {
            EntryTransactionError::Validation(format!(
                "Invalid hex characters in Ethereum address '{hex_str}'"
            ))
        })?;

        Ok(address_bytes)
    }

    /// Validates and parses CLI arguments into an EntryTransactionRequest
    pub fn validate_and_parse_request(
        recipient: &str,
        amount: &str,
        l2_address: &str,
    ) -> Result<EntryTransactionRequest, EntryTransactionError> {
        let amount_sompi = validate_and_convert_amount(amount)?;
        let recipient = validate_kaspa_address(recipient)?;
        let l2_address = validate_and_parse_ethereum_address(l2_address)?;

        Ok(EntryTransactionRequest {
            recipient,
            amount_sompi,
            l2_address,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_amount_conversion() {
        // Test valid amounts using kaspa-wallet-core conversion
        assert_eq!(
            validation::validate_and_convert_amount("1")
                .expect("1 KAS should convert to 100M SOMPI"),
            100_000_000
        );
        assert_eq!(
            validation::validate_and_convert_amount("1.5")
                .expect("1.5 KAS should convert to 150M SOMPI"),
            150_000_000
        );

        // Test invalid amounts (below minimum)
        assert!(validation::validate_and_convert_amount("0.5").is_err());
        assert!(validation::validate_and_convert_amount("0.00000001").is_err());
        assert!(validation::validate_and_convert_amount("0").is_err());
        assert!(validation::validate_and_convert_amount("-1").is_err());
        assert!(validation::validate_and_convert_amount("invalid").is_err());
        assert!(validation::validate_and_convert_amount("").is_err());
    }

    #[test]
    fn test_validation_minimum_amount() {
        let result = validation::validate_and_parse_request(
            "kaspadev:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xxva9eetl",
            "0.5",
            "0x742d35Cc6634C0532925a3b8D0b16e5E3dd7b9c0",
        );
        assert!(result.is_err());

        let result = validation::validate_and_parse_request(
            "kaspadev:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xxva9eetl",
            "1",
            "0x742d35Cc6634C0532925a3b8D0b16e5E3dd7b9c0",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validation_zero_amount() {
        let result = validation::validate_and_parse_request(
            "kaspadev:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xxva9eetl",
            "0",
            "0x742d35Cc6634C0532925a3b8D0b16e5E3dd7b9c0",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_l2data_construction() {
        let address = [1u8; ETHEREUM_ADDRESS_SIZE];
        let amount = 100_000_000u64;

        let l2_data = L2Data::new(address, amount);

        assert_eq!(l2_data.as_bytes().len(), L2_DATA_SIZE);
        assert_eq!(l2_data.ethereum_address(), address);
        assert_eq!(l2_data.amount_sompi(), amount);
    }

    #[test]
    fn test_ethereum_address_validation() {
        let valid_address = "742d35Cc6634C0532925a3b8D0b16e5E3dd7b9c0";
        let result = validation::validate_and_parse_ethereum_address(valid_address);
        assert!(result.is_ok());

        let bytes = result.expect("Validation should succeed for valid Ethereum address");
        assert_eq!(bytes.len(), ETHEREUM_ADDRESS_SIZE);
    }
}
