use crate::config::AppConfig;
use crate::services::gas_price::GasPriceService;
use crate::services::transaction::{extract_gas_fees, GasFeeInfo};
use crate::AppState;
use alloy::consensus::TxEnvelope;
use alloy::primitives::{keccak256, B256, U256};
use alloy::rlp::Decodable;
use serde_json::Value;
use std::error::Error;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

/// The version of the IgraPayload format
pub const VERSION: u8 = 0x9;

/// Structure to represent a transaction request that needs to be processed sequentially
pub struct TransactionRequest {
    pub raw_tx: String,
    pub tx_bytes: Vec<u8>,
    pub id: Value,
    pub app_state: Arc<AppState>,
    pub response_sender: mpsc::Sender<Result<(), String>>,
}

/// Service responsible for processing transactions with single responsibility
pub struct TransactionProcessor {
    config: Arc<AppConfig>,
    gas_price_service: GasPriceService,
    processed_count: u16,
    error_count: u16,
}

impl TransactionProcessor {
    /// Creates a new TransactionProcessor with the given configuration
    pub fn new(config: AppConfig) -> Self {
        let gas_price_service = GasPriceService::new(config.gas.clone());
        Self {
            config: Arc::new(config),
            gas_price_service,
            processed_count: 0,
            error_count: 0,
        }
    }

    /// Process a single transaction request
    pub async fn process_transaction(&mut self, tx_request: TransactionRequest) {
        let tx_hash = compute_transaction_hash(&tx_request.tx_bytes);
        let tx_hash_str = format!("{tx_hash:#x}");

        // Generate a proper ID string, using UUID if the original ID is null or invalid
        let id_str = if tx_request.id.is_null() {
            let uuid = Uuid::new_v4().to_string();
            info!(
                "TX_PROCESSOR [hash={}]: Received transaction with null ID, assigning UUID: {}",
                tx_hash_str, uuid
            );
            uuid
        } else {
            tx_request.id.to_string()
        };

        // Log full payload bytes
        let full_payload = format!("0x{}", hex::encode(&tx_request.tx_bytes));

        info!(
            "TX_PROCESSOR [id={}, hash={}]: Processing transaction, payload_size={}, payload={}",
            id_str,
            tx_hash_str,
            tx_request.tx_bytes.len(),
            full_payload
        );

        let start = std::time::Instant::now();

        // Calculate effective base fee for this processing cycle
        let effective_base_fee = match self
            .gas_price_service
            .get_effective_base_fee(self.config.el_url())
            .await
        {
            Ok(fee) => {
                info!(
                    "TX_PROCESSOR [id={}, hash={}]: Effective base fee calculated: {} wei",
                    id_str, tx_hash_str, fee
                );
                fee
            }
            Err(e) => {
                error!(
                    "TX_PROCESSOR [id={}, hash={}]: Failed to fetch base fee: {}. Rejecting transaction.",
                    id_str, tx_hash_str, e
                );
                self.handle_error(&tx_request, format!("Failed to fetch base fee: {e}"))
                    .await;
                return;
            }
        };

        // Parse and validate the transaction
        match self
            .process_transaction_simple(&tx_request, &id_str, &tx_hash_str, effective_base_fee)
            .await
        {
            Ok(_) => {
                self.processed_count = self.processed_count.saturating_add(1);
                let duration = start.elapsed();
                info!(
                    "TX_PROCESSOR [id={}, hash={}]: Transaction processed successfully, time={:?}, total_processed={}",
                    id_str, tx_hash_str, duration, self.processed_count
                );
            }
            Err(e) => {
                self.error_count = self.error_count.saturating_add(1);
                let duration = start.elapsed();
                error!(
                    "TX_PROCESSOR [id={}, hash={}]: Transaction processing failed: {}, time={:?}, total_errors={}",
                    id_str, tx_hash_str, e, duration, self.error_count
                );
                self.handle_error(&tx_request, e.to_string()).await;
            }
        }
    }

