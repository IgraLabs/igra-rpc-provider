use crate::clients::wallet_caller::TransactionParams;
use crate::config::AppConfig;
use crate::error::AppError;
use crate::errors::transaction::TransactionError;
use crate::errors::ToJsonRpcError;
use crate::services::gas_price::GasPriceService;
use crate::types::rpc::{IgraPayload, RpcRequest, TxTypeId};
use crate::AppState;
use alloy::consensus::TxEnvelope;
use alloy::primitives::{keccak256, B256, U256};
use alloy::rlp::Decodable;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// The version of the IgraPayload format
pub const VERSION: u8 = 0x9;

/// Maximum number of transactions that can be queued for sequential processing.
const TRANSACTION_QUEUE_CAPACITY: usize = 1024;

/// Maximum time (in seconds) to wait for a queued transaction to be processed
/// before returning a timeout error to the caller.
const PROCESSING_TIMEOUT_SECS: u64 = 120;

/// Transaction type constants for Ethereum transaction types
pub mod tx_types {
    pub const LEGACY: u8 = 0; // Legacy transaction
    pub const EIP2930: u8 = 1; // EIP-2930 (Access List)
    pub const EIP1559: u8 = 2; // EIP-1559 (Fee Market)
    pub const BLOB: u8 = 3; // EIP-4844 (Blob transactions)
}

/// Transaction validation context containing all necessary information
/// for validation operations
#[derive(Debug)]
struct TransactionValidationContext {
    transaction_id: String,
    transaction_hash: String,
    transaction_bytes: Vec<u8>,
    effective_base_fee: Option<U256>,
}

impl TransactionValidationContext {
    fn new(tx_id: String, tx_bytes: Vec<u8>, base_fee: Option<U256>) -> Self {
        let tx_hash = format!("{:#x}", compute_transaction_hash(&tx_bytes));
        Self {
            transaction_id: tx_id,
            transaction_hash: tx_hash,
            transaction_bytes: tx_bytes,
            effective_base_fee: base_fee,
        }
    }

    fn id(&self) -> &str {
        &self.transaction_id
    }

    fn hash(&self) -> &str {
        &self.transaction_hash
    }

    fn bytes(&self) -> &[u8] {
        &self.transaction_bytes
    }

    fn base_fee(&self) -> Option<U256> {
        self.effective_base_fee
    }
}

// Structure to represent a transaction request that needs to be processed sequentially
pub struct TransactionRequest {
    pub raw_tx: String,
    pub tx_bytes: Vec<u8>,
    pub id: Value,
    pub app_state: Arc<AppState>,
    pub response_sender: mpsc::Sender<Result<(), String>>,
}

/// Creates and starts the background transaction processor
/// Returns a channel sender that can be used to queue transactions
pub fn start_transaction_processor(config: AppConfig) -> mpsc::Sender<TransactionRequest> {
    let (transaction_sender, mut transaction_receiver) =
        mpsc::channel::<TransactionRequest>(TRANSACTION_QUEUE_CAPACITY);
    info!(
        "TX_PROCESSOR: Starting transaction processor with queue size={}",
        TRANSACTION_QUEUE_CAPACITY
    );

    // Start the sequential transaction processor task
    let config = Arc::new(config);
    tokio::spawn(async move {
        info!("TX_PROCESSOR: Background worker started");

        let mut processed_count: u16 = 0;
        let mut error_count: u16 = 0;

        // Create GasPriceService for fee validation
        let gas_price_service = GasPriceService::new(config.gas.clone());

        // Process transactions one at a time
        while let Some(tx_request) = transaction_receiver.recv().await {
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
            let effective_base_fee = match gas_price_service
                .get_effective_base_fee(config.el_url())
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
                    let error_msg = format!("Failed to fetch base fee: {e}");
                    let send_response_result =
                        tx_request.response_sender.send(Err(error_msg)).await;
                    if let Err(send_err) = send_response_result {
                        error!(
                            "TX_PROCESSOR [id={}, hash={}]: Failed to send error response: {}",
                            id_str, tx_hash_str, send_err
                        );
                    }
                    continue;
                }
            };

            // Create a Value with the proper ID for passing to process_wallet_call
            let id_value = if tx_request.id.is_null() {
                json!(id_str)
            } else {
                tx_request.id.clone()
            };

            // Create validation context for better organization
            let validation_context = TransactionValidationContext::new(
                id_str.clone(),
                tx_request.tx_bytes.clone(),
                Some(effective_base_fee),
            );

            // Validate transaction fee before processing
            let fee_validation_result = validate_transaction_fees(&validation_context);

            // If fee validation failed, don't proceed to wallet call
            let process_wallet_result = match fee_validation_result {
                Ok(_) => {
                    // Call the wallet sequentially for each transaction
                    process_wallet_call(
                        &tx_request.tx_bytes,
                        &config,
                        id_value,
                        tx_request.app_state,
                    )
                    .await
                }
                Err(transaction_error) => {
                    let error_msg = transaction_error.to_string();
                    error!(
                        "TX_PROCESSOR [id={}, hash={}]: Fee validation failed: {}",
                        id_str, tx_hash_str, error_msg
                    );
                    Err(error_msg)
                }
            };
            let response = match process_wallet_result {
                Ok(_) => {
                    let duration = start.elapsed();
                    processed_count = processed_count.saturating_add(1);
                    info!("TX_PROCESSOR [id={}, hash={}]: Transaction processed successfully, time={:?}, payload_size={}, payload={}, total_success={}, total_errors={}",
                        id_str, tx_hash_str, duration, tx_request.tx_bytes.len(), full_payload, processed_count, error_count);
                    Ok(())
                }
                Err(err) => {
                    let duration = start.elapsed();
                    error_count = error_count.saturating_add(1);
                    error!("TX_PROCESSOR [id={}, hash={}]: Transaction failed: {}, time={:?}, payload_size={}, total_success={}, total_errors={}",
                        id_str, tx_hash_str, err, duration, tx_request.tx_bytes.len(), processed_count, error_count);
                    Err(err)
                }
            };
            // Send the response back to the sender
            let send_response_result = tx_request.response_sender.send(response).await;
            if let Err(e) = send_response_result {
                error!(
                    "TX_PROCESSOR [id={}, hash={}]: Failed to send response: {}",
                    id_str, tx_hash_str, e
                );
            }
        }
    });

    transaction_sender
}

