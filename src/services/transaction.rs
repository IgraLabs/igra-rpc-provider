use crate::clients::wallet_caller::TransactionParams;
use crate::config::AppConfig;
use crate::error::AppError;
use crate::errors::transaction::TransactionError;
use crate::errors::GasError;
use crate::errors::ToJsonRpcError;
use crate::services::gas_price::GasPriceService;
use crate::services::mining::TransactionMiner;
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
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// The version of the IgraPayload format
pub const VERSION: u8 = 0x9;

/// Maximum number of transactions that can be queued for sequential processing.
const TRANSACTION_QUEUE_CAPACITY: usize = 1024;

/// Transaction type constants for Ethereum transaction types
pub mod tx_types {
    pub const LEGACY: u8 = 0; // Legacy transaction
    pub const EIP2930: u8 = 1; // EIP-2930 (Access List)
    pub const EIP1559: u8 = 2; // EIP-1559 (Fee Market)
    pub const BLOB: u8 = 3; // EIP-4844 (Blob transactions)
    pub const EIP7702: u8 = 4; // EIP-7702 (Set-Code transactions)
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

// Structure to represent a transaction request that needs to be processed sequentially.
// Fire-and-forget: the synchronous accept path returns the hash to the client as soon as the
// request is enqueued, so there is no response channel back to the caller.
pub struct TransactionRequest {
    pub tx_bytes: Vec<u8>,
    pub id: Value,
    pub app_state: Arc<AppState>,
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

        // Process transactions one at a time. Fee validation already happened synchronously on the
        // accept path, so the worker only mines, signs, and broadcasts. Base fee is not used
        // downstream. Failures here are post-accept (the client already received the hash) and are
        // surfaced as structured `transaction_alerts`, never returned to a caller.
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
            let payload_size = tx_request.tx_bytes.len();

            info!(
                "TX_PROCESSOR [id={}, hash={}]: Processing transaction, payload_size={}, payload={}",
                id_str, tx_hash_str, payload_size, full_payload
            );

            let start = std::time::Instant::now();

            // Create a Value with the proper ID for passing to process_wallet_call
            let id_value = if tx_request.id.is_null() {
                json!(id_str)
            } else {
                tx_request.id.clone()
            };

            // Mining + signing + L1 broadcast.
            let process_wallet_result = process_wallet_call(
                &tx_request.tx_bytes,
                &config,
                id_value,
                tx_request.app_state,
            )
            .await;

            match process_wallet_result {
                Ok(_) => {
                    let duration = start.elapsed();
                    processed_count = processed_count.saturating_add(1);
                    info!("TX_PROCESSOR [id={}, hash={}]: Transaction processed successfully, time={:?}, payload_size={}, payload={}, total_success={}, total_errors={}",
                        id_str, tx_hash_str, duration, payload_size, full_payload, processed_count, error_count);
                }
                Err(err) => {
                    let duration = start.elapsed();
                    error_count = error_count.saturating_add(1);
                    // The client already received the hash on the accept path, so this failure is
                    // only observable here. Emit a structured alert mirroring `mining_alerts` so it
                    // routes through the same pipeline; this is the "never silent" guarantee.
                    error!(
                        target: "transaction_alerts",
                        alert_type = "operational",
                        alert_severity = "critical",
                        alert_name = "AsyncTransactionFailed",
                        id = %id_str,
                        hash = %tx_hash_str,
                        payload_size = payload_size,
                        duration_ms = duration.as_millis(),
                        total_errors = error_count,
                        error = %err,
                        timestamp = %chrono::Utc::now().to_rfc3339(),
                        "transaction failed after enqueue (client already received hash; reconcile by tx hash)"
                    );
                }
            }
        }
    });

    transaction_sender
}

