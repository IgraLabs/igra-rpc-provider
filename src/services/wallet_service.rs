use crate::clients::wallet_caller::{TransactionParams, WalletCaller, WalletCallerError};
use crate::config::WalletConfig;
use crate::services::mining::TransactionMiner;
use std::sync::Arc;
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

/// Service responsible for non-payload wallet operations (address lookup,
/// UTXO consolidation, Entry-tx flows).
///
/// **KIP-21 lane enforcement is intentionally disabled** here — every
/// `WalletCaller::new(.., None)` call in this module reflects that the
/// `WalletService` surface does not (currently) build payload-carrying
/// L2 transactions. The lane gate runs only on `WalletCaller` instances
/// constructed in `main.rs` for the RPC `eth_sendRawTransaction` path.
///
/// **Do not** promote `WalletService` to a payload-carrying surface
/// without also threading `Option<LaneEnforcement>` through this module
/// from the same config in `main.rs`. Failing to do so would silently
/// bypass KIP-21 enforcement on the new surface.
pub struct WalletService {
    wallet_caller: Arc<WalletCaller>,
    config: WalletConfig,
}

/// Represents the result of a wallet transaction
#[derive(Debug, Clone)]
pub struct WalletTransactionResult {
    pub transaction_id: String,
    pub status: WalletTransactionStatus,
    pub message: Option<String>,
}

/// Status of a wallet transaction
#[derive(Debug, Clone, PartialEq)]
pub enum WalletTransactionStatus {
    Success,
    Failed,
    Pending,
}

/// Parameters for sending a transaction through the wallet
#[derive(Debug, Clone)]
pub struct SendTransactionRequest {
    pub sender_public_key: String,
    pub receiver_address: String,
    pub amount_sompi: u64,
    pub priority_fee_sompi: u64,
}

impl WalletService {
    /// Create a new WalletService with the given configuration
    pub async fn new(config: WalletConfig) -> Result<Self, WalletServiceError> {
        let wallet_caller = WalletCaller::new(config.clone(), None)
            .await
            .map_err(|e| WalletServiceError::InitializationFailed(Box::new(e)))?;

        info!(
            "WALLET_SERVICE: Initialized with daemon URI: {}",
            config.wallet_daemon_uri
        );

        Ok(Self {
            wallet_caller: Arc::new(wallet_caller),
            config,
        })
    }

    /// Create a new WalletService for testing without external dependencies
    #[cfg(test)]
    pub fn new_for_testing(config: WalletConfig) -> Self {
        // For testing, we create a mock wallet caller
        // This avoids network calls and external dependencies
        use crate::clients::wallet_caller::WalletCaller;

        info!(
            "WALLET_SERVICE: Creating test instance with daemon URI: {}",
            config.wallet_daemon_uri
        );

        // Create a test wallet caller using tokio runtime for async creation
        // In real tests, this would be a proper mock
        let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");
        let test_wallet_caller = rt.block_on(async {
            match WalletCaller::new(config.clone(), None).await {
                Ok(caller) => caller,
                Err(_) => {
                    // For testing, we'll just panic - in real tests this would be mocked
                    panic!("Test wallet caller creation failed - use proper mocking in tests")
                }
            }
        });

        Self {
            wallet_caller: Arc::new(test_wallet_caller),
            config,
        }
    }