// Process transaction immediately but queue for sequential wallet calls
pub async fn process_transaction(req: RpcRequest, state: Arc<AppState>) -> Value {
    // If ID is null, generate a UUID
    let id_value = if req.id.is_null() {
        let uuid = Uuid::new_v4().to_string();
        json!(uuid)
    } else {
        req.id.clone()
    };

    let id = id_value.to_string();

    // Get the full transaction params for logging
    let full_params = match req.params.get(0) {
        Some(param) => param.to_string(),
        None => "empty".to_string(),
    };

    info!(
        "TX [id={}]: Processing transaction request, params={}",
        id, full_params
    );

    // Validate transaction
    let (validation_result, tx_bytes_opt) = validate_transaction_request(&req, None);

    // If validation failed, return the error
    if let Err(error_json) = validation_result {
        error!("TX [id={}]: Validation failed: {:?}", id, error_json);
        return error_json;
    }

    let tx_bytes = match tx_bytes_opt {
        Some(bytes) => bytes,
        None => {
            // This case should theoretically not be reached if validation passes
            // but we handle it gracefully to avoid a panic.
            let err_msg = "Transaction validation passed but no bytes were returned";
            error!("TX [id={}]: {}", id, err_msg);
            return json!({
                "jsonrpc": "2.0",
                "error": { "code": -32000, "message": err_msg },
                "id": id_value
            });
        }
    };

    // Log full bytes
    let full_bytes = format!("0x{}", hex::encode(&tx_bytes));

    debug!(
        "TX [id={}]: Transaction validated successfully, tx_bytes_len={}, bytes={}",
        id,
        tx_bytes.len(),
        full_bytes
    );

    // Compute transaction hash immediately
    let tx_hash = compute_transaction_hash(&tx_bytes);
    let tx_hash_str = format!("{tx_hash:#x}");

    // Log available capacity
    let available = state.transaction_sender.capacity();
    info!("TX [id={}, hash={}]: Computed hash, now queueing for background processing, payload_size={}, available_capacity={}",
        id, tx_hash_str, tx_bytes.len(), available);

    let (response_sender, mut response_receiver) = mpsc::channel::<Result<(), String>>(1);
    // Queue the transaction for sequential processing
    let tx_request = TransactionRequest {
        raw_tx: req.params[0].as_str().unwrap_or("").to_string(),
        tx_bytes,
        id: id_value.clone(),
        app_state: state.clone(),
        response_sender,
    };

    let queue_start = std::time::Instant::now();
    if let Err(e) = state.transaction_sender.send(tx_request).await {
        error!(
            "TX [id={}, hash={}]: Failed to queue transaction: {}, available_capacity={}",
            id,
            tx_hash_str,
            e,
            state.transaction_sender.capacity()
        );
        return TransactionError::queue_full(TRANSACTION_QUEUE_CAPACITY)
            .to_json_rpc_error(id_value);
    }
    let queue_time = queue_start.elapsed();
    info!(
        "TX [id={}, hash={}]: Transaction queued successfully, queue_time={:?}, available_capacity={}",
        id, tx_hash_str, queue_time, state.transaction_sender.capacity()
    );

    let response = match timeout(
        Duration::from_secs(PROCESSING_TIMEOUT_SECS),
        response_receiver.recv(),
    )
    .await
    {
        Ok(Some(result)) => result,
        Ok(None) => {
            error!(
                "TX [id={}, hash={}]: Response channel closed unexpectedly (processor may have crashed)",
                id, tx_hash_str
            );
            return TransactionError::InternalError(
                "Transaction processor channel closed unexpectedly".to_string(),
            )
            .to_json_rpc_error(id_value);
        }
        Err(_) => {
            error!(
                "TX [id={}, hash={}]: Processing timed out after {}s",
                id, tx_hash_str, PROCESSING_TIMEOUT_SECS
            );
            return TransactionError::processing_timeout(PROCESSING_TIMEOUT_SECS)
                .to_json_rpc_error(id_value);
        }
    };

    if let Err(e) = response {
        return json!({
            "jsonrpc": "2.0",
            "error": {
                "code": -32603,
                "message": format!("Error processing transaction: {}", e)
            },
            "id": id_value
        });
    }

    debug!(
        "TX [id={}, hash={}]: Returning hash to client",
        id, tx_hash_str
    );
    json!({
        "jsonrpc": "2.0",
        "result": tx_hash_str,
        "id": id_value
    })
}

/// Validates transaction fees using the validation context
fn validate_transaction_fees(
    context: &TransactionValidationContext,
) -> Result<(), TransactionError> {
    validate_comprehensive_transaction(context)
}

/// Parse transaction from validation context with enhanced error handling
fn parse_transaction_with_context(
    context: &TransactionValidationContext,
) -> Result<(TxEnvelope, u8), TransactionError> {
    match parse_rlp_transaction(context.bytes()) {
        Ok((tx, tx_type)) => {
            debug!(
                "TX_VALIDATION [id={}, hash={}]: Successfully parsed {} transaction",
                context.id(),
                context.hash(),
                get_transaction_type_name(tx_type)
            );
            Ok((tx, tx_type))
        }
        Err(app_error) => {
            warn!(
                "TX_VALIDATION [id={}, hash={}]: Failed to parse transaction: {}",
                context.id(),
                context.hash(),
                app_error
            );
            Err(TransactionError::invalid_transaction_format(
                app_error.to_string(),
            ))
        }
    }
}

/// Validates an incoming transaction request and returns the decoded transaction bytes
pub fn validate_transaction_request(
    req: &RpcRequest,
    effective_base_fee: Option<U256>,
) -> (Result<(), Value>, Option<Vec<u8>>) {
    let transaction_id = extract_transaction_id(req);

    // Step 1: Extract and validate raw transaction format
    let raw_tx = match extract_raw_transaction(req, &transaction_id) {
        Ok(tx) => tx,
        Err(error) => return (Err(error), None),
    };

    // Step 2: Decode hex transaction to bytes
    let tx_bytes = match decode_hex_transaction(&raw_tx, &transaction_id) {
        Ok(bytes) => bytes,
        Err(error) => return (Err(error), None),
    };

    // Step 3: Validate RLP structure and gas fees
    let validation_context = TransactionValidationContext::new(
        transaction_id.clone(),
        tx_bytes.clone(),
        effective_base_fee,
    );

    if let Err(transaction_error) = validate_comprehensive_transaction(&validation_context) {
        return (
            Err(transaction_error.to_json_rpc_error(req.id.clone())),
            None,
        );
    }

    debug!(
        "TX_VALIDATE [id={}, hash={}]: Validation successful",
        validation_context.id(),
        validation_context.hash()
    );
    (Ok(()), Some(tx_bytes))
}