// Validate synchronously on the accept path, then enqueue for fire-and-forget background
// processing. The transaction hash is returned to the client as soon as the transaction is
// accepted into the queue (Ethereum mempool-accept semantics); mining/signing/broadcast happen
// asynchronously and are observable via `transaction_alerts`, not the RPC response.
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

    // Synchronous accept-path validation: format/RLP first, then base-fee fetch (fail-closed),
    // then the gas-fee floor. Returns the decoded bytes or a ready-to-return JSON-RPC error.
    let tx_bytes = match validate_accept_request(
        &req,
        &id_value,
        &state.gas_price_service,
        state.config.el_url(),
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(error_json) => {
            error!(
                "TX [id={}]: Accept-path validation failed: {:?}",
                id, error_json
            );
            return error_json;
        }
    };

    // Compute transaction hash (returned to the client below).
    let tx_hash = compute_transaction_hash(&tx_bytes);
    let tx_hash_str = format!("{tx_hash:#x}");

    info!(
        "TX [id={}, hash={}]: Validated, queueing for background processing, payload_size={}, available_capacity={}",
        id, tx_hash_str, tx_bytes.len(), state.transaction_sender.capacity()
    );

    let tx_request = TransactionRequest {
        tx_bytes,
        id: id_value.clone(),
        app_state: state.clone(),
    };

    // Non-blocking enqueue. With immediate return there is no caller-side deadline, so a full
    // queue must fail fast (retryable -32000) rather than park the handler. A closed channel means
    // the worker is gone — surface it as a critical alert, not a silent -32603.
    match state.transaction_sender.try_send(tx_request) {
        Ok(()) => {
            info!(
                "TX [id={}, hash={}]: Transaction accepted into queue, available_capacity={}",
                id,
                tx_hash_str,
                state.transaction_sender.capacity()
            );
        }
        Err(TrySendError::Full(_)) => {
            error!(
                "TX [id={}, hash={}]: Queue full, rejecting (backpressure), capacity={}",
                id, tx_hash_str, TRANSACTION_QUEUE_CAPACITY
            );
            return TransactionError::queue_full(TRANSACTION_QUEUE_CAPACITY)
                .to_json_rpc_error(id_value);
        }
        Err(TrySendError::Closed(_)) => {
            error!(
                target: "transaction_alerts",
                alert_type = "operational",
                alert_severity = "critical",
                alert_name = "WorkerChannelClosed",
                id = %id,
                hash = %tx_hash_str,
                timestamp = %chrono::Utc::now().to_rfc3339(),
                "transaction processor channel closed; worker is gone"
            );
            return TransactionError::InternalError(
                "Transaction processor channel closed".to_string(),
            )
            .to_json_rpc_error(id_value);
        }
    }

    debug!(
        "TX [id={}, hash={}]: Returning hash to client (mempool-accept)",
        id, tx_hash_str
    );
    json!({
        "jsonrpc": "2.0",
        "result": tx_hash_str,
        "id": id_value
    })
}

