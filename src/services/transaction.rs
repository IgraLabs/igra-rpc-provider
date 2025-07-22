use crate::clients::wallet_caller::TransactionParams;
use crate::config::AppConfig;
use crate::error::AppError;
use crate::services::mining::TransactionMiner;
use crate::types::rpc::{IgraPayload, RpcRequest, TxTypeId};
use crate::AppState;
use ethers::types::{Transaction, H256, U256};
use ethers::utils::{keccak256, rlp};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::error::Error;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Transaction type constants
mod tx_types {
    pub const LEGACY: u8 = 0;
    pub const EIP2930: u8 = 1;
    pub const EIP1559: u8 = 2;
    pub const BLOB: u8 = 3;
}

/// The version of the IgraPayload format
pub const VERSION: u8 = 0x9;

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
    let (transaction_sender, mut transaction_receiver) = mpsc::channel::<TransactionRequest>(1024);
    info!(
        "TX_PROCESSOR: Starting transaction processor with queue size={}",
        1024
    );

    // Start the sequential transaction processor task
    let config = Arc::new(config);
    tokio::spawn(async move {
        info!("TX_PROCESSOR: Background worker started");

        let mut processed_count: u16 = 0;
        let mut error_count: u16 = 0;

        // Process transactions one at a time
        while let Some(tx_request) = transaction_receiver.recv().await {
            let tx_hash = compute_transaction_hash(&tx_request.tx_bytes);
            let tx_hash_str = format!("{:#x}", tx_hash);

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

            // Get minimum protocol fee for validation
            let min_protocol_fee = config.gas.min_protocol_fee_per_gas_wei();

            // Create a Value with the proper ID for passing to process_wallet_call
            let id_value = if tx_request.id.is_null() {
                json!(id_str)
            } else {
                tx_request.id.clone()
            };

            // Validate transaction fee before processing
            let fee_validation_result =
                validate_transaction_fee(&tx_request.raw_tx, min_protocol_fee, &id_str);

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
                Err(error_msg) => {
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
    let (validation_result, tx_bytes_opt) = validate_transaction(&req, None);

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
    let tx_hash_str = format!("{:#x}", tx_hash);

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
        // Even if queueing fails, we still return the hash to the user
    } else {
        let queue_time = queue_start.elapsed();
        info!(
            "TX [id={}, hash={}]: Transaction queued successfully, queue_time={:?}, available_capacity={}",
            id, tx_hash_str, queue_time, state.transaction_sender.capacity()
        );
    }

    let response = response_receiver
        .recv()
        .await
        .expect("Failed to receive response from transaction processor");

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

/// Validates just the transaction fee without needing a full RPC request
fn validate_transaction_fee(raw_tx: &str, min_protocol_fee: U256, id: &str) -> Result<(), String> {
    // Convert internal AppError to String for compatibility with existing callers
    validate_transaction_fee_internal(raw_tx, min_protocol_fee, id).map_err(|e| e.to_string())
}

/// Internal validation function that uses AppError for better error handling
fn validate_transaction_fee_internal(
    raw_tx: &str,
    min_protocol_fee: U256,
    id: &str,
) -> Result<(), AppError> {
    let tx_bytes =
        decode_hex_transaction(raw_tx, id).map_err(|_| AppError::InvalidTransactionFormat)?;

    // Log the detected transaction type for debugging
    let tx_type = detect_transaction_type_from_bytes(&tx_bytes);
    info!(
        "TX_VALIDATE [id={}]: Detected transaction type {} ({}) from byte prefix",
        id,
        tx_type,
        get_transaction_type_name(tx_type)
    );

    validate_rlp_and_gas_fee(&tx_bytes, Some(min_protocol_fee), id)
}

/// Validates an incoming transaction request and returns the decoded transaction bytes
pub fn validate_transaction(
    req: &RpcRequest,
    min_protocol_fee: Option<U256>,
) -> (Result<(), Value>, Option<Vec<u8>>) {
    let id = extract_transaction_id(req);

    let raw_tx = match extract_raw_transaction(req, &id) {
        Ok(tx) => tx,
        Err(error) => return (Err(error), None),
    };

    let tx_bytes = match decode_hex_transaction(&raw_tx, &id) {
        Ok(bytes) => bytes,
        Err(error) => return (Err(error), None),
    };

    if let Err(error) = validate_rlp_and_gas_fee(&tx_bytes, min_protocol_fee, &id) {
        return (Err(error.to_json_rpc_error(req.id.clone())), None);
    }

    let hash = format!("{:#x}", compute_transaction_hash(&tx_bytes));
    info!(
        "TX_VALIDATE [id={}, hash={}]: Validation successful",
        id, hash
    );
    (Ok(()), Some(tx_bytes))
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
    let hex_str: Cow<str> = if raw_tx.len() % 2 != 0 {
        debug!("TX_VALIDATE [id={}]: Odd-length hex string, padding", id);
        Cow::Owned(format!("0{}", &raw_tx[2..]))
    } else {
        Cow::Borrowed(&raw_tx[2..])
    };

    // Decode from hex
    match hex::decode(hex_str.as_ref()) {
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

/// Validates RLP format and gas fee parameters
fn validate_rlp_and_gas_fee(
    tx_bytes: &[u8],
    min_protocol_fee: Option<U256>,
    id: &str,
) -> Result<(), AppError> {
    // First detect transaction type from bytes to catch unsupported types early
    let tx_type_from_bytes = detect_transaction_type_from_bytes(tx_bytes);

    // Reject unsupported transaction types before RLP decoding
    if tx_type_from_bytes >= tx_types::BLOB {
        let hash = format!("{:#x}", compute_transaction_hash(tx_bytes));
        error!(
            "TX_VALIDATE [id={}, hash={}]: Unsupported transaction type detected from bytes: {}",
            id, hash, tx_type_from_bytes
        );
        return Err(AppError::Internal(format!(
            "Unsupported transaction type: {}",
            tx_type_from_bytes
        )));
    }

    match rlp::decode::<Transaction>(tx_bytes) {
        Ok(tx) => {
            let hash = format!("{:#x}", compute_transaction_hash(tx_bytes));
            info!("TX_VALIDATE [id={}, hash={}]: RLP decoded successfully, nonce={:?}, gas_price={:?}, detected_type={} ({})",
                id, hash, tx.nonce, tx.gas_price, tx_type_from_bytes, get_transaction_type_name(tx_type_from_bytes));

            // Validate gas fee if min_protocol_fee is provided
            if let Some(min_protocol_fee) = min_protocol_fee {
                validate_gas_fee_with_type(&tx, min_protocol_fee, tx_type_from_bytes, id)?;
            }
            Ok(())
        }
        Err(e) => {
            warn!("TX_VALIDATE [id={}]: Failed to decode RLP: {}", id, e);
            Err(AppError::InvalidTransactionFormat)
        }
    }
}

/// Validates the gas fee with a known transaction type
///
/// This function validates different fee fields based on transaction type:
/// - Type 0 (Legacy): Validates gas_price >= min_protocol_fee_per_gas
/// - Type 1 (EIP-2930): Validates gas_price >= min_protocol_fee_per_gas
/// - Type 2 (EIP-1559): Validates max_priority_fee_per_gas >= min_protocol_fee_per_gas
/// - Type 3+ (Blob, etc.): Rejects as unsupported
fn validate_gas_fee_with_type(
    tx: &Transaction,
    min_protocol_fee: U256,
    tx_type: u8,
    id: &str,
) -> Result<(), AppError> {
    match tx_type {
        tx_types::LEGACY | tx_types::EIP2930 => {
            // Type 0 (Legacy) and Type 1 (EIP-2930): Check gas_price
            let gas_price = tx.gas_price.ok_or_else(|| {
                warn!(
                    "TX_VALIDATE [id={}]: Type {} transaction missing gas_price field",
                    id, tx_type
                );
                AppError::InvalidTransactionFormat
            })?;

            info!(
                "TX_VALIDATE [id={}]: Type {} ({}) transaction, gas_price={} wei",
                id,
                tx_type,
                get_transaction_type_name(tx_type),
                gas_price
            );

            if gas_price < min_protocol_fee {
                warn!(
                    "TX_VALIDATE [id={}]: Gas price too low. Required: {} wei, Provided: {} wei",
                    id, min_protocol_fee, gas_price
                );
                return Err(AppError::insufficient_gas_fee(min_protocol_fee, gas_price));
            }
        }
        tx_types::EIP1559 => {
            // Type 2 (EIP-1559): Check max_priority_fee_per_gas
            let max_priority_fee = tx.max_priority_fee_per_gas.ok_or_else(|| {
                warn!("TX_VALIDATE [id={}]: Type 2 (EIP-1559) transaction missing max_priority_fee_per_gas field", id);
                AppError::InvalidTransactionFormat
            })?;

            info!(
                "TX_VALIDATE [id={}]: Type 2 (EIP-1559) transaction, max_priority_fee_per_gas={} wei",
                id, max_priority_fee
            );

            if max_priority_fee < min_protocol_fee {
                warn!(
                    "TX_VALIDATE [id={}]: Max priority fee too low. Required: {} wei, Provided: {} wei",
                    id, min_protocol_fee, max_priority_fee
                );
                return Err(AppError::insufficient_gas_fee(
                    min_protocol_fee,
                    max_priority_fee,
                ));
            }
        }
        _ => {
            // Type 3+ (Blob transactions, etc.): Reject as unsupported
            error!(
                "TX_VALIDATE [id={}]: Unsupported transaction type: {}",
                id, tx_type
            );
            return Err(AppError::Internal(format!(
                "Unsupported transaction type: {}",
                tx_type
            )));
        }
    }

    info!(
        "TX_VALIDATE [id={}]: Gas fee validation passed for type {} ({}) transaction",
        id,
        tx_type,
        get_transaction_type_name(tx_type)
    );

    Ok(())
}

/// Gets a human-readable name for a transaction type
fn get_transaction_type_name(tx_type: u8) -> &'static str {
    match tx_type {
        tx_types::LEGACY => "Legacy",
        tx_types::EIP2930 => "EIP-2930",
        tx_types::EIP1559 => "EIP-1559",
        tx_types::BLOB => "Blob (EIP-4844)",
        _ => "Unknown",
    }
}

/// Detects the transaction type from the raw transaction bytes
///
/// Transaction types are identified by their prefix byte:
/// - Type 0 (Legacy): No prefix, starts with RLP encoding (0xc0-0xff)
/// - Type 1 (EIP-2930): Prefixed with 0x01
/// - Type 2 (EIP-1559): Prefixed with 0x02
/// - Type 3 (EIP-4844): Prefixed with 0x03
/// - Type 4+: Future types with corresponding prefix bytes
fn detect_transaction_type_from_bytes(tx_bytes: &[u8]) -> u8 {
    if tx_bytes.is_empty() {
        return tx_types::LEGACY; // Default to legacy for empty bytes
    }

    let first_byte = tx_bytes[0];

    // Check if it's a typed transaction (first byte < 0x80)
    if first_byte < 0x80 {
        // It's a typed transaction, the first byte is the type
        first_byte
    } else {
        // It's a legacy transaction (starts with RLP encoding)
        tx_types::LEGACY
    }
}

/// Computes transaction hash from raw bytes
pub fn compute_transaction_hash(tx_bytes: &[u8]) -> H256 {
    keccak256(tx_bytes).into()
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

    // Construct the IgraPayload
    let igra_payload = IgraPayload {
        version: VERSION,
        tx_type_id: TxTypeId::UnzippedPayload, // later we will support other types
        l2_data: tx_bytes.to_vec(),
        nonce,
    };
    let tx_hash = compute_transaction_hash(&igra_payload.l2_data);
    let tx_hash_str = format!("{:#x}", tx_hash);

    // Serialize the payload
    let final_payload_bytes = match serialize_payload(&igra_payload) {
        Ok(bytes) => bytes,
        Err(e) => {
            let error_message = format!("Failed to serialize payload: {}", e);
            error!(
                "TX_PROCESSOR [hash={}]: Serialization failed: {}",
                tx_hash_str, error_message
            );
            return Err(error_message);
        }
    };

    // Prepare payload for Kaspa wallet (e.g., zipping)
    let wallet_payload_bytes = match prepare_payload(&final_payload_bytes) {
        Ok(bytes) => bytes,
        Err(e) => {
            let error_message = format!("Failed to prepare payload: {}", e);
            error!(
                "TX_PROCESSOR [hash={}]: Payload preparation failed: {}",
                tx_hash_str, error_message
            );
            return Err(error_message);
        }
    };

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
    debug!("WALLET_CALL [hash={}]: Created transaction miner with config: required_prefix=0x{}, timeout={}s",
        tx_hash_str, hex::encode(&config.mining.required_prefix), config.mining.timeout_seconds);

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
        let error_msg = format!("KASPA Wallet call failed: {}", err);
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

pub fn prepare_payload(tx_bytes: &[u8]) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
    // For now, we just pass the bytes through without zipping.
    // The previous implementation of zipping and adding a header is now incorrect
    // because the new `serialize_payload` function handles the header.
    // Zipping logic can be re-introduced here if needed for specific TxTypeIds.
    Ok(tx_bytes.to_vec())
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

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::U64;

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
            panic!("Expected a SerializationError, but got {:?}", result)
        }
    }

    #[test]
    fn test_detect_transaction_type_from_bytes() {
        // Test Legacy transaction (starts with RLP encoding)
        let legacy_bytes = vec![0xf8, 0x6c, 0x01]; // RLP-encoded legacy tx
        assert_eq!(detect_transaction_type_from_bytes(&legacy_bytes), 0);

        // Test EIP-2930 transaction (Type 1)
        let eip2930_bytes = vec![0x01, 0xf8, 0x6c]; // Type 1 prefix
        assert_eq!(detect_transaction_type_from_bytes(&eip2930_bytes), 1);

        // Test EIP-1559 transaction (Type 2)
        let eip1559_bytes = vec![0x02, 0xf8, 0x6c]; // Type 2 prefix
        assert_eq!(detect_transaction_type_from_bytes(&eip1559_bytes), 2);

        // Test Blob transaction (Type 3)
        let blob_bytes = vec![0x03, 0xf8, 0x6c]; // Type 3 prefix
        assert_eq!(detect_transaction_type_from_bytes(&blob_bytes), 3);

        // Test future transaction type
        let future_bytes = vec![0x7f, 0xf8, 0x6c]; // Type 127 prefix
        assert_eq!(detect_transaction_type_from_bytes(&future_bytes), 127);

        // Test empty bytes
        let empty_bytes = vec![];
        assert_eq!(detect_transaction_type_from_bytes(&empty_bytes), 0);
    }

    #[test]
    fn test_validate_gas_fee_type_0_legacy() {
        // Create a legacy transaction with gas_price
        let tx = Transaction {
            gas_price: Some(U256::from(1000u64)),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            access_list: None,
            nonce: U256::zero(),
            gas: U256::from(21000u64),
            to: None,
            value: U256::zero(),
            input: vec![].into(),
            v: U64::zero(),
            r: U256::zero(),
            s: U256::zero(),
            hash: H256::zero(),
            from: Default::default(),
            block_hash: None,
            block_number: None,
            transaction_index: None,
            chain_id: None,
            transaction_type: None,
            other: Default::default(),
        };

        // Test with gas price above minimum
        let min_fee = U256::from(500u64);
        assert!(validate_gas_fee_with_type(&tx, min_fee, 0, "test").is_ok());

        // Test with gas price equal to minimum
        let min_fee = U256::from(1000u64);
        assert!(validate_gas_fee_with_type(&tx, min_fee, 0, "test").is_ok());

        // Test with gas price below minimum
        let min_fee = U256::from(2000u64);
        let result = validate_gas_fee_with_type(&tx, min_fee, 0, "test");
        assert!(result.is_err());
        assert!(matches!(
            result.expect_err("Expected InsufficientGasFee error"),
            AppError::InsufficientGasFee { .. }
        ));
    }

    #[test]
    fn test_validate_gas_fee_type_1_eip2930() {
        // Create an EIP-2930 transaction with gas_price and access_list
        let tx = Transaction {
            gas_price: Some(U256::from(1500u64)),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            access_list: Some(Default::default()), // Has access list
            nonce: U256::zero(),
            gas: U256::from(21000u64),
            to: None,
            value: U256::zero(),
            input: vec![].into(),
            v: U64::zero(),
            r: U256::zero(),
            s: U256::zero(),
            hash: H256::zero(),
            from: Default::default(),
            block_hash: None,
            block_number: None,
            transaction_index: None,
            chain_id: None,
            transaction_type: None,
            other: Default::default(),
        };

        // Test with gas price above minimum
        let min_fee = U256::from(1000u64);
        assert!(validate_gas_fee_with_type(&tx, min_fee, 1, "test").is_ok());

        // Test with gas price below minimum
        let min_fee = U256::from(2000u64);
        let result = validate_gas_fee_with_type(&tx, min_fee, 1, "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_gas_fee_type_2_eip1559() {
        // Create an EIP-1559 transaction with max_priority_fee_per_gas
        let tx = Transaction {
            gas_price: None,
            max_fee_per_gas: Some(U256::from(3000u64)),
            max_priority_fee_per_gas: Some(U256::from(2000u64)),
            access_list: None,
            nonce: U256::zero(),
            gas: U256::from(21000u64),
            to: None,
            value: U256::zero(),
            input: vec![].into(),
            v: U64::zero(),
            r: U256::zero(),
            s: U256::zero(),
            hash: H256::zero(),
            from: Default::default(),
            block_hash: None,
            block_number: None,
            transaction_index: None,
            chain_id: None,
            transaction_type: None,
            other: Default::default(),
        };

        // Test with max_priority_fee above minimum
        let min_fee = U256::from(1500u64);
        assert!(validate_gas_fee_with_type(&tx, min_fee, 2, "test").is_ok());

        // Test with max_priority_fee equal to minimum
        let min_fee = U256::from(2000u64);
        assert!(validate_gas_fee_with_type(&tx, min_fee, 2, "test").is_ok());

        // Test with max_priority_fee below minimum
        let min_fee = U256::from(2500u64);
        let result = validate_gas_fee_with_type(&tx, min_fee, 2, "test");
        assert!(result.is_err());
        assert!(matches!(
            result.expect_err("Expected InsufficientGasFee error"),
            AppError::InsufficientGasFee { .. }
        ));
    }

    #[test]
    fn test_validate_gas_fee_type_3_unsupported() {
        // Create any transaction (content doesn't matter for type 3+)
        let tx = Transaction {
            gas_price: Some(U256::from(5000u64)),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            access_list: None,
            nonce: U256::zero(),
            gas: U256::from(21000u64),
            to: None,
            value: U256::zero(),
            input: vec![].into(),
            v: U64::zero(),
            r: U256::zero(),
            s: U256::zero(),
            hash: H256::zero(),
            from: Default::default(),
            block_hash: None,
            block_number: None,
            transaction_index: None,
            chain_id: None,
            transaction_type: None,
            other: Default::default(),
        };

        // Test type 3 (blob transaction)
        let min_fee = U256::from(1000u64);
        let result = validate_gas_fee_with_type(&tx, min_fee, 3, "test");
        assert!(result.is_err());
        assert!(matches!(
            result.expect_err("Expected Internal error"),
            AppError::Internal(_)
        ));

        // Test type 4 (future transaction)
        let result = validate_gas_fee_with_type(&tx, min_fee, 4, "test");
        assert!(result.is_err());
        assert!(matches!(
            result.expect_err("Expected Internal error"),
            AppError::Internal(_)
        ));
    }

    #[test]
    fn test_validate_gas_fee_missing_fields() {
        // Test legacy transaction with missing gas_price
        let tx = Transaction {
            gas_price: None, // Missing gas price
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            access_list: None,
            nonce: U256::zero(),
            gas: U256::from(21000u64),
            to: None,
            value: U256::zero(),
            input: vec![].into(),
            v: U64::zero(),
            r: U256::zero(),
            s: U256::zero(),
            hash: H256::zero(),
            from: Default::default(),
            block_hash: None,
            block_number: None,
            transaction_index: None,
            chain_id: None,
            transaction_type: None,
            other: Default::default(),
        };

        let min_fee = U256::from(1000u64);
        let result = validate_gas_fee_with_type(&tx, min_fee, tx_types::LEGACY, "test");
        assert!(result.is_err()); // Should fail because gas_price is missing
        assert!(matches!(
            result.expect_err("Expected InvalidTransactionFormat error"),
            AppError::InvalidTransactionFormat
        ));

        // Test EIP-1559 transaction with missing max_priority_fee_per_gas
        let tx_eip1559 = Transaction {
            gas_price: None,
            max_fee_per_gas: Some(U256::from(3000u64)),
            max_priority_fee_per_gas: None, // Missing priority fee
            access_list: None,
            nonce: U256::zero(),
            gas: U256::from(21000u64),
            to: None,
            value: U256::zero(),
            input: vec![].into(),
            v: U64::zero(),
            r: U256::zero(),
            s: U256::zero(),
            hash: H256::zero(),
            from: Default::default(),
            block_hash: None,
            block_number: None,
            transaction_index: None,
            chain_id: None,
            transaction_type: None,
            other: Default::default(),
        };

        let result = validate_gas_fee_with_type(&tx_eip1559, min_fee, tx_types::EIP1559, "test");
        assert!(result.is_err()); // Should fail because max_priority_fee is missing
        assert!(matches!(
            result.expect_err("Expected InvalidTransactionFormat error"),
            AppError::InvalidTransactionFormat
        ));
    }

    #[test]
    fn test_validate_rlp_and_gas_fee_type_3_rejection() {
        // Create a blob transaction (type 3) - these bytes represent a type 3 tx prefix
        let blob_tx_bytes = vec![0x03, 0xf8, 0x6c, 0x01, 0x02, 0x03];
        let min_fee = U256::from(1000u64);

        let result = validate_rlp_and_gas_fee(&blob_tx_bytes, Some(min_fee), "test");
        assert!(result.is_err());

        // Check that it's rejected before RLP decoding
        if let Err(AppError::Internal(msg)) = result {
            assert!(msg.contains("Unsupported transaction type: 3"));
        } else {
            panic!("Expected Internal error for unsupported transaction type");
        }
    }
}