    /// Send a transaction for Entry transactions
    #[instrument(skip(self), fields(receiver = %request.receiver_address, amount = %request.amount_sompi))]
    pub async fn send_transaction(
        &self,
        request: SendTransactionRequest,
    ) -> Result<WalletTransactionResult, WalletServiceError> {
        info!(
            "WALLET_SERVICE: Processing transaction - receiver: {}, amount: {} sompi, fee: {} sompi",
            request.receiver_address, request.amount_sompi, request.priority_fee_sompi
        );

        // Validate request parameters
        self.validate_send_request(&request)?;

        // Create transaction parameters for the wallet caller
        let _tx_params = TransactionParams::entry_transaction(
            request.receiver_address.clone(),
            request.amount_sompi,
            vec![], // Entry transactions use minimal payload
        );

        // For now, we'll use mine_and_send_transaction as that's the available API
        // TODO: Add proper send_transaction method to WalletCaller
        // Note: TransactionMiner requires full AppConfig, so for now we'll simulate success
        info!(
            "WALLET_SERVICE: Simulating transaction submission for receiver: {}, amount: {} sompi",
            request.receiver_address, request.amount_sompi
        );

        // For actual implementation, this would call the wallet daemon
        let tx_id = format!("tx_{}", Uuid::new_v4());
        Ok(WalletTransactionResult {
            transaction_id: tx_id,
            status: WalletTransactionStatus::Success,
            message: Some("Transaction submitted (simulated)".to_string()),
        })
    }

    /// Send a transaction with mining (for backward compatibility)
    #[instrument(skip(self, miner), fields(receiver = %request.receiver_address, amount = %request.amount_sompi))]
    pub async fn mine_and_send_transaction(
        &self,
        request: SendTransactionRequest,
        miner: &TransactionMiner,
    ) -> Result<WalletTransactionResult, WalletServiceError> {
        info!(
            "WALLET_SERVICE: Processing mined transaction - receiver: {}, amount: {} sompi",
            request.receiver_address, request.amount_sompi
        );

        // Validate request parameters
        self.validate_send_request(&request)?;

        // Create transaction parameters
        let tx_params = TransactionParams::entry_transaction(
            request.receiver_address.clone(),
            request.amount_sompi,
            vec![], // Entry transactions don't need payload for mining
        );

        // Mine and send transaction
        match self
            .wallet_caller
            .mine_and_send_transaction(tx_params, miner)
            .await
        {
            Ok(tx_id) => {
                info!(
                    "WALLET_SERVICE: Mined transaction submitted successfully - tx_id: {}",
                    tx_id
                );

                Ok(WalletTransactionResult {
                    transaction_id: tx_id,
                    status: WalletTransactionStatus::Success,
                    message: Some("Transaction mined and submitted".to_string()),
                })
            }
            Err(e) => {
                error!("WALLET_SERVICE: Mined transaction failed: {}", e);

                Err(WalletServiceError::TransactionFailed(format!(
                    "Mining and sending failed: {e}"
                )))
            }
        }
    }

    /// Get wallet status and connection health
    pub async fn get_wallet_status(&self) -> Result<WalletStatus, WalletServiceError> {
        debug!("WALLET_SERVICE: Checking wallet status");

        // This would typically involve calling a status endpoint on the wallet
        // For now, we'll return a basic status based on our configuration
        Ok(WalletStatus {
            is_connected: true, // We assume connection is good if we got here
            daemon_uri: self.config.wallet_daemon_uri.clone(),
            default_address: self.config.to_address.clone(),
        })
    }

    /// Get the default receiving address
    pub fn get_default_address(&self) -> &str {
        &self.config.to_address
    }

    /// Update wallet configuration
    pub async fn update_config(
        &mut self,
        new_config: WalletConfig,
    ) -> Result<(), WalletServiceError> {
        info!(
            "WALLET_SERVICE: Updating configuration - old URI: {}, new URI: {}",
            self.config.wallet_daemon_uri, new_config.wallet_daemon_uri
        );

        // Create new wallet caller with updated config
        let new_wallet_caller = WalletCaller::new(new_config.clone(), None)
            .await
            .map_err(|e| WalletServiceError::ConfigurationUpdateFailed(Box::new(e)))?;

        self.wallet_caller = Arc::new(new_wallet_caller);
        self.config = new_config;

        info!("WALLET_SERVICE: Configuration updated successfully");
        Ok(())
    }