/// Comprehensive transaction validation that combines RLP and fee validation
fn validate_comprehensive_transaction(
    context: &TransactionValidationContext,
) -> Result<(), TransactionError> {
    // Parse and validate RLP structure
    let (transaction, tx_type) = parse_transaction_with_context(context)?;

    // Validate fees if base fee is available
    if let Some(base_fee) = context.base_fee() {
        validate_gas_fee_with_type(&transaction, base_fee, tx_type, context.hash())?;
    }

    debug!(
        "TX_VALIDATION [id={}, hash={}]: Validation completed successfully",
        context.id(),
        context.hash()
    );

    Ok(())
}

/// Extracts and formats the transaction ID from the request
fn extract_transaction_id(req: &RpcRequest) -> String {
    if req.id.is_null() {
        "null".to_string()
    } else {
        req.id.to_string()
    }
}

/// Extracts the raw transaction string from the request parameters
fn extract_raw_transaction(req: &RpcRequest, id: &str) -> Result<String, Value> {
    let raw_tx = req.params[0].as_str().unwrap_or("");

    debug!(
        "TX_VALIDATE [id={}]: Validating transaction, raw_tx_len={}, raw_tx={}",
        id,
        raw_tx.len(),
        raw_tx
    );

    if !raw_tx.starts_with("0x") {
        warn!(
            "TX_VALIDATE [id={}]: Raw transaction doesn't start with '0x'",
            id
        );
        return Err(AppError::InvalidTransactionFormat.to_json_rpc_error(req.id.clone()));
    }

    Ok(raw_tx.to_string())
}

/// Decodes hex string to transaction bytes with proper validation
fn decode_hex_transaction(raw_tx: &str, id: &str) -> Result<Vec<u8>, Value> {
    // Remove "0x" prefix and pad with leading zero if the length is odd
    let hex_str = if !raw_tx.len().is_multiple_of(2) {
        debug!("TX_VALIDATE [id={}]: Odd-length hex string, padding", id);
        format!("0{}", &raw_tx[2..])
    } else {
        raw_tx[2..].to_string()
    };

    // Decode from hex
    match hex::decode(hex_str) {
        Ok(bytes) => {
            let hash = format!("{:#x}", compute_transaction_hash(&bytes));
            let full_bytes = format!("0x{}", hex::encode(&bytes));

            debug!(
                "TX_VALIDATE [id={}, hash={}]: Hex decoded successfully, bytes_len={}, bytes={}",
                id,
                hash,
                bytes.len(),
                full_bytes
            );
            Ok(bytes)
        }
        Err(e) => {
            warn!("TX_VALIDATE [id={}]: Invalid hex format: {}", id, e);
            Err(AppError::InvalidTransactionFormat
                .to_json_rpc_error(serde_json::Value::String(id.to_string())))
        }
    }
}

/// Computes transaction hash from raw bytes
pub fn compute_transaction_hash(tx_bytes: &[u8]) -> B256 {
    keccak256(tx_bytes)
}

/// Processes a transaction through the wallet caller.
///
/// Builds the IGRA payload (optionally ZLIB-compressed), hands it to the
/// kaswallet daemon for IGRA-lane construction, validates the lane,
/// signs, and broadcasts.
pub async fn process_wallet_call(
    tx_bytes: &[u8],
    config: &AppConfig,
    id: Value,
    app_state: Arc<AppState>,
) -> Result<Value, String> {
    // Post-Toccata: lane binding is on the consensus side via the
    // configured subnetwork_id, not via a payload nonce. Keep the
    // 4-byte nonce slot zeroed for wire-format stability with
    // downstream payload parsers.
    let nonce = [0u8; 4];

    // Conditionally compress — use zipped only if it actually saves space
    let (l2_data, tx_type_id) = match compress_zlib(tx_bytes) {
        Ok(compressed) if compressed.len() < tx_bytes.len() => {
            debug!(
                "TX_PROCESSOR: Using ZippedPayload, compressed {}->{} bytes",
                tx_bytes.len(),
                compressed.len()
            );
            (compressed, TxTypeId::ZippedPayload)
        }
        Ok(_) => {
            debug!(
                "TX_PROCESSOR: Using UnzippedPayload, {} bytes (compression not beneficial)",
                tx_bytes.len()
            );
            (tx_bytes.to_vec(), TxTypeId::UnzippedPayload)
        }
        Err(e) => {
            warn!(
                "TX_PROCESSOR: ZLIB compression failed, falling back to unzipped ({} bytes): {e}",
                tx_bytes.len()
            );
            (tx_bytes.to_vec(), TxTypeId::UnzippedPayload)
        }
    };

    let igra_payload = IgraPayload {
        version: VERSION,
        tx_type_id,
        l2_data,
        nonce,
    };
    // Hash the ORIGINAL (uncompressed) bytes — this is the L2 tx hash returned to user
    let tx_hash = compute_transaction_hash(tx_bytes);
    let tx_hash_str = format!("{tx_hash:#x}");

    // Serialize the payload
    let final_payload_bytes = match serialize_payload(&igra_payload) {
        Ok(bytes) => bytes,
        Err(e) => {
            let error_message = format!("Failed to serialize payload: {e}");
            error!(
                "TX_PROCESSOR [hash={}]: Serialization failed: {}",
                tx_hash_str, error_message
            );
            return Err(error_message);
        }
    };

    let wallet_payload_bytes = final_payload_bytes;

    // Call the KASPA Wallet for sending the transaction to the Base Layer
    info!(
        "WALLET_CALL [hash={}]: Connecting to KASPA Wallet at {}, payload_size={}",
        tx_hash_str,
        config.wallet.wallet_daemon_uri,
        wallet_payload_bytes.len()
    );

    let wallet_caller = app_state.wallet_caller.clone();

    info!(
        "WALLET_CALL [hash={}]: Sending transaction to wallet, payload_size={}",
        tx_hash_str,
        wallet_payload_bytes.len()
    );
    let send_start = std::time::Instant::now();

    // Capture payload size before moving it
    let payload_size = wallet_payload_bytes.len();

    let transaction_params = TransactionParams::send_all(
        wallet_caller.default_to_address().to_string(),
        wallet_payload_bytes,
        Some(tx_hash_str.clone()),
    );

    if let Err(err) = wallet_caller
        .create_sign_and_broadcast_igra_lane_transaction(transaction_params, &config.retry)
        .await
    {
        let error_msg = format!("KASPA Wallet call failed: {err}");
        let duration = send_start.elapsed();
        error!(
            "WALLET_CALL [hash={}]: Send failed: {}, time={:?}",
            tx_hash_str, error_msg, duration
        );
        return Err(error_msg);
    }

    let send_time = send_start.elapsed();

    info!(
        "WALLET_CALL [hash={}]: Transaction accepted by wallet, payload_size={}, send_time={:?}",
        tx_hash_str, payload_size, send_time
    );

    // Create success response with hash
    let response = json!({
        "jsonrpc": "2.0",
        "result": tx_hash_str,
        "id": id
    });

    Ok(response)
}

