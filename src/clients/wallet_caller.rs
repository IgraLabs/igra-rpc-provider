use crate::config::{RetryConfig, WalletConfig};
use crate::error::AppError;
use crate::services::lane::{self, LaneEnforcement, Stage};
use crate::services::mining::TransactionMiner;
use crate::types::wallet::{
    proto_payload_len, proto_to_signable_transaction, update_proto_with_mined_transaction,
};
use proto::kaswallet_proto::wallet_client::WalletClient;
use proto::kaswallet_proto::{
    BroadcastRequest, CreateUnsignedTransactionsRequest, NewAddressRequest, SignRequest,
    TransactionDescription, WalletSignableTransaction,
};
use std::env;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, instrument, warn};

const PASSWORD_ENV_VAR: &str = "KASWALLET_PASSWORD";
const GRPC_TIMEOUT_SECS: u64 = 30;

/// Parameters for creating a transaction
#[derive(Debug, Clone)]
pub struct TransactionParams {
    pub to_address: String,
    pub amount: u64,
    pub is_send_all: bool,
    pub payload: Vec<u8>,
    pub l2_transaction_hash: Option<String>,
}

impl TransactionParams {
    /// Create transaction params for Entry transactions
    pub fn entry_transaction(to_address: String, amount: u64, payload: Vec<u8>) -> Self {
        Self {
            to_address,
            amount,
            is_send_all: false,
            payload,
            l2_transaction_hash: None,
        }
    }

    /// Create transaction params for existing RPC behavior (send all to default address)
    pub fn send_all(
        to_address: String,
        payload: Vec<u8>,
        transaction_hash: Option<String>,
    ) -> Self {
        Self {
            to_address,
            amount: 0,
            is_send_all: true,
            payload,
            l2_transaction_hash: transaction_hash,
        }
    }
}

/// Implementation of wallet caller for interacting with KASPA wallet daemon.
pub struct WalletCaller {
    wallet_daemon_client: Mutex<WalletClient<tonic::transport::Channel>>,
    to_address: String,
    password: String,
    /// `Some` enables KIP-21 enforcement on every payload-carrying tx (RPC
    /// path); `None` is the legacy unvalidated path used by the entry-tx
    /// CLI and UTXO-consolidation flows. The non-empty-prefix invariant
    /// is enforced by `LaneEnforcement::new`, so an empty-prefix footgun
    /// is unrepresentable at the type level.
    lane_enforcement: Option<LaneEnforcement>,
}

impl WalletCaller {
    /// Construct a `WalletCaller`. `lane_enforcement` is required at the
    /// API boundary so every call site explicitly chooses between KIP-21
    /// enforcement on (the RPC path) and legacy native-subnetwork behavior
    /// (entry-tx CLI, consolidation, tests). The type system prevents the
    /// "forgot to opt in" footgun a builder pattern would leave open.
    ///
    /// - `Some(LaneEnforcement)` → every payload-carrying tx returned by
    ///   the daemon is validated against the lane (and Toccata version, and
    ///   payload length) before mining, and against all four KIP-21
    ///   invariants (lane + version + payload + final tx-id prefix) before
    ///   broadcast.
    /// - `None` → legacy unvalidated behavior; matches the ticket's
    ///   "pre-stage UTXO consolidation txs stay untouched" rule.
    pub async fn new(
        wallet_config: WalletConfig,
        lane_enforcement: Option<LaneEnforcement>,
    ) -> Result<Self, WalletCallerError> {
        let mut wallet_daemon_client =
            WalletClient::connect(wallet_config.wallet_daemon_uri.clone())
                .await
                .map_err(WalletCallerError::ConnectionFailed)?;

        let to_address = wallet_config.to_address.clone();
        let to_address = if to_address.is_empty() {
            wallet_daemon_client
                .new_address(NewAddressRequest {})
                .await
                .map_err(WalletCallerError::AddressGenerationFailed)?
                .into_inner()
                .address
        } else {
            to_address
        };

        let password = env::var(PASSWORD_ENV_VAR).map_err(|e| match e {
            env::VarError::NotPresent => WalletCallerError::PasswordNotSet,
            env::VarError::NotUnicode(_) => WalletCallerError::PasswordInvalidUnicode,
        })?;

        match &lane_enforcement {
            Some(e) => info!(
                "WalletCaller: KIP-21 lane enforcement ENABLED (lane={}, tx_id_prefix=0x{})",
                e.lane(),
                hex::encode(e.tx_id_prefix()),
            ),
            None => debug!("WalletCaller: KIP-21 lane enforcement disabled (legacy mode)"),
        }

        Ok(Self {
            wallet_daemon_client: Mutex::new(wallet_daemon_client),
            to_address,
            password,
            lane_enforcement,
        })
    }