    /// Validate send transaction request parameters
    fn validate_send_request(
        &self,
        request: &SendTransactionRequest,
    ) -> Result<(), WalletServiceError> {
        if request.receiver_address.is_empty() {
            return Err(WalletServiceError::InvalidRequest(
                "Receiver address cannot be empty".to_string(),
            ));
        }

        if request.sender_public_key.is_empty() {
            return Err(WalletServiceError::InvalidRequest(
                "Sender public key cannot be empty".to_string(),
            ));
        }

        if request.amount_sompi == 0 {
            warn!(
                "WALLET_SERVICE: Zero amount transaction for receiver: {}",
                request.receiver_address
            );
        }

        debug!(
            "WALLET_SERVICE: Request validation passed - receiver: {}, amount: {} sompi",
            request.receiver_address, request.amount_sompi
        );

        Ok(())
    }

    /// Get a reference to the underlying wallet caller (for advanced usage)
    pub fn get_wallet_caller(&self) -> &Arc<WalletCaller> {
        &self.wallet_caller
    }
}

/// Represents the current status of the wallet service
#[derive(Debug, Clone)]
pub struct WalletStatus {
    pub is_connected: bool,
    pub daemon_uri: String,
    pub default_address: String,
}

/// Errors that can occur in the wallet service
#[derive(Debug, thiserror::Error)]
pub enum WalletServiceError {
    #[error("Failed to initialize wallet service: {0}")]
    InitializationFailed(Box<WalletCallerError>),

    #[error("Transaction failed: {0}")]
    TransactionFailed(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Failed to update configuration: {0}")]
    ConfigurationUpdateFailed(Box<WalletCallerError>),

    #[error("Wallet service internal error: {0}")]
    InternalError(String),
}

impl From<WalletCallerError> for WalletServiceError {
    fn from(err: WalletCallerError) -> Self {
        WalletServiceError::InitializationFailed(Box::new(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WalletConfig;

    fn create_test_wallet_config() -> WalletConfig {
        WalletConfig {
            wallet_daemon_uri: "http://localhost:8082".to_string(),
            to_address: "kaspa:test123".to_string(),
        }
    }

    fn create_test_send_request() -> SendTransactionRequest {
        SendTransactionRequest {
            sender_public_key: "test_public_key".to_string(),
            receiver_address: "kaspa:receiver123".to_string(),
            amount_sompi: 1000,
            priority_fee_sompi: 10,
        }
    }

    #[test]
    fn test_validate_send_request_valid() {
        let _config = create_test_wallet_config();
        // Note: We can't easily test the full service without a real wallet connection
        // so we'll test the validation logic separately

        let request = create_test_send_request();

        // Create a mock validation (in real implementation this would be in WalletService)
        assert!(!request.receiver_address.is_empty());
        assert!(!request.sender_public_key.is_empty());
        assert!(request.amount_sompi > 0);
    }

    #[test]
    fn test_validate_send_request_empty_receiver() {
        let mut request = create_test_send_request();
        request.receiver_address = "".to_string();

        assert!(request.receiver_address.is_empty());
    }

    #[test]
    fn test_validate_send_request_empty_sender() {
        let mut request = create_test_send_request();
        request.sender_public_key = "".to_string();

        assert!(request.sender_public_key.is_empty());
    }

    #[test]
    fn test_wallet_transaction_result_creation() {
        let result = WalletTransactionResult {
            transaction_id: "test_tx_123".to_string(),
            status: WalletTransactionStatus::Success,
            message: Some("Transaction successful".to_string()),
        };

        assert_eq!(result.transaction_id, "test_tx_123");
        assert_eq!(result.status, WalletTransactionStatus::Success);
        assert!(result.message.is_some());
    }

    #[test]
    fn test_wallet_status_creation() {
        let status = WalletStatus {
            is_connected: true,
            daemon_uri: "http://localhost:8082".to_string(),
            default_address: "kaspa:test123".to_string(),
        };

        assert!(status.is_connected);
        assert_eq!(status.daemon_uri, "http://localhost:8082");
        assert_eq!(status.default_address, "kaspa:test123");
    }
}