/// ZLIB compression level — pinned to the standard default (6) so that compressed
/// payloads are reproducible regardless of future library default changes.
const ZLIB_COMPRESSION_LEVEL: u32 = 6;

/// Compresses data using ZLIB compression with a fixed level for reproducibility.
fn compress_zlib(data: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut encoder = ZlibEncoder::new(
        Vec::with_capacity(data.len()),
        Compression::new(ZLIB_COMPRESSION_LEVEL),
    );
    encoder.write_all(data)?;
    encoder.finish()
}

/// Serializes an `IgraPayload` into a byte vector according to the new format.
///
/// The format is:
/// - `version` (4 bits) + `tx_type_id` (4 bits) in one byte
/// - `l2_data` (variable length)
/// - `nonce` (4 bytes)
pub fn serialize_payload(payload: &IgraPayload) -> Result<Vec<u8>, AppError> {
    // Validate payload fields before serialization
    if payload.l2_data.is_empty() {
        return Err(AppError::SerializationError(
            "l2_data cannot be empty".to_string(),
        ));
    }

    if payload.version > 0x0F {
        return Err(AppError::SerializationError(format!(
            "Version must be a 4-bit value, but got {:#x}",
            payload.version
        )));
    }

    let mut buffer = Vec::new();

    // 1. Version (4 bits) and TxTypeId (4 bits)
    let version_and_type_id = (payload.version << 4) | (payload.tx_type_id as u8);
    buffer.push(version_and_type_id);

    // 2. L2 Data (variable length)
    buffer.extend_from_slice(&payload.l2_data);

    // 3. Nonce (4 bytes)
    buffer.extend_from_slice(&payload.nonce);

    Ok(buffer)
}

/// Detect transaction type from RLP-encoded bytes
/// Returns the transaction type byte based on the RLP encoding structure
pub fn detect_transaction_type(data: &[u8]) -> Result<u8, AppError> {
    if data.is_empty() {
        return Err(AppError::Internal("Empty transaction data".to_string()));
    }

    // Check if the first byte indicates a typed transaction (EIP-2718)
    // Typed transactions start with a transaction type byte (0x01, 0x02, 0x03, etc.)
    // Legacy transactions start with RLP list encoding (0xc0 or higher)
    let first_byte = data[0];

    if first_byte < 0x80 {
        // This is a typed transaction - first byte is the transaction type
        Ok(first_byte)
    } else {
        // This is a legacy transaction (starts with RLP list encoding)
        Ok(tx_types::LEGACY)
    }
}

/// Maximum transaction size in bytes (128KB - sufficient for all Ethereum transaction types)
/// This limit prevents DoS attacks via oversized RLP payloads.
const MAX_TRANSACTION_SIZE: usize = 128 * 1024;

/// Parse RLP-encoded transaction data and return both the transaction and its type.
///
/// Alloy's TxEnvelope automatically handles all transaction types (Legacy, EIP-2930, EIP-1559, EIP-4844)
/// without requiring manual fallback parsing.
///
/// # Security
/// - Validates input size before decoding to prevent DoS via oversized payloads
/// - Rejects unsupported transaction types (EIP-4844, EIP-7702)
pub fn parse_rlp_transaction(data: &[u8]) -> Result<(TxEnvelope, u8), AppError> {
    // Validate size before decoding to prevent DoS attacks
    if data.len() > MAX_TRANSACTION_SIZE {
        warn!(
            "TX_PARSE: Transaction too large: {} bytes (max: {} bytes)",
            data.len(),
            MAX_TRANSACTION_SIZE
        );
        return Err(AppError::Internal(format!(
            "Transaction too large: {} bytes (max: {} bytes)",
            data.len(),
            MAX_TRANSACTION_SIZE
        )));
    }

    let tx_type = detect_transaction_type(data)?;

    if tx_type >= tx_types::BLOB {
        return Err(AppError::Internal(format!(
            "Unsupported transaction type: {tx_type}"
        )));
    }

    let tx = TxEnvelope::decode(&mut &data[..]).map_err(|e| {
        warn!("Failed to decode type {} transaction: {:?}", tx_type, e);
        AppError::Internal(format!(
            "Failed to decode transaction (type {tx_type}): {e}"
        ))
    })?;

    debug!(
        "Successfully decoded type {} transaction using Alloy decoder",
        tx_type
    );

    Ok((tx, tx_type))
}

/// Get human-readable transaction type name for logging
pub fn get_transaction_type_name(tx_type: u8) -> &'static str {
    match tx_type {
        tx_types::LEGACY => "Legacy",
        tx_types::EIP2930 => "EIP-2930",
        tx_types::EIP1559 => "EIP-1559",
        tx_types::BLOB => "EIP-4844 (Blob)",
        _ => "Unknown",
    }
}

/// Extracted gas fee information from a transaction.
/// This consolidates the pattern matching logic used across validation functions.
#[derive(Debug, Clone)]
pub enum GasFeeInfo {
    /// Legacy and EIP-2930 transactions use a single gas_price field
    Legacy { gas_price: U256 },
    /// EIP-1559 transactions use max_fee and max_priority_fee fields
    Eip1559 {
        max_fee_per_gas: U256,
        max_priority_fee_per_gas: U256,
    },
}