    /// Mine and send transaction using the focused mining service with retry support
    #[instrument(skip(self, transaction_params, miner, retry_config))]
    pub async fn mine_and_send_transaction_with_retry(
        &self,
        transaction_params: TransactionParams,
        miner: &TransactionMiner,
        retry_config: &RetryConfig,
    ) -> Result<String, WalletCallerError> {
        let unsigned_transactions = self
            .create_unsigned_transaction_with_retry(transaction_params.clone(), retry_config)
            .await?;

        self.complete_transaction_flow(unsigned_transactions, transaction_params, miner)
            .await
    }

    /// Mine and send transaction using the focused mining service (legacy without retry)
    #[instrument(skip(self, transaction_params, miner))]
    pub async fn mine_and_send_transaction(
        &self,
        transaction_params: TransactionParams,
        miner: &TransactionMiner,
    ) -> Result<String, WalletCallerError> {
        let unsigned_transactions = self
            .create_unsigned_transaction(transaction_params.clone())
            .await?;

        self.complete_transaction_flow(unsigned_transactions, transaction_params, miner)
            .await
    }

    /// Complete the transaction flow after creating unsigned transactions
    async fn complete_transaction_flow(
        &self,
        mut unsigned_transactions: Vec<WalletSignableTransaction>,
        transaction_params: TransactionParams,
        miner: &TransactionMiner,
    ) -> Result<String, WalletCallerError> {
        if unsigned_transactions.is_empty() {
            return Err(WalletCallerError::NoTransactionIds);
        }

        info!(
            "Created {} unsigned transactions, starting mining process",
            unsigned_transactions.len()
        );

        let last_index = unsigned_transactions.len().saturating_sub(1);

        // Defense-in-depth: we only mine and validate the LAST tx. Earlier
        // entries must be pre-stage UTXO consolidation txs with empty
        // payload (native subnetwork). If the daemon ever returns a
        // payload-carrying staging tx, we'd sign and broadcast an IGRA
        // payload that bypassed both lane enforcement gates. Fail loudly.
        // Runs unconditionally — independent of lane enforcement — so the
        // assumption is enforced even on legacy paths.
        for (i, ut) in unsigned_transactions[..last_index].iter().enumerate() {
            let payload_len = proto_payload_len(ut);
            if payload_len > 0 {
                error!(
                    staging_index = i,
                    payload_len, "wallet daemon returned payload on a staging tx; expected only the last tx to carry payload"
                );
                return Err(WalletCallerError::LaneEnforcementFailed(
                    AppError::LaneEnforcementFailed(format!(
                        "staging tx {i} carries {payload_len}-byte payload; only the \
                         last tx in a multi-tx wallet response may carry payload"
                    )),
                ));
            }
        }

        let last_transaction = &unsigned_transactions[last_index];

        let signable_tx = proto_to_signable_transaction(last_transaction)
            .map_err(|e| WalletCallerError::TransactionDecodingFailed(e.to_string()))?;

        let original_tx_id = signable_tx.id();

        // KIP-21 pre-mining gate: catch a kaswallet/RPC config mismatch
        // (wrong subnetwork, pre-Toccata version, short payload) before
        // we burn CPU on prefix mining. The prefix invariant is deferred
        // to the post-mining gate.
        if let Some(e) = &self.lane_enforcement {
            lane::validate_lane_transaction(
                &signable_tx,
                e.lane(),
                e.tx_id_prefix(),
                Stage::PreMining,
            )
            .map_err(WalletCallerError::LaneEnforcementFailed)?;
        }

        info!(
            "Extracted SignableTransaction {}, starting mining",
            original_tx_id
        );

        let (mined_transaction, mining_stats) = miner
            .mine_transaction(signable_tx)
            .await
            .map_err(WalletCallerError::MiningFailed)?;

        // KIP-21 pre-broadcast gate: enforce all four invariants on the
        // mined tx. Mining is supposed to guarantee the prefix, but we
        // re-check here so a bug in the mining loop (or a future change
        // to `Transaction::finalize`) cannot let a non-conforming tx
        // through to `sign_transactions`.
        if let Some(e) = &self.lane_enforcement {
            lane::validate_lane_transaction(
                &mined_transaction,
                e.lane(),
                e.tx_id_prefix(),
                Stage::PreBroadcast,
            )
            .map_err(WalletCallerError::LaneEnforcementFailed)?;
        }

        info!(
            "Mining completed: {} nonces in {:?}, hash rate: {:.2} H/s",
            mining_stats.nonces_tried, mining_stats.duration, mining_stats.hashes_per_second,
        );

        info!(
            "Mined transaction: igra_payload={} (to_address={}, amount={}, is_send_all={}, payload_size={} bytes)",
            hex::encode(&mined_transaction.tx.payload),
            &transaction_params.to_address,
            transaction_params.amount,
            transaction_params.is_send_all,
            transaction_params.payload.len()
        );

        update_proto_with_mined_transaction(
            &mut unsigned_transactions[last_index],
            mined_transaction,
        )
        .map_err(|e| WalletCallerError::TransactionEncodingFailed(e.to_string()))?;

        info!("Mining completed successfully, proceeding with signing and broadcasting");

        let signed_transactions = self.sign_transactions(unsigned_transactions).await?;
        info!("Transactions signed successfully");

        let transaction_ids = self.broadcast_transactions(signed_transactions).await?;

        let last_tx_id = transaction_ids
            .last()
            .ok_or(WalletCallerError::NoTransactionIds)?;

        info!(
            "Transaction broadcast successfully! Transaction ID: {}",
            last_tx_id
        );
        Ok(last_tx_id.clone())
    }