/// Synchronous accept-path validation for `eth_sendRawTransaction`.
///
/// Ordering is deliberate and load-bearing:
/// 1. Format/RLP/size validation runs FIRST with no I/O, so malformed input is rejected with its
///    format code (`-32602`/`-32001`/`-32700`) even when the EL is unreachable, and garbage never
///    triggers an EL round-trip.
/// 2. The effective base fee is fetched (1s-cached). Failure is **fail-closed** for well-formed
///    transactions: a retryable `-32000` (`GasError::BaseFetchFailed`).
/// 3. The gas-fee floor is enforced against the effective base fee, reusing `validate_transaction_fees`
///    on the already-decoded bytes (`InsufficientGasFee` -> `-32602`).
///
/// Returns the decoded transaction bytes on success, or a ready-to-return JSON-RPC error `Value`.
async fn validate_accept_request(
    req: &RpcRequest,
    id_value: &Value,
    gas_price_service: &GasPriceService,
    el_url: &str,
) -> Result<Vec<u8>, Value> {
    // 1. Format / RLP / size validation (no I/O; base fee intentionally `None` to skip the fee gate).
    //    On failure the error `Value` propagates unchanged (the function's error type is `Value`).
    let (format_result, tx_bytes_opt) = validate_transaction_request(req, None);
    format_result?;
    let tx_bytes = match tx_bytes_opt {
        Some(bytes) => bytes,
        None => {
            // Should be unreachable when validation passes; handled to avoid a panic.
            return Err(json!({
                "jsonrpc": "2.0",
                "error": { "code": -32000, "message": "Transaction validation passed but no bytes were returned" },
                "id": id_value
            }));
        }
    };

    // 2. Fetch the effective base fee (fail-closed for well-formed transactions).
    let effective_base_fee = match gas_price_service.get_effective_base_fee(el_url).await {
        Ok(fee) => fee,
        Err(e) => {
            warn!(
                "TX [id={}]: base fee fetch failed; rejecting (fail-closed): {}",
                id_value, e
            );
            return Err(
                GasError::base_fetch_failed(e.to_string()).to_json_rpc_error(id_value.clone())
            );
        }
    };

    // 3. Enforce the gas-fee floor against the effective base fee, reusing the existing helper on
    //    the already-decoded bytes (no second hex decode).
    let fee_ctx = TransactionValidationContext::new(
        extract_transaction_id(req),
        tx_bytes.clone(),
        Some(effective_base_fee),
    );
    if let Err(tx_err) = validate_transaction_fees(&fee_ctx) {
        return Err(tx_err.to_json_rpc_error(id_value.clone()));
    }

    Ok(tx_bytes)
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

/// Processes a transaction through the wallet caller
pub async fn process_wallet_call(
    tx_bytes: &[u8],
    config: &AppConfig,
    id: Value,
    app_state: Arc<AppState>,
) -> Result<Value, String> {
    // For now, we'll use a mock nonce. In the future, this will be the result of mining.
    let nonce = [0u8, 0u8, 0u8, 1u8];

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

    // Actually send the transaction
    info!(
        "WALLET_CALL [hash={}]: Sending transaction to wallet, payload_size={}",
        tx_hash_str,
        wallet_payload_bytes.len()
    );
    let send_start = std::time::Instant::now();

    // Capture payload size before moving it
    let payload_size = wallet_payload_bytes.len();

    let miner = TransactionMiner::new(config.mining.clone());
    debug!("WALLET_CALL [hash={}]: Created transaction miner with config: tx_id_prefix=0x{}, timeout={}s",
        tx_hash_str, hex::encode(&config.mining.tx_id_prefix), config.mining.timeout_seconds);

    let transaction_params = TransactionParams::send_all(
        wallet_caller.default_to_address().to_string(),
        wallet_payload_bytes,
        Some(tx_hash_str.clone()),
    );

    // Use retry-enabled method with retry config
    if let Err(err) = wallet_caller
        .mine_and_send_transaction_with_retry(transaction_params, &miner, &config.retry)
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
/// Alloy's TxEnvelope automatically handles all transaction types (Legacy, EIP-2930, EIP-1559,
/// EIP-4844, EIP-7702) without requiring manual fallback parsing.
///
/// # Security
/// - Validates input size before decoding to prevent DoS via oversized payloads
/// - Accepts Legacy, EIP-2930, EIP-1559, and EIP-7702; rejects EIP-4844 blob (0x03) and unknown types (>= 0x05)
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

    if tx_type == tx_types::BLOB || tx_type > tx_types::EIP7702 {
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
        tx_types::EIP7702 => "EIP-7702",
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
        TxEnvelope::Eip7702(signed_tx) => {
            // EIP-7702 carries the same dynamic-fee fields as EIP-1559, so reuse that variant.
            let inner = signed_tx.tx();
            Ok(GasFeeInfo::Eip1559 {
                max_fee_per_gas: U256::from(inner.max_fee_per_gas),
                max_priority_fee_per_gas: U256::from(inner.max_priority_fee_per_gas),
            })
        }
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
        tx_types::EIP1559 | tx_types::EIP7702 => validate_eip1559(tx, min_protocol_fee, tx_hash),
        tx_type if tx_type == tx_types::BLOB || tx_type > tx_types::EIP7702 => {
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

/// Validate EIP-1559 (type 0x02) and EIP-7702 (type 0x04) transactions
/// (both share the same dynamic-fee fields).
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

    // Extract both fee fields and resolve the concrete type using pattern matching on TxEnvelope.
    // EIP-1559 and EIP-7702 share the same dynamic-fee fields, so both are validated here; the
    // resolved type drives accurate logging without changing this function's signature.
    let (max_fee_per_gas, max_priority_fee_per_gas, tx_type) = match tx {
        TxEnvelope::Eip1559(signed_tx) => {
            let inner = signed_tx.tx();
            (
                U256::from(inner.max_fee_per_gas),
                U256::from(inner.max_priority_fee_per_gas),
                tx_types::EIP1559,
            )
        }
        TxEnvelope::Eip7702(signed_tx) => {
            let inner = signed_tx.tx();
            (
                U256::from(inner.max_fee_per_gas),
                U256::from(inner.max_priority_fee_per_gas),
                tx_types::EIP7702,
            )
        }
        _ => {
            warn!(
                "TX_VALIDATION [hash={}]: Expected EIP-1559 or EIP-7702 transaction, got different type",
                tx_hash
            );
            return Err(TransactionError::invalid_transaction_format(
                "transaction has unexpected envelope type (expected EIP-1559 or EIP-7702)"
                    .to_string(),
            ));
        }
    };
    let tx_type_name = get_transaction_type_name(tx_type);

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

    log_validation_success(tx_hash, tx_type);
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
        assert_eq!(get_transaction_type_name(tx_types::EIP7702), "EIP-7702");
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

    /// Helper to get a real signed EIP-7702 (set-code, type 0x04) transaction for testing.
    /// Generated out-of-band with a deterministic key (alloy 1.8.3) and an empty
    /// authorization_list (encoded as the single byte 0xc0; the RPC layer enforces no non-empty
    /// minimum, per task scope). max_fee_per_gas == max_priority_fee_per_gas == 2,000,000,000,000
    /// wei (2000 gwei), matching the EIP-1559 fixture's fee scale so the same thresholds apply.
    fn get_test_eip7702_tx() -> (TxEnvelope, u8) {
        let tx_hex = "04f872824bd8808601d1a94a20008601d1a94a2000830186a09400000000000000000000000000000000000000008080c0c080a0580b9a9ddc636ce8399deff5c902d55719a4206a66a27912eda734c08c21eda3a04b536afcd9f3dd7face8c7c00e8734440df51d2de9d8b3155770ca80866b0c86";
        let tx_bytes = hex::decode(tx_hex).expect("Valid hex string");
        parse_rlp_transaction(&tx_bytes).expect("Should parse test EIP-7702 transaction")
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
    fn test_parse_real_eip7702_transaction() {
        let (tx, tx_type) = get_test_eip7702_tx();

        // Verify it is detected and decoded as EIP-7702 (type 0x04)
        assert_eq!(
            tx_type,
            tx_types::EIP7702,
            "Should be detected as EIP-7702 transaction"
        );

        match &tx {
            TxEnvelope::Eip7702(signed_tx) => {
                let inner = signed_tx.tx();
                assert!(inner.max_fee_per_gas > 0, "Should have max_fee_per_gas");
                assert!(
                    inner.max_priority_fee_per_gas > 0,
                    "Should have max_priority_fee_per_gas"
                );
            }
            _ => panic!("Expected EIP-7702 transaction variant"),
        }
    }

    #[test]
    fn test_validate_gas_fee_with_type_eip7702_success() {
        let (tx, tx_type) = get_test_eip7702_tx();
        assert_eq!(tx_type, tx_types::EIP7702);

        // EIP-7702 tx has max_priority_fee ~2000 gwei, so 1000 gwei min should pass
        let min_protocol_fee = U256::from(1_000_000_000_000u64); // 1000 gwei
        let result = validate_gas_fee_with_type(&tx, min_protocol_fee, tx_type, "0xtest");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_gas_fee_with_type_eip7702_insufficient_fee() {
        use crate::errors::transaction::TransactionError;

        let (tx, tx_type) = get_test_eip7702_tx();
        assert_eq!(tx_type, tx_types::EIP7702);

        // EIP-7702 tx has max_priority_fee ~2000 gwei, so 3000 gwei min should fail
        let min_protocol_fee = U256::from(3_000_000_000_000u64); // 3000 gwei
        let result = validate_gas_fee_with_type(&tx, min_protocol_fee, tx_type, "0xtest");
        assert!(result.is_err());
        assert!(matches!(
            result.expect_err("Should fail with insufficient gas fee for EIP-7702"),
            TransactionError::InsufficientGasFee { .. }
        ));
    }

    #[test]
    fn test_extract_gas_fees_eip7702() {
        let (tx, _) = get_test_eip7702_tx();
        let gas_info = extract_gas_fees(&tx).expect("Should extract gas fees");

        // EIP-7702 reuses the Eip1559 fee shape (shared dynamic-fee fields)
        match gas_info {
            GasFeeInfo::Eip1559 {
                max_fee_per_gas,
                max_priority_fee_per_gas,
            } => {
                assert!(max_fee_per_gas > U256::ZERO);
                assert!(max_priority_fee_per_gas > U256::ZERO);
                // EIP-1559 invariant: priority fee <= max fee
                assert!(max_priority_fee_per_gas <= max_fee_per_gas);
            }
            _ => panic!("Expected EIP-1559 gas fee info for EIP-7702 transaction"),
        }
    }

    #[test]
    fn test_parse_rlp_transaction_type5_rejected() {
        // Type 0x05 is unknown/unsupported and must stay rejected at the parse gate,
        // locking the new `tx_type > tx_types::EIP7702` branch of the filter.
        let type5_data = [0x05, 0xf8, 0x64, 0x01];
        let result = parse_rlp_transaction(&type5_data);
        assert!(result.is_err());
        let error_msg = result
            .expect_err("Should fail for unsupported type 0x05")
            .to_string();
        assert!(
            error_msg.contains("Unsupported transaction type: 5"),
            "Expected 'Unsupported transaction type: 5', got: {error_msg}"
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

    // ---- Accept-path tests (ENG-1145: synchronous validate_accept_request) ----

    /// Real legacy transaction with gas_price = 20 gwei (same fixture as get_test_legacy_tx).
    const LEGACY_TX_HEX_20GWEI: &str = "f86c098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a76400008025a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276a067cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83";

    const GWEI: u128 = 1_000_000_000;

    fn make_send_raw_tx_request(raw_tx: &str) -> RpcRequest {
        RpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "eth_sendRawTransaction".to_string(),
            params: json!([raw_tx]),
            id: json!(1),
        }
    }

    fn gas_service(min_protocol_fee_per_gas_gwei: u64) -> GasPriceService {
        GasPriceService::new(crate::config::GasConfig {
            min_protocol_fee_per_gas_gwei,
        })
    }

    /// Mount an `eth_getBlockByNumber` response returning `base_fee_wei` on the mock EL server.
    async fn mount_base_fee(server: &wiremock::MockServer, base_fee_wei: u128) {
        use wiremock::matchers::method;
        use wiremock::{Mock, ResponseTemplate};
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "number": "0x1b4",
                "hash": "0xabc",
                "baseFeePerGas": format!("0x{base_fee_wei:x}")
            }
        });
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    fn err_code(v: &Value) -> i64 {
        v["error"]["code"]
            .as_i64()
            .expect("error.code should be present")
    }

    /// Format-first ordering: a malformed transaction must return its format code even when the EL
    /// is unreachable, and the EL must not be contacted at all (no DoS surface from garbage).
    #[tokio::test]
    async fn accept_malformed_tx_returns_format_code_even_when_el_down() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // The EL returns 500, so a base-fee fetch WOULD fail if attempted.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let gas = gas_service(100);

        // Valid hex but not a valid RLP transaction -> format error (-32602), never -32000.
        let req = make_send_raw_tx_request("0x1234");
        let id = json!(1);
        let result = validate_accept_request(&req, &id, &gas, &server.uri()).await;

        let err = result.expect_err("malformed tx must be rejected");
        assert_eq!(
            err_code(&err),
            -32602,
            "malformed input must return its format code, not the base-fetch -32000"
        );
        assert_eq!(
            err["error"]["data"]["error_code"],
            "INVALID_TRANSACTION_FORMAT"
        );
        let received = server.received_requests().await.unwrap_or_default();
        assert!(
            received.is_empty(),
            "format-first: the EL must not be contacted for malformed input"
        );
    }

    /// Format-first also covers the -32001 path (missing 0x prefix / invalid hex), an AppError
    /// surfaced before any EL contact. Plan step 9(a) asked for both -32001 and -32602.
    #[tokio::test]
    async fn accept_no_prefix_tx_returns_minus32001_even_when_el_down() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let gas = gas_service(100);

        // No "0x" prefix -> extract_raw_transaction rejects with InvalidTransactionFormat (-32001).
        let req = make_send_raw_tx_request("deadbeef");
        let id = json!(1);
        let result = validate_accept_request(&req, &id, &gas, &server.uri()).await;

        let err = result.expect_err("no-0x-prefix tx must be rejected");
        assert_eq!(
            err_code(&err),
            -32001,
            "hex/prefix format error must return -32001, not the base-fetch -32000"
        );
        let received = server.received_requests().await.unwrap_or_default();
        assert!(
            received.is_empty(),
            "format-first: the EL must not be contacted for malformed input"
        );
    }

    /// Fail-closed: a well-formed tx whose base fee cannot be fetched is rejected with a retryable
    /// -32000 (BASE_FETCH_FAILED), not silently accepted.
    #[tokio::test]
    async fn accept_base_fee_fetch_failure_is_fail_closed_32000() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let gas = gas_service(10);

        let req = make_send_raw_tx_request(&format!("0x{LEGACY_TX_HEX_20GWEI}"));
        let id = json!(1);
        let result = validate_accept_request(&req, &id, &gas, &server.uri()).await;

        let err = result.expect_err("base-fee fetch failure must reject (fail-closed)");
        assert_eq!(err_code(&err), -32000, "fail-closed retryable server error");
        assert_eq!(err["error"]["data"]["error_code"], "BASE_FETCH_FAILED");
        assert_eq!(err["error"]["data"]["retryable"], true);
    }

    /// Fee floor is enforced synchronously on the accept path (the sharp edge): a tx priced below
    /// the effective base fee is rejected with -32602 INSUFFICIENT_GAS_FEE.
    #[tokio::test]
    async fn accept_below_floor_returns_insufficient_gas_fee_32602() {
        let server = wiremock::MockServer::start().await;
        // Network base fee 5 gwei, protocol floor 30 gwei -> effective 30 gwei.
        mount_base_fee(&server, 5 * GWEI).await;
        let gas = gas_service(30);

        // Legacy tx gas_price is 20 gwei < 30 gwei floor.
        let req = make_send_raw_tx_request(&format!("0x{LEGACY_TX_HEX_20GWEI}"));
        let id = json!(1);
        let result = validate_accept_request(&req, &id, &gas, &server.uri()).await;

        let err = result.expect_err("below-floor fee must be rejected synchronously");
        assert_eq!(err_code(&err), -32602);
        assert_eq!(err["error"]["data"]["error_code"], "INSUFFICIENT_GAS_FEE");
    }

    /// Happy path: a well-formed, sufficiently-priced tx returns its decoded bytes for enqueue.
    #[tokio::test]
    async fn accept_at_or_above_floor_returns_bytes() {
        let server = wiremock::MockServer::start().await;
        // Network base fee 5 gwei, floor 10 gwei -> effective 10 gwei <= tx's 20 gwei.
        mount_base_fee(&server, 5 * GWEI).await;
        let gas = gas_service(10);

        let req = make_send_raw_tx_request(&format!("0x{LEGACY_TX_HEX_20GWEI}"));
        let id = json!(1);
        let result = validate_accept_request(&req, &id, &gas, &server.uri()).await;

        let bytes = result.expect("well-formed, sufficiently-priced tx must be accepted");
        assert_eq!(
            bytes,
            hex::decode(LEGACY_TX_HEX_20GWEI).expect("valid hex"),
            "accepted bytes must match the submitted transaction"
        );
    }

    /// Backpressure contract: a full queue maps to a retryable -32000 QUEUE_FULL (the try_send
    /// Full arm returns exactly this).
    #[test]
    fn queue_full_maps_to_retryable_32000() {
        let err =
            TransactionError::queue_full(TRANSACTION_QUEUE_CAPACITY).to_json_rpc_error(json!(1));
        assert_eq!(err_code(&err), -32000);
        assert_eq!(err["error"]["data"]["error_code"], "QUEUE_FULL");
        assert_eq!(err["error"]["data"]["retryable"], true);
    }
}