/// Extract gas fee information from a TxEnvelope.
/// This is the single source of truth for gas fee extraction, used by both
/// validation functions in transaction.rs and transaction_processor.rs.
pub fn extract_gas_fees(tx: &TxEnvelope) -> Result<GasFeeInfo, AppError> {
    match tx {
        TxEnvelope::Legacy(signed_tx) => Ok(GasFeeInfo::Legacy {
            gas_price: U256::from(signed_tx.tx().gas_price),
        }),
        TxEnvelope::Eip2930(signed_tx) => Ok(GasFeeInfo::Legacy {
            gas_price: U256::from(signed_tx.tx().gas_price),
        }),
        TxEnvelope::Eip1559(signed_tx) => {
            let inner = signed_tx.tx();
            Ok(GasFeeInfo::Eip1559 {
                max_fee_per_gas: U256::from(inner.max_fee_per_gas),
                max_priority_fee_per_gas: U256::from(inner.max_priority_fee_per_gas),
            })
        }
        TxEnvelope::Eip4844(_) => Err(AppError::Internal(
            "EIP-4844 blob transactions are not supported".into(),
        )),
        TxEnvelope::Eip7702(_) => Err(AppError::Internal(
            "EIP-7702 transactions are not supported".into(),
        )),
    }
}

/// Main validation function that routes to appropriate validation based on transaction type
pub fn validate_gas_fee_with_type(
    tx: &TxEnvelope,
    min_protocol_fee: U256,
    tx_type: u8,
    tx_hash: &str,
) -> Result<(), crate::errors::transaction::TransactionError> {
    use crate::errors::transaction::TransactionError;

    match tx_type {
        tx_types::LEGACY | tx_types::EIP2930 => {
            validate_legacy_or_eip2930(tx, min_protocol_fee, tx_type, tx_hash)
        }
        tx_types::EIP1559 => validate_eip1559(tx, min_protocol_fee, tx_hash),
        tx_type if tx_type >= tx_types::BLOB => {
            error!(
                "TX_VALIDATION [hash={}]: Unsupported transaction type: {} ({})",
                tx_hash,
                tx_type,
                get_transaction_type_name(tx_type)
            );
            Err(TransactionError::invalid_transaction_format(format!(
                "Unsupported transaction type: {} ({})",
                tx_type,
                get_transaction_type_name(tx_type)
            )))
        }
        _ => {
            error!(
                "TX_VALIDATION [hash={}]: Unknown transaction type: {}",
                tx_hash, tx_type
            );
            Err(TransactionError::invalid_transaction_format(format!(
                "Unknown transaction type: {tx_type}"
            )))
        }
    }
}

/// Validate Legacy (Type 0) and EIP-2930 (Type 1) transactions
/// Both use gas_price field and validate: gas_price >= min_protocol_fee
pub fn validate_legacy_or_eip2930(
    tx: &TxEnvelope,
    min_protocol_fee: U256,
    tx_type: u8,
    tx_hash: &str,
) -> Result<(), crate::errors::transaction::TransactionError> {
    use crate::errors::transaction::TransactionError;

    let tx_type_name = get_transaction_type_name(tx_type);

    // Extract gas_price using pattern matching on TxEnvelope
    let gas_price = match tx {
        TxEnvelope::Legacy(signed_tx) => U256::from(signed_tx.tx().gas_price),
        TxEnvelope::Eip2930(signed_tx) => U256::from(signed_tx.tx().gas_price),
        _ => {
            warn!(
                "TX_VALIDATION [hash={}]: Expected Legacy or EIP-2930 transaction, got different type",
                tx_hash
            );
            return Err(TransactionError::invalid_transaction_format(format!(
                "{tx_type_name} transaction has unexpected envelope type"
            )));
        }
    };

    // Validate gas_price >= min_protocol_fee
    if gas_price < min_protocol_fee {
        warn!(
            "TX_VALIDATION [hash={}]: {} transaction gas_price below protocol minimum - gas_price: {} wei, required: {} wei",
            tx_hash, tx_type_name, gas_price, min_protocol_fee
        );
        return Err(TransactionError::insufficient_gas_fee(
            min_protocol_fee.to_string(),
            gas_price.to_string(),
        ));
    }

    log_validation_success(tx_hash, tx_type);
    Ok(())
}

/// Validate EIP-1559 (Type 2) transactions
/// Validates:
/// - EIP-1559 invariant: max_priority_fee_per_gas <= max_fee_per_gas
/// - max_fee_per_gas >= min_protocol_fee (must cover base fee)
/// - max_priority_fee_per_gas >= min_protocol_fee (minimum tip requirement)
pub fn validate_eip1559(
    tx: &TxEnvelope,
    min_protocol_fee: U256,
    tx_hash: &str,
) -> Result<(), crate::errors::transaction::TransactionError> {
    use crate::errors::transaction::TransactionError;

    let tx_type_name = get_transaction_type_name(tx_types::EIP1559);

    // Extract both fee fields using pattern matching on TxEnvelope
    let (max_fee_per_gas, max_priority_fee_per_gas) = match tx {
        TxEnvelope::Eip1559(signed_tx) => {
            let inner = signed_tx.tx();
            (
                U256::from(inner.max_fee_per_gas),
                U256::from(inner.max_priority_fee_per_gas),
            )
        }
        _ => {
            warn!(
                "TX_VALIDATION [hash={}]: Expected EIP-1559 transaction, got different type",
                tx_hash
            );
            return Err(TransactionError::invalid_transaction_format(format!(
                "{tx_type_name} transaction has unexpected envelope type"
            )));
        }
    };

    // EIP-1559 invariant: max_priority_fee_per_gas must not exceed max_fee_per_gas
    if max_priority_fee_per_gas > max_fee_per_gas {
        warn!(
            "TX_VALIDATION [hash={}]: {} transaction violates EIP-1559 invariant - max_priority_fee_per_gas ({}) > max_fee_per_gas ({})",
            tx_hash, tx_type_name, max_priority_fee_per_gas, max_fee_per_gas
        );
        return Err(TransactionError::eip1559_validation_failed(
            max_fee_per_gas.to_string(),
            max_priority_fee_per_gas.to_string(),
            min_protocol_fee.to_string(),
        ));
    }

    // Validate max_fee_per_gas >= min_protocol_fee (must cover base fee)
    if max_fee_per_gas < min_protocol_fee {
        warn!(
            "TX_VALIDATION [hash={}]: {} transaction max_fee_per_gas below protocol minimum - max_fee_per_gas: {} wei, required: {} wei",
            tx_hash, tx_type_name, max_fee_per_gas, min_protocol_fee
        );
        return Err(TransactionError::insufficient_gas_fee(
            min_protocol_fee.to_string(),
            max_fee_per_gas.to_string(),
        ));
    }

    // Validate max_priority_fee_per_gas >= min_protocol_fee (minimum tip requirement)
    if max_priority_fee_per_gas < min_protocol_fee {
        warn!(
            "TX_VALIDATION [hash={}]: {} transaction priority fee below protocol minimum - max_priority_fee_per_gas: {} wei, required: {} wei",
            tx_hash, tx_type_name, max_priority_fee_per_gas, min_protocol_fee
        );
        return Err(TransactionError::insufficient_gas_fee(
            min_protocol_fee.to_string(),
            max_priority_fee_per_gas.to_string(),
        ));
    }

    log_validation_success(tx_hash, tx_types::EIP1559);
    Ok(())
}