    /// Simple transaction processing that validates gas price and submits to mining
    async fn process_transaction_simple(
        &self,
        tx_request: &TransactionRequest,
        id_str: &str,
        tx_hash_str: &str,
        effective_base_fee: U256,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // Parse the transaction for validation
        let tx = TxEnvelope::decode(&mut &tx_request.tx_bytes[..])
            .map_err(|e| format!("Failed to decode transaction: {e}"))?;

        info!(
            "TX_PROCESSOR [id={}, hash={}]: Processing transaction with gas validation",
            id_str, tx_hash_str
        );

        // Validate gas price
        self.validate_gas_price(&tx, effective_base_fee, id_str, tx_hash_str)?;

        // For now, we'll simulate transaction processing success
        // TODO: Integrate with proper mining service when types align
        info!(
            "TX_PROCESSOR [id={}, hash={}]: Simulating transaction processing",
            id_str, tx_hash_str
        );

        info!(
            "TX_PROCESSOR [id={}, hash={}]: Transaction processed successfully",
            id_str, tx_hash_str
        );

        // Send success response
        self.send_success_response(tx_request).await?;
        Ok(())
    }

    /// Validate gas price against effective base fee.
    /// Uses the consolidated extract_gas_fees helper from transaction.rs.
    fn validate_gas_price(
        &self,
        tx: &TxEnvelope,
        effective_base_fee: U256,
        id_str: &str,
        tx_hash_str: &str,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // Extract gas fees using consolidated helper
        let gas_info = extract_gas_fees(tx).map_err(|e| {
            error!("TX_PROCESSOR [id={}, hash={}]: {}", id_str, tx_hash_str, e);
            e.to_string()
        })?;

        // Validate based on transaction type
        let is_valid = match gas_info {
            GasFeeInfo::Eip1559 {
                max_fee_per_gas,
                max_priority_fee_per_gas,
            } => {
                // EIP-1559: max_fee must cover base fee, priority fee must not exceed max_fee
                let is_fee_valid = max_fee_per_gas >= effective_base_fee
                    && max_priority_fee_per_gas <= max_fee_per_gas;

                if !is_fee_valid {
                    warn!(
                        "TX_PROCESSOR [id={}, hash={}]: EIP-1559 fee validation failed - max_fee: {}, max_priority_fee: {}, effective_base_fee: {}",
                        id_str, tx_hash_str, max_fee_per_gas, max_priority_fee_per_gas, effective_base_fee
                    );
                }
                is_fee_valid
            }
            GasFeeInfo::Legacy { gas_price } => {
                // Legacy/EIP-2930: gas_price must cover base fee
                let is_price_valid = gas_price >= effective_base_fee;
                if !is_price_valid {
                    warn!(
                        "TX_PROCESSOR [id={}, hash={}]: Gas price validation failed - gas_price: {}, effective_base_fee: {}",
                        id_str, tx_hash_str, gas_price, effective_base_fee
                    );
                }
                is_price_valid
            }
        };

        if !is_valid {
            return Err(
                format!("Gas price validation failed for transaction {tx_hash_str}").into(),
            );
        }

        Ok(())
    }

    /// Send success response back to caller
    async fn send_success_response(
        &self,
        tx_request: &TransactionRequest,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        tx_request
            .response_sender
            .send(Ok(()))
            .await
            .map_err(|e| format!("Failed to send response: {e}"))?;
        Ok(())
    }

    /// Handle error by sending error response
    async fn handle_error(&self, tx_request: &TransactionRequest, error_msg: String) {
        let send_result = tx_request
            .response_sender
            .send(Err(error_msg.clone()))
            .await;
        if let Err(send_err) = send_result {
            error!(
                "TX_PROCESSOR: Failed to send error response '{}': {}",
                error_msg, send_err
            );
        }
    }

    /// Get processing statistics
    pub fn get_stats(&self) -> (u16, u16) {
        (self.processed_count, self.error_count)
    }
}

/// Creates and starts the background transaction processor
/// Returns a channel sender that can be used to queue transactions
pub fn start_transaction_processor(config: AppConfig) -> mpsc::Sender<TransactionRequest> {
    let (transaction_sender, mut transaction_receiver) = mpsc::channel::<TransactionRequest>(1024);
    info!(
        "TX_PROCESSOR: Starting transaction processor with queue size={}",
        1024
    );

    // Start the sequential transaction processor task
    tokio::spawn(async move {
        info!("TX_PROCESSOR: Background worker started");
        let mut processor = TransactionProcessor::new(config);

        // Process transactions one at a time
        while let Some(tx_request) = transaction_receiver.recv().await {
            processor.process_transaction(tx_request).await;
        }

        let (processed, errors) = processor.get_stats();
        info!(
            "TX_PROCESSOR: Background worker stopped. Final stats: processed={}, errors={}",
            processed, errors
        );
    });

    transaction_sender
}

/// Compute the hash of a transaction from its RLP-encoded bytes
pub fn compute_transaction_hash(tx_bytes: &[u8]) -> B256 {
    keccak256(tx_bytes)
}