    /// Get the default to_address for this wallet caller
    pub fn default_to_address(&self) -> &str {
        &self.to_address
    }

    /// Creates unsigned transactions using the wallet daemon
    #[instrument(skip(self, transaction_params))]
    async fn create_unsigned_transaction(
        &self,
        transaction_params: TransactionParams,
    ) -> Result<Vec<WalletSignableTransaction>, WalletCallerError> {
        let transaction_description = Some(TransactionDescription {
            to_address: transaction_params.to_address.clone(),
            amount: transaction_params.amount,
            is_send_all: transaction_params.is_send_all,
            payload: transaction_params.payload.into(),
            from_addresses: vec![],
            utxos: vec![],
            use_existing_change_address: true,
            fee_policy: None,
        });

        let mut wallet_daemon_client = self.wallet_daemon_client.lock().await;

        let response = match timeout(
            Duration::from_secs(GRPC_TIMEOUT_SECS),
            wallet_daemon_client.create_unsigned_transactions(CreateUnsignedTransactionsRequest {
                transaction_description,
            }),
        )
        .await
        {
            Ok(result) => result.map_err(|e| {
                if Self::is_no_funds_error(&e) {
                    error!("UTXO exhaustion detected: {}", e.message());
                    info!(
                        "UTXO exhaustion details - to_address: {}, amount: {}, is_send_all: {}",
                        transaction_params.to_address,
                        transaction_params.amount,
                        transaction_params.is_send_all
                    );
                }
                WalletCallerError::TransactionCreationFailed(e)
            })?,
            Err(_) => {
                warn!(
                    "gRPC create_unsigned_transactions timed out after {}s",
                    GRPC_TIMEOUT_SECS
                );
                return Err(WalletCallerError::GrpcTimeout {
                    operation: "create_unsigned_transactions",
                    timeout_seconds: GRPC_TIMEOUT_SECS,
                });
            }
        };

        let unsigned_transactions = response.into_inner().unsigned_transactions;
        debug!(
            "Created {} unsigned transactions with to_address={}, amount={}, is_send_all={}",
            unsigned_transactions.len(),
            transaction_params.to_address,
            transaction_params.amount,
            transaction_params.is_send_all
        );

        Ok(unsigned_transactions)
    }

