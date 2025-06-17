use crate::config::AppConfig;
use crate::error::AppError;
use crate::types::rpc::{IgraPayload, RpcRequest, TxTypeId};
use crate::AppState;
use ethers::types::{Transaction, H256};
use ethers::utils::{keccak256, rlp};
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

const VERSION: u8 = 0x9;

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
                "TX_PROCESSOR [id={}, hash={}]: Processing transaction, bytes={}, payload={}",
                id_str,
                tx_hash_str,
                tx_request.tx_bytes.len(),
                full_payload
            );

            let start = std::time::Instant::now();

            // Create a Value with the proper ID for passing to process_wallet_call
            let id_value = if tx_request.id.is_null() {
                json!(id_str)
            } else {
                tx_request.id.clone()
            };

            // Call the wallet sequentially for each transaction
            let process_wallet_result = process_wallet_call(
                &tx_request.tx_bytes,
                &config,
                id_value,
                tx_request.app_state,
            )
            .await;
            let response = match process_wallet_result {
                Ok(_) => {
                    let duration = start.elapsed();
                    processed_count = processed_count.saturating_add(1);
                    info!("TX_PROCESSOR [id={}, hash={}]: Transaction processed successfully, time={:?}, payload_size={}, total_success={}, total_errors={}",
                        id_str, tx_hash_str, duration, tx_request.tx_bytes.len(), processed_count, error_count);
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
    let (validation_result, tx_bytes_opt) = validate_transaction(&req);

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

/// Validates an incoming transaction request and returns the decoded transaction bytes
pub fn validate_transaction(req: &RpcRequest) -> (Result<(), Value>, Option<Vec<u8>>) {
    let id = if req.id.is_null() {
        "null".to_string()
    } else {
        req.id.to_string()
    };

    // Extract the raw transaction from the request
    let raw_tx = req.params[0].as_str().unwrap_or("");

    // Log full transaction request
    debug!(
        "TX_VALIDATE [id={}]: Validating transaction, raw_tx_len={}, raw_tx={}",
        id,
        raw_tx.len(),
        raw_tx
    );

    // Check if transaction starts with 0x
    if !raw_tx.starts_with("0x") {
        warn!(
            "TX_VALIDATE [id={}]: Raw transaction doesn't start with '0x'",
            id
        );
        return (
            Err(AppError::InvalidTransactionFormat.to_json_rpc_error(req.id.clone())),
            None,
        );
    }

    // Remove "0x" prefix and pad with leading zero if the length is odd
    let hex_str = if raw_tx.len() % 2 != 0 {
        debug!("TX_VALIDATE [id={}]: Odd-length hex string, padding", id);
        format!("0{}", &raw_tx[2..])
    } else {
        raw_tx[2..].to_string()
    };

    // Decode from hex
    let tx_bytes = match hex::decode(hex_str) {
        Ok(bytes) => {
            // Compute hash after we have the bytes for better logging
            let hash = format!("{:#x}", compute_transaction_hash(&bytes));

            // Log full bytes
            let full_bytes = format!("0x{}", hex::encode(&bytes));

            debug!(
                "TX_VALIDATE [id={}, hash={}]: Hex decoded successfully, bytes_len={}, bytes={}",
                id,
                hash,
                bytes.len(),
                full_bytes
            );
            bytes
        }
        Err(e) => {
            warn!("TX_VALIDATE [id={}]: Invalid hex format: {}", id, e);
            return (
                Err(AppError::InvalidTransactionFormat.to_json_rpc_error(req.id.clone())),
                None,
            );
        }
    };

    // Validate RLP format
    match rlp::decode::<Transaction>(&tx_bytes) {
        Ok(tx) => {
            let hash = format!("{:#x}", compute_transaction_hash(&tx_bytes));
            debug!("TX_VALIDATE [id={}, hash={}]: RLP decoded successfully, nonce={:?}, gas_price={:?}",
                id, hash, tx.nonce, tx.gas_price);
        }
        Err(e) => {
            warn!("TX_VALIDATE [id={}]: Failed to decode RLP: {}", id, e);
            return (
                Err(AppError::InvalidTransactionFormat.to_json_rpc_error(req.id.clone())),
                None,
            );
        }
    };

    // Return successful validation with decoded bytes
    let hash = format!("{:#x}", compute_transaction_hash(&tx_bytes));
    debug!(
        "TX_VALIDATE [id={}, hash={}]: Validation successful",
        id, hash
    );
    (Ok(()), Some(tx_bytes))
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
    let id_str = if id.is_null() {
        "null".to_string()
    } else {
        id.to_string()
    };

    // For now, we'll use a mock nonce. In the future, this will be the result of mining.
    let nonce = [0u8, 0u8, 0u8, 1u8];

    // Construct the IgraPayload
    let igra_payload = IgraPayload {
        version: VERSION,
        tx_type_id: TxTypeId::UnzippedPayload, // later we will support other types
        l2_data: tx_bytes.to_vec(),
        nonce,
    };

    // Serialize the payload
    let final_payload_bytes = match serialize_payload(&igra_payload) {
        Ok(bytes) => bytes,
        Err(e) => {
            let error_message = format!("Failed to serialize payload: {}", e);
            error!(
                "TX_PROCESSOR [id={}]: Serialization failed: {}",
                id_str, error_message
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
                "TX_PROCESSOR [id={}]: Payload preparation failed: {}",
                id_str, error_message
            );
            return Err(error_message);
        }
    };

    let tx_hash = compute_transaction_hash(&wallet_payload_bytes);
    let tx_hash_str = format!("{:#x}", tx_hash);

    // Call the KASPA Wallet for sending the transaction to the Base Layer
    info!(
        "WALLET_CALL [id={}, hash={}]: Connecting to KASPA Wallet at {}, payload_size={}",
        id_str,
        tx_hash_str,
        config.wallet.wallet_daemon_uri,
        wallet_payload_bytes.len()
    );

    let wallet_caller = app_state.wallet_caller.clone();

    // Actually send the transaction
    info!(
        "WALLET_CALL [id={}, hash={}]: Sending transaction to wallet, payload_size={}",
        id_str,
        tx_hash_str,
        wallet_payload_bytes.len()
    );
    let send_start = std::time::Instant::now();

    // Capture payload size before moving it
    let payload_size = wallet_payload_bytes.len();

    if let Err(err) = wallet_caller
        .send_transaction(
            wallet_payload_bytes,
            Some(id_str.clone()),
            Some(tx_hash_str.clone()),
        )
        .await
    {
        let error_msg = format!("KASPA Wallet call failed: {}", err);
        let duration = send_start.elapsed();
        error!(
            "WALLET_CALL [id={}, hash={}]: Send failed: {}, time={:?}",
            id_str, tx_hash_str, error_msg, duration
        );
        return Err(error_msg);
    }

    let send_time = send_start.elapsed();

    info!("WALLET_CALL [id={}, hash={}]: Transaction accepted by wallet, payload_size={}, send_time={:?}",
        id_str, tx_hash_str, payload_size, send_time);

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
}