/// Log successful validation with transaction type and fee information
pub fn log_validation_success(tx_hash: &str, tx_type: u8) {
    let tx_type_name = get_transaction_type_name(tx_type);
    info!(
        "TX_VALIDATION [hash={}]: {} transaction gas fee validation passed",
        tx_hash, tx_type_name
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_payload_success() {
        let payload = IgraPayload {
            version: 0x9,
            tx_type_id: TxTypeId::Entry,
            l2_data: vec![1, 2, 3, 4],
            nonce: [5, 6, 7, 8],
        };

        let result =
            serialize_payload(&payload).expect("Serialization of a valid payload should not fail");

        assert_eq!(result.len(), 1 + 4 + 4);
        assert_eq!(result[0], 0x92);
        assert_eq!(result[1..5], [1, 2, 3, 4]);
        assert_eq!(result[5..9], [5, 6, 7, 8]);
    }

    #[test]
    fn test_serialize_payload_empty_l2_data() {
        let payload = IgraPayload {
            version: 0x9,
            tx_type_id: TxTypeId::Entry,
            l2_data: vec![],
            nonce: [0; 4],
        };

        let result = serialize_payload(&payload);
        assert!(result.is_err());
        if let Err(AppError::SerializationError(msg)) = result {
            assert_eq!(msg, "l2_data cannot be empty");
        } else {
            panic!("Expected a SerializationError, but got {result:?}")
        }
    }

    // Tests for transaction type detection functions

    #[test]
    fn test_detect_legacy_transaction_type() {
        // Legacy transaction starts with RLP list encoding (0xf8 or higher)
        let legacy_tx_data = [0xf8, 0x64, 0x01]; // Simplified legacy transaction
        let result = detect_transaction_type(&legacy_tx_data);
        assert!(result.is_ok());
        assert_eq!(
            result.expect("Should detect legacy transaction type"),
            tx_types::LEGACY
        );
    }

    #[test]
    fn test_detect_eip2930_transaction_type() {
        // EIP-2930 transaction starts with 0x01
        let eip2930_tx_data = [0x01, 0xf8, 0x64, 0x01]; // Type 1 transaction
        let result = detect_transaction_type(&eip2930_tx_data);
        assert!(result.is_ok());
        assert_eq!(
            result.expect("Should detect EIP-2930 transaction type"),
            tx_types::EIP2930
        );
    }

    #[test]
    fn test_detect_eip1559_transaction_type() {
        // EIP-1559 transaction starts with 0x02
        let eip1559_tx_data = [0x02, 0xf8, 0x64, 0x01]; // Type 2 transaction
        let result = detect_transaction_type(&eip1559_tx_data);
        assert!(result.is_ok());
        assert_eq!(
            result.expect("Should detect EIP-1559 transaction type"),
            tx_types::EIP1559
        );
    }

    #[test]
    fn test_detect_blob_transaction_type() {
        // EIP-4844 blob transaction starts with 0x03
        let blob_tx_data = [0x03, 0xf8, 0x64, 0x01]; // Type 3 transaction
        let result = detect_transaction_type(&blob_tx_data);
        assert!(result.is_ok());
        assert_eq!(
            result.expect("Should detect blob transaction type"),
            tx_types::BLOB
        );
    }

    #[test]
    fn test_detect_future_transaction_type() {
        // Future transaction type (e.g., 0x04)
        let future_tx_data = [0x04, 0xf8, 0x64, 0x01]; // Type 4 transaction
        let result = detect_transaction_type(&future_tx_data);
        assert!(result.is_ok());
        assert_eq!(result.expect("Should detect future transaction type"), 4);
    }

    #[test]
    fn test_detect_transaction_type_empty_data() {
        let empty_data = [];
        let result = detect_transaction_type(&empty_data);
        assert!(result.is_err());
        assert!(result
            .expect_err("Should fail on empty transaction data")
            .to_string()
            .contains("Empty transaction data"));
    }

    #[test]
    fn test_get_transaction_type_name() {
        assert_eq!(get_transaction_type_name(tx_types::LEGACY), "Legacy");
        assert_eq!(get_transaction_type_name(tx_types::EIP2930), "EIP-2930");
        assert_eq!(get_transaction_type_name(tx_types::EIP1559), "EIP-1559");
        assert_eq!(get_transaction_type_name(tx_types::BLOB), "EIP-4844 (Blob)");
        assert_eq!(get_transaction_type_name(255), "Unknown");
    }

    #[test]
    fn test_parse_rlp_transaction_unsupported_type() {
        // Test blob transaction type (should be rejected)
        let blob_tx_data = [0x03, 0xf8, 0x64, 0x01];
        let result = parse_rlp_transaction(&blob_tx_data);
        assert!(result.is_err());
        assert!(result
            .expect_err("Should fail on unsupported transaction type")
            .to_string()
            .contains("Unsupported transaction type: 3"));
    }

    #[test]
    fn test_parse_rlp_transaction_empty_data() {
        let empty_data = [];
        let result = parse_rlp_transaction(&empty_data);
        assert!(result.is_err());
        assert!(result
            .expect_err("Should fail on empty transaction data")
            .to_string()
            .contains("Empty transaction data"));
    }

    #[test]
    fn test_parse_rlp_transaction_too_short_typed() {
        let short_data = [0x02]; // Only type byte, no RLP data
        let result = parse_rlp_transaction(&short_data);
        assert!(result.is_err());
        let error_msg = result
            .expect_err("Should fail on short typed transaction")
            .to_string();
        assert!(
            error_msg.contains("Failed to decode"),
            "Expected error message to contain 'Failed to decode', got: {}",
            error_msg
        );
    }

    #[test]
    fn test_parse_real_eip1559_transaction() {
        // Real EIP-1559 transaction from the user's logs
        let tx_hex = "02f8d7824bd8820b558601d1a94a20018601d1a94a200182bf68940000000000000000000000000000000000feedad80b8645f872f55000000000000000000000000000000000000000000000000000000000026337595a0dc7c603d4296b70f5422daa22482d4afb088b29c426b4c9ec5ef019715a11978688306685db4f631b116ed0eeae19876fc9da3f3517653c8b35dee36ee90c080a01d70b4425acf0c6089788788fc51c1c2ef4e3f18203a2653fd462d9fc16bc0bba06b21e05530fa4d4b4ea243d15b4afcb08728cfbe3b788fcbf11687f542fa446f";
        let tx_bytes = hex::decode(tx_hex).expect("Valid hex string");

        // Test that we can parse this transaction
        let result = parse_rlp_transaction(&tx_bytes);
        assert!(
            result.is_ok(),
            "Failed to parse EIP-1559 transaction: {result:?}"
        );

        let (tx, tx_type) = result.expect("Should parse successfully");

        // Verify it's detected as EIP-1559
        assert_eq!(
            tx_type,
            tx_types::EIP1559,
            "Should be detected as EIP-1559 transaction"
        );

        // Verify key fields using pattern matching on TxEnvelope
        match &tx {
            TxEnvelope::Eip1559(signed_tx) => {
                let inner = signed_tx.tx();
                println!("Parsed EIP-1559 transaction:");
                println!("  Chain ID: {}", inner.chain_id);
                println!("  Max Fee Per Gas: {}", inner.max_fee_per_gas);
                println!(
                    "  Max Priority Fee Per Gas: {}",
                    inner.max_priority_fee_per_gas
                );
                println!("  Gas Limit: {}", inner.gas_limit);
                println!("  To: {:?}", inner.to);
                assert!(inner.max_fee_per_gas > 0, "Should have max_fee_per_gas");
                assert!(
                    inner.max_priority_fee_per_gas > 0,
                    "Should have max_priority_fee_per_gas"
                );
            }
            _ => panic!("Expected EIP-1559 transaction variant"),
        }
    }

    // Tests for transaction fee validation using real RLP-encoded transactions

    /// Helper to get a real EIP-1559 transaction for testing
    /// This transaction has max_priority_fee_per_gas = 2,000,000,000,001 wei (about 2000 gwei)
    fn get_test_eip1559_tx() -> (TxEnvelope, u8) {
        let tx_hex = "02f8d7824bd8820b558601d1a94a20018601d1a94a200182bf68940000000000000000000000000000000000feedad80b8645f872f55000000000000000000000000000000000000000000000000000000000026337595a0dc7c603d4296b70f5422daa22482d4afb088b29c426b4c9ec5ef019715a11978688306685db4f631b116ed0eeae19876fc9da3f3517653c8b35dee36ee90c080a01d70b4425acf0c6089788788fc51c1c2ef4e3f18203a2653fd462d9fc16bc0bba06b21e05530fa4d4b4ea243d15b4afcb08728cfbe3b788fcbf11687f542fa446f";
        let tx_bytes = hex::decode(tx_hex).expect("Valid hex string");
        parse_rlp_transaction(&tx_bytes).expect("Should parse test transaction")
    }

    /// Helper to get a real Legacy transaction for testing
    /// gas_price = 20 gwei
    fn get_test_legacy_tx() -> (TxEnvelope, u8) {
        // Real legacy transaction with gas_price = 20 gwei (0x4a817c800 = 20_000_000_000)
        let tx_hex = "f86c098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a76400008025a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276a067cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83";
        let tx_bytes = hex::decode(tx_hex).expect("Valid hex string");
        parse_rlp_transaction(&tx_bytes).expect("Should parse test legacy transaction")
    }

    #[test]
    fn test_validate_gas_fee_with_type_legacy_success() {
        let (tx, tx_type) = get_test_legacy_tx();
        assert_eq!(tx_type, tx_types::LEGACY);

        // Legacy tx has gas_price = 20 gwei, so 10 gwei min should pass
        let min_protocol_fee = U256::from(10_000_000_000u64); // 10 gwei
        let result = validate_gas_fee_with_type(&tx, min_protocol_fee, tx_type, "0xtest");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_gas_fee_with_type_legacy_insufficient_fee() {
        use crate::errors::transaction::TransactionError;

        let (tx, tx_type) = get_test_legacy_tx();
        assert_eq!(tx_type, tx_types::LEGACY);

        // Legacy tx has gas_price = 20 gwei, so 30 gwei min should fail
        let min_protocol_fee = U256::from(30_000_000_000u64); // 30 gwei
        let result = validate_gas_fee_with_type(&tx, min_protocol_fee, tx_type, "0xtest");
        assert!(result.is_err());
        assert!(matches!(
            result.expect_err("Should fail with insufficient gas fee"),
            TransactionError::InsufficientGasFee { .. }
        ));
    }

    #[test]
    fn test_validate_gas_fee_with_type_eip1559_success() {
        let (tx, tx_type) = get_test_eip1559_tx();
        assert_eq!(tx_type, tx_types::EIP1559);

        // EIP-1559 tx has max_priority_fee ~2000 gwei, so 1000 gwei min should pass
        let min_protocol_fee = U256::from(1_000_000_000_000u64); // 1000 gwei
        let result = validate_gas_fee_with_type(&tx, min_protocol_fee, tx_type, "0xtest");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_gas_fee_with_type_eip1559_insufficient_fee() {
        use crate::errors::transaction::TransactionError;

        let (tx, tx_type) = get_test_eip1559_tx();
        assert_eq!(tx_type, tx_types::EIP1559);

        // EIP-1559 tx has max_priority_fee ~2000 gwei, so 3000 gwei min should fail
        let min_protocol_fee = U256::from(3_000_000_000_000u64); // 3000 gwei
        let result = validate_gas_fee_with_type(&tx, min_protocol_fee, tx_type, "0xtest");
        assert!(result.is_err());
        assert!(matches!(
            result.expect_err("Should fail with insufficient gas fee for EIP1559"),
            TransactionError::InsufficientGasFee { .. }
        ));
    }

    #[test]
    fn test_validate_gas_fee_with_type_blob_transaction_rejected() {
        use crate::errors::transaction::TransactionError;

        let (tx, _) = get_test_eip1559_tx();
        let min_protocol_fee = U256::from(2_000_000_000u64);

        // Force tx_type to BLOB - should be rejected as unsupported
        let result = validate_gas_fee_with_type(&tx, min_protocol_fee, tx_types::BLOB, "0xtest");
        assert!(result.is_err());
        let error = result.expect_err("Should fail for blob transaction type");
        assert!(matches!(
            error,
            TransactionError::InvalidTransactionFormat(_)
        ));
        assert!(error
            .to_string()
            .contains("Unsupported transaction type: 3"));
    }

    #[test]
    fn test_validate_gas_fee_with_type_future_type_rejected() {
        use crate::errors::transaction::TransactionError;

        let (tx, _) = get_test_eip1559_tx();
        let min_protocol_fee = U256::from(2_000_000_000u64);

        // Force tx_type to 99 - should be rejected as unsupported
        let result = validate_gas_fee_with_type(&tx, min_protocol_fee, 99, "0xtest");
        assert!(result.is_err());
        let error = result.expect_err("Should fail for future transaction type");
        assert!(matches!(
            error,
            TransactionError::InvalidTransactionFormat(_)
        ));
        assert!(error
            .to_string()
            .contains("Unsupported transaction type: 99"));
    }

    #[test]
    fn test_parse_rlp_transaction_size_limit() {
        // Create oversized transaction (larger than MAX_TRANSACTION_SIZE = 128KB)
        let oversized_data = vec![0x02; 130 * 1024]; // 130KB
        let result = parse_rlp_transaction(&oversized_data);
        assert!(result.is_err());
        let error_msg = result
            .expect_err("Should fail for oversized transaction")
            .to_string();
        assert!(error_msg.contains("Transaction too large"));
        assert!(error_msg.contains("131072 bytes")); // 130 * 1024
    }

    #[test]
    fn test_extract_gas_fees_eip1559() {
        let (tx, _) = get_test_eip1559_tx();
        let gas_info = extract_gas_fees(&tx).expect("Should extract gas fees");

        match gas_info {
            GasFeeInfo::Eip1559 {
                max_fee_per_gas,
                max_priority_fee_per_gas,
            } => {
                // The test transaction has max_fee = max_priority_fee = ~2000 gwei
                assert!(max_fee_per_gas > U256::ZERO);
                assert!(max_priority_fee_per_gas > U256::ZERO);
                // EIP-1559 invariant: priority fee <= max fee
                assert!(max_priority_fee_per_gas <= max_fee_per_gas);
            }
            _ => panic!("Expected EIP-1559 gas fee info"),
        }
    }

    #[test]
    fn test_extract_gas_fees_legacy() {
        let (tx, _) = get_test_legacy_tx();
        let gas_info = extract_gas_fees(&tx).expect("Should extract gas fees");

        match gas_info {
            GasFeeInfo::Legacy { gas_price } => {
                // Legacy tx has gas_price = 20 gwei
                assert_eq!(gas_price, U256::from(20_000_000_000u64));
            }
            _ => panic!("Expected Legacy gas fee info"),
        }
    }

    #[test]
    fn test_eip1559_invariant_passes_for_valid_tx() {
        // The test EIP-1559 transaction has max_fee == max_priority_fee
        // which satisfies the invariant max_priority_fee <= max_fee
        let (tx, tx_type) = get_test_eip1559_tx();
        assert_eq!(tx_type, tx_types::EIP1559);

        // Use a very low min fee to ensure the validation passes
        let min_protocol_fee = U256::from(1_000_000_000u64); // 1 gwei
        let result = validate_eip1559(&tx, min_protocol_fee, "0xtest");
        assert!(
            result.is_ok(),
            "Valid EIP-1559 transaction should pass validation"
        );
    }

    #[test]
    fn test_serialize_payload_zipped() {
        let payload = IgraPayload {
            version: 0x9,
            tx_type_id: TxTypeId::ZippedPayload,
            l2_data: vec![1, 2, 3, 4],
            nonce: [5, 6, 7, 8],
        };

        let result =
            serialize_payload(&payload).expect("Serialization of a valid payload should not fail");

        // Header byte: version(0x9) << 4 | type(0x5) = 0x95
        assert_eq!(result[0], 0x95);
        assert_eq!(result[1..5], [1, 2, 3, 4]);
        assert_eq!(result[5..9], [5, 6, 7, 8]);
    }

    #[test]
    fn test_compress_zlib_deterministic() {
        let data = vec![
            0x02, 0xf8, 0x70, 0x00, 0x00, 0x00, 0xab, 0xcd, 0xef, 0x12, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xab, 0xcd, 0xef, 0x12, 0x34, 0x56, 0x78, 0x9a,
            0xbc, 0xde, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xab, 0xcd, 0xab, 0xcd, 0xab, 0xcd,
            0xab, 0xcd, 0xab, 0xcd, 0xab, 0xcd, 0xab, 0xcd, 0xab, 0xcd, 0xab, 0xcd, 0xab, 0xcd,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ];
        let result1 = compress_zlib(&data).expect("Compression should succeed");
        let result2 = compress_zlib(&data).expect("Compression should succeed");
        assert_eq!(result1, result2, "ZLIB compression must be deterministic");
        assert!(
            result1.len() < data.len(),
            "Structured data should compress smaller"
        );

        // Verify round-trip: decompress and compare to original
        use flate2::read::ZlibDecoder;
        use std::io::Read;
        let mut decoder = ZlibDecoder::new(&result1[..]);
        let mut decompressed = Vec::new();
        decoder
            .read_to_end(&mut decompressed)
            .expect("Decompression should succeed");
        assert_eq!(
            decompressed, data,
            "Round-trip compression must preserve data"
        );
    }

    #[test]
    fn test_compress_zlib_small_data_larger() {
        // Very small random-like data should compress larger due to ZLIB overhead
        let small_data = vec![0xab, 0xcd, 0xef];
        let result = compress_zlib(&small_data).expect("Compression should succeed");
        assert!(
            result.len() > small_data.len(),
            "Small data should be larger after compression"
        );
    }
}