    /// Signs transactions using the wallet daemon
    #[instrument(skip(self, unsigned_transactions))]
    async fn sign_transactions(
        &self,
        unsigned_transactions: Vec<WalletSignableTransaction>,
    ) -> Result<Vec<WalletSignableTransaction>, WalletCallerError> {
        let mut wallet_daemon_client = self.wallet_daemon_client.lock().await;

        let response = match timeout(
            Duration::from_secs(GRPC_TIMEOUT_SECS),
            wallet_daemon_client.sign(SignRequest {
                unsigned_transactions,
                password: self.password.clone(),
            }),
        )
        .await
        {
            Ok(result) => result.map_err(WalletCallerError::TransactionSigningFailed)?,
            Err(_) => {
                warn!("gRPC sign timed out after {}s", GRPC_TIMEOUT_SECS);
                return Err(WalletCallerError::GrpcTimeout {
                    operation: "sign",
                    timeout_seconds: GRPC_TIMEOUT_SECS,
                });
            }
        };

        let transactions = response.into_inner().signed_transactions;
        debug!("Signed {} transactions", transactions.len());

        Ok(transactions)
    }

    /// Broadcasts signed transactions using the wallet daemon
    #[instrument(skip(self, signed_transactions))]
    async fn broadcast_transactions(
        &self,
        signed_transactions: Vec<WalletSignableTransaction>,
    ) -> Result<Vec<String>, WalletCallerError> {
        let mut wallet_daemon_client = self.wallet_daemon_client.lock().await;

        let response = match timeout(
            Duration::from_secs(GRPC_TIMEOUT_SECS),
            wallet_daemon_client.broadcast(BroadcastRequest {
                transactions: signed_transactions,
            }),
        )
        .await
        {
            Ok(result) => result.map_err(WalletCallerError::TransactionBroadcastFailed)?,
            Err(_) => {
                warn!("gRPC broadcast timed out after {}s", GRPC_TIMEOUT_SECS);
                return Err(WalletCallerError::GrpcTimeout {
                    operation: "broadcast",
                    timeout_seconds: GRPC_TIMEOUT_SECS,
                });
            }
        };

        let transaction_ids = response.into_inner().transaction_ids;
        debug!(
            "Broadcast {} transactions with IDs: {:?}",
            transaction_ids.len(),
            transaction_ids
        );

        Ok(transaction_ids)
    }

    /// Checks if a tonic::Status error indicates "No funds to send"
    pub fn is_no_funds_error(status: &tonic::Status) -> bool {
        // Check both error code and message for robustness
        // ResourceExhausted is the appropriate gRPC code for this type of error
        let is_resource_exhausted = matches!(status.code(), tonic::Code::ResourceExhausted);

        // Also check the message content (case-insensitive) for backward compatibility
        let message_lower = status.message().to_lowercase();
        let has_no_funds_message = message_lower.contains("no funds")
            || message_lower.contains("utxo exhausted")
            || message_lower.contains("insufficient utxo");

        is_resource_exhausted || has_no_funds_message
    }

    pub fn is_utxo_exhaustion_error(error: &WalletCallerError) -> bool {
        match error {
            WalletCallerError::TransactionCreationFailed(status) => Self::is_no_funds_error(status),
            _ => false,
        }
    }

    /// Retry transaction creation with exponential backoff for UTXO exhaustion errors
    #[instrument(skip(self, transaction_params, retry_config))]
    pub async fn create_unsigned_transaction_with_retry(
        &self,
        transaction_params: TransactionParams,
        retry_config: &RetryConfig,
    ) -> Result<Vec<WalletSignableTransaction>, WalletCallerError> {
        let mut last_error = None;

        for attempt in 1..=retry_config.max_attempts {
            info!(
                "Creating unsigned transaction, attempt {}/{}",
                attempt, retry_config.max_attempts
            );

            match self
                .create_unsigned_transaction(transaction_params.clone())
                .await
            {
                Ok(result) => {
                    if attempt > 1 {
                        info!("Transaction creation succeeded after {} attempts", attempt);
                    }
                    return Ok(result);
                }
                Err(e) => {
                    if Self::is_utxo_exhaustion_error(&e) {
                        last_error = Some(e);

                        if attempt < retry_config.max_attempts {
                            let delay_ms = retry_config.calculate_delay_ms(attempt);
                            let jittered_delay = retry_config.add_jitter(delay_ms);

                            warn!(
                                "UTXO exhaustion detected, retrying in {} ms (attempt {}/{})",
                                jittered_delay, attempt, retry_config.max_attempts
                            );

                            sleep(Duration::from_millis(jittered_delay)).await;
                        }
                    } else {
                        // Not a retryable error, return immediately
                        return Err(e);
                    }
                }
            }
        }

        // All attempts exhausted
        error!(
            "UTXO exhaustion persists after {} attempts, giving up",
            retry_config.max_attempts
        );

        // Convert the last error to RetryExhausted
        match last_error {
            Some(WalletCallerError::TransactionCreationFailed(_)) => {
                Err(WalletCallerError::MiningFailed(AppError::RetryExhausted {
                    attempts: retry_config.max_attempts,
                    reason: "UTXO exhaustion".to_string(),
                }))
            }
            Some(e) => Err(e),
            None => {
                error!("Logic error: retry loop completed without capturing error");
                Err(WalletCallerError::MiningFailed(AppError::RetryExhausted {
                    attempts: retry_config.max_attempts,
                    reason: "UTXO exhaustion (no error captured)".to_string(),
                }))
            }
        }
    }
}

