use crate::config::{IgraConfig, RetryConfig, WalletConfig};
use crate::error::AppError;
use crate::types::wallet::{is_partially_signed, partial_proto_transaction};
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

/// Upper bound on outputs per kaswallet-emitted IGRA-lane tx.
///
/// A correct IGRA submission produces at most two outputs: the recipient
/// output (or the wallet's own change address in `send_all` mode) plus
/// optionally one change output. We reject any tx with more outputs than
/// this — a daemon emitting a wider output set is either buggy or trying
/// to redirect funds, and we'd rather fail at the boundary than spend a
/// signing round-trip on it.
const MAX_OUTPUTS_PER_LANE_TX: usize = 2;

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
    /// Full 20-byte SubnetworkId of the configured IGRA lane. Used to
    /// validate that the kaswallet daemon produced a tx in the expected
    /// lane before we sign and broadcast it.
    lane_id: [u8; 20],
}

impl WalletCaller {
    pub async fn new(
        wallet_config: WalletConfig,
        igra_config: IgraConfig,
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

        Ok(Self {
            wallet_daemon_client: Mutex::new(wallet_daemon_client),
            to_address,
            password,
            lane_id: igra_config.lane_id(),
        })
    }

    /// Construct, validate, sign, and broadcast an IGRA-lane transaction.
    ///
    /// 1. Ask kaswallet for an unsigned transaction (with UTXO-exhaustion
    ///    retry).
    /// 2. Validate the returned tx is on the configured IGRA lane, carries
    ///    the expected payload, and uses v1 `ComputeBudget` input mass.
    /// 3. Sign via the wallet daemon.
    /// 4. Broadcast and return the last broadcast tx id.
    ///
    /// Validation runs **before** signing, so a misbehaving daemon never
    /// gets a signature on a wrong-lane tx.
    #[instrument(skip(self, transaction_params, retry_config))]
    pub async fn create_sign_and_broadcast_igra_lane_transaction(
        &self,
        transaction_params: TransactionParams,
        retry_config: &RetryConfig,
    ) -> Result<String, WalletCallerError> {
        let unsigned_transactions = self
            .create_unsigned_transaction_with_retry(&transaction_params, retry_config)
            .await?;

        info!(
            "Created {} unsigned transactions, validating IGRA lane before signing",
            unsigned_transactions.len()
        );

        self.validate_lane_transaction(&unsigned_transactions, &transaction_params.payload)
            .map_err(WalletCallerError::LaneValidationFailed)?;

        info!(
            "Lane validation passed (to_address={}, amount={}, is_send_all={}, payload_size={} bytes); proceeding to sign and broadcast",
            transaction_params.to_address,
            transaction_params.amount,
            transaction_params.is_send_all,
            transaction_params.payload.len()
        );

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

    /// Validate every tx kaswallet returned, before signing.
    ///
    /// For each tx: enforce v1 + configured IGRA `subnetwork_id` + zero
    /// `lock_time` + zero `gas` + a bounded output count (recipient +
    /// optional change) + non-empty inputs + per-input `ComputeBudget`
    /// mass (`sig_op_count == 0`, `0 < compute_budget <= u16::MAX`). The
    /// *last* tx must additionally carry our expected payload — pre-stage
    /// UTXO consolidations carry no IGRA payload but still must be on the
    /// configured lane (otherwise a daemon could exfiltrate signatures on
    /// off-lane consolidations).
    ///
    /// **Trust boundary**: this is defence-in-depth against daemon bugs and
    /// misconfiguration. It does NOT defend against a fully compromised
    /// kaswallet daemon — the daemon holds wallet keys and can sign and
    /// broadcast independently of RPC. In particular, output
    /// `script_public_key` matching against the wallet's address pool is
    /// not enforced here; we only bound output count to
    /// [`MAX_OUTPUTS_PER_LANE_TX`] (recipient + change) per tx.
    ///
    /// Returns the validation-failure reason as a plain `String` so the
    /// caller can wrap it in `WalletCallerError::LaneValidationFailed`
    /// without forcing the larger error type onto a fallible sync return
    /// path (avoids `clippy::result_large_err`).
    fn validate_lane_transaction(
        &self,
        txs: &[WalletSignableTransaction],
        expected_payload: &[u8],
    ) -> Result<(), String> {
        if txs.is_empty() {
            return Err("kaswallet returned no unsigned transactions".to_string());
        }

        // saturating because the project lints deny direct arithmetic; we
        // already guaranteed `txs.len() >= 1` above so the subtraction is
        // exact.
        let last_index = txs.len().saturating_sub(1);
        for (i, tx) in txs.iter().enumerate() {
            let label = if i == last_index {
                "payload tx"
            } else {
                "pre-stage tx"
            };
            self.validate_single_lane_tx(i, tx, label)?;

            if i == last_index {
                let proto_tx = partial_proto_transaction(tx)
                    .ok_or_else(|| format!("{label} #{i} is missing its proto Transaction body"))?;
                if proto_tx.payload.as_ref() != expected_payload {
                    let got_len = proto_tx.payload.len();
                    let expected_len = expected_payload.len();
                    return Err(if got_len == expected_len {
                        format!("{label} #{i} payload: expected {expected_len} bytes, got {got_len} bytes (contents differ)")
                    } else {
                        format!("{label} #{i} payload: expected {expected_len} bytes, got {got_len} bytes")
                    });
                }
            }
        }
        Ok(())
    }

    /// Structural lane-shape checks applied to every tx in the batch.
    fn validate_single_lane_tx(
        &self,
        i: usize,
        tx: &WalletSignableTransaction,
        label: &str,
    ) -> Result<(), String> {
        if !is_partially_signed(tx) {
            return Err(format!("{label} #{i} is not Partially signed"));
        }

        let proto_tx = partial_proto_transaction(tx)
            .ok_or_else(|| format!("{label} #{i} is missing its proto Transaction body"))?;

        if proto_tx.version != 1 {
            return Err(format!(
                "{label} #{i} version: expected 1 (Toccata v1), got {}",
                proto_tx.version,
            ));
        }

        if proto_tx.subnetwork_id.as_ref() != self.lane_id.as_slice() {
            let got_bytes = proto_tx.subnetwork_id.len();
            let got = if got_bytes == 0 {
                "empty subnetwork_id".to_string()
            } else if got_bytes != self.lane_id.len() {
                format!(
                    "{got_bytes}-byte subnetwork_id 0x{}",
                    hex::encode(&proto_tx.subnetwork_id),
                )
            } else {
                format!("0x{}", hex::encode(&proto_tx.subnetwork_id))
            };
            return Err(format!(
                "{label} #{i} subnetwork_id: expected 0x{} (configured IGRA lane), got {got}",
                hex::encode(self.lane_id),
            ));
        }

        if proto_tx.lock_time != 0 {
            return Err(format!(
                "{label} #{i} lock_time: expected 0 on IGRA-lane tx, got {}",
                proto_tx.lock_time,
            ));
        }

        if proto_tx.gas != 0 {
            return Err(format!(
                "{label} #{i} gas: expected 0 on IGRA-lane tx \
                 (gas lives in the L2 payload, not the L1 tx), got {}",
                proto_tx.gas,
            ));
        }

        if proto_tx.outputs.is_empty() {
            return Err(format!(
                "{label} #{i} outputs: expected at least 1, got 0 \
                 (a valid send must produce at least the recipient output)"
            ));
        }

        if proto_tx.outputs.len() > MAX_OUTPUTS_PER_LANE_TX {
            return Err(format!(
                "{label} #{i} outputs: expected at most {MAX_OUTPUTS_PER_LANE_TX} \
                 (recipient + change), got {}",
                proto_tx.outputs.len(),
            ));
        }

        if proto_tx.inputs.is_empty() {
            return Err(format!(
                "{label} #{i} has no inputs (IGRA-lane tx must consume at least one UTXO)"
            ));
        }

        // v1 input mass dispatch: kaswallet's wire contract
        // (common/src/proto_convert.rs) populates `compute_budget` as
        // authoritative and leaves `sig_op_count == 0`. We enforce that
        // contract here plus a u16 upper bound matching the kaspa consensus
        // mass type and a positive-value requirement (zero-mass v1 inputs
        // are consensus-invalid).
        for (j, input) in proto_tx.inputs.iter().enumerate() {
            if input.sig_op_count != 0 {
                return Err(format!(
                    "{label} #{i} input #{j} sig_op_count: expected 0 on v1 tx \
                     (use compute_budget mass), got {}",
                    input.sig_op_count,
                ));
            }
            if input.compute_budget == 0 {
                return Err(format!(
                    "{label} #{i} input #{j} compute_budget: expected > 0 on v1 tx \
                     (IGRA lane requires positive per-input mass), got 0"
                ));
            }
            if input.compute_budget > u32::from(u16::MAX) {
                return Err(format!(
                    "{label} #{i} input #{j} compute_budget: expected <= {} (u16::MAX, kaspa \
                     consensus mass type) on v1 tx, got {}",
                    u16::MAX,
                    input.compute_budget,
                ));
            }
        }
        Ok(())
    }

    /// Get the default to_address for this wallet caller
    pub(crate) fn default_to_address(&self) -> &str {
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
    fn is_no_funds_error(status: &tonic::Status) -> bool {
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

    fn is_utxo_exhaustion_error(error: &WalletCallerError) -> bool {
        match error {
            WalletCallerError::TransactionCreationFailed(status) => Self::is_no_funds_error(status),
            _ => false,
        }
    }

    /// Retry transaction creation with exponential backoff for UTXO exhaustion errors
    #[instrument(skip(self, transaction_params, retry_config))]
    pub(crate) async fn create_unsigned_transaction_with_retry(
        &self,
        transaction_params: &TransactionParams,
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
            Some(WalletCallerError::TransactionCreationFailed(_)) => Err(
                WalletCallerError::WalletDaemonError(AppError::RetryExhausted {
                    attempts: retry_config.max_attempts,
                    reason: "UTXO exhaustion".to_string(),
                }),
            ),
            Some(e) => Err(e),
            None => {
                error!("Logic error: retry loop completed without capturing error");
                Err(WalletCallerError::WalletDaemonError(
                    AppError::RetryExhausted {
                        attempts: retry_config.max_attempts,
                        reason: "UTXO exhaustion (no error captured)".to_string(),
                    },
                ))
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

    /// Carries application-level errors (e.g. retry exhaustion) surfaced
    /// from below the gRPC layer.
    #[error("Wallet daemon error: {0}")]
    WalletDaemonError(#[from] crate::error::AppError),

    #[error("IGRA lane validation failed: {0}")]
    LaneValidationFailed(String),

    #[error("No transaction IDs returned from broadcast")]
    NoTransactionIds,

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
            WalletCallerError::WalletDaemonError(e) => e,
            WalletCallerError::LaneValidationFailed(reason) => crate::error::AppError::WalletError(
                format!("IGRA lane validation failed: {reason}"),
            ),
            WalletCallerError::NoTransactionIds => {
                crate::error::AppError::WalletError("No transaction IDs returned".to_string())
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
    use proto::kaswallet_proto as proto_types;

    /// Convenience: assemble a `[u8; 20]` lane id from its 4-byte
    /// namespace, zero-padded per KIP-21.
    fn lane(namespace: [u8; 4]) -> [u8; 20] {
        let mut out = [0u8; 20];
        out[..4].copy_from_slice(&namespace);
        out
    }

    /// Construct a `WalletCaller` directly (without a live daemon) for
    /// unit-testing pure functions like `validate_lane_transaction`.
    fn caller_with_lane(lane_id: [u8; 20]) -> WalletCaller {
        // We need to build a `WalletClient` value to satisfy the Mutex
        // field, but we never make calls on it. `tonic::transport::Channel`
        // can be constructed lazily without a live endpoint.
        let endpoint = tonic::transport::Endpoint::from_static("http://127.0.0.1:1");
        let channel = endpoint.connect_lazy();
        WalletCaller {
            wallet_daemon_client: Mutex::new(WalletClient::new(channel)),
            to_address: "kaspa:test".to_string(),
            password: "test".to_string(),
            lane_id,
        }
    }

    fn dummy_output() -> proto_types::TransactionOutput {
        proto_types::TransactionOutput {
            value: 1,
            script_public_key: Some(proto_types::ScriptPublicKey {
                version: 0,
                script_public_key: "00".to_string(),
            }),
        }
    }

    fn make_proto_tx(
        version: u32,
        subnetwork_id: Vec<u8>,
        payload: Vec<u8>,
        inputs: Vec<proto_types::TransactionInput>,
    ) -> proto_types::Transaction {
        proto_types::Transaction {
            version,
            inputs,
            outputs: vec![dummy_output()],
            lock_time: 0,
            subnetwork_id: subnetwork_id.into(),
            gas: 0,
            payload: payload.into(),
            mass: 0,
            id: vec![].into(),
        }
    }

    fn wrap_signed(
        tx: proto_types::Transaction,
        variant: impl FnOnce(
            proto_types::SignableTransaction,
        ) -> proto_types::signed_transaction::Signed,
    ) -> WalletSignableTransaction {
        let signable = proto_types::SignableTransaction {
            tx: Some(tx),
            entries: vec![],
            calculated_fee: None,
            calculated_non_contextual_masses: None,
        };
        WalletSignableTransaction {
            transaction: Some(proto_types::SignedTransaction {
                signed: Some(variant(signable)),
            }),
            derivation_paths: vec![],
            address_by_input_index: vec![],
            address_by_output_index: vec![],
        }
    }

    fn wrap_partial(tx: proto_types::Transaction) -> WalletSignableTransaction {
        wrap_signed(tx, proto_types::signed_transaction::Signed::Partially)
    }

    fn wrap_fully(tx: proto_types::Transaction) -> WalletSignableTransaction {
        wrap_signed(tx, proto_types::signed_transaction::Signed::Fully)
    }

    fn compute_budget_input(compute_budget: u32) -> proto_types::TransactionInput {
        proto_types::TransactionInput {
            previous_outpoint: Some(proto_types::TransactionOutpoint {
                transaction_id: vec![0u8; 32].into(),
                index: 0,
            }),
            signature_script: vec![].into(),
            sequence: 0,
            sig_op_count: 0,
            compute_budget,
        }
    }

    #[tokio::test]
    async fn validate_accepts_v1_matching_lane_and_payload() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8, 2, 3, 4, 5];
        let tx = make_proto_tx(
            1,
            lane_id.to_vec(),
            payload.clone(),
            vec![compute_budget_input(42)],
        );
        let txs = vec![wrap_partial(tx)];
        caller
            .validate_lane_transaction(&txs, &payload)
            .expect("v1 + matching lane + matching payload + compute_budget input must pass");
    }

    #[tokio::test]
    async fn validate_rejects_native_lane_tx() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8, 2, 3];
        let native = [0u8; 20];
        let tx = make_proto_tx(
            1,
            native.to_vec(),
            payload.clone(),
            vec![compute_budget_input(1)],
        );
        let txs = vec![wrap_partial(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("native lane id must be rejected");
        assert!(err.contains("subnetwork_id"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_wrong_non_native_lane_tx() {
        let configured = lane([0x97, 0xb1, 0x00, 0x00]);
        let other = lane([0x12, 0x34, 0x56, 0x78]);
        let caller = caller_with_lane(configured);
        let payload = vec![9u8];
        let tx = make_proto_tx(
            1,
            other.to_vec(),
            payload.clone(),
            vec![compute_budget_input(1)],
        );
        let txs = vec![wrap_partial(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("wrong lane id must be rejected");
        assert!(err.contains("subnetwork_id"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_v0_transaction() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![0xAB];
        let tx = make_proto_tx(0, lane_id.to_vec(), payload.clone(), vec![]);
        let txs = vec![wrap_partial(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("v0 transaction must be rejected");
        assert!(err.contains("version"), "msg was: {err}");
        assert!(err.contains("expected 1"), "msg was: {err}");
        assert!(err.contains("got 0"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_payload_mismatch() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let expected = vec![1u8, 2, 3];
        let mismatched = vec![9u8, 9, 9];
        let tx = make_proto_tx(
            1,
            lane_id.to_vec(),
            mismatched,
            vec![compute_budget_input(1)],
        );
        let txs = vec![wrap_partial(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &expected)
            .expect_err("payload mismatch must be rejected");
        assert!(err.contains("payload"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_v1_input_using_sig_op_count() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8];
        let legacy_input = proto_types::TransactionInput {
            previous_outpoint: Some(proto_types::TransactionOutpoint {
                transaction_id: vec![0u8; 32].into(),
                index: 0,
            }),
            signature_script: vec![].into(),
            sequence: 0,
            sig_op_count: 1, // illegal on v1
            compute_budget: 42,
        };
        let tx = make_proto_tx(1, lane_id.to_vec(), payload.clone(), vec![legacy_input]);
        let txs = vec![wrap_partial(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("v1 input with non-zero sig_op_count must be rejected");
        assert!(err.contains("sig_op_count"), "msg was: {err}");
        assert!(err.contains("expected 0"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_v1_input_with_zero_compute_budget() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8];
        let zero_mass_input = compute_budget_input(0);
        let tx = make_proto_tx(1, lane_id.to_vec(), payload.clone(), vec![zero_mass_input]);
        let txs = vec![wrap_partial(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("v1 input with compute_budget == 0 must be rejected");
        assert!(err.contains("compute_budget"), "msg was: {err}");
        assert!(err.contains("expected > 0"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_v1_with_no_inputs() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8];
        let tx = make_proto_tx(1, lane_id.to_vec(), payload.clone(), vec![]);
        let txs = vec![wrap_partial(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("v1 tx with zero inputs must be rejected");
        assert!(err.contains("no inputs"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_fully_signed_transaction() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8];
        let tx = make_proto_tx(
            1,
            lane_id.to_vec(),
            payload.clone(),
            vec![compute_budget_input(1)],
        );
        let txs = vec![wrap_fully(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("fully signed transaction must be rejected at validation step");
        assert!(err.contains("Partially"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_empty_tx_vector() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let err = caller
            .validate_lane_transaction(&[], &[])
            .expect_err("empty tx vector must be rejected");
        assert!(err.contains("no unsigned transactions"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_v1_input_with_compute_budget_above_u16_max() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8];
        let oversized = u32::from(u16::MAX) + 1;
        let tx = make_proto_tx(
            1,
            lane_id.to_vec(),
            payload.clone(),
            vec![compute_budget_input(oversized)],
        );
        let txs = vec![wrap_partial(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("compute_budget above u16::MAX must be rejected");
        assert!(err.contains("compute_budget"), "msg was: {err}");
        assert!(err.contains("u16::MAX"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_non_zero_lock_time() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8];
        let mut tx = make_proto_tx(
            1,
            lane_id.to_vec(),
            payload.clone(),
            vec![compute_budget_input(1)],
        );
        tx.lock_time = 42;
        let txs = vec![wrap_partial(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("non-zero lock_time must be rejected on IGRA-lane tx");
        assert!(err.contains("lock_time"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_non_zero_gas() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8];
        let mut tx = make_proto_tx(
            1,
            lane_id.to_vec(),
            payload.clone(),
            vec![compute_budget_input(1)],
        );
        tx.gas = 7;
        let txs = vec![wrap_partial(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("non-zero gas must be rejected on IGRA-lane tx");
        assert!(err.contains("gas"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_empty_outputs() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8];
        let mut tx = make_proto_tx(
            1,
            lane_id.to_vec(),
            payload.clone(),
            vec![compute_budget_input(1)],
        );
        tx.outputs.clear();
        let txs = vec![wrap_partial(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("zero outputs must be rejected");
        assert!(err.contains("outputs"), "msg was: {err}");
        assert!(err.contains("at least 1"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_too_many_outputs() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8];
        let mut tx = make_proto_tx(
            1,
            lane_id.to_vec(),
            payload.clone(),
            vec![compute_budget_input(1)],
        );
        tx.outputs = vec![dummy_output(), dummy_output(), dummy_output()];
        let txs = vec![wrap_partial(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("more than MAX_OUTPUTS_PER_LANE_TX outputs must be rejected");
        assert!(err.contains("outputs"), "msg was: {err}");
        assert!(err.contains("at most"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_empty_subnetwork_id() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8];
        let tx = make_proto_tx(1, vec![], payload.clone(), vec![compute_budget_input(1)]);
        let txs = vec![wrap_partial(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("empty subnetwork_id must be rejected with explicit naming");
        assert!(err.contains("empty subnetwork_id"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_rejects_short_subnetwork_id() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8];
        let tx = make_proto_tx(
            1,
            vec![0x97, 0xb1],
            payload.clone(),
            vec![compute_budget_input(1)],
        );
        let txs = vec![wrap_partial(tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("short subnetwork_id must be rejected");
        assert!(err.contains("2-byte"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_walks_every_tx_and_rejects_off_lane_pre_stage() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let other = lane([0x12, 0x34, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8];
        let prestage = make_proto_tx(1, other.to_vec(), vec![], vec![compute_budget_input(1)]);
        let payload_tx = make_proto_tx(
            1,
            lane_id.to_vec(),
            payload.clone(),
            vec![compute_budget_input(1)],
        );
        let txs = vec![wrap_partial(prestage), wrap_partial(payload_tx)];
        let err = caller
            .validate_lane_transaction(&txs, &payload)
            .expect_err("pre-stage tx on a different lane must be rejected");
        assert!(err.contains("pre-stage tx #0"), "msg was: {err}");
        assert!(err.contains("subnetwork_id"), "msg was: {err}");
    }

    #[tokio::test]
    async fn validate_accepts_multi_tx_when_all_on_lane_and_last_carries_payload() {
        let lane_id = lane([0x97, 0xb1, 0x00, 0x00]);
        let caller = caller_with_lane(lane_id);
        let payload = vec![1u8, 2, 3];
        let prestage = make_proto_tx(
            1,
            lane_id.to_vec(),
            vec![], // pre-stage tx carries no IGRA payload
            vec![compute_budget_input(1)],
        );
        let payload_tx = make_proto_tx(
            1,
            lane_id.to_vec(),
            payload.clone(),
            vec![compute_budget_input(1)],
        );
        let txs = vec![wrap_partial(prestage), wrap_partial(payload_tx)];
        caller
            .validate_lane_transaction(&txs, &payload)
            .expect("multi-tx batch with all txs on lane and payload on the last must pass");
    }

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

    #[test]
    fn test_lane_validation_failed_to_app_error_conversion() {
        let wallet_error = WalletCallerError::LaneValidationFailed("test reason".to_string());
        let app_error: AppError = wallet_error.into();
        match app_error {
            AppError::WalletError(msg) => {
                assert!(
                    msg.contains("IGRA lane validation failed"),
                    "msg was: {msg}"
                );
                assert!(msg.contains("test reason"), "msg was: {msg}");
            }
            _ => panic!("expected WalletError"),
        }
    }
}