/// Comprehensive wallet error types for better error handling and RPC responses
#[derive(Debug, thiserror::Error)]
pub enum WalletCallerError {
    #[error("Failed to connect to wallet daemon: {0}")]
    ConnectionFailed(#[from] tonic::transport::Error),

    #[error("Failed to generate new address: {0}")]
    AddressGenerationFailed(#[source] tonic::Status),

    #[error("Wallet password environment variable {PASSWORD_ENV_VAR} is not set")]
    PasswordNotSet,

    #[error("Wallet password environment variable {PASSWORD_ENV_VAR} contains invalid Unicode")]
    PasswordInvalidUnicode,

    #[error("Failed to create unsigned transactions: {0}")]
    TransactionCreationFailed(#[source] tonic::Status),

    #[error("Failed to sign transactions: {0}")]
    TransactionSigningFailed(#[source] tonic::Status),

    #[error("Failed to broadcast transactions: {0}")]
    TransactionBroadcastFailed(#[source] tonic::Status),

    #[error("Mining failed: {0}")]
    MiningFailed(#[from] crate::error::AppError),

    /// KIP-21 lane gate rejected the daemon-built (pre-mining) or mined
    /// (pre-broadcast) tx. Kept distinct from `MiningFailed` so logs,
    /// metrics, and alert routing can tell lane misconfiguration from
    /// actual mining failures.
    #[error("Lane enforcement failed: {0}")]
    LaneEnforcementFailed(crate::error::AppError),

    #[error("No transaction IDs returned from broadcast")]
    NoTransactionIds,

    #[error("Failed to decode wallet transaction: {0}")]
    TransactionDecodingFailed(String),

    #[error("Failed to encode wallet transaction: {0}")]
    TransactionEncodingFailed(String),

    #[error("Failed to extract signable transaction: {0}")]
    TransactionExtractionFailed(String),

    #[error("Wallet gRPC call '{operation}' timed out after {timeout_seconds}s")]
    GrpcTimeout {
        operation: &'static str,
        timeout_seconds: u64,
    },
}

// Conversion to AppError for consistent error handling across the application
impl From<WalletCallerError> for crate::error::AppError {
    fn from(err: WalletCallerError) -> Self {
        match err {
            WalletCallerError::ConnectionFailed(e) => {
                crate::error::AppError::WalletError(format!("Connection failed: {e}"))
            }
            WalletCallerError::AddressGenerationFailed(e) => {
                crate::error::AppError::WalletError(format!("Address generation failed: {e}"))
            }
            WalletCallerError::PasswordNotSet => {
                crate::error::AppError::WalletError("Password not set".to_string())
            }
            WalletCallerError::PasswordInvalidUnicode => {
                crate::error::AppError::WalletError("Password invalid unicode".to_string())
            }
            WalletCallerError::TransactionCreationFailed(e) => {
                if WalletCaller::is_no_funds_error(&e) {
                    crate::error::AppError::UtxoExhausted
                } else {
                    crate::error::AppError::WalletError(format!("Transaction creation failed: {e}"))
                }
            }
            WalletCallerError::TransactionSigningFailed(e) => {
                crate::error::AppError::WalletError(format!("Transaction signing failed: {e}"))
            }
            WalletCallerError::TransactionBroadcastFailed(e) => {
                crate::error::AppError::WalletError(format!("Transaction broadcast failed: {e}"))
            }
            WalletCallerError::MiningFailed(e) => e,
            WalletCallerError::LaneEnforcementFailed(e) => e,
            WalletCallerError::NoTransactionIds => {
                crate::error::AppError::WalletError("No transaction IDs returned".to_string())
            }
            WalletCallerError::TransactionDecodingFailed(e) => {
                crate::error::AppError::transaction_codec_error("decode", &e)
            }
            WalletCallerError::TransactionEncodingFailed(e) => {
                crate::error::AppError::transaction_codec_error("encode", &e)
            }
            WalletCallerError::TransactionExtractionFailed(e) => {
                crate::error::AppError::WalletError(format!("Transaction extraction failed: {e}"))
            }
            WalletCallerError::GrpcTimeout {
                operation,
                timeout_seconds,
            } => crate::error::AppError::WalletError(format!(
                "Wallet gRPC {operation} timed out after {timeout_seconds}s"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RetryConfig;

    #[test]
    fn test_is_no_funds_error() {
        // Test with "No funds to send" message
        let status = tonic::Status::invalid_argument("No funds to send");
        assert!(WalletCaller::is_no_funds_error(&status));

        // Test with ResourceExhausted code
        let status = tonic::Status::resource_exhausted("Some other message");
        assert!(WalletCaller::is_no_funds_error(&status));

        // Test with different error code and message
        let status = tonic::Status::invalid_argument("Different error");
        assert!(!WalletCaller::is_no_funds_error(&status));

        // Test with partial match
        let status = tonic::Status::invalid_argument("Error: No funds to send for transaction");
        assert!(WalletCaller::is_no_funds_error(&status));

        // Test case insensitive matching
        let status = tonic::Status::invalid_argument("NO FUNDS available");
        assert!(WalletCaller::is_no_funds_error(&status));

        // Test UTXO exhausted variant
        let status = tonic::Status::invalid_argument("UTXO exhausted");
        assert!(WalletCaller::is_no_funds_error(&status));

        // Test insufficient UTXO variant
        let status = tonic::Status::invalid_argument("Insufficient UTXO balance");
        assert!(WalletCaller::is_no_funds_error(&status));
    }

    #[test]
    fn test_is_utxo_exhaustion_error() {
        let status = tonic::Status::invalid_argument("No funds to send");
        let error = WalletCallerError::TransactionCreationFailed(status);
        assert!(WalletCaller::is_utxo_exhaustion_error(&error));

        let status = tonic::Status::invalid_argument("Different error");
        let error = WalletCallerError::TransactionCreationFailed(status);
        assert!(!WalletCaller::is_utxo_exhaustion_error(&error));

        let error = WalletCallerError::PasswordNotSet;
        assert!(!WalletCaller::is_utxo_exhaustion_error(&error));
    }

    #[test]
    fn test_retry_config_delay_calculation() {
        let config = RetryConfig::default();

        // Test exponential backoff
        assert_eq!(config.calculate_delay_ms(0), 0);
        assert_eq!(config.calculate_delay_ms(1), 100); // initial_delay
        assert_eq!(config.calculate_delay_ms(2), 200); // 2x initial
        assert_eq!(config.calculate_delay_ms(3), 400); // 4x initial
        assert_eq!(config.calculate_delay_ms(4), 800); // 8x initial
        assert_eq!(config.calculate_delay_ms(5), 1600); // 16x initial
        assert_eq!(config.calculate_delay_ms(6), 3000); // capped at max_delay
    }

    #[test]
    fn test_retry_config_jitter() {
        let config = RetryConfig::default();
        let base_delay = 1000u64;

        // Test that jitter produces values in expected range
        for _ in 0..10 {
            let jittered = config.add_jitter(base_delay);
            assert!(jittered >= 750); // 75% of base
            assert!(jittered <= 1250); // 125% of base
        }
    }

    #[test]
    fn test_wallet_error_to_app_error_conversion() {
        let status = tonic::Status::invalid_argument("No funds to send");
        let wallet_error = WalletCallerError::TransactionCreationFailed(status);
        let app_error: AppError = wallet_error.into();

        match app_error {
            AppError::UtxoExhausted => {}
            _ => panic!("Expected UtxoExhausted error"),
        }

        let status = tonic::Status::invalid_argument("Different error");
        let wallet_error = WalletCallerError::TransactionCreationFailed(status);
        let app_error: AppError = wallet_error.into();

        match app_error {
            AppError::WalletError(msg) => {
                assert!(msg.contains("Transaction creation failed"));
            }
            _ => panic!("Expected WalletError"),
        }
    }
}
